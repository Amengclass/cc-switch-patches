#!/usr/bin/env node
// merge-structured.mjs — C 层：把「结构化载荷」合并进官方源码树
//   1) i18n：按 JSON key 深合并（新增/改值，官方 key 原样保留）
//   2) config-overrides：按 dot-path 覆盖 JSON 叶子节点
//   3) cargo-additions：按「存在即跳过」追加依赖 + windows-sys features
// 用法: node merge-structured.mjs <structured 目录> <目标仓库目录>
import fs from "node:fs";
import path from "node:path";

const [, , structuredDir, targetDir] = process.argv;
if (!structuredDir || !targetDir) {
  console.error("用法: node merge-structured.mjs <structured 目录> <目标仓库目录>");
  process.exit(1);
}

const stats = { i18nKeys: 0, configLeaves: 0, cargoDeps: 0, cargoFeatures: 0 };

// ---------- 工具 ----------
function deepMerge(base, delta) {
  let n = 0;
  for (const [k, v] of Object.entries(delta)) {
    const isObj = v && typeof v === "object" && !Array.isArray(v);
    const baseIsObj = base[k] && typeof base[k] === "object" && !Array.isArray(base[k]);
    if (isObj && baseIsObj) {
      n += deepMerge(base[k], v);
    } else {
      base[k] = v;
      n += 1;
    }
  }
  return n;
}

function parsePath(p) {
  return p
    .replace(/\[(\d+)\]/g, ".$1")
    .split(".")
    .filter(Boolean);
}

function setByPath(obj, p, value) {
  const parts = parsePath(p);
  let cur = obj;
  for (let i = 0; i < parts.length - 1; i++) {
    const k = parts[i];
    if (cur[k] == null) cur[k] = /^\d+$/.test(parts[i + 1]) ? [] : {};
    cur = cur[k];
  }
  const last = parts[parts.length - 1];
  const changed = JSON.stringify(cur[last]) !== JSON.stringify(value);
  cur[last] = value;
  return changed;
}

// ---------- 1) i18n ----------
const i18nDir = path.join(structuredDir, "i18n");
if (fs.existsSync(i18nDir)) {
  for (const f of fs.readdirSync(i18nDir).filter((x) => x.endsWith(".json"))) {
    const target = path.join(targetDir, "src", "i18n", "locales", f);
    if (!fs.existsSync(target)) {
      console.warn(`  [警告] 目标不存在，跳过: src/i18n/locales/${f}`);
      continue;
    }
    const delta = JSON.parse(fs.readFileSync(path.join(i18nDir, f), "utf8"));
    const doc = JSON.parse(fs.readFileSync(target, "utf8"));
    const n = deepMerge(doc, delta);
    fs.writeFileSync(target, JSON.stringify(doc, null, 2) + "\n", "utf8");
    stats.i18nKeys += n;
    console.log(`  [i18n]   ${f}: 合并 ${n} 个 key`);
  }
}

// ---------- 2) config-overrides ----------
const cfgPath = path.join(structuredDir, "config-overrides.json");
if (fs.existsSync(cfgPath)) {
  const overrides = JSON.parse(fs.readFileSync(cfgPath, "utf8"));
  for (const [relFile, leaves] of Object.entries(overrides)) {
    if (relFile.startsWith("_")) continue;
    const target = path.join(targetDir, relFile);
    if (!fs.existsSync(target)) {
      console.warn(`  [警告] 配置不存在，跳过: ${relFile}`);
      continue;
    }
    const doc = JSON.parse(fs.readFileSync(target, "utf8"));
    let n = 0;
    for (const [p, v] of Object.entries(leaves)) if (setByPath(doc, p, v)) n++;
    fs.writeFileSync(target, JSON.stringify(doc, null, 2) + "\n", "utf8");
    stats.configLeaves += n;
    console.log(`  [config] ${relFile}: 覆盖 ${n} 个叶子`);
  }
}

// ---------- 3) Cargo.toml ----------
const cargoAddPath = path.join(structuredDir, "cargo-additions.json");
if (fs.existsSync(cargoAddPath)) {
  const add = JSON.parse(fs.readFileSync(cargoAddPath, "utf8"));
  const cargoPath = path.join(targetDir, "src-tauri", "Cargo.toml");
  if (fs.existsSync(cargoPath)) {
    let lines = fs.readFileSync(cargoPath, "utf8").split(/\r?\n/);

    // 3a) 新增依赖：插在 [dependencies] 之后（依赖名已存在则跳过）
    const depLineRe = /^\s*([A-Za-z0-9_-]+)\s*=/;
    const existingDeps = new Set();
    let depHeader = -1;
    lines.forEach((l, i) => {
      if (/^\[dependencies\]\s*$/.test(l)) depHeader = i;
      const m = l.match(depLineRe);
      if (m) existingDeps.add(m[1]);
    });
    const toAdd = [];
    for (const dep of add.dependencies || []) {
      const name = dep.split("=")[0].trim();
      if (existingDeps.has(name)) {
        console.log(`  [cargo]  依赖已存在，跳过: ${name}`);
      } else {
        toAdd.push(dep);
        stats.cargoDeps++;
      }
    }
    if (toAdd.length && depHeader >= 0) {
      lines.splice(depHeader + 1, 0, ...toAdd);
      console.log(`  [cargo]  新增依赖 ${toAdd.length} 个: ${toAdd.map((d) => d.split("=")[0].trim()).join(", ")}`);
    }

    // 3b) windows-sys features：追加进 features 数组（已存在则跳过）
    const wsIdx = lines.findIndex((l) => /\bwindows-sys\s*=\s*\{/.test(l));
    if (wsIdx >= 0 && (add.windows_sys_features || []).length) {
      let closeIdx = -1;
      for (let i = wsIdx; i < lines.length; i++) {
        if (/\bsys-locale\b/.test(lines[i])) break;
        if (lines[i].includes("]")) { closeIdx = i; break; }
      }
      if (closeIdx > wsIdx) {
        const block = lines.slice(wsIdx, closeIdx + 1).join("\n");
        const missing = add.windows_sys_features.filter((f) => !block.includes(`"${f}"`));
        if (missing.length) {
          const indentMatch = lines[closeIdx - 1].match(/^(\s*)/);
          const ind = indentMatch ? indentMatch[1] : "    ";
          const insert = missing.map((f) => `${ind}"${f}",`);
          lines.splice(closeIdx, 0, ...insert);
          stats.cargoFeatures += missing.length;
          console.log(`  [cargo]  windows-sys 追加 features ${missing.length} 个: ${missing.join(", ")}`);
        } else {
          console.log(`  [cargo]  windows-sys features 已齐全，跳过`);
        }
      } else {
        console.warn("  [警告] 未定位到 windows-sys features 数组结尾，跳过 features 追加");
      }
    } else if (wsIdx < 0) {
      console.warn("  [警告] Cargo.toml 未找到 windows-sys，跳过 features 追加");
    }

    fs.writeFileSync(cargoPath, lines.join("\n"), "utf8");
  } else {
    console.warn("  [警告] 目标无 src-tauri/Cargo.toml，跳过");
  }
}

console.log(
  `\n  结构化合并完成: i18n ${stats.i18nKeys} key / 配置 ${stats.configLeaves} 叶子 / 依赖 ${stats.cargoDeps} 个 / features ${stats.cargoFeatures} 个`
);
