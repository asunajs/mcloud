/**
 * 推送（通知）那一族的**界面文案表**：渠道显示名、字段显示名、门槛档位。
 *
 * # 为什么是纯数据、为什么放在 `ui/`
 *
 * 代码指南 §9 约定 2：逻辑与文案抽成纯函数 / 常量放 `ui/`，页面只负责摆。
 * 这一张表**不**含任何 Vue 依赖，所以它可被单测直接读（将来加 vitest 时不用改形状）。
 *
 * # ⚠ 这里没有"字段清单"，只有"字段叫什么"
 *
 * 十家各自的字段名、类型档位（secret / plain / keys）、渲染顺序，**全部来自后端**：
 * `GET /api/notify/config` 的 `order` 与每家的 `fields` 数组（数组序 = 渲染序；权威是
 * `internal/server/notify_view.go` 那张表，再往上是 `internal/core/notify.go` 的 json tag）。
 * 前端**不抄一份字段表**，因为抄了就会漂移，而漂移有两种表现，都不报错：
 *  - 后端新增一格而界面上没有 ⇒ 用户配不到它，以为不支持；
 *  - 前端多出一格而后端不认识 ⇒ PUT 被 400 拦下（"有不认识的键"），
 *    看起来像"我填错了"，其实是这里多写了。
 * 所以这里只放**显示名**：认不出的键名一律原样显示（`labelOf` 的兜底那一行）。
 */

/** 渠道显示名。键 = 后端的渠道名（`order` 里给的那十个）。 */
export const CHANNEL_LABEL: Record<string, string> = {
  pushplus: 'pushplus（微信）',
  serverChan: 'Server 酱（方糖）',
  workWeixin: '企业微信应用',
  workWeixinBot: '企业微信群机器人',
  tgBot: 'Telegram Bot',
  bark: 'Bark（iOS）',
  twoIm: '回逍（2Im）',
  dingTalk: '钉钉群机器人',
  email: '邮件（SMTP）',
  customPost: '自定义 HTTP',
}

/** 字段显示名。键 = 后端 `fields[].name` 里那个**逐字**的 json tag 名。 */
export const FIELD_LABEL: Record<string, string> = {
  token: 'Token',
  key: 'Key',
  sid: 'SID',
  corpid: '企业 ID（corpid）',
  corpsecret: '应用密钥（corpsecret）',
  agentid: '应用 ID（agentid）',
  touser: '接收人（touser）',
  msgtype: '消息类型（msgtype）',
  chat_id: '聊天 ID（chat_id）',
  api_host: 'API 主机（api_host）',
  disable_web_page_preview: '关闭网页预览',
  level: '级别（level）',
  url: '地址（url）',
  host: 'SMTP 主机',
  port: '端口',
  from: '发件地址',
  pass: 'SMTP 授权码 / 口令',
  to: '收件地址',
  method: '方法（method）',
  timeout: '超时（毫秒）',
  extra: '附加字段（extra）',
  query: '查询串附加键（query）',
  headers: '请求头（headers）',
  data: '请求体 data',
  body: '请求体 body',
}

/**
 * 几个**看不懂就会填错**的字段的说明。
 *
 * 只列这几条的理由：其余字段的语义写在名字上就够了，而这几条不写一定有人填错，
 * 且填错的后果各不相同（一条是"看着配好了其实没生效"，两条是"改了不该改的东西"）。
 *
 * ⚠ 这里是**界面上直接显示的文案**，不是文档：不许用 `**粗体**` 之类的标记语言 ——
 * 页面按纯文本插值渲染，星号会原样出现在输入框下面。
 */
