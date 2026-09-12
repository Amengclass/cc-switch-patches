#!/usr/bin/env node
// gen-structured.mjs — 生成 i18n 的结构化 delta（只含我们新增/改值的 key）
// 用法: node gen-structured.mjs <官方源码目录> <我们的仓库> <输出 structured 目录>
import fs from "node:fs";
import path from "node:path";

const [, , officialDir, ourDir, outDir] = process.argv;
if (!officialDir || !ourDir || !outDir) {
  console.error("用法: node gen-structured.mjs <官方源码目录> <我们的仓库> <输出 structured 目录>");
  process.exit(1);
}

const LOCALES = ["zh", "en", "ja", "zh-TW"];

/** 返回 ours 相对 base 的「新增 + 改值」delta（递归，只保留叶子差异） */
function deepDiff(base, ours, trail = "") {
  const out = {};
  for (const [k, v] of Object.entries(ours)) {
    const p = trail ? `${trail}.${k}` : k;
    if (!(k in base)) {
      out[k] = v;
      continue;
    }
    const bv = base[k];
    const bothObj =
      v && typeof v === "object" && !Array.isArray(v) &&
      bv && typeof bv === "object" && !Array.isArray(bv);
    if (bothObj) {
      const d = deepDiff(bv, v, p);
      if (Object.keys(d).length) out[k] = d;
    } else if (JSON.stringify(v) !== JSON.stringify(bv)) {
      out[k] = v;
    }
  }
  return out;
}

/** 找出 base 有、ours 没有的 key（我们删掉的官方 key，需人工确认） */
function missingKeys(base, ours, trail = "") {
  const out = [];
  for (const [k, v] of Object.entries(base)) {
    const p = trail ? `${trail}.${k}` : k;
    if (!(k in ours)) {
      out.push(p);
      continue;
    }
    const ov = ours[k];
    const bothObj =
      v && typeof v === "object" && !Array.isArray(v) &&
      ov && typeof ov === "object" && !Array.isArray(ov);
    if (bothObj) out.push(...missingKeys(v, ov, p));
  }
  return out;
}

function countLeaves(o) {
  let n = 0;
  for (const v of Object.values(o)) {
    if (v && typeof v === "object" && !Array.isArray(v)) n += countLeaves(v);
    else n += 1;
  }
  return n;
}

const i18nOut = path.join(outDir, "i18n");
fs.mkdirSync(i18nOut, { recursive: true });

let totalAdded = 0;
const allMissing = [];

for (const loc of LOCALES) {
  const basePath = path.join(officialDir, "src", "i18n", "locales", `${loc}.json`);
  const ourPath = path.join(ourDir, "src", "i18n", "locales", `${loc}.json`);
  if (!fs.existsSync(basePath) || !fs.existsSync(ourPath)) {
    console.warn(`  [跳过] ${loc}.json (缺失)`);
    continue;
  }
  const base = JSON.parse(fs.readFileSync(basePath, "utf8"));
  const ours = JSON.parse(fs.readFileSync(ourPath, "utf8"));
  const delta = deepDiff(base, ours);
  const miss = missingKeys(base, ours);
  const n = countLeaves(delta);
  totalAdded += n;
  allMissing.push(...miss.map((m) => `${loc}: ${m}`));
  fs.writeFileSync(path.join(i18nOut, `${loc}.json`), JSON.stringify(delta, null, 2) + "\n", "utf8");
  console.log(`  [生成] i18n/${loc}.json  (新增/改值叶子 ${n} 个${miss.length ? `, 缺失官方 key ${miss.length} 个` : ""})`);
}

console.log(`\n  i18n delta 合计: ${totalAdded} 个叶子`);
if (allMissing.length) {
  console.log(`  ⚠ 我们这边「缺失」的官方 key（${allMissing.length} 个，需人工确认是否有意为之）:`);
  for (const m of allMissing.slice(0, 40)) console.log(`     ${m}`);
  if (allMissing.length > 40) console.log(`     ... 还有 ${allMissing.length - 40} 个`);
}
