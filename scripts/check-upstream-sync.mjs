#!/usr/bin/env node
/**
 * 官方更新「零丢失」门禁
 *
 * 解决的问题
 * ----------
 * 升级官方时，最危险的错误不是冲突报错（那看得见），而是**静默丢掉官方的更新** ——
 * 比如「这个文件冲突了，整个文件取我们的版本吧」，官方在同文件里的新功能就这么没了。
 * 这个脚本把这件事变成机械可判定的。
 *
 * 判据
 * ----
 *   oldD = diff(官方旧版 → 我们的版本)      = 我们**打算**改掉的行
 *   newD = diff(官方新版 → 合并结果)        = 我们**实际**改掉的行
 *   officialAdded = 官方在 旧→新 之间新增的行
 *
 *   若 newD 里删掉了某行，而这行是**官方新增的**、且不在 oldD 里
 *   → 说明官方的新增被我们丢掉了 = **失败**
 *
 * 只有一种例外：**显式豁免**（--waivers）。每条豁免必须写明「等价改写成什么」。
 * 这样门禁既不被「等价改写」（如把官方测试适配到我们重构后的函数名）的噪音卡死，
 * 也不会漏掉真丢失。
 *
 * 用法
 * ----
 *   node check-upstream-sync.mjs --from <官方旧版树> --to <官方新版树> \
 *        --ours <我们的树> --merged <合并结果树> [--waivers <豁免.json>] [--files <清单>]
 */
import fs from 'node:fs';
import path from 'node:path';

const argv = process.argv.slice(2);
const opt = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};

const fromDir = opt('--from');
const toDir = opt('--to');
const oursDir = opt('--ours');
const mergedDir = opt('--merged');
const waiversFile = opt('--waivers');

if (!fromDir || !toDir || !oursDir || !mergedDir) {
  console.error(
    '用法: node check-upstream-sync.mjs --from <官方旧版树> --to <官方新版树> --ours <我们的树> --merged <合并结果树> [--waivers <豁免.json>]',
  );
  process.exit(2);
}

/**
 * 行频次表（多重集）。
 *
 * 为什么不用 git diff 的 +/- 行：价目表这类文件里有几百行**完全重复**的 `),` / `"10",`，
 * 我们在中间插几行后 git diff 会重新对齐，产出大量「假的删除」（那些行其实还在，只是挪了位置）。
 * 按**出现次数**比对与对齐无关，才是正确判据。
 */
function counts(file) {
  const m = new Map();
  for (const raw of fs.readFileSync(file, 'utf8').split('\n')) {
    const l = raw.trim();
    if (!l) continue;
    m.set(l, (m.get(l) ?? 0) + 1);
  }
  return m;
}

const waivers = waiversFile && fs.existsSync(waiversFile)
  ? JSON.parse(fs.readFileSync(waiversFile, 'utf8'))
  : {};

/* ── 不适用本门禁的文件 ──
 * 本门禁用**逐行文本**比对，所以两类文件天然不适用：
 *   1) 走「结构化合并」的文件（i18n / tauri.conf.json / Cargo.toml）——
 *      它们是按 key 合并的，文本形态本来就会变（缩进、顺序），由 compare-trees.mjs 做语义比对；
 *   2) cargo 重新生成的文件（Cargo.lock）。
 * 排除名单直接从补丁层自己的配置推导，避免两处配置漂移。
 */
const SCRIPT_DIR = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));
const structuredDir = path.join(SCRIPT_DIR, '..', 'structured');

const REGENERATED = [/^src-tauri\/Cargo\.lock$/];

