# findings-261004-upstream

- 基线: HEAD 2ca9cf7
- 范围: crates/mcloud-upstream/（63 文件 / 31751 行）
- 方法: **只读**。逐文件读 `client.rs` / `h5_client.rs` / `http_log.rs` / `session.rs` / `session_source.rs` / `endpoints.rs` / `auth.rs` / `exchange.rs` / `app_cloud_upload.rs` / `cloud_trash.rs` / `app_signin.rs` / `app_task_list_v2.rs` / `live_room.rs` / `live_ws_client.rs` / `live_flower.rs` / `ai_avatar.rs` / `cloud_phone_redpack.rs` / `app_task_entry.rs`；
  参考侧只读 `mcloud-sign/packages/core/src/apis/`、`mcloud-sign/packages/shared/src/http/base.ts`、`mcloud-tasks` 的少数消费点（用于判定影响）。
  跑过的命令只有 `git rev-parse`、`find|wc -l`、`grep -rn`、`sed -n`（见每条 `复现`）。
  **未跑 cargo**（另一条线在编译）；**未发任何上游请求**；未改动任何源码。

## 摘要（≤8 行）

1. 第 1 问的答案与直觉相反：这个 crate 的**业务码判定**几乎都站在"读不到 ⇒ 判失败"这一侧
   （`cloud_trash.rs:171`、`ai_avatar.rs:508`、`smid.rs:700`、`live_room.rs:103`、`app_cloud_upload.rs:602` 逐条核过）。
   真正危险的是**字段级默认值**，最硬的一条是 up-1（`exist` 兜成 `false` ⇒ 可能删用户文件）。
2. 第 2 问：超时只接了**一个**（reqwest 的全局总 timeout）；`mcloud-upstream` 里
   **`CancelToken` 零引用**（up-4），且 WS 握手完全没有超时（up-2）。
3. 第 3 问：`http_log.rs` **自身脱敏没有漏** —— 每个文本出口都过 `safe()`，且是先脱敏再按字符截断；
   漏的是**覆盖面**：登录链（带密码）与 smid 风控链不在日志层里（up-7）。这条与"找泄漏"的预期相反。
4. 第 4 问：只报了有 grep 证据的两条 —— 默认 UA（up-5）与 `endpoints.rs` 注释与行为相反（up-6）。
5. 第 5 问：`POST /ycloud/visitlog/journaling` 有两份实现，**表单字段一个在 query 一个在 body**（up-8）；
   另有"哪台主机拿 `authorization`"的两份实现结论相反（up-12）。
6. 每条都标了「已实测/仅静态」。**本次没有任何一条是打上游实测的** ⇒ 除注明外全部是「仅静态」；
   凡引用 `mcloud-sign` 的，都是逐字读该文件后写下的，命令可复现。
7. 没读完的：`live_msync.rs`、`live_kouling.rs`、`app_funai.rs`、`app_play_ai.rs`、`app_notice.rs`、
   `app_msg_push.rs`、`app_cloud_outlink.rs`、`login.rs`/`app_login.rs` 的深水区、以及全部 `tests.rs`。
8. 行号一律先用 `grep -n` 确认过（见各条 `复现`）。

## 发现

### up-1 [高] `exist` 被静默兜成 `false`，而 `true` 正是"别删用户文件"的唯一依据
- 位置: `crates/mcloud-upstream/src/app_cloud_upload.rs:745-747`（`cloud_flag`）、`:707`（用法）；
  消费点 `crates/mcloud-tasks/src/task_expansion.rs:517`、`:523`、`:611`
