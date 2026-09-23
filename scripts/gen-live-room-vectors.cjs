#!/usr/bin/env node
/**
 * 生成「直播 / 小红花」黄金向量（test/golden/live-room-vectors.json）。
 *
 * # 为什么需要它
 *
 * 这条链路里有三段**纯计算**，它们的正确性不能靠"读起来对"：
 *
 *   1. `member/apply` 的两个签名头（`systemMd5` / `tokenMd5`）；
 *   2. MSync 的 **protobuf wire 字节**（Go 侧要手写编解码，没有 codegen）；
 *   3. 弹幕口令提取与小红书主页抽取（一串正则与分支顺序）。
 *
 * 三段的**参考实现都在原项目里**（`packages/core/src/apis/live-room-chat.ts`、
 * `live-room/proto.ts`、`live-room/kouling.ts`、`live-room/xiaohongshu.ts`），
 * 所以本脚本不是"再写一遍算法"，而是**把原实现的输出固化成可断言的数据** ——
 * 与 `gen-crypto-vectors.cjs` 用原版 crypto-js 复算是同一条路子：
 * 预期的字节不是手算的，是原实现真跑出来的。
 *
 * # 为什么 MSync 要逐字节钉
 *
 * Go 侧手写 proto3 编码器，最容易错的恰好是**看不出错**的地方：
 *   - `repeated` 标量在 proto3 里**默认 packed**（wire type 2），
 *     写成非 packed（wire type 0 重复）时 protobufjs 照样解得开 ——
 *     只有逐字节比才知道不一样；
 *   - proto3 的 singular 字段取默认值（0 / "" / 空 bytes）时**整条不写**，
 *     而 `repeated` 里的 0 **要写**（`encryptType=[0]` 就是这一支）。
 * 两条都不会让"解回来对得上"这个弱断言变红。
 *
 * # 用法
 *
 * 需要一个能 require 到 **protobufjs** 的 node_modules（原项目那份）：
 *
 *   NODE_PATH=/Users/catlair/Documents/workspace/mcloud-sign/node_modules \
 *     node scripts/gen-live-room-vectors.cjs
 *
 * 另外**必须**能跑到原项目的 **TS 源码**（`packages/core/src/live-room/kouling.ts`
 * 与 `xiaohongshu.ts`）—— 口令与小红书两段的期望值是**跑原实现**得到的，
 * 不是手写的。跑法是用**原项目自己的 `tsx`**（见 [runOriginal]）。
 * 原项目路径取 `MCLOUD_SIGN_SRC`（与 `gen-auth-vectors.cjs` 同一个约定），
 * 默认 `/Users/catlair/Documents/workspace/mcloud-sign`。
 *
 * ⚠⚠ **基准是 `src/`，不是 `packages/core/dist/`** —— 理由见 [runOriginal] 的注释：
 * 那份编译产物**已经过期过**（dist 2026-08-01 / src 2026-08-26，那一版缺
 * `isQuestion` 过滤），拿它当基准会把**正确的** Go 实现判成红。
 * 〔本段原先写的是"必须能 require 到编译产物"，是废弃做法留下的残迹 ——
 * 它与本文件 `$evidence` 里那句"走 `packages/core/**src**` 的 TS 源码"自相矛盾。〕
 *
 * ⚠⚠ 这条"必须真跑原实现"不是洁癖：第一版本脚本把这两段的 `expect` **手写**了，
 * 结果 34 条里有 3 条是"我以为的行为"，与真实现不符
 * （`小红花口令：AAA 无随机` 真值是两个口令、小红书那条真值是 `A B`）。
 * 手写的期望值一旦进了黄金向量，**就成了一条假基准** —— 它会把
 * "Go 侧照抄了原实现"判成红，或者更糟：把"Go 侧抄错了"判成绿。
 * 所以下面 [runOriginal] 找不到原实现时**直接退出**，不退回手写值。
 *
 * ⚠ 本文件不含任何账号凭据：token / phone / 设备串一律是**合成值**
 * （手机号以 `199` 开头，见 scripts/check-identifiers.cjs）。
 */

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const nodeCrypto = require('node:crypto')
const childProcess = require('node:child_process')
const protobuf = require('protobufjs')

// ───────────────────── 原项目的**源码**（口令 / 小红书） ─────────────────────
// ⚠ 是 `packages/core/src` 的 TS，不是 `dist` 的编译产物 —— 理由见 [runOriginal]。

// 原项目路径沿用既有脚本的约定（`gen-auth-vectors.cjs` / `gen-legacy-fixtures.cjs`
// 用的是 `MCLOUD_SIGN_SRC`）—— 不另立一个环境变量名。
const SIGN_DIR = process.env.MCLOUD_SIGN_SRC || '/Users/catlair/Documents/workspace/mcloud-sign'
const CORE_SRC = path.join(SIGN_DIR, 'packages', 'core', 'src')
const TSX_BIN = path.join(SIGN_DIR, 'node_modules', '.bin', 'tsx')

