#!/usr/bin/env node
/**
 * 锚点重放引擎 —— 补丁层对抗「官方重构」的核心机制
 *
 * 为什么需要它
 * ------------
 * `patches/*.patch` 是**行上下文补丁**：官方一旦重构到补丁上下文附近，`git apply` 就打不上。
 * 我们以前的兜底是「整个文件取一边」，结果把官方的更新静默丢掉（见方案文档第十二节）。
 *
 * 但我们的很多改动其实是**加性**的（加 `pub(crate)`、加结构体字段、加一行调用、加个菜单项）——
 * 这些根本不依赖上下文，只依赖**锚点**（函数名 / 字段名 / 固定语句）还在。
 * 把这类改动写成 `anchors/*.json`，官方随便重构，只要锚点还在就能贴上去。
 *
 * 铁律：**找不到锚点 = 报错退出**。任何 `expect` 对不上都算失败，绝不静默跳过或半途而废。
 *
 * 用法
 * ----
 *   node replay-anchors.mjs <目标树目录> [--anchors <锚点目录>] [--check]
 *
 *   --check   只检查锚点是否命中，不写文件
 */
import fs from 'node:fs';
import path from 'node:path';
import url from 'node:url';

const here = path.dirname(url.fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);

const flag = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const has = (name) => argv.includes(name);

const targetDir = argv[0];
if (!targetDir || targetDir.startsWith('--')) {
  console.error('用法: node replay-anchors.mjs <目标树目录> [--anchors <锚点目录>] [--check]');
  process.exit(2);
}
const anchorsDir = flag('--anchors', path.join(here, '..', 'anchors'));
const checkOnly = has('--check');

if (!fs.existsSync(anchorsDir)) {
  console.log(`锚点目录不存在，跳过：${anchorsDir}`);
  process.exit(0);
}

const files = fs.readdirSync(anchorsDir).filter((f) => f.endsWith('.json')).sort();
if (!files.length) {
  console.log('锚点目录为空，跳过');
  process.exit(0);
}

let failed = 0;
let applied = 0;
let skipped = 0;
let conditionSkipped = 0;

for (const f of files) {
  const spec = JSON.parse(fs.readFileSync(path.join(anchorsDir, f), 'utf8'));
  const target = path.join(targetDir, ...spec.file.split('/'));
  console.log(`\n[${f}] ${spec.file}${spec.topic ? `  —— ${spec.topic}` : ''}`);

  // ── 条件锚点（先判条件，再判文件存在）──
  // 官方某些版本改了接口签名，我们的 overlay 得跟着适配（如 v3.20.3 起
  // `tray::format_usage_suffix` 的第 1 个参数从 &AppState 变成 &UsageCache）。
  // 判据用「官方某个文件里有没有某段文本」而不是版本号 ——
  // 版本号只是线索，内容才是事实；内容判据跨版本自动正确，也不会随打 tag 的时机出错。
  if (spec.when) {
    const wf = path.join(targetDir, ...spec.when.file.split('/'));
    const wtext = fs.existsSync(wf) ? fs.readFileSync(wf, 'utf8') : '';
    if (!wtext.includes(spec.when.contains)) {
      console.log(`  – 条件不满足，整份跳过（${spec.when.file} 里没有判据文本）`);
      conditionSkipped++;
      continue;
    }
    console.log(`  ✓ 条件满足（${spec.when.file} 里找到判据文本）`);
  }

  if (!fs.existsSync(target)) {
    console.error(`  ✗ 目标文件不存在：${target}`);
    failed++;
    continue;
  }

  let src = fs.readFileSync(target, 'utf8');
  const before = src;

  for (const e of spec.edits ?? []) {
    const want = e.expect ?? 1;
    const gotFind = src.split(e.find).length - 1;
    const gotRepl = src.split(e.replace).length - 1;

    // 幂等：若「目标状态」已经在了（行补丁已经改过 / 之前跑过一次），直接跳过。
    // 这样锚点重放可以无条件地在 patches 之后再跑一次，不会重复插入。
    if (gotRepl === want) {
      skipped++;
      console.log(`  = 已是目标状态，跳过：${e.why ?? e.find.slice(0, 50)}`);
      continue;
    }

    if (gotFind !== want) {
      console.error(
        `  ✗ 锚点未命中（期望 ${want} 处，实际 ${gotFind} 处）` +
          `\n      why: ${e.why ?? '(未说明)'}` +
          `\n      find: ${JSON.stringify(e.find.slice(0, 90))}`,
      );
      failed++;
      continue;
    }
    src = src.split(e.find).join(e.replace);
    applied++;
    console.log(`  ✓ ${e.why ?? e.find.slice(0, 50)} (${gotFind})`);
  }

  if (!checkOnly && src !== before) {
    fs.writeFileSync(target, src);
    console.log(`  → 已写入 ${spec.file}`);
  }
}

console.log('');
if (failed) {
  console.error(`✗ ${failed} 个锚点失败 —— 官方大概率改了这些位置，需要人工按锚点重贴（不要整文件覆盖！）`);
  process.exit(1);
}
console.log(
  `✓ 全部锚点命中（新应用 ${applied} 处，已是目标状态跳过 ${skipped} 处` +
    `${conditionSkipped ? `，条件不满足整份跳过 ${conditionSkipped} 份` : ''}）`,
);