function structuredExcludes() {
  const out = [];
  const cfg = path.join(structuredDir, 'config-overrides.json');
  if (fs.existsSync(cfg)) {
    for (const k of Object.keys(JSON.parse(fs.readFileSync(cfg, 'utf8')))) {
      out.push(new RegExp('^' + k.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/\//g, '\\/') + '$'));
    }
  }
  const i18n = path.join(structuredDir, 'i18n');
  if (fs.existsSync(i18n)) {
    out.push(/[/\\]i18n[/\\]locales[/\\].*\.json$/);
    out.push(/^src[/\\]locales[/\\].*\.json$/);
  }
  return out;
}

const EXCLUDE = [...REGENERATED, ...structuredExcludes(), ...argv.filter((a, i) => argv[i - 1] === '--exclude').map((r) => new RegExp(r))];

const isExcluded = (rel) => EXCLUDE.some((re) => re.test(rel));

/** 递归列出相对路径（跳过构建产物） */
const SKIP = new Set(['.git', 'node_modules', 'target', 'dist', 'gen']);
function walk(root, rel = '', out = []) {
  for (const e of fs.readdirSync(path.join(root, rel), { withFileTypes: true })) {
    if (SKIP.has(e.name)) continue;
    const r = rel ? `${rel}/${e.name}` : e.name;
    if (e.isDirectory()) walk(root, r, out);
    else out.push(r);
  }
  return out;
}

if (!fs.existsSync(oursDir)) {
  console.error(`✗ 我们的树不存在：${oursDir}`);
  process.exit(2);
}

const ours = walk(oursDir);
let failed = 0;
let checked = 0;
let waivedCount = 0;

for (const rel of ours) {
  const f = (...d) => path.join(...[d[0], ...rel.split('/')]);
  const pFrom = f(fromDir);
  const pTo = f(toDir);
  const pOurs = f(oursDir);
  const pMerged = f(mergedDir);

  // 只看四处都存在、且我们确实改过的文件
  if (![pFrom, pTo, pOurs, pMerged].every((p) => fs.existsSync(p))) continue;
  if (isExcluded(rel)) continue;

  const cFrom = counts(pFrom);
  const cOurs = counts(pOurs);

  // 只看我们确实改过的文件
  let touched = false;
  for (const [l, n] of cOurs) if ((cFrom.get(l) ?? 0) !== n) { touched = true; break; }
  if (!touched) for (const [l, n] of cFrom) if ((cOurs.get(l) ?? 0) !== n) { touched = true; break; }
  if (!touched) continue;

  checked++;
  const cTo = counts(pTo);
  const cMerged = counts(pMerged);
  const w = new Map((waivers[rel] ?? []).map((x) => [x.line, x.why]));

  const leaked = [];
  const waived = [];
  for (const [line, nTo] of cTo) {
    if (nTo <= (cFrom.get(line) ?? 0)) continue;      // 官方没新增这一行
    if ((cMerged.get(line) ?? 0) >= nTo) continue;    // 官方新增的出现次数都保住了
    if (w.has(line)) { waived.push(line); continue; } // 人工确认的等价改写
    leaked.push(`${line}  （官方 ${nTo} 处 → 合并后只剩 ${cMerged.get(line) ?? 0} 处）`);
  }
  waivedCount += waived.length;

  // 反向：我方新增的行是否落地
  const missingOurs = [];
  for (const [line, nOurs] of cOurs) {
    if (nOurs <= (cFrom.get(line) ?? 0)) continue;
    if ((cMerged.get(line) ?? 0) >= nOurs) continue;
    missingOurs.push(line);
  }

  if (leaked.length || missingOurs.length) {
    console.log(`\n${rel}`);
    for (const l of leaked) {
      failed++;
      console.log(`  ✗ 官方新增被丢掉: ${JSON.stringify(l.slice(0, 100))}`);
    }
    if (missingOurs.length) {
      console.log(`  ⚠ 我方新增行未原样落到结果（可能被官方重构掉了，人工确认）: ${missingOurs.length}`);
      for (const l of missingOurs.slice(0, 6)) console.log(`      · ${JSON.stringify(l.slice(0, 80))}`);
    }
  }
}

console.log('');
console.log(`检查了 ${checked} 个我们改动过的文件；豁免 ${waivedCount} 处。`);
if (failed) {
  console.error(`✗ 有 ${failed} 处官方更新被丢掉 —— 不要整文件覆盖，按锚点/三方合并重贴！`);
  process.exit(1);
}
console.log('✓ 官方更新零丢失');