- 证据:
  ```rust
  // app_cloud_upload.rs:741-747
  /// 读一个布尔格：非布尔（含 `null`、数字、缺席）一律 `false`。
  ///
  /// ⚠ 刻意**不**「把 `1` 当 true」：…而「形状变了却念成 false」与「念成 true」一样危险，
  /// 两档都不如让它可断言。
  fn cloud_flag(obj: &serde_json::Map<String, Value>, key: &str) -> bool {
      obj.get(key).and_then(Value::as_bool).unwrap_or(false)
  }
  // :707-708
  exist: cloud_flag(obj, "exist"),
  rapid_upload: cloud_flag(obj, "rapidUpload"),
  ```
  它守的是什么（`mcloud-tasks/src/task_expansion.rs:16-18` 逐字）：
  ```text
  ⚠⚠ 但**撞同名那一档绝不删**：`exist == true` 时那个 `fileId` 是
  **用户本来就有的文件**（上游按 `refuse` 语义把已有那枚交回来了），
  把它放进清理名单就是**删用户的东西**。所以那一档**不交 id**（`upload_file_id` 留空）。
  ```
  而这条不变量只由 `if created.exist`（`task_expansion.rs:517`）保证；`exist` 一旦念成 `false`，
  下一行就把它登记进清理名单：
  ```rust
  // task_expansion.rs:517-524
  if created.exist { result.upload = BackupUpload::Exists; return; }
  // 从这一格起，这个 id 是**我们造的** ⇒ 链尾要删。
  result.upload_file_id = created.file_id.clone();
  if created.rapid_upload { result.upload = BackupUpload::Rapid; return; }
  ```
  链尾照着删（`task_expansion.rs:611`：`let Some(file_id) = result.upload_file_id.clone() else { …NotNeeded… }`）。
- 复现:
  ```bash
  grep -n "exist: cloud_flag\|rapid_upload: cloud_flag\|fn cloud_flag" crates/mcloud-upstream/src/app_cloud_upload.rs
  grep -n "created.exist\|created.rapid_upload\|upload_file_id" crates/mcloud-tasks/src/task_expansion.rs
  grep -n "created.exist" crates/mcloud-tasks/src/app_task.rs
  ```
- 影响: `:745` 的注释自己说"「形状变了却念成 false」与「念成 true」一样危险，两档都不如让它可断言"——
  但 `cloud_flag` 返回的是 `bool`，**运行期没有任何可断言的位置**，那句论证只落在一份用例里（喂 `null` 断言 false）。
  上游哪天把 `exist` 改成 `"true"` 字符串、`1`、或改名，本仓的读数就是"这个文件是我造的"，
  而它其实是用户网盘里**本来就有的**那个 ⇒ 链尾把它移进回收站。
  唯一缓和项：删的是**回收站**（`task_expansion.rs:20`），捞得回来。
  同样形状还影响 `app_task.rs:1305`（那边读错会去 PUT 一个已存在的文件）。
- 建议: 把 `cloud_flag` 改成 `Option<bool>`/`Result<bool>`，让"这一格读不出"变成**这一枪失败**；
  至少在 `exist` 缺席/非布尔时返回 `Err(Error::Parse)`。安全侧的不变量不该挂在 `unwrap_or(false)` 上。
- 状态: 仅静态 · 置信度 高（代码路径与消费点都已 grep 确认；形状漂移未实测）

### up-2 [高] 直播 WS 握手没有超时：`connect_async` 可无限挂起，`listen_for` 只覆盖"连上之后"
- 位置: `crates/mcloud-upstream/src/live_ws_client.rs:319`（`connect_async`）、`:399`（deadline 起点）、`:344-345`（许诺"整体上限"的注释）
- 证据:
  ```rust
  // live_ws_client.rs:319（connect_live_ws 内，没有任何 timeout 包装）
  let (stream, _response) = connect_async(request)
      .await
      .map_err(|e| Error::Network(format!("连接直播间 WebSocket 失败：{e}")))?;

  // live_ws_client.rs:399（listen_on_stream，**在 connect 成功之后**才执行）
  let deadline = Instant::now() + listen_for;
  ```
  `listen_danmaku`（`:346-368`）的顺序是 `connect_live_ws(&url, endpoint).await?` → `listen_on_stream(stream, …, listen_for, …)`。
  同一文件 `:344-345` 的注释却写：「[`ListenRequest::listen_for`] 是**整体上限**」。
  调用侧也没有外层超时：`crates/mcloud-tasks/src/live_room.rs:961` 直接 `.await?`，
  并在 `:1013` 用 `futures::join!` 与小红书那支并发（`join!` 等齐两边，任一挂死则整轮挂死）。
- 复现:
  ```bash
  grep -n "connect_async\|let deadline = Instant::now()\|整体上限" crates/mcloud-upstream/src/live_ws_client.rs
  grep -n "listen_danmaku(\|futures::join!" crates/mcloud-tasks/src/live_room.rs
  ```
