#!/usr/bin/env node
// 结构体字段的 `json:"…"` tag 门禁。
//
// # 为什么要有这条
//
// Rust 侧的 `#[derive(Serialize)]` 默认给 **snake_case** 字段名
// （`password_hash` / `cloud_count`），而 Go 不带 tag 会输出**字段名原样**
// （`PasswordHash` / `CloudCount`）。同一个字段两个名字，而且**不报错** ——
// 只有真跑起来、另一侧读这条记录的时候才看得见。
//
// 这个坑在 mcloud 里已经犯了**三次**，分属三个不同模块：
//   - `CloudStats`（pure 那一步带过来的）；
//   - `User` / `Session`（user 那一步带过来的）；
//   - `model` 里那一批（这次迁移时逐个补上的）。
//
// 三次的共同点是：**发现它们靠的是"改坏了却不响"，不是靠测试**。
// 往返测试用什么字段名都是绿的 —— 它只证明"自己解得回来"，
// 不证明"和另一侧同名"。所以必须有一条不依赖往返的守卫。
//
// # 判据
//
//   - **混合**（有些字段有 tag、有些没有）→ **直接失败**。
//     这是无歧义的错误：一个结构体不可能"一半进 JSON、一半不进"。
//   - **全都没有 tag** → 必须在下面的白名单里，否则**失败**。
//     零 tag 有两种可能：① 它确实不进 JSON（内部类型）；
//     ② 它整个漏写了。脚本分不出来，所以交给白名单 ——
//     **新增一个无 tag 的结构体必须显式登记并写明理由**，
//     这就是"让它自己报出来"的那一下。
//
// # ⚠ 白名单不是"批准"，是"待复查"
//
// 白名单里的每一条都写了理由。**理由过期了就要删**：
// 哪天 `AuthInfo` 开始要进 JSON，它的正确做法是**加上 tag 并从这里删掉**，
// 而不是继续待在名单里。
//
// 用法：node scripts/check-go-jsontags.cjs

'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');
const SCAN_DIRS = ['cmd', 'internal'];

// 白名单：`相对路径:结构体名` → 为什么不进 JSON。
//
// ⚠ 加条目时必须写理由。没有理由的条目等于"shenme 都没说"，
// 下一次有人看到它时无法判断该不该继续留着。
// ⚠ 只有"**有导出字段**、但一个 tag 都没有"的才需要登记。
// 全是非导出字段的结构体（`Error` / `enumJSON` / `phcParams` / `cronField` /
// `CronSchedule`）脚本会**直接跳过** —— `encoding/json` 本来就看不见它们，
// 登记进去反而会让白名单越来越长、越来越没人看。
const UNTAGGED_ALLOWED = {
  'internal/core/auth.go:AuthInfo':
    'Rust 侧只 derive `Clone, PartialEq`——**刻意不** Serialize（里面是 token）',
  'internal/core/crypto.go:SignSaltPair':
    '签名所需的 (sign, salt) 对，由 crypto 的返回值组装，不整块落库',
  'internal/core/stage.go:StageError':
    'Rust 侧 derive `Debug, Clone, PartialEq, Eq, thiserror::Error`，**不** Serialize；对外文案走 `Error()`，不整块 marshal',
  'internal/app/app.go:Deps': '组装根的**依赖集**，不是数据模型',
  'internal/server/server.go:Deps': '组装根的**依赖集**，不是数据模型',
};

function walk(dir, out = []) {
  const abs = path.join(ROOT, dir);
  if (!fs.existsSync(abs)) return out;
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    const rel = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      // ⚠ 只走 Go 源码目录：别进 testdata / target 之类。
      walk(rel, out);
    } else if (entry.name.endsWith('.go') && !entry.name.endsWith('_test.go')) {
      out.push(rel);
    }
  }
  return out;
}

