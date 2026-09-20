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
  // 这两条确实不进 JSON：`QrFailure` 实现了 error（对外走 `Error()` 文案），
  // `QrProgress` 是内存里的推进结果（`UserInfo any` 已经是解开的 JSON 值）。
  //
  // ⚠ 后者那条理由**带一个前提**：一旦哪天要把扫码进度直接推给前端
  // （比如 WebUI 想原样展示"已扫码，请在手机上确认"），正确做法是
  // 在 app 层**另写一个带 tag 的响应体**，而不是给 `QrProgress` 补 tag ——
  // `UserInfo any` 的形状由上游决定，把它钉进响应体等于把上游的字段约定
  // 写进我们自己的契约。
  'internal/capabilities/login.go:QrFailure':
    '实现 error 接口，对外走 `Error()` 文案，不整块 marshal',
  'internal/capabilities/login.go:QrProgress':
    '扫码登录的**内存**推进结果（`UserInfo any` 已是解开的 JSON 值），本身不是响应体',
  'internal/upstream/app_signin_status.go:SignInDay':
    'infoV3 日历格里的一天（`cal[]`），内存解析结果；Rust 侧 derive `Debug, Clone, Copy, PartialEq, Eq`，**不** Serialize',
  'internal/upstream/app_signin_status.go:ReceiveItem':
    '账目里的一笔（`receiveList[]`），内存解析结果；它要进 JSON 时走 `mcloud-app` 手工拼的响应体，不整块 marshal',
  'internal/upstream/app_signin_status.go:SignInStatus':
    'infoV3 的**整份解析结果**（余额 / 日历 / 账目 / 两个翻倍门槛），同上：Rust 侧不 Serialize，对外由 app 层另拼',
  'internal/upstream/app_signin_signin.go:SignInResult':
    'startSignIn 的解析结果（**调用前**的快照），同上；⚠ 它**不是**成败判据，别把它当成对外结论序列化出去',
  'internal/upstream/app_signin_session.go:H5Signature':
    '一次 H5 请求的**四个签名头**（request-id / timestamp / nonce / signature），是内存里的计算结果，不落库也不出响应',
  'internal/upstream/app_signin_session.go:H5Login':
    'tyrzLogin 换回的**会话凭据**（jwt + 回显手机号），进程内存里用；Rust 侧 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/upstream/smid.go:SmUpload':
    '一次上报的**时间与上一次的 deviceId**。时间由调用方给（东八区），旧 deviceId 只在本次请求里用；Rust 侧同样是纯入参结构，不 Serialize',
  'internal/upstream/smid.go:SmRule':
    'DES 规则表的**一条**（密钥 + 混淆后的键名），是常量表的元素不是数据模型；Rust 侧这条 rule 表也是 `const` 不是 Serialize',
  'internal/upstream/app_login.go:AppSendSmsOutcome':
    '一次发码的**判定结果**（分类 + 配额余量 / captchaType / code+msg），进程内存里用；Rust 侧是 `enum`，**不** Serialize',
  'internal/upstream/app_login.go:AppLoginOutcome':
    '一次 App 登录的**判定结果**（分类 + `data` 密文原文 / code+msg），进程内存里用；Rust 侧是 `enum`，**不** Serialize',
  // 领取族与流水族的**解析结果**：由 `ParseXxx(envelope)` 手工从泛型 payload 里取值
  // 构造（`RequireInt64(payload, "receive")` / `payload["records"]`），
  // **不是** `json.Unmarshal` 的目标 —— 字段名因此不参与任何解析，
  // 加 tag 只会让人以为"字段名是这里的解析键"。
  //
  // ⚠ 理由的边界：若哪天要把它们**序列化**给前端（比如 CLI `--json` 直接吐），
  // 正确做法是补上 tag 并从这里删掉 —— 而不是继续待在名单里。
  // 那时字段名就成了对外契约，要由 `test/golden/enum-json-vectors.json` 那类
  // serde 产出的向量来定，不能凭手感写。
  'internal/upstream/app_signin_receive.go:ReceiveResult':
    'receiveV3 的解析结果（`RequireInt64(payload, "receive")`），手工构造，不做反序列化',
  'internal/upstream/app_signin_receive.go:RevivalReward':
    'receiveRevivalReward 的解析结果（用掉一张卡换回的云朵），同上',
  'internal/upstream/app_signin_receive.go:ReceiveTaskExpansion':
    'receiveTaskExpansion 的解析结果（月度任务奖励），同上',
  'internal/upstream/app_signin_receive.go:MultipleResult':
    'multiple 的解析结果（本次额外领到的豆），同上',
  'internal/upstream/app_signin_records.go:CloudRecords':
    'cloudRecordV2 的解析结果（一页流水 + 分页总数），手工构造，不做反序列化',
  'internal/upstream/app_signin_records.go:PrizeRecordItem':
    '奖品流水里的一条，同上',
  'internal/upstream/app_signin_records.go:PrizeRecords':
    '奖品流水的整份解析结果（条目 + 分页），同上',
  'internal/upstream/app_signin_records.go:BackupGift':
    '备用礼包的状态解析结果，同上',
  'internal/upstream/app_signin_records.go:BackupGiftReceive':
    '领取备用礼包的结果解析结果，同上',
  'internal/core/auth.go:AuthInfo':
    'Rust 侧只 derive `Clone, PartialEq`——**刻意不** Serialize（里面是 token）',
  'internal/core/crypto.go:SignSaltPair':
    '签名所需的 (sign, salt) 对，由 crypto 的返回值组装，不整块落库',
  'internal/core/stage.go:StageError':
    'Rust 侧 derive `Debug, Clone, PartialEq, Eq, thiserror::Error`，**不** Serialize；对外文案走 `Error()`，不整块 marshal',
  'internal/core/legacy.go:LegacyError':
    'Rust 侧 derive `Debug, Clone, PartialEq, Eq, thiserror::Error`，**不** Serialize；对外文案走 `Error()`',
  'internal/core/legacy.go:ParseBad':
    '解析失败的**报告条目**（下标 + 信息），进迁移报告而非 JSON',
  'internal/core/legacy.go:LegacyAccountView':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 迁移期的**中间派生结果**，不落库也不出响应',
  'internal/core/legacy.go:LegacyTarget':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 描述"旧版会怎么做"的中间结果',
  'internal/core/legacy.go:ReferenceAdvice':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 迁移诊断，交给报告而不是 JSON',
  'internal/capabilities/registry.go:Capability':
    'Rust 侧**没有** derive `Serialize`：对外形状由 `mcloud-app::payload` 手工拼装（camelCase，与 serde 的 snake_case 默认不同）。刻意不加 tag —— 加了会让人以为它序列化出来就是给前端的那个形状',
  'internal/exec/exec.go:Concurrency':
    '并发闸**配置**（全局上限），进程内存里用，不进 JSON',
  'internal/exec/exec.go:Admission':
    '一次 run 准入的**判定结果**（Kind + 重复的 runId / 被挡的键与占用者），进程内存里用，不进 JSON；Rust 侧是 enum，无 Serialize',
  'internal/exec/exec.go:Delivery':
    '事件总线的一次**投递记录**，进程内存里用，不进 JSON',
  'internal/upstream/login.go:SliderPuzzle':
    '一次滑块挑战的**内存表示**（两张图 + 加密的缺口坐标）；Rust 侧 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/upstream/login.go:PollOutcome':
    '一次扫码轮询的**判定结果**（Go 无 sum type，用"类别 + 字段"近似），进程内存里用，不进 JSON',
  'internal/upstream/login.go:SendSmsOutcome':
    '一次发码响应的**判定结果**，同上；Rust 侧是 enum，无 Serialize',
  'internal/app/app.go:Deps': '组装根的**依赖集**，不是数据模型',
  'internal/server/server.go:Deps': '组装根的**依赖集**，不是数据模型',
  // `imports` 表的**一行**。字段名对应 SQL 列名，不是 JSON 键 ——
  // 它由 `scanImport` 逐列读出来，从不经过 `json.Unmarshal`。
  // 对外展示走 `Summary()`（刻意不含 `Raw`，那段可能带凭据）。
  // ⚠ 若哪天要把导入台账吐给前端（比如 CLI `--json` 列导入历史），
  // 正确做法是**另写一个带 tag 的响应体**并从这里删掉 ——
  // 它现在是"数据库的形状"，与"响应的形状"是两件事。
  'internal/io/configstore.go:ImportRecord':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq`（**不** Serialize）—— 它是 imports 表的行，字段名对应 SQL 列名',
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
