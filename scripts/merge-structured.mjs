#!/usr/bin/env node
// merge-structured.mjs — C 层：把「结构化载荷」合并进官方源码树
//   1) i18n：按 JSON key 深合并（新增/改值，官方 key 原样保留）
//   2) config-overrides：按 dot-path 覆盖 JSON 叶子节点（Cargo.toml 已改走补丁 0310）

// 用法: node merge-structured.mjs <structured 目录> <目标仓库目录>
import fs from "node:fs";
import path from "node:path";

const [, , structuredDir, targetDir] = process.argv;
if (!structuredDir || !targetDir) {
  console.error("用法: node merge-structured.mjs <structured 目录> <目标仓库目录>");
  process.exit(1);
}

const stats = { i18nKeys: 0, configLeaves: 0 };

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

console.log(
  `\n  结构化合并完成: i18n ${stats.i18nKeys} key / 配置 ${stats.configLeaves} 叶子`
);