- 影响: 上游只要"TCP 连上但不回 WS 升级响应"（防火墙黑洞 / 半开连接 / 中间设备），
  这个任务就**永久挂起**：外看是"正在听"，不结束、不超时、不报错，一直占着这条账号的执行额度。
  SockJS 的踢人超时只管**已升级**的连接，救不了握手期。
- 建议: `tokio::time::timeout(connect_timeout, connect_async(...))`，与 `listen_for` 分开计时；
  编排层再加一层兜底。**并修正 `:344-345` 那句注释**（它今天是不成立的）。
- 状态: 仅静态 · 置信度 高（代码路径无歧义；"上游真会黑洞"未实测）

### up-3 [中高] 全局 30s 是 reqwest 的**总** deadline，却套在预签名 PUT 的整段上传上
- 位置: `crates/mcloud-upstream/src/client.rs:206`；默认值 `crates/mcloud-app/src/builder.rs:22`；
  上传请求 `crates/mcloud-upstream/src/app_cloud_upload.rs:500`
- 证据:
  ```rust
  // client.rs:204-209 —— 一个 Client 一种超时，所有请求共用（legacy 那份也一样）
  let base = || reqwest::Client::builder()
      .timeout(Duration::from_secs(timeout))
      .cookie_store(true)
      .gzip(true);
  ```
  `upload_put_request`（`app_cloud_upload.rs:500-506`）用 `.bytes_body(content)` 把**整份文件**挂在同一个客户端上；
  reqwest 的 `.timeout()` 是**从发起到读完响应的总时长**，不区分首字节与体。
- 复现:
  ```bash
  grep -n "\.timeout(Duration::from_secs" crates/mcloud-upstream/src/client.rs
  grep -n "timeout_secs" crates/mcloud-app/src/builder.rs
  grep -n "fn upload_put_request" -A 7 crates/mcloud-upstream/src/app_cloud_upload.rs
  ```
- 影响: 上传超过 30s 的文件必然以超时结束；而这一枪的错误文案**刻意不带 URL、不带响应体**
  （`ensure_put_ok`，`app_cloud_upload.rs:520-527`）⇒ 用户只看到一句"超时"。
  更隐蔽的是超时前可能已经上去了一段字节，而 `file/complete` 不会发 ⇒ 网盘里留一个未完成段，
  任务报的是网络错误，与"文件太大"在日志上长得一样。
  ⚠ 缓和项：`mcloud-sign` 也是 `timeout: 30000`（`packages/shared/src/http/base.ts:62`）⇒
  **这是两边共有的潜在问题，不是本仓的回归**（本仓只是把它固定成了单一全局值，无法按请求放宽）。
- 建议: 让 `HttpRequest`/`ClientProfile` 能表达"这一枪要多久"，至少别让 PUT 共用 30s。
- 状态: 仅静态 · 置信度 中高（reqwest 超时语义是文档口径；"真超时"未实测）

### up-4 [中高] 取消令牌根本没接进网络层：`mcloud-upstream` 里 `CancelToken` 零引用
- 位置: 反面证据在 `crates/mcloud-tasks/src/live_room.rs:1581-1591`（唯一形态：**发请求前**检查）
- 证据:
  ```bash
  grep -rn "CancelToken\|check_cancelled\|cancel" crates/mcloud-upstream/src/ | wc -l    # → 0
  ```
  ```rust
  // mcloud-tasks/src/live_room.rs:1581-1587 —— 检查在 request **之前**，请求本身不可打断
  while !session.done() {
      ctx.check_cancelled()?;
      let (step, req) = session.next()?;
      let resp = ctx.http.request(req).await.map_err(|e| wrap_step_error(step, e))?;
  ```
  `CancelToken` 存在于 `mcloud-core`（`crates/mcloud-core/src/cancel.rs`）并在任务层被
  `ctx.check_cancelled()?` 使用（`app_task.rs:987/1045/1181/1476`、`notice.rs:361`、`play_ai.rs:235`、
  `task_expansion.rs:386`、`exchange.rs:1843`、`live_room.rs:1582`），**全部是步骤边界**。