export const FIELD_HINT: Record<string, string> = {
  chat_id:
    '填数字（群或个人的 chat id，负数群 id 要带负号）。填成 @username 的形态本仓没有样本，界面不担保。',
  disable_web_page_preview: '勾上 = 关掉链接的网页预览卡片。',
  url: '整条地址里就带着密钥（形如 …/send?key=…），所以它按凭据处理：不回显、留空表示不改。',
  from: '它在后端那一族里被归成凭据（跟随 core 的分类，不在界面另判一次），所以同样不回显。',
  extra:
    'JSON 对象，整块替换：提交什么就是什么，留空则保持库里那一份。里面放什么由各家服务决定（如 topic、sound、icon）。',
  query: 'JSON 对象，整块替换；这些键会进 URL 查询串。',
  headers: 'JSON 对象，整块替换。⚠ 值不会显示出来（只出键名），所以改了要重填整条。',
  data: 'JSON 对象，整块替换（与 body 的区别由用户自己那台接收端决定）。',
  body: 'JSON 对象，整块替换。',
  level: '留空 = 用后端默认（bark 是 passive）。',
  api_host: '留空 = api.telegram.org。自建反代或镜像才需要改这一格。',
  msgtype: '留空 = 用该家默认（多数是 text）。',
  port: '465 是隐式 TLS；其余端口只在服务端支持 STARTTLS 时才发得出授权码（明文连接会被拒）。',
  timeout: '单位毫秒，留空 = 用后端默认。整块替换的那一族里每条都要重填。',
}

/** 门槛四档的人话。值 = 后端 `levels` 里那四个字符串。 */
export const LEVEL_LABEL: Record<string, string> = {
  error: '只推 error 及以上',
  warn: '推 warn 与 error',
  info: '推 info / warn / error（默认）',
  debug: '全推（含 debug）',
}

/** `kind` 三档 → 输入控件的形态。查表而不是拼类名（代码指南 §9.1①）。 */
export type NotifyFieldKind = 'secret' | 'plain' | 'keys'

/** 渠道显示名（认不出来的名字原样显示 —— 宁缺勿错，不替用户猜一家新的叫什么）。 */
export const channelLabel = (name: string): string => CHANNEL_LABEL[name] ?? name

/** 字段显示名，同上。 */
export const fieldLabel = (name: string): string => FIELD_LABEL[name] ?? name

/** 字段说明（没有就返回空串，界面上那一行**不占位置**）。 */
export const fieldHint = (name: string): string => FIELD_HINT[name] ?? ''

/** 门槛档位显示（后端给了未知值时也原样显示，不替它编一句）。 */
export const levelLabel = (level: string): string =>
  LEVEL_LABEL[level] ?? `门槛 ${level}`

/**
 * 一家渠道的状态那句话（界面徽章与试发选项都靠它）。
 *
 * 三态**必须**分得开，因为它们的下一步动作完全不同：
 *  - `未配置` ⇒ 去填；
 *  - `缺必填：X` ⇒ 那一格里少了一格，**填上就行**（不是删了重来）；
 *  - `已配齐` ⇒ 可以直接试发。
 */
export const channelStatus = (
  configured: boolean,
  missing: string,
): { text: string; tone: 'ok' | 'warn' | 'off' } => {
  if (!configured) return { text: '未配置', tone: 'off' }
  if (missing) return { text: `缺必填：${missing}`, tone: 'warn' }
  return { text: '已配齐', tone: 'ok' }
}

/**
 * 徽章类名。
 *
 * ⚠ 三个值都是**完整字面量**：`badge-${tone}` 这种拼法 Tailwind 扫不到，
 * 类名不会进产物，表现是"徽章变成没样式的裸文字"（代码指南 §9.1①）。
 */
export const STATUS_BADGE: Record<'ok' | 'warn' | 'off', string> = {
  ok: 'badge badge-soft badge-success',
  warn: 'badge badge-soft badge-warning',
  off: 'badge badge-soft',
}

/** 试发逐家结论的徽章（三档，与后端 `NotifyOutcome` 一字不差）。 */
export const OUTCOME_BADGE: Record<string, string> = {
  sent: 'badge badge-soft badge-success',
  failed: 'badge badge-soft badge-error',
  skipped: 'badge badge-soft badge-warning',
}

/** 逐家结论的人话（后端没给的新档位原样显示，不替它编）。 */
export const OUTCOME_LABEL: Record<string, string> = {
  sent: '已发',
  failed: '失败',
  skipped: '跳过',
}
