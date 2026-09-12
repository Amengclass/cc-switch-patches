#!/usr/bin/env node
// compare-trees.mjs — 比对「参考树(我们的仓库)」与「组装树(apply 产物)」
//   字节比对为主；对 JSON / Cargo.toml 这类结构化文件做「语义比对」，
//   以区分「真差异」与「仅格式/顺序差异」。
// 用法: node compare-trees.mjs <参考目录> <目标目录> [--quiet]
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";

const [, , refDir, tgtDir, ...flags] = process.argv;
if (!refDir || !tgtDir) {
  console.error("用法: node compare-trees.mjs <参考目录> <目标目录>");
  process.exit(1);
}
const quiet = flags.includes("--quiet");
const SKIP = /[\\/](\.git|node_modules|target|dist|\.vite|tmp-official)[\\/]/;

function walk(root, base = "") {
  const out = [];
  for (const e of fs.readdirSync(root, { withFileTypes: true })) {
    const rel = base ? `${base}/${e.name}` : e.name;
    const full = path.join(root, e.name);
    if (SKIP.test(full) || SKIP.test(`${full}/`)) continue;
    if (e.isDirectory()) out.push(...walk(full, rel));
    else out.push(rel);
  }
  return out;
}

const sha = (p) => crypto.createHash("md5").update(fs.readFileSync(p)).digest("hex");
const deepEq = (x, y) => {
  if (x === y) return true;
  if (typeof x !== typeof y) return false;
  if (x && y && typeof x === "object") {
    const kx = Object.keys(x).sort();
    const ky = Object.keys(y).sort();
    if (kx.join(",") !== ky.join(",")) return false;
    return kx.every((k) => deepEq(x[k], y[k]));
  }
  return false;
};

/** 语义等价判定：返回 true 表示「仅格式/顺序不同，语义一致」 */
function semanticallyEqual(rel, a, b) {
  if (rel.endsWith(".json")) {
    try {
      return deepEq(JSON.parse(fs.readFileSync(a, "utf8")), JSON.parse(fs.readFileSync(b, "utf8")));
    } catch {
      return false;
    }
  }
  if (rel === "src-tauri/Cargo.toml") {
    const parseDeps = (p) => {
      const t = fs.readFileSync(p, "utf8");
      const sec = t.split(/^\[/m).find((s) => s.startsWith("dependencies]")) || "";
      const m = {};
      for (const l of sec.split("\n")) {
        const x = l.match(/^\s*([A-Za-z0-9_-]+)\s*=/);
        if (x) m[x[1]] = l.trim();
      }
      const i = t.indexOf("windows-sys");
      const feats = i < 0 ? [] : (t.slice(i, t.indexOf("] }", i)).match(/"Win32_[^"]+"/g) || []).sort();
      return { m, feats };
    };
    const A = parseDeps(a);
    const B = parseDeps(b);
    return deepEq(A.m, B.m) && JSON.stringify(A.feats) === JSON.stringify(B.feats);
  }
  return false;
}

const refFiles = new Set(walk(refDir));
const tgtFiles = new Set(walk(tgtDir));

const onlyRef = [...refFiles].filter((f) => !tgtFiles.has(f)).sort();
const onlyTgt = [...tgtFiles].filter((f) => !refFiles.has(f)).sort();

const byteDiff = [];
const semanticOnly = [];
const regenerated = [];
// 由工具链自动重生成、本就不打补丁的文件（cargo 首次构建时会重写）
const REGENERATED = new Set(["src-tauri/Cargo.lock"]);
let same = 0;
for (const f of [...refFiles].filter((f) => tgtFiles.has(f))) {
  const a = path.join(refDir, f);
  const b = path.join(tgtDir, f);
  if (sha(a) === sha(b)) {
    same++;
    continue;
  }
  if (REGENERATED.has(f)) {
    regenerated.push(f);
    continue;
  }
  if (semanticallyEqual(f, a, b)) semanticOnly.push(f);
  else byteDiff.push(f);
}

console.log(`参考树文件: ${refFiles.size}   组装树文件: ${tgtFiles.size}`);
console.log(`  字节相同:   ${same}`);
console.log(`  仅格式/顺序不同(语义相同): ${semanticOnly.length}${semanticOnly.length ? "  → " + semanticOnly.slice(0, 8).join(", ") + (semanticOnly.length > 8 ? " ..." : "") : ""}`);
console.log(`  工具链自动重生成(可忽略): ${regenerated.length}${regenerated.length ? "  → " + regenerated.join(", ") : ""}`);
console.log(`  内容真不同: ${byteDiff.length}`);
console.log(`  仅参考树有: ${onlyRef.length}`);
console.log(`  仅组装树有: ${onlyTgt.length}`);

if (!quiet) {
  if (byteDiff.length) {
    console.log("\n  --- 内容真不同 ---");
    for (const f of byteDiff.slice(0, 50)) console.log(`    ${f}`);
  }
  if (onlyRef.length) {
    console.log("\n  --- 仅参考树有 ---");
    for (const f of onlyRef.slice(0, 30)) console.log(`    ${f}`);
  }
  if (onlyTgt.length) {
    console.log("\n  --- 仅组装树有 ---");
    for (const f of onlyTgt.slice(0, 30)) console.log(`    ${f}`);
  }
}

// 退出码：有「真差异」或文件集合不一致 → 1
process.exit(byteDiff.length === 0 && onlyRef.length === 0 && onlyTgt.length === 0 ? 0 : 1);