- 复现:
  ```bash
  grep -rn "CancelToken\|check_cancelled" crates/mcloud-upstream/src/ | wc -l
  grep -rn "check_cancelled" crates/mcloud-tasks/src/ | grep -v tests
  ```
- 影响: "取消"在最坏情况下要等**一整发请求**（含 up-2 里那个无上限的 WS 握手、以及 up-3 的 30s PUT）
  才能生效。用户在 WebUI 上点"停止"之后，进程仍可能挂着不动 —— 而这正是取消功能存在的理由。
  超时/取消/重试三者里，本仓实际只接了"超时"一个（重试是刻意不做的，见 `app_cloud_upload.rs:487-490`）。
- 建议: 把 `CancelToken` 传进 `HttpClient`（或在 `H5Client`/`ReqwestClient` 那一层用 `select!` 与
  `cancel.cancelled()` 竞争），至少让 WS 那条链可被打断。
- 状态: 仅静态 · 置信度 高

### up-5 [中] 默认 UA 与 mcloud-sign 不一致，而本仓已经有那个正确常量却没用
- 位置: 上线值 `crates/mcloud-upstream/src/client.rs:62-63`（`DEFAULT_UA`）；
  正确串 `crates/mcloud-upstream/src/smid.rs:79`（`CAIYUN_ANDROID_WEB_UA`）；
  组装层 `crates/mcloud-app/src/builder.rs:151`（`user_agent: None`）；
  本仓自己写下的要求 `crates/mcloud-upstream/src/app_cloud_upload.rs:58-62`
- 证据:
  ```rust
  // client.rs:62-63 —— App 账号（pc: None）每一条请求都用它
  pub const DEFAULT_UA: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 \
  (KHTML, like Gecko) Chrome/128.0.0.0 Mobile Safari/537.36";
  ```
  ```rust
  // smid.rs:79 —— 与 mcloud-sign 逐字相同的那一串
  pub const CAIYUN_ANDROID_WEB_UA: &str = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36";
  ```
  mcloud-sign 的**静态头**就是后者（`packages/shared/src/http/base.ts:28-29` 的 `APP_ENVIRONMENT`，
  以及 `:157` 的 `SHAKE_UA`，两处同串）⇒ 参考实现**每一条**请求都带它。
  而本仓 `app_cloud_upload.rs:58-62` 自己写着：「要复现参考行为：**组装层**把 profile 的 UA 设成
  `CAIYUN_ANDROID_WEB_UA`」——组装层却是 `user_agent: None`（`builder.rs:81`、`:151` 两处都是）。
- 复现:
  ```bash
  grep -n "pub const DEFAULT_UA" -A 2 crates/mcloud-upstream/src/client.rs
  grep -n "CAIYUN_ANDROID_WEB_UA" crates/mcloud-upstream/src/smid.rs crates/mcloud-upstream/src/app_cloud_upload.rs
  grep -n "user_agent: None" crates/mcloud-app/src/builder.rs
  grep -n "Android 14; 22041216C" /Users/catlair/Documents/workspace/mcloud-sign/packages/shared/src/http/base.ts
  ```
- 影响: UA 是上游风控画像的一部分（`smid.rs` 那一整族就是拿它去换 `deviceId` 的）。
  本仓自己在 smid 指纹体里用的是 `CAIYUN_ANDROID_WEB_UA`，而在**真正上网的请求头**里用的是另一串
  —— 同一台"设备"在两个地方报了不同的 UA。PC 账号那条同理：参考实现的文件操作档要求裸 `Mozilla/5.0`
  （`mcloud-sign/packages/core/src/apis/file.ts:24`），本仓用的是 `PcClient::user_agent()` 那一串 PC 登录 UA。
- 建议: 在 `builder.rs` 的 `ClientProfile` 里把 `user_agent: Some(CAIYUN_ANDROID_WEB_UA)` 落下去
  （PC 账号那条按 `x-yun-*` 的档位另议），或把 `DEFAULT_UA` 直接换成 `CAIYUN_ANDROID_WEB_UA` 并只留一份。
- 状态: 仅静态 · 置信度 高（字符串逐字比对过）；"上游是否因此区别对待"未实测

### up-6 [中] `endpoints.rs` 的注释与 `accepts_basic` 的行为**相反**：登录主机其实命中 basic 注入
- 位置: `crates/mcloud-upstream/src/endpoints.rs:43-44`（注释）、`:107-111`（实现）、`:125`（测试断言）；
  组装层 `crates/mcloud-app/src/builder.rs:78` / `:144`