// 抓 `type X struct {` … 到行首的 `}` 为止。
//
// ⚠ 只认"行首的 `}`"：Go 的 struct 声明结束时 `}` 一定在第 0 列。
// 内联的匿名 `struct{…}{…}`（marshalJSON 里那些）不以 `type` 开头，不会被抓到。
const STRUCT_RE = /^type ([A-Za-z_]\w*) struct \{$/;

function structsOf(src) {
  const lines = src.split('\n');
  const found = [];
  for (let i = 0; i < lines.length; i++) {
    const m = STRUCT_RE.exec(lines[i]);
    if (!m) continue;
    const fields = [];
    for (let j = i + 1; j < lines.length; j++) {
      if (lines[j] === '}') break;
      const body = lines[j].trim();
      if (!body || body.startsWith('//')) continue;
      fields.push(body);
    }
    found.push({ name: m[1], fields });
  }
  return found;
}

// 一个"字段行"的粗略判定：行首是大写字母 ⇒ 导出字段。
//
// ⚠ 排除只有**一个** token 的行 —— 那是嵌入字段（`Base`）或类型名，
// 不是"名字 + 类型"。
function isExportedField(line) {
  const head = line.split(/\s+/)[0];
  if (!head) return false;
  if (!/^[A-Z]/.test(head)) return false;
  return /\s/.test(line);
}

function main() {
  const files = SCAN_DIRS.flatMap((d) => walk(d));
  const mixed = [];
  const untaggedNotAllowed = [];
  const allowedSeen = new Set();
  let tagged = 0;

  for (const rel of files) {
    const src = fs.readFileSync(path.join(ROOT, rel), 'utf8');
    for (const st of structsOf(src)) {
      const exported = st.fields.filter(isExportedField);
      if (exported.length === 0) continue; // 全是非导出字段 ⇒ 不进 JSON，跳过
      const withTag = exported.filter((l) => l.includes('json:"'));
      const key = `${rel}:${st.name}`;

      if (withTag.length === exported.length) {
        tagged++;
        continue;
      }
      if (withTag.length > 0) {
        const missing = exported.filter((l) => !l.includes('json:"'));
        mixed.push({ key, missing });
        continue;
      }
      if (UNTAGGED_ALLOWED[key]) {
        allowedSeen.add(key);
      } else {
        untaggedNotAllowed.push(key);
      }
    }
  }

  let failed = false;

  if (mixed.length > 0) {
    failed = true;
    console.error('✗ 以下结构体**部分**字段缺 `json` tag —— 不可能"一半进 JSON"：\n');
    for (const m of mixed) {
      console.error(`  ${m.key}`);
      for (const f of m.missing) console.error(`      缺: ${f}`);
    }
    console.error('');
  }

  if (untaggedNotAllowed.length > 0) {
    failed = true;
    console.error('✗ 以下结构体**整个**没有 `json` tag，且不在白名单里：\n');
    for (const k of untaggedNotAllowed) console.error(`  ${k}`);
    console.error(
      '\n  若它确实不进 JSON，把它加进 scripts/check-go-jsontags.cjs 的' +
        ' UNTAGGED_ALLOWED 并**写明理由**；\n' +
        '  若它要进 JSON，就给它加 snake_case 的 tag —— ' +
        '字段名是外部契约，由 test/golden/enum-json-vectors.json（serde 产出）比对。\n',
    );
  }

  // 白名单里已经不存在的条目 = 该复查了（结构体改名 / 加上了 tag）。
  const stale = Object.keys(UNTAGGED_ALLOWED).filter((k) => !allowedSeen.has(k));
  if (stale.length > 0) {
    failed = true;
    console.error('✗ 白名单里有已经对不上的条目（结构体改名或已加上 tag）—— 请删掉：\n');
    for (const k of stale) console.error(`  ${k}`);
    console.error('');
  }

  if (failed) process.exit(1);

  console.log(
    `jsontags 通过：${tagged} 个结构体字段齐全，${allowedSeen.size} 个按白名单不进 JSON`,
  );
}

main();
