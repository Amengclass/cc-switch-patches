#!/usr/bin/env node
/**
 * 修正「CRLF 文件」的补丁换行
 *
 * 问题
 * ----
 * `git diff` **会吞掉行尾的 CR**（已用最小复现验证），所以生成出来的补丁永远是 LF-only。
 * 但官方有些文件是以 CRLF 提交的（`.gitignore`、`.gitattributes`、`LICENSE`、
 * `tailwind.config.cjs` …）。在**字节保真**模式下（`git apply -c core.autocrlf=false`），
 * LF 补丁 vs CRLF 目标 → 补丁打不上；而如果放开 autocrlf，git 会把**所有**被打补丁的
 * 文件写成 CRLF，把本来 LF 的文件全污染（实测一次污染了 115 个文件）。
 *
 * 做法
 * ----
 * 对每个补丁段：如果官方那边这个文件是 CRLF，就给该段的**内容行**（hunk 内以 空格/-/+ 开头的行）
 * 补上行尾 `\r`。这样补丁与目标逐字节匹配，应用后新增行也是 CRLF，结果与我们源码一致。
 *
 * 用法: node fix-crlf-patches.mjs <官方源码树> <补丁目录>
 */
import fs from 'node:fs';
import path from 'node:path';

const [officialDir, patchesDir] = process.argv.slice(2);
if (!officialDir || !patchesDir) {
  console.error('用法: node fix-crlf-patches.mjs <官方源码树> <补丁目录>');
  process.exit(2);
}

/** 该路径在官方树里是否 CRLF（只要有 CRLF 且 CRLF 数 == LF 数即认为是 CRLF 文件） */
const crlfCache = new Map();
function isCrlf(rel) {
  if (crlfCache.has(rel)) return crlfCache.get(rel);
  const p = path.join(officialDir, ...rel.split('/'));
  let v = false;
  if (fs.existsSync(p)) {
    const b = fs.readFileSync(p);
    let crlf = 0;
    let lf = 0;
    for (let i = 0; i < b.length; i++) {
      if (b[i] === 10) {
        lf++;
        if (i > 0 && b[i - 1] === 13) crlf++;
      }
    }
    v = crlf > 0 && crlf === lf;
  }
  crlfCache.set(rel, v);
  return v;
}

let patched = 0;
for (const name of fs.readdirSync(patchesDir).filter((f) => f.endsWith('.patch'))) {
  const file = path.join(patchesDir, name);
  const src = fs.readFileSync(file, 'utf8');
  const lines = src.split('\n');
  const out = [];
  let currentRel = null; // 当前补丁段的文件
  let inHunk = false;
  let changed = 0;

  for (const line of lines) {
    if (line.startsWith('diff --git ')) {
      const m = line.match(/ b\/(.+)$/);
      currentRel = m ? m[1] : null;
      inHunk = false;
      out.push(line);
      continue;
    }
    if (line.startsWith('@@')) {
      inHunk = true;
      out.push(line);
      continue;
    }
    if (line.startsWith('--- ') || line.startsWith('+++ ') || line.startsWith('index ') || !inHunk) {
      out.push(line);
      continue;
    }
    // hunk 内容行：以 空格 / - / + 开头（以及 "\ No newline" 标记）
    if (line.startsWith('\\')) {
      out.push(line);
      continue;
    }
    if (currentRel && isCrlf(currentRel) && !line.endsWith('\r')) {
      out.push(line + '\r');
      changed++;
    } else {
      out.push(line);
    }
  }

  if (changed) {
    fs.writeFileSync(file, out.join('\n'));
    patched++;
    console.log(`  ✓ ${name}：${changed} 行补上 CR`);
  }
}

console.log(patched ? `\n✓ 修正了 ${patched} 个补丁` : '\n没有需要修正的补丁（官方那边没有 CRLF 的目标文件）');