- 证据:
  ```rust
  // endpoints.rs:40-44
  // 四条都在**同一个**用户域主机 `user-njs.yun.139.com` 上。
  // 注意这个主机**不在** `AUTH_DOMAINS` 里 —— 登录时还没有凭据，
  // 本来就不该注入 `authorization`；登录成功后拿到的 auth 串才用于后续请求。

  // endpoints.rs:107-111
  pub fn accepts_basic(host: &str) -> bool {
      AUTH_DOMAINS.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
  }
  ```
  `AUTH_DOMAINS` 含 `"yun.139.com"`（`:6-11`），而 `user-njs.yun.139.com` 以 `.yun.139.com` 结尾 ⇒ **返回 true**。
  同文件的测试正把这件事钉成"正确"：`endpoints.rs:125` `assert!(accepts_basic("user-njs.yun.139.com"));`
- 复现:
  ```bash
  grep -n "user-njs.yun.139.com" crates/mcloud-upstream/src/endpoints.rs
  grep -n "basic_token" crates/mcloud-app/src/builder.rs
  ```
- 影响: 登录四枪（`THIRD_LOGIN`/`SEND_SMS_CODE`/`SLIDER_PUZZLE`/`LOGOUT`，`endpoints.rs:47-57`）今天**没**带凭据，
  唯一原因是调用方走的是**裸传输**（`builder.rs:78 basic_token: None`），而业务工厂
  `builder.rs:144` **永远**是 `Some(account.auth.raw)`。也就是说这条边界只由"用哪个工厂"维持，
  类型上没有任何东西拦着；而注释说的却是一条**不存在**的域名边界。
  按注释理解去写"重新登录/换绑"（复用业务客户端）就会在登录请求上静默多带一枚 `Authorization`，
  那一发零样本，且与 `login::PcClient::headers` 的 5 头口径互斥（`client.rs:316-339` 讲的就是这个）。
- 建议: 二选一 —— 把 `user-njs.yun.139.com` 显式排除（并改 `:125` 的断言），或把 `:43-44` 改成
  "它在 `AUTH_DOMAINS` 内，靠调用方用裸传输避免注入"。别让注释与代码各说一套。
- 状态: 仅静态 · 置信度 高（行为）；中（影响是否真会发生）

### up-7 [中] `http_log.rs` 的脱敏**没有漏**，漏的是覆盖面：登录链与 smid 风控链不在这层里
- 位置: `crates/mcloud-upstream/src/http_log.rs:80-147`（两个出口）、`:153-176`（`log_outgoing`）、`:185-192`（`safe`）；
  未覆盖处 `crates/mcloud-app/src/builder.rs:75-91`（smid 裸传输）、`crates/mcloud-app/src/login.rs:299-305`（登录裸传输）
- 证据（"没漏"这一侧，逐出口核过）:
  ```rust
  // http_log.rs:163-174 —— 头与体都过 safe()
  .map(|(k, v)| format!("{k}={}", safe(v, HEADER_CHARS)))
  .map(|b| safe(&String::from_utf8_lossy(b), PREVIEW_CHARS))
  // http_log.rs:185-192 —— 先脱敏、再按**字符**截断（顺序错了会漏半截 token，注释已说明）
  fn safe(text: &str, limit: usize) -> String {
      let redacted = mcloud_core::event::redact(text);
  ```
  响应头一处都**没记**（`:84-105` 只记 status/ms/bytes/body 预览）⇒ "漏记响应头"这一侧不存在；
  `account` 字段传的是 `account.id`（SHA-256 前缀，`builder.rs:161-164`），不是手机号。
- 证据（覆盖率这一侧）:
  ```bash
  grep -rn "LoggingClient::new" crates/    # 只有 builder.rs:161（业务客户端）+ http_log 自己的测试
  ```
  `builder.rs:75-91` 的 `smid_http`（打第三方风控 `deviceprofile/v4`，`:69-74` 明说"一个凭据都不给"）
  与 `login.rs:299` 的 `bare_transport`（打 `thirdlogin`/`getSmsCode`，**带密码**）都是裸 `ReqwestClient`。
  而 http_log 的模块头（`:11-14`）把链子写成 `SmidClient → H5Client → ReqwestClient` 并称
  "到这一层时 `deviceid`/`jwttoken`/`authorization` 都已经注入完毕"，读者很容易读成"每一枪都有日志"。