/** 记录用到的原实现文件与其 sha256（基准是哪一份源码，可复核）。 */
const originalFiles = {}

function sha256(file) {
  return nodeCrypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')
}

/**
 * 用**原项目的源码**跑一批输入。
 *
 * ⚠⚠ 为什么不用 `packages/core/dist`：**编译产物会过期，而这次真的过期了**
 * （dist 2026-08-01 / src 2026-08-26）—— 那一版缺 `isQuestion` 那层过滤，
 * 于是 9 条"观众提问"用例的期望值全变成"**应该**解出口令"。
 * 拿它当基准的后果不是"多跑一次"，而是**把正确的 Go 实现判成红**，
 * 然后逼着人去"修"一个本来没错的地方。
 *
 * 所以基准取 **src**：用原项目自己的 `tsx` 直接跑 TS 源码
 * （原项目根 `package.json` 是 `"type": "module"`，tsx 顺带把 ESM 也处理了）。
 *
 * @param {{module: string, fn: string, inputs: string[]}[]} calls
 * @returns {string[][]} 每个 call 的输出（一组"口令数组"）
 */
function runOriginal(calls) {
  if (!fs.existsSync(TSX_BIN)) {
    throw new Error(
      `找不到原项目的 tsx：${TSX_BIN}\n` +
        `口令 / 小红书的期望值**必须**由原实现算出（见文件头）。\n` +
        `请确认原项目已安装依赖，或用 MCLOUD_SIGN_SRC 指定路径。`,
    )
  }

  const probe = calls
    .map((call, i) => {
      const file = path.join(CORE_SRC, call.module)
      if (!fs.existsSync(file)) throw new Error(`原实现不存在：${file}`)
      originalFiles[call.module] = { path: file, sha256: sha256(file) }
      return (
        `import { ${call.fn} as fn${i} } from ${JSON.stringify(file)}\n` +
        `out.push(cases[${i}].inputs.map((x) => fn${i}(x)))\n`
      )
    })
    .join('')

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-vectors-'))
  try {
    const probePath = path.join(dir, 'probe.mts')
    const casesPath = path.join(dir, 'cases.json')
    fs.writeFileSync(
      probePath,
      `import { readFileSync } from 'node:fs'\n` +
        `const cases = JSON.parse(readFileSync(${JSON.stringify(casesPath)}, 'utf8'))\n` +
        `const out: unknown[] = []\n` +
        probe +
        `process.stdout.write(JSON.stringify(out))\n`,
    )
    fs.writeFileSync(casesPath, JSON.stringify(calls.map((c) => ({ inputs: c.inputs }))))

    // cwd 设在原项目里：让 tsx 用上它自己的解析规则。
    const stdout = childProcess.execFileSync(TSX_BIN, [probePath], {
      cwd: SIGN_DIR,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    const parsed = JSON.parse(stdout)
    if (!Array.isArray(parsed) || parsed.length !== calls.length) {
      throw new Error(`原实现探针返回的形状不对：${stdout.slice(0, 200)}`)
    }
    return parsed
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

// ───────────────────────── 原项目的 PROTO_DEF（逐字） ─────────────────────────
//
// 来源：packages/core/src/live-room/proto.ts 的内联 schema。
// **一个字符都不能改**：改了就是另一套 wire 格式，而向量会跟着一起错。
const PROTO_DEF = `
syntax = "proto3";
package cmos.pb;

message JID {
  string appKey = 1;
  string name = 2;
  string domain = 3;
  string clientResource = 4;
}

message MessageBody {
  Type type = 1;
  JID from = 2;
  JID to = 3;
  repeated Content contents = 4;
  repeated KeyValue ext = 5;
  uint64 ackMessageId = 6;
  MessageConfig msgConfig = 7;
  string ackContent = 8;

  message Content {
    Type type = 1;
    string text = 2;
    double latitude = 3;
    double longitude = 4;
    string address = 5;
    string displayName = 6;
    string remotePath = 7;
    string secretKey = 8;
    int32 fileLength = 9;
    string action = 10;
    repeated KeyValue params = 11;
    int32 duration = 12;
    Size size = 13;
    string thumbnailRemotePath = 14;
    string thumbnailSecretKey = 15;
    string thumbnailDisplayName = 16;
    int32 thumbnailFileLength = 17;
    Size thumbnailSize = 18;
    message Size { double width = 1; double height = 2; }
    enum Type { TEXT=0; IMAGE=1; VIDEO=2; LOCATION=3; VOICE=4; FILE=5; COMMAND=6; CUSTOM=7; }
  }
  message MessageConfig { bool allowGroupAck = 1; }
  enum Type { NORMAL=0; CHAT=1; GROUPCHAT=2; CHATROOM=3; READ_ACK=4; DELIVER_ACK=5; RECALL=6; REMOVE=7; }
}

message KeyValue {
  string key = 1;
  ValueType type = 2;
  oneof value {
    uint64 varintValue = 3;
    float floatValue = 4;
    double doubleValue = 5;
    string stringValue = 6;
  }
  enum ValueType { BOOL=1; INT=2; UINT=3; LLINT=4; FLOAT=5; DOUBLE=6; STRING=7; JSON_STRING=8; }
}

message MSync {
  Version version = 1;
  JID guid = 2;
  string auth = 3;
  uint32 compressAlgorimth = 4;
  uint32 crypto = 5;
  string userAgent = 6;
  uint64 pov = 7;
  Command command = 8;
  bytes payload = 9;
  uint32 deviceId = 10;
  repeated EncryptType encryptType = 11;
  string encryptKey = 12;
  bytes deviceInfo = 13;
  bytes timestamp = 14;
  bytes identification = 15;
  string srcIdc = 16;
  string dstIdc = 17;
  repeated JID tos = 18;
  enum Version { MSYNC_V1=0; MSYNC_V2=1; }
  enum Command { SYNC=0; UNREAD=1; NOTICE=2; PROVISION=3; LOGOUT=4; HEARTBEAT=5; ROUTE=6; EVT_BROADCAST=7; }
}

enum EncryptType { ENCRYPT_NONE=0; ENCRYPT_AES_128_CBC=1; ENCRYPT_AES_256_CBC=2; }

message Provision {
  uint32 osType=1; string version=2; uint32 networkType=3; string appSign=4;
  repeated uint32 compressType=5; repeated uint32 encryptType=6;
  string encryptKey=7; Status status=8; string deviceUuid=9;
  bool isManualLogin=10; string password=11; string deviceName=12;
  string resource=13; string auth=14; string province_code=15;
}
message Status { uint32 errorCode=1; string reason=2; }

message CommSyncUL {
  Meta meta = 1;
  uint64 key = 2;
  JID queue = 3;
  bool isRoam = 4;
  uint64 lastFullRoamKey = 5;
}

// NOTICE 消息的 payload 结构: { queue: JID, meta: Meta }
message NoticePayload {
  JID queue = 1;
  Meta meta = 2;
}

message Meta {
  uint64 id = 1;
  JID from = 2;
  JID to = 3;
  uint64 timestamp = 4;
  NameSpace ns = 5;
  bytes payload = 6;
  RouteType routetype = 7;
  bool offStore = 8;
  string exts = 9;
  enum NameSpace { STATISTIC=0; CHAT=1; MUC=2; ROSTER=3; CONFERENCE=4; }
  enum RouteType { ROUTE_ALL=0; ROUTE_ONLINE=1; }
}
`

const root = protobuf.parse(PROTO_DEF).root
const MSyncType = root.lookupType('cmos.pb.MSync')
const JIDType = root.lookupType('cmos.pb.JID')
const ProvisionType = root.lookupType('cmos.pb.Provision')
const MessageBodyType = root.lookupType('cmos.pb.MessageBody')
const CommSyncULType = root.lookupType('cmos.pb.CommSyncUL')
const NoticePayloadType = root.lookupType('cmos.pb.NoticePayload')
const MetaType = root.lookupType('cmos.pb.Meta')

// ───────────────────────── 与 proto.ts 逐字对齐的常量 ─────────────────────────

const APP_KEY = 'cmos_js_sdk'
const LIVE_APP_KEY = 'cmos#ngvlcs'
const DOMAIN = 'cmosim.com'
const DEVICE_INFO_HEX =
  '830652084dda69b6b957e2c3a7bd9b18db65922e29051b1b342ccf15bebc0fdbcc96f61dadab12f3cab1e66cb28ec05b'
const USER_AGENT = 'Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15'

// 合成凭据：**不是**任何真实账号的值。
const ACCESS_TOKEN = 'A03CBHoAAAAA19900000000SYNTHETICTOKENFORVECTORS000000000000000000'
const PHONE = '19900000000'
const ROOM_ID = '2026072314302675201220000000000'

const hexToBuf = (hex) => Buffer.from(hex, 'hex')

/** 对齐 proto.ts: genTimestamp —— 8 字节**大端**。 */
function genTimestamp(millis) {
  const buf = Buffer.alloc(8)
  buf.writeBigUInt64BE(BigInt(millis))
  return buf
}

/** 对齐 proto.ts: genIdentification —— 固定前缀 `cmos_im` + 16 字节随机。 */
function genIdentification(seedHex) {
  return Buffer.concat([Buffer.from('cmos_im'), hexToBuf(seedHex)])
}

/**
 * 对齐 proto.ts: encodeMSync（**逐行照抄**，含字段是否出现的每一处判断）。
 *
 * ⚠ 与 proto.ts 的两处差别都是**为了可复现**，不是行为差异：
 * `resource` 不取 `Date.now()` 而由入参给；`timestamp` / `identification`
 * 由固定种子生成（原实现取当前时刻与随机字节）。
 */
function encodeMSync({
  accessToken,
  roomId,
  command,
  payload,
  resource,
  phone,
  timestampMillis,
  identificationHex,
}) {
  const isLiveRoom = !!phone
  const guid = JIDType.create({
    appKey: isLiveRoom ? LIVE_APP_KEY : APP_KEY,
    name: isLiveRoom ? `cmlive_c_${phone}` : `cmlive_${roomId}`,
    domain: DOMAIN,
    clientResource: resource,
  })
  const msyncData = {
    version: 0,
    guid,
    auth: `$t$${accessToken}`,
    crypto: 0,
    userAgent: USER_AGENT,
    pov: BigInt(0),
    command,
    deviceId: 0,
    encryptType: [0],
    timestamp: genTimestamp(timestampMillis),
    identification: genIdentification(identificationHex),
    deviceInfo: hexToBuf(DEVICE_INFO_HEX),
  }
  if (payload) msyncData.payload = payload
  return Buffer.from(MSyncType.encode(MSyncType.create(msyncData)).finish())
}

/** 对齐 proto.ts: createProvisionPayload（**逐行照抄**）。 */
function createProvisionPayload({ accessToken, resource, deviceUuid }) {
  return Buffer.from(
    ProvisionType.encode(
      ProvisionType.create({
        osType: 7,
        version: '23.0.2',
        networkType: 1,
        appSign: 'cmlive_cmos_js_sdk',
        compressType: [0],
        encryptType: [0],
        deviceUuid: String(deviceUuid),
        resource,
        auth: `$t$${accessToken}`,
        deviceName: 'webim',
        province_code: '',
      }),
    ).finish(),
  )
}

// ───────────────────────── member/apply 的两个签名头 ─────────────────────────

/** 对齐 live-room-chat.ts: applyRoom 的算式。 */
function applySignatures({ timestamp, accessToken, deviceInfoHex }) {
  const ts10 = timestamp.slice(-10)
  const systemMd5 = nodeCrypto
    .createHash('md5')
    .update(Buffer.concat([hexToBuf(deviceInfoHex), Buffer.from(ts10, 'utf8')]))
    .digest('hex')
  const tokenMd5 = nodeCrypto
    .createHash('md5')
    .update(`Bearer ${accessToken.slice(0, 10)}${ts10}${accessToken.slice(10)}`, 'utf8')
    .digest('hex')
  return { systemMd5, tokenMd5 }
}

// ───────────────────────── 向量 ─────────────────────────

const msyncFrames = []
const applySigs = []

// ① PROVISION（command=3）：唯一带 payload 的那一帧。
{
  const tsMillis = 1784790184674
  const resource = `js_${tsMillis}`
  const deviceUuid = String(tsMillis)
  const identificationHex = '000102030405060708090a0b0c0d0e0f'
  const payload = createProvisionPayload({ accessToken: ACCESS_TOKEN, resource, deviceUuid })
  const frame = encodeMSync({
    accessToken: ACCESS_TOKEN,
    roomId: ROOM_ID,
    command: 3,
    payload,
    resource,
    phone: PHONE,
    timestampMillis: tsMillis,
    identificationHex,
  })
  msyncFrames.push({
    name: 'provision',
    command: 3,
    timestampMillis: tsMillis,
    identificationHex,
    resource,
    deviceUuid,
    deviceInfoHex: DEVICE_INFO_HEX,
    base64: frame.toString('base64'),
    hex: frame.toString('hex'),
    payloadHex: payload.toString('hex'),
    // 解码侧要断言到的字段（Node 校验器自己解一遍、Go 测试解一遍）
    expect: {
      version: 0,
      guid: {
        appKey: LIVE_APP_KEY,
        name: `cmlive_c_${PHONE}`,
        domain: DOMAIN,
        clientResource: resource,
      },
      auth: `$t$${ACCESS_TOKEN}`,
      crypto: 0,
      userAgent: USER_AGENT,
      command: 3,
      deviceId: 0,
      encryptType: [0],
      deviceInfoHex: DEVICE_INFO_HEX,
      identificationHex: `636d6f735f696d${identificationHex}`,
    },
    expectProvision: {
      osType: 7,
      version: '23.0.2',
      networkType: 1,
      appSign: 'cmlive_cmos_js_sdk',
      compressType: [0],
      encryptType: [0],
      deviceUuid,
      deviceName: 'webim',
      resource,
      auth: `$t$${ACCESS_TOKEN}`,
      provinceCode: '',
    },
  })
}

// ② UNREAD（command=1）：PROVISION 响应之后必须补发的那一帧（无 payload）。
{
  const tsMillis = 1784790185000
  const resource = `js_${tsMillis}`
  const frame = encodeMSync({
    accessToken: ACCESS_TOKEN,
    roomId: ROOM_ID,
    command: 1,
    resource,
    phone: PHONE,
    timestampMillis: tsMillis,
    identificationHex: '101112131415161718191a1b1c1d1e1f',
  })
  msyncFrames.push({
    name: 'unread',
    command: 1,
    timestampMillis: tsMillis,
    identificationHex: '101112131415161718191a1b1c1d1e1f',
    resource,
    deviceInfoHex: DEVICE_INFO_HEX,
    base64: frame.toString('base64'),
    hex: frame.toString('hex'),
    payloadHex: '',
    expect: {
      version: 0,
      guid: {
        appKey: LIVE_APP_KEY,
        name: `cmlive_c_${PHONE}`,
        domain: DOMAIN,
        clientResource: resource,
      },
      auth: `$t$${ACCESS_TOKEN}`,
      crypto: 0,
      userAgent: USER_AGENT,
      command: 1,
      deviceId: 0,
      encryptType: [0],
      deviceInfoHex: DEVICE_INFO_HEX,
      identificationHex: `636d6f735f696d101112131415161718191a1b1c1d1e1f`,
    },
  })
}

// ③ 非直播形态（无 phone）—— 钉住 `appKey` / `name` 两个分支的差别。
{
  const tsMillis = 1784790186000
  const resource = `js_${tsMillis}`
  const frame = encodeMSync({
    accessToken: ACCESS_TOKEN,
    roomId: ROOM_ID,
    command: 0,
    resource,
    timestampMillis: tsMillis,
    identificationHex: '202122232425262728292a2b2c2d2e2f',
  })
  msyncFrames.push({
    name: 'sync-no-phone',
    command: 0,
    timestampMillis: tsMillis,
    identificationHex: '202122232425262728292a2b2c2d2e2f',
    resource,
    deviceInfoHex: DEVICE_INFO_HEX,
    base64: frame.toString('base64'),
    hex: frame.toString('hex'),
    payloadHex: '',
    expect: {
      version: 0,
      guid: {
        appKey: APP_KEY,
        name: `cmlive_${ROOM_ID}`,
        domain: DOMAIN,
        clientResource: resource,
      },
      auth: `$t$${ACCESS_TOKEN}`,
      crypto: 0,
      userAgent: USER_AGENT,
      command: 0,
      deviceId: 0,
      encryptType: [0],
      deviceInfoHex: DEVICE_INFO_HEX,
      identificationHex: `636d6f735f696d202122232425262728292a2b2c2d2e2f`,
    },
  })
}

// ④ 两个签名头：三条，其中两条刻意选在 `ts` 末 10 位含前导 0 的时刻。
for (const [name, timestamp] of [
  ['plain', '1784790184674'],
  ['leading-zero-in-last10', '1784700000001'],
  ['short-token', '1784790184674'],
]) {
  const token = name === 'short-token' ? 'A03SHORT' : ACCESS_TOKEN
  const sig = applySignatures({ timestamp, accessToken: token, deviceInfoHex: DEVICE_INFO_HEX })
  applySigs.push({
    name,
    timestamp,
    accessToken: token,
    deviceInfoHex: DEVICE_INFO_HEX,
    timestampSuffix10: timestamp.slice(-10),
    systemMd5: sig.systemMd5,
    tokenMd5: sig.tokenMd5,
  })
}

// ───────────────────────── 入站 payload：三种形态都要解出同一句话 ─────────────────────────
//
// 服务端推来的那一帧，原实现走的是**三级兜底**：
//   SYNC(0)   → CommSyncUL.meta.payload → MessageBody
//   NOTICE(2) → NoticePayload.meta.payload → MessageBody
//   兜底      → 直接把整段 payload 当 MessageBody
// 三条路都得能解出 `contents[0].text`。这里把**同一句话**按三种形态各编码一份，
// Go 侧的 `ExtractDanmakuText` 必须三条都命中 —— 少一条就是少一条兜底。

const DANMAKU_TEXT = '小红花口令：毕业之旅MClaw相伴'
const DANMAKU_TEXT2 = '口令1:MClaw全能小助手 口令2:毕业之旅MClaw相伴'

const messageBody = (text) =>
  MessageBodyType.encode(
    MessageBodyType.create({
      type: 2, // GROUPCHAT
      contents: [{ type: 0, text }], // Content.Type.TEXT
    }),
  ).finish()

const roomJID = JIDType.create({
  appKey: LIVE_APP_KEY,
  name: `cmlive_c_${PHONE}`,
  domain: DOMAIN,
  clientResource: `js_1784790184674`,
})
const roomTargetJID = JIDType.create({
  appKey: LIVE_APP_KEY,
  name: ROOM_ID,
  domain: DOMAIN,
  clientResource: `js_1784790184674`,
})

const syncPayload = Buffer.from(
  CommSyncULType.encode(
    CommSyncULType.create({
      meta: {
        id: 1995092351920434743n,
        from: roomJID,
        to: roomTargetJID,
        timestamp: 1784790184674n,
        ns: 1, // Meta.NameSpace.CHAT
        payload: messageBody(DANMAKU_TEXT),
      },
      key: 1n,
    }),
  ).finish(),
)

const noticePayload = Buffer.from(
  NoticePayloadType.encode(
    NoticePayloadType.create({
      queue: roomTargetJID,
      meta: {
        id: 1995092351920434744n,
        from: roomJID,
        to: roomTargetJID,
        timestamp: 1784790184700n,
        ns: 1,
        payload: messageBody(DANMAKU_TEXT2),
      },
    }),
  ).finish(),
)

const barePayload = Buffer.from(messageBody(DANMAKU_TEXT))

const msyncInbound = [
  {
    name: 'sync-comm-sync-ul',
    kind: 'sync',
    text: DANMAKU_TEXT,
    payloadBase64: syncPayload.toString('base64'),
    payloadHex: syncPayload.toString('hex'),
  },
  {
    name: 'notice-payload',
    kind: 'notice',
    text: DANMAKU_TEXT2,
    payloadBase64: noticePayload.toString('base64'),
    payloadHex: noticePayload.toString('hex'),
  },
  {
    name: 'bare-message-body',
    kind: 'bare',
    text: DANMAKU_TEXT,
    payloadBase64: barePayload.toString('base64'),
    payloadHex: barePayload.toString('hex'),
  },
]

// 用到的 protobufjs 类型清单 —— 少了任何一条，上面的编码会静默退化成空消息。
const protoTypesUsed = [
  MSyncType,
  JIDType,
  ProvisionType,
  MessageBodyType,
  CommSyncULType,
  NoticePayloadType,
  MetaType,
].map((t) => t.fullName.replace(/^\./, ''))

// ───────────────────────── 弹幕文本与解码结果 ─────────────────────────
//
// 输入全部来自实拍：`test/fixtures/http/直播wss.json` 里 13 片
// `imChatRoomHistoryMessage` 共 814 条弹幕中，命中「口令 / 小红花」的 38 条**唯一文本**
// （脱敏：无手机号、无凭据）。另有 14 条逐字搬自原项目 `__tests__/kouling.test.ts`。
//
// ⚠⚠ `expect` 一律由**原实现**算出（见下面 `originalKouling.extractKouling`），
// 这里只写 `why`（"为什么要有这条"）。**不要**往这里手写 expect —— 理由见文件头。

const koulingCases = [
  // —— 原项目单测（逐字搬）——
  { name: 'ts-basic', text: '小红花口令：云盘AI高效复盘', why: '格式 1 的基本形态' },
  {
    name: 'ts-multiline',
    text: '复制口令1：\n短漫上新免费看\n复制口令2：\n云盘AI高效复盘\n无随机口令',
    why: '格式 2：整行是头 ⇒ 取下一行。⚠ 这条曾因 Go 侧漏了"取下一行"这一支而解出空',
  },
  {
    name: 'ts-multiline-sameline',
    text: '复制口令1：短漫上新免费看\n复制口令2：云盘AI高效复盘\n无随机口令',
    why: '格式 2 的同名行形态（头与内容在同一行）',
  },
  {
    name: 'ts-single-multiline',
    text: '复制口令：\n云盘AI高效复盘',
    why: '⚠ 单个口令头 + 下一行：与 ts-multiline 同一支，但没有编号',
  },
  { name: 'ts-shi', text: '口令是云盘AI高效复盘', why: '格式 3' },
  { name: 'ts-question-1', text: '口令是什么？', why: '疑问句：问号' },
  { name: 'ts-question-2', text: '口令是什么', why: '疑问句：整条等于疑问词' },
  { name: 'ts-question-3', text: '口令是多少啊', why: '疑问句：去语气词后等于疑问词' },
  { name: 'ts-question-4', text: '口令是啥呢', why: '疑问句：同上，另一个词' },
  {
    name: 'ts-question-keeps-real',
    text: '口令是带着云盘去上学',
    why: '⚠ 疑问句过滤的**反向**边界：真口令不能误伤',
  },
  {
    name: 'ts-digit-format',
    text: '1:短漫上新免费看 2:云盘AI高效复盘 无随机',
    why: '格式 4 兜底（只在 ①②③ 全空时生效）',
  },
  { name: 'ts-empty', text: '', why: '空串' },
  { name: 'ts-dedupe', text: '小红花口令：AAA\n小红花口令：AAA', why: '保序去重' },
  { name: 'ts-english-colon', text: '小红花口令:云盘AI高效复盘', why: '半角冒号' },

  // —— 2026-09-17 实拍弹幕（唯一 38 条里的代表性样本）——
  { name: 'live-real-1', text: '小红花口令：毕业之旅MClaw相伴', why: '实拍：真实主播格式' },
  {
    name: 'live-real-2',
    text: '口令1:MClaw全能小助手 口令2:毕业之旅MClaw相伴',
    why: '实拍：同一行两个口令（头之间以空格分隔）',
  },
  { name: 'live-real-3', text: '口令2：毕业之旅MClaw相伴', why: '实拍：单个编号口令' },
  { name: 'live-real-4', text: '口令1：MClaw全能小助手', why: '实拍：同上，另一个编号' },
  // ⚠ 这一条钉住一个**已知的假阳性**：格式 3 只要求「口令是」开头，
  //    所以「口令是相伴什么」会解出 `相伴什么`。原实现如此，服务端回 200108。
  //    照抄不"顺手修好" —— 修了就不再是同一份行为，而重试机制正是为它准备的。
  { name: 'live-false-positive', text: '口令是相伴什么', why: '⚠ 已知假阳性，钉成行为契约' },
  // 下面这批**都不能**被解出口令（观众的提问弹幕）。
  {
    name: 'live-question-1',
    text: '口令在哪里',
    why: '疑问句（"哪里"不在疑问词表里，靠整条不等判掉）',
  },
  { name: 'live-question-2', text: '口令是什么', why: '疑问句' },
  { name: 'live-question-3', text: '口令是啥', why: '疑问句' },
  { name: 'live-question-4', text: '口令啥啊', why: '疑问句' },
  { name: 'live-question-5', text: '口令二是什么', why: '疑问句（带汉字编号）' },
  { name: 'live-question-6', text: '口令是什么啊', why: '疑问句' },
  { name: 'live-question-7', text: '第2个口令是什么？', why: '疑问句（问号）' },
  { name: 'live-question-8', text: '场控随机口令是多少', why: '疑问句（去语气词后等于"多少"）' },
  { name: 'live-question-9', text: '怎么复制口令', why: '无口令头、无"口令是"' },
  { name: 'live-question-10', text: '口令兑换不了', why: '同上' },
  { name: 'live-bare', text: '口令', why: '裸关键词' },
  { name: 'live-no-random', text: '没有随机口令', why: '含"口令"但解不出（也不含"口令是"）' },
  { name: 'live-only-this-one', text: '只有这一个口令', why: '同上' },
  // 「无随机」兜底那支（格式 4）只在 ①②③ 全空时才生效。
  // ⚠ 原实现在这里给出**两个**口令（格式 1 收下整串、同一行切分再收一个）——
  //   手写期望值的第一版脚本把它记成 `['AAA']`，与真实现不符，已改为真跑。
  {
    name: 'live-no-random-with-head',
    text: '小红花口令：AAA 无随机',
    why: '⚠ 格式 1 与同一行切分**同时**命中（两个结果）',
  },
  // 全角空格：JS 的 `\s` 含 U+3000，Go 的 `\s` **不含** ⇒ 这条钉住那个差异。
  {
    name: 'live-ideographic-space',
    text: '口令1：AAA\u3000口令2：BBB',
    why: '⚠ 全角空格：JS `\\s` 含 U+3000，Go 的不含',
  },
  // 「取下一行」那支的**两道排除**各一条。原实现在取下一行之后还要判
  // "下一行自己是不是又是个口令头 / 含不含 `无随机`"，两道都不能省 ——
  // 省了会把 `复制口令2：` 或 `无随机口令` 当口令收下，然后换一个 200108 回来。
  {
    name: 'live-head-then-head',
    text: '复制口令1：\n复制口令2：\nAAA',
    why: '⚠ 下一行自己也是口令头 ⇒ 必须排除（否则把 `复制口令2：` 当口令）',
  },
  {
    name: 'live-head-then-no-random',
    text: '复制口令1：\n无随机口令',
    why: '⚠ 下一行含 `无随机` ⇒ 必须排除（它是收尾标记，不是口令）',
  },
]

// ───────────────────────── 小红书主页抽取 ─────────────────────────
//
// 页面是**服务端渲染**的，所以这几条用的是真实形态的片段（笔记卡片 / 内嵌 JSON /
// meta 标签），但内容里的口令是合成的 —— 真实主页 HTML 不进仓库（含账号标识）。

const xiaohongshuCases = [
  {
    name: 'inline-json-first-hit-wins',
    html:
      '<html><head><script>window.__INITIAL_STATE__={"noteList":[' +
      '{"displayTitle":"小红花口令：最新一期口令在此"},' +
      '{"displayTitle":"小红花口令：上一期的旧口令"}]}</script></head><body></body></html>',
    why: '内嵌 JSON 优先；主页倒序 ⇒ 第一条命中即最新（不能再往下取历史口令）',
  },
  {
    name: 'server-rendered-element',
    html: '<html><body><div class="note">口令是云盘AI高效复盘</div></body></html>',
    why: '没有内嵌 JSON 时退到服务端渲染的卡片',
  },
  {
    name: 'meta-content',
    html: '<html><head><meta name="description" content="小红花口令：元数据里的口令" /></head></html>',
    why: '再退到 `<meta content>`',
  },
  {
    name: 'no-kouling-at-all',
    html: '<html><body><div>今天天气不错</div></body></html>',
    why: '空结果的形状（返回 `[]` 而不是 nil）',
  },
  {
    name: 'html-entities-and-json-escape',
    html:
      '<html><body><div>小红花口令：&#35270;&#39057;&#20307;&#39564;</div>' +
      '<script>{"desc":"口令是A\\nB"}</script></body></html>',
    why:
      '⚠ 内嵌 JSON **优先于**文档顺序 ⇒ 解出的是 `A B`（换行折成空格），' +
      '不是 div 里的「视频体验」。手写期望值的第一版脚本记成了「视频体验」，与真实现不符，已改为真跑',
  },
]

// ───────────────────────── 期望值：跑**原实现的源码** ─────────────────────────
//
// 两段都不手写（理由见文件头），也不读编译产物（理由见 [runOriginal]）：
// 口令走 `live-room/kouling.ts` 的 `extractKouling`，
// 小红书走 `live-room/xiaohongshu.ts` 的 `extractKoulingFromXiaohongshuHtml`。

const [koulingResults, xiaohongshuResults] = runOriginal([
  {
    module: path.join('live-room', 'kouling.ts'),
    fn: 'extractKouling',
    inputs: koulingCases.map((c) => c.text),
  },
  {
    module: path.join('live-room', 'xiaohongshu.ts'),
    fn: 'extractKoulingFromXiaohongshuHtml',
    inputs: xiaohongshuCases.map((c) => c.html),
  },
])

koulingCases.forEach((item, i) => {
  item.expect = koulingResults[i]
})
xiaohongshuCases.forEach((item, i) => {
  item.expect = xiaohongshuResults[i]
})

// 自检：至少要有几条**非空**结果，否则说明探针没真的跑起来
// （全返回 `[]` 时，那些"期望空"的用例会静默地"全部通过"）。
if (
  !koulingCases.some((c) => c.expect.length > 0) ||
  !xiaohongshuCases.some((c) => c.expect.length > 0)
) {
  throw new Error('原实现对所有用例都返回空 —— 探针多半没跑对，先别写向量')
}

// ───────────────────────── 写出 ─────────────────────────

const out = {
  $source:
    '2026-09-22 生成：MSync 字节与两个签名头由**原项目实现**（protobufjs + node:crypto，' +
    'scripts/gen-live-room-vectors.cjs）算出；口令 / 小红书用例的输入取自 2026-09-17 实拍夹具 ' +
    'test/fixtures/http/直播wss.json 与原项目 __tests__/kouling.test.ts。',
  $note:
    'Go 侧 internal/upstream 消费本文件。**全部**期望值都出自原实现：' +
    'MSync 字节 / 两个签名头走 protobufjs + node:crypto，口令 / 小红书走原项目 ' +
    'packages/core/**src** 的 TS 源码（用原项目的 tsx 直接跑，sha256 见 $evidence.originalSrc）。' +
    '⚠ 这两段曾一度是**手写**的期望值（34 条里 3 条与真实现不符），' +
    '随后又一度改成读 packages/core/dist —— 而那份产物已过期（缺 isQuestion 过滤），' +
    '两种做法都会把**正确的**实现判成红。手写的期望值/过期的产物都是假基准。',
  $evidence: {
    fixture: 'test/fixtures/http/直播wss.json（13 片 imChatRoomHistoryMessage，814 条弹幕）',
    fixture2: 'test/fixtures/http/直播723.json、直播716.json（member/apply 的实拍头集合）',
    doc: 'docs/capture/app-live-redflower-samples.txt',
    originalTs: 'mcloud-sign: packages/core/src/live-room/{proto,kouling,xiaohongshu}.ts',
    // 口令 / 小红书两段的期望值是**跑原项目的 TS 源码**得到的（不是手写、也不是读
    // 过期的编译产物）。附 sha256 是为了让"基准是哪一份源码"可复核。
    originalSrc: originalFiles,
    protobufjs: require('protobufjs/package.json').version,
    tsx: '原项目 node_modules/.bin/tsx（直接跑 TS 源码，不经编译产物）',
  },
  constants: {
    APP_KEY,
    LIVE_APP_KEY,
    DOMAIN,
    DEVICE_INFO_HEX,
    USER_AGENT,
    ACCESS_TOKEN,
    PHONE,
    ROOM_ID,
    protoTypesUsed,
  },
  msyncFrames,
  msyncInbound,
  memberApplySignatures: applySigs,
  koulingExtract: koulingCases,
  xiaohongshuExtract: xiaohongshuCases,
}

const target = path.join(__dirname, '..', 'test', 'golden', 'live-room-vectors.json')
fs.writeFileSync(target, JSON.stringify(out, null, 2) + '\n')
console.log(`已写出 ${target}`)
console.log(`  msyncFrames           ${msyncFrames.length}`)
console.log(`  msyncInbound          ${msyncInbound.length}`)
console.log(`  memberApplySignatures ${applySigs.length}`)
console.log(`  koulingExtract        ${koulingCases.length}（期望值由原实现算出）`)
console.log(`  xiaohongshuExtract    ${xiaohongshuCases.length}（期望值由原实现算出）`)
console.log(`  protobufjs            ${require('protobufjs/package.json').version}`)
for (const [file, info] of Object.entries(originalFiles)) {
  console.log(`  原实现 ${file}  sha256 ${info.sha256.slice(0, 16)}…`)
}