- 复现:
  ```bash
  grep -n "safe(" crates/mcloud-upstream/src/http_log.rs
  grep -rn "LoggingClient::new" crates/
  grep -n "fn bare_transport" -A 8 crates/mcloud-app/src/login.rs
  grep -n "let smid_http" -A 8 crates/mcloud-app/src/builder.rs
  ```
- 影响: ① **登录失败零现场** —— 而登录恰恰是用户最先遇到的失败（扫码/短信/账密），
  除了一句 `Error::Network` 看不到"发了什么、回来什么"；② 风控主机那条链同样看不到。
  反过来说这是安全侧的幸运：密码那一枪没进日志。若后续为排查把它套上，
  必须先钉一条"`thirdlogin` 的字段名会被脱敏"的用例（那一发零实拍样本；
  `SENSITIVE_KEYS` 有 `password`/`pwd`/`verifycode`/`captcha`，大体够但没被这条链验证过）。
- 建议: 把 `LoggingClient` 也套到 smid 链路（那一枪无凭据，安全）；登录链路要么显式写"故意不记"，
  要么套上并补脱敏钉子。
- 状态: 仅静态 · 置信度 高

### up-8 [中] 同一条 `POST /ycloud/visitlog/journaling` 有两份实现，**表单字段一个在 query、一个在 body**
- 位置: `crates/mcloud-upstream/src/exchange.rs:194-201`（query 串 + 空体）vs
  `crates/mcloud-upstream/src/live_flower.rs:253-268`（全在 body）
- 证据:
  ```rust
  // exchange.rs:194-201 —— 字段放**查询串**，体是空串
  let url = format!(
      "{VISIT_LOG_JOURNALING}?module=uservisit&optkeyword={}&sourceid=1002&marketName={}",
      percent_encode(keyword), percent_encode(market_name));
  let req = HttpRequest::post(url).text_body("");
  ```
  ```rust
  // live_flower.rs:253-268 —— 同一个端点，字段放**请求体**
  let body = format!("module=uservisit&optkeyword={}&sourceid={}&marketName={}",
      form_encode(optkeyword), LIVE_ROOM_JOURNALING_SOURCE_ID, LIVE_ROOM_MARKET_NAME);
  live_room_headers(HttpRequest::post(format!("{BASE_LIVE_ROOM_FEEDBACK}{JOURNALING_PATH}"))
      .text_body(body).header("Content-Type", "application/x-www-form-urlencoded;charset=UTF-8"))
  ```
  两处都声称照 `mcloud-sign` 逐字，而 `mcloud-sign` 自己就不一致：
  `packages/core/src/apis/exchange.ts:16-22` 是 query + `""` 体；
  `packages/core/src/apis/live-room.ts:69-77` 是 `http.post(url, "module=uservisit&…")` 落在 body。
- 复现:
  ```bash
  grep -rn "journaling" crates/mcloud-upstream/src/exchange.rs crates/mcloud-upstream/src/live_flower.rs
  grep -n "reportVisitLog" -A 8 /Users/catlair/Documents/workspace/mcloud-sign/packages/core/src/apis/exchange.ts
  grep -n "liveRoomJournaling" -A 12 /Users/catlair/Documents/workspace/mcloud-sign/packages/core/src/apis/live-room.ts
  ```
- 影响: 同一上游接口在本仓有两种"都权威"的形状。第三个调用点（今天已经有第三处提到这一枪：
  `app_play_ai.rs:16` 把它当门槛用）无论照哪一份写，都会与另一份不一致；
  而这类错的表现是"埋点没生效"，不报错、只影响下游判定的精度。
  另注：`sourceid`（`1002` vs `1007`）与 `marketName` 不同是**有意**的（不同活动，见 `live_consts.rs:73-74`），
  只有"字段在 query 还是在 body"这一项是真正的形状分歧。
- 建议: 抽一个共享的构造器，把"字段放哪"变成参数并在注释里写明两族为何不同；或至少在两处互相加一条指针注释。
- 状态: 仅静态 · 置信度 高（两侧代码与 TS 原文都读过）

### up-9 [中] `dedup_headers` 保留**最后一个** ⇒ 会话头静默压掉调用方头，而 `referer` 恰好相反
- 位置: `crates/mcloud-upstream/src/h5_client.rs:234-252`（`dedup_headers`）、`:165-175`（`own_referer`）；
  `crates/mcloud-upstream/src/session.rs:389-394`（`h5_headers` 追加）
- 证据:
  ```rust
  // h5_client.rs:176-184 —— h5_headers 在调用方已有的头**之后**追加
  let out = crate::session::h5_headers(strip_referer(req.clone()), params, referer, Some(&sess.jwt_token), "true");
  dedup_headers(out)
  // h5_client.rs:237-243 —— 逆序遍历、见重即弃 ⇒ 留下的等价于"原序里最后一个"
  for (k, v) in req.headers.into_iter().rev() { if seen.contains(&key) { continue; } … }
  ```
  `h5_headers`（`session.rs:389-394`）无条件追加 `activityid`/`appversion`/`showloading`；
  而 `referer` 走的是显式分支（`h5_client.rs:165-169` 先取调用方的，`:175` 再 `unwrap_or(sess.referer)`）⇒ **调用方赢**。
  同一层里两个同名头冲突，一个"调用方赢"、一个"会话赢"，两边都不报错。
- 复现:
  ```bash
  grep -n "fn dedup_headers" -A 20 crates/mcloud-upstream/src/h5_client.rs
  grep -n "activityid\|appversion\|showloading" crates/mcloud-upstream/src/session.rs
  grep -rn '\.header("activityid"' crates/     # 只有 session.rs:391 一处
  ```
- 影响: 今天**潜伏** —— 全仓只有 `session.rs:391` 设 `activityid`，还没有受害调用方。
  但 `app_funai.rs:38` 说明"这一页的 `activityid` 不一样"这种现实确实存在；
  哪天某个 `app_*` 给自己的 `HttpRequest` 挂上 `activityid`/`appversion`，它会被**静默换成签到页那一份**，
  表现只是一个业务码（与 `h5_client.rs:158-160` 那段"会话与签名头必须同源"的告诫正好互为例外）。
- 建议: 让 `h5_headers` 改成"只在缺省时补"（与 `client.rs:245 should_inject_credential` 同一口径）。
- 状态: 仅静态 · 置信度 高（行为）；中（是否会被踩到）

### up-10 [低] `parse_spec_token` 刻意不判 `code`：非 0 码 + 有 `result` 就当成"拿到票了"
- 位置: `crates/mcloud-upstream/src/cloud_phone_redpack.rs:285-303`
- 证据:
  ```rust
  // cloud_phone_redpack.rs:290-292（注释）
  // ⚠ 旧 TS 只判 result 真假、**不判 code** —— 这里照做：code 非 0 但 result
  // 有值仍然算拿到了票；只是 code/msg 会被带进错误文案…
  // cloud_phone_redpack.rs:293-297（实现）
  match v.get("result").and_then(Value::as_str) {
      Some(t) if !t.is_empty() => Ok(t.to_string()),
  ```
  与同仓另一个"换票"解析器口径不同：`session.rs:408-423` 的 `parse_sso_token` 要求
  `success == true` **且** `data.token` 非空，缺一即 `Err`。
- 复现:
  ```bash
  grep -n "fn parse_spec_token" -A 18 crates/mcloud-upstream/src/cloud_phone_redpack.rs
  grep -n "fn parse_sso_token" -A 16 crates/mcloud-upstream/src/session.rs
  ```
- 影响: 上游若回 `{"code":500,"msg":"…","result":"<一句人话>"}`（`result` 是文案而不是票），
  这一族会把那句人话当票往下带，直到 `tokenValidate` 才失败 —— 错误现场停在第二步，
  而真正的原因（第一步的 code）留在被丢掉的那次响应里。这一族整体零实拍，"照 TS"是合理选择，
  但"两处换票口径不同"这件事没有任何地方写着。
- 建议: 至少在 `result` 非空**且** `code` 非 0 时打一条 `tracing::warn!`。
- 状态: 仅静态 · 置信度 中

### up-11 [低] 房间 `id` 读不到就成 `0`，而 `0` 会一路进 `roomBeat` 与 `Referer`
- 位置: `crates/mcloud-upstream/src/live_room.rs:482`（`id: int_at(room, "id").unwrap_or(0)`）、
  `:869-871`（`live_room_id_string`）
- 证据:
  ```rust
  // live_room.rs:482（同一循环里 status 用的是可判别的 -1）
  id: int_at(room, "id").unwrap_or(0),
  ```
  同文件的 `int_at` 文档（`:846-850`）已经把这类事故写出来了：
  "只写「`as_i64` 一种形态」的取值助手会**静默返回 0** —— 表现为「tab id 全是 0、拉到 0 间房」"。
- 复现:
  ```bash
  grep -n 'int_at(room, "id")\|int_at(room, "status")' crates/mcloud-upstream/src/live_room.rs
  grep -n "fn int_at" -A 10 crates/mcloud-upstream/src/live_room.rs
  ```
- 影响: 上游若把 `id` 换成非数字形态或改名，列表里的房间**全部**变成 id `0`：
  门照旧"有房间"、`roomBeat` 照发、`Referer` 里是 `cmlive_0` ⇒ 表现为"进房间失败/听不到弹幕"，
  而日志上一片正常。（`channelId`/`anchorId`/`authorId` 同为 `unwrap_or(0)`（`:485-488`），但它们不驱动后续请求，故不列。）
- 建议: `id` 缺席/形态不对的房间直接 `continue` 丢掉，或改成 `Option<i64>` 逼调用方处理。
- 状态: 仅静态 · 置信度 中

### up-12 [低] 同一条"哪台主机该拿 `authorization`"两份实现且结论相反；`BASE_MCLOUD` 也定义了两遍
- 位置: `crates/mcloud-upstream/src/app_cloud_upload.rs:316-325`（`sends_authorization_to`）
  vs `crates/mcloud-upstream/src/endpoints.rs:107-111`（`accepts_basic`）；
  重复常量 `crates/mcloud-upstream/src/session.rs:44` vs `crates/mcloud-upstream/src/live_consts.rs:169`
- 证据:
  ```rust
  // app_cloud_upload.rs:324 —— 裸后缀（照 mcloud-sign base.ts:80 的 hostname.endsWith(d)）
  AUTH_DOMAINS.iter().any(|suffix| host.ends_with(suffix))
  // endpoints.rs:110 —— 要求域名边界
  AUTH_DOMAINS.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
  ```
  分歧格：`host = "evilyun.139.com"` ⇒ 前者 `true`、后者 `false`。
  mcloud-sign 用的是**松**的那套（`packages/shared/src/http/base.ts:10` 的 `AUTH_DOMAINS` + `:80` 的 `endsWith`），
  所以本仓是**收紧**了 —— 收紧这一步是对的，但它把松的那份实现留在仓里。
  使用面：`grep -rn "sends_authorization_to" crates/` ⇒ 只有定义处 + `app_cloud_upload/tests.rs:805/818`。
  同类的重复还有 `BASE_MCLOUD`：`session.rs:44` 与 `live_consts.rs:169` 同名同值，
  且 `app_signin.rs:53` 又把前者 re-export 了一次。
- 复现:
  ```bash
  grep -n "fn sends_authorization_to" -A 12 crates/mcloud-upstream/src/app_cloud_upload.rs
  grep -n "fn accepts_basic" -A 5 crates/mcloud-upstream/src/endpoints.rs
  grep -rn "sends_authorization_to" crates/
  grep -rn "const BASE_MCLOUD" crates/mcloud-upstream/src/
  ```
- 影响: `sends_authorization_to` 今天**不在上线路径上**（只被测试引用）⇒ 不是活 bug。
  危险在于它是"看起来更权威、还引了旧 TS 作依据"的同名判据，而这条规则是**安全边界**
  （发出去不可回滚）：下一个人拿它写新的直传链路，会得到比上线规则**更宽**的答案。
- 建议: 删掉 `sends_authorization_to`（或改成 `accepts_basic` 的转发），并把 `BASE_MCLOUD` 收敛成一处。
- 状态: 仅静态 · 置信度 高
