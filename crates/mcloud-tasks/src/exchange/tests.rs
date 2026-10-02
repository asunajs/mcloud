//! 兑换（判定层 + 链）测试。
//!
//! 全程离线：假 HTTP 按脚本回放**实拍形状**的响应，
//! 断言的是"发了哪几枪、按哪一档走、结论说成什么"。
//!
//! # 两组测试，两个目的
//!
//! 1. **判定层逐档**：`plan_exchange` 十七档每一档都有对应用例
//!    （[`each_plan_tier_is_reachable`] 逐个点名）。反向改坏哪一档，
//!    哪一条用例红。
//! 2. **链的不变式**：`attempts == 0`（判据没过 / 算不出 offset /
//!    取不到 deviceId ⇒ 一枪都不能发）、`514` 只消耗一轮、
//!    传输错误**绝不重发**、`code 0` 之后余额差对不上判 `Unknown`。
//!
//! # 数字来源
//!
//! 用实拍值：成功那笔扣 `1800` 豆、余额 `10496`、商品
//! `"移动云盘100G云空间月卡"`、兑掉之后 `limit` 由 `0` 变 `2`。
//! 滑块题面那一对 PNG 用**真实可解**的合成图（见 [`slide_pngs`]），
//! 因为链上"解算失败就不发"这条判据必须靠一个真能解出来的题面才测得准。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mcloud_core::{Clock, HttpClient, HttpRequest, HttpResponse};

use super::*;

// ══════════════════════════════ 测试替身 ══════════════════════════════

/// 按顺序回放响应，并记录收到过哪些请求。
///
/// 与签到族那个 `Scripted` 同形（见 `crates/mcloud-tasks/src/signin/tests.rs`）：
/// 用尽后重复最后一个，于是"某一支多发了一枪"会表现为最后那个响应被复用，
/// 而请求条数断言会立刻红。
#[derive(Default)]
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<String>>,
    /// 让第 n 个请求（1 起）在传输层失败（`Err`）而不是回一个响应。
    ///
    /// "写请求超时 ⇒ 判 Unknown 且绝不重发"这条判据必须与"上游回 514"
    /// 区分开，所以失败要能精确钉在某一个请求上。
    fail_at: Mutex<Option<usize>>,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取。
            replies: Mutex::new(replies.iter().rev().map(|s| s.to_string()).collect()),
            fail_at: Mutex::new(None),
        }
    }

    fn failing_at(self, n: usize) -> Self {
        *self.fail_at.lock().unwrap() = Some(n);
        self
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn url_at(&self, n: usize) -> String {
        self.requests()[n].url.clone()
    }

    /// 发出去的 `exchangeV3` 有几条。
    fn attempts(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("exchangeV3"))
            .count()
    }

    /// 发出去的 `getSlide` 有几条。
    fn slides(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("getSlide"))
            .count()
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        let n = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(req);
            seen.len()
        };
        if *self.fail_at.lock().unwrap() == Some(n) {
            return Err(Error::Network(format!(
                "第 {n} 个请求超时（模拟传输层失败）"
            )));
        }
        let body = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or_else(|| "{}".into())
            }
        };
        Ok(HttpResponse {
            status: 200,
            body,
            headers: Vec::new(),
        })
    }
}

struct FixedClock(i64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

/// 可变时钟：等完整点之后"现在"要往前走，否则重判那一轮会拿到同一个时刻。
struct AdvancingClock(AtomicUsize);

impl Clock for AdvancingClock {
    fn now_ms(&self) -> i64 {
        let steps = self.0.fetch_add(1, Ordering::SeqCst) as i64;
        BASE_NOW_MS + steps * 3_600_000
    }
}

/// 2026-09-19 14:30 CST（东八区）—— 实拍那一轮附近的一个时刻。
const BASE_NOW_MS: i64 = 1_789_799_400_000;

/// 假 deviceId 来源，可记录"有没有被调用过"。
///
/// 刻意存两个 `Option<&'static str>` 而不是 `Result<String, Error>`：
/// `Error` 不实现 `Clone`，而闭包是 `Fn`、会被调用多次。
#[derive(Default)]
struct DeviceId {
    ok: Option<&'static str>,
    err: Option<&'static str>,
    calls: AtomicUsize,
}

impl DeviceId {
    fn ok(id: &'static str) -> Arc<Self> {
        Arc::new(Self {
            ok: Some(id),
            err: None,
            calls: AtomicUsize::new(0),
        })
    }

    fn fails(message: &'static str) -> Arc<Self> {
        Arc::new(Self {
            ok: None,
            err: Some(message),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// 包成链上的注入点。
    fn as_fn(self: &Arc<Self>) -> Box<DeviceIdFn<'static>> {
        let me = Arc::clone(self);
        Box::new(move || {
            me.calls.fetch_add(1, Ordering::SeqCst);
            match (me.ok, me.err) {
                (Some(id), _) => Ok(id.to_string()),
                (None, Some(message)) => Err(Error::Network(message.to_string())),
                (None, None) => unreachable!("测试替身必须二选一"),
            }
        })
    }
}

/// 一次链调用的**自持有**环境。
///
/// `ExchangeEnv.device_id` 是引用，所以闭包必须比 `env` 活得久 ——
/// 把 HTTP、时钟、闭包三样一起装进这个结构体，就不会出现"env 引用了
/// 已经析构的局部变量"。
struct Fixture {
    http: Scripted,
    clock: Box<dyn Clock>,
    /// `Some` ⇒ 注入了 deviceId 来源；`None` ⇒ 这一趟没有来源。
    ///
    /// ⚠ 这里**不再另存一份** `Arc<DeviceId>`：断言调用次数时测试自己持有那个
    /// `Arc`（`device.calls()`），存第二份只会多出一个没人读的字段。
    device_fn: Option<Box<DeviceIdFn<'static>>>,
}

impl Fixture {
    /// HTTP + 固定时刻时钟 + 可选的 deviceId 来源。
    fn new(replies: &[&str], now_ms: i64, device: Option<Arc<DeviceId>>) -> Self {
        Self::build(replies, Box::new(FixedClock(now_ms)), device)
    }

    /// HTTP + 可变时钟（等完整点后要往前走）+ 可选的 deviceId 来源。
    fn advancing(replies: &[&str], device: Option<Arc<DeviceId>>) -> Self {
        Self::build(
            replies,
            Box::new(AdvancingClock(AtomicUsize::new(0))),
            device,
        )
    }

    fn build(replies: &[&str], clock: Box<dyn Clock>, device: Option<Arc<DeviceId>>) -> Self {
        let device_fn = device.as_ref().map(|d| d.as_fn());
        Self {
            http: Scripted::new(replies),
            clock,
            device_fn,
        }
    }

    /// 让第 n 个请求（1 起）在传输层失败。
    fn failing_at(mut self, n: usize) -> Self {
        self.http = self.http.failing_at(n);
        self
    }

    /// 链上那份环境（带 deviceId 来源 —— 没有的话就是 `None`）。
    fn env(&self) -> ExchangeEnv<'_> {
        ExchangeEnv {
            http: &self.http,
            clock: self.clock.as_ref(),
            device_id: self.device_fn.as_deref(),
        }
    }

    /// 故意**不**带 deviceId 来源的那一份（测"没接上来源"那一档）。
    fn env_without_device_id(&self) -> ExchangeEnv<'_> {
        ExchangeEnv {
            http: &self.http,
            clock: self.clock.as_ref(),
            device_id: None,
        }
    }

    fn http(&self) -> &Scripted {
        &self.http
    }
}

/// 装配一份"链上标准的选项"：`skip_wait = true`，不注入等待。
fn ready_opts(fx: &Fixture) -> ExchangeOptions<'_> {
    ExchangeOptions {
        prize: "251230053",
        skip_wait: true,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: fx.env().device_id,
        wait: None,
    }
}

// ── 响应构造 ────────────────────────────────────────────────────────────

/// 一件商品的清单条目（实拍字段，可逐项覆盖）。
fn prize_json(limit: i64, price: i64, count: i64) -> serde_json::Value {
    serde_json::json!({
        "prizeId": 251230053,
        "prizeName": "移动云盘100G云空间月卡",
        "groupId": 1,
        "type": 1,
        "pOrder": price,
        "POrder": price,           // 实拍：两个键同时存在
        "limit": limit,
        "count": count,
        "totalCount": 839,
        "dailyCount": 100,
        "dailyRemainderCount": 12,
        "minRemainderCount": 1,
        "onLine": 1,
        "nextTimeStatus": 1,
        "prizeSectionStatus": 1,
        "startTime": serde_json::Value::Null,
        "endTime": serde_json::Value::Null,
        "marketName": "sign_in_3_ex"
    })
}

/// `exchangeList` 响应，单组。
fn list_body(prize: serde_json::Value) -> String {
    serde_json::json!({"code": 0, "msg": "success", "result": {"1": [prize]}}).to_string()
}

/// `exchangeList` 响应，多件同组。
fn list_body_many(prizes: Vec<serde_json::Value>) -> String {
    serde_json::json!({"code": 0, "msg": "success", "result": {"1": prizes}}).to_string()
}

/// `infoV3`：余额是 `total`。
/// `visitlog/journaling` 的响应 —— **只读埋点**，链上只做 `ensure_http_ok`，
/// 所以结果体是空对象即可（字段没有语义，别在这里编造上游字段）。
fn visitlog_body() -> String {
    serde_json::json!({ "code": 0, "msg": "success", "result": {} }).to_string()
}

fn info_body(total: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": total,
            "beforeTotal": 8027,      // 实拍：与 total **不相等**，别读错那个
            "signInPoints": 3,
            "signCount": 1,
            "monthDays": 7,
            "cal": [{"d": 19, "s": true, "n": 3, "p": 0, "t": true, "currentMonth": 1}]
        }
    })
    .to_string()
}

/// `exchangeV3` 成功（扣 `cost` 豆）。
fn order_body(cost: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "oid": 900123456,
            "prizeId": 251230053,
            "prizeName": "移动云盘100G云空间月卡",
            "uorder": cost,
            "insertTime": "2026-09-19 14:30:00",
            "expireTime": "2026-10-19 14:30:00",
            "marketName": "sign_in_3_ex",
            "linkUrl": "251230053"
        }
    })
    .to_string()
}

/// `exchangeV3` 业务失败（**没有 result 键** —— 实拍 `514` 就是这个形状）。
fn v3_fail(code: i64, msg: &str) -> String {
    serde_json::json!({"code": code, "msg": msg}).to_string()
}

/// 清单（可兑）+ 余额：够走到"取题"那一步。
fn replies_ready() -> Vec<String> {
    vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
    ]
}

/// 清单 + 余额 + 题面 + 成交 + 复查（余额少 1800）。
fn replies_ordered() -> Vec<String> {
    vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        order_body(1800),
        info_body(8696),
    ]
}

/// 把 `Vec<String>` 借成 `&[&str]`。
fn as_refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

// ── 合成 PNG（真能被解算器解出来的题面）──────────────────────────────

/// 确定性的纹理：有足够变化，方差不会退化。
///
/// # ⚠ 这条纹理**逐字**沿用 `mcloud_crypto::slide` 自己那套夹具
///
/// 见 `crates/mcloud-crypto/src/slide/tests.rs` 的 `texture_at`。
/// 别把它换成"看起来更随机"的哈希纹理 —— 这是一条**实测**结论：
///
/// 口径②（轮廓打分）把**拼图轮廓点**投到底图的 Sobel 边缘图上求和，
/// 取和最大的落点。哈希噪声让底图**处处**都是强边缘 ⇒ 和值对落点几乎不敏感
/// ⇒ 噪声赢过真值，两口径差出 56px（① 172 vs ② 228），解算器按设计回
/// "分歧过大，不要提交"。而这条线性纹理的边缘结构单调、真值处显著，
/// 两个口径都落在真值 ±1。
///
/// 换句话说：**"线性 + 取模"是能让解算器解出来的一部分，不是随便挑的。**
fn texture_at(x: usize, y: usize) -> u8 {
    let v = (x * 7 + y * 13 + (x * y) % 31) % 256;
    (v as u8).wrapping_add(20)
}

/// ⭐ **真能解出来**的一对滑块 PNG（底图调色板图 + 拼图 RGBA）。
///
/// 链上那条"算不出 offset 就一枪不发"的判据，只有在一个**真能解出**的题面上
/// 才能对照着测。
///
/// # 造法与解算器的假设一致
///
/// 缺口是"原图压暗一半"，拼图块取的是**原值的两倍** ⇒ 正确落点上
/// `底图 = 拼图 / 2` 精确成立，这正是口径①假设的那个关系。
///
/// ⚠ 走的是与生产**同一条** [`mcloud_crypto::solve_slide`]，不 mock 解算器：
/// mock 了就等于把"解算失败 ⇒ 不发"这条判据测成一句空话。
///
/// ⚠ 为什么**不**读 `test/fixtures/slide-evidence/` 那两张真机图：
/// 它们在 `.gitignore` 里（**刻意不入库**，见 `crates/mcloud-crypto/tests/slide_parity.rs`），
/// 拿它们当夹具会让这套测试在干净检出上直接失败。所以这里自造。
fn slide_pngs() -> (Vec<u8>, Vec<u8>) {
    const BG_W: usize = 300;
    const BG_H: usize = 160;
    const BLOCK_W: usize = 60;
    const BLOCK_Y0: usize = 40;
    const BLOCK_Y1: usize = 100;
    /// 真值：块的左边缘落在底图的这个 x（与下面的深度值不重叠）。
    const NOTCH_X: usize = 173;

    // 底图：缺口处压暗一半。
    let mut bg_px = vec![0u8; BG_W * BG_H];
    for y in 0..BG_H {
        for x in 0..BG_W {
            let mut v = texture_at(x, y);
            if (NOTCH_X..NOTCH_X + BLOCK_W).contains(&x) && (BLOCK_Y0..BLOCK_Y1).contains(&y) {
                v /= 2;
            }
            bg_px[y * BG_W + x] = v;
        }
    }

    // 拼图：块在 x=0 起、y 与底图对齐。
    //
    // ⚠⚠ 块里装的是**缺口值 × 2**（不是原值、也不是缺口值本身）——
    // 这一条是**实测**定的，不是推出来的：两种写法各跑一遍，
    // `texture_at(...)`（原值）得到分歧 129px（① 157 vs ② 28），
    // 而 `(texture_at(...)/2) * 2` 得到分歧 1px（① 173 vs ② 174）并把答案
    // 落在真值 173 上。口径②（轮廓打分）吃的是"拼图轮廓点处的底图边缘和"，
    // 而 `t/2` 的整数取整会在缺口边缘造出一条**与底图不一致**的灰度台阶，
    // 原值写法下那条台阶与底图缺口的边缘错开 ⇒ 和值最大的落点跑偏。
    //
    // 所以这两行（底图 `v /= 2`、拼图 `(notch)*2`）必须**成对**出现：
    // 它们共同保证"缺口处 `底图 ≈ 拼图 / 2`"，而边缘又落在同一处。
    // 这与 `mcloud_crypto::slide` 自己那套夹具逐字一致
    // （`crates/mcloud-crypto/src/slide/tests.rs` 的 `synthetic_slide`）。
    //
    // ⚠ 这条约束的代价说清楚：真值允许 ±1 的固有偏差，所以下面的断言都用
    // "解出来了"（而不是"offset == 173"）—— 精确对拍在真机证据图上
    // （`crates/mcloud-crypto/tests/slide_parity.rs`），不在这里冒充。
    let pz_w = BLOCK_W + 8; // 块右侧留 8 列透明
    let pz = synthetic_png(
        pz_w as u32,
        BG_H as u32,
        &|x, y| {
            let (x, y) = (x as usize, y as usize);
            if x < BLOCK_W && (BLOCK_Y0..BLOCK_Y1).contains(&y) {
                let notch = texture_at(NOTCH_X + x, y) / 2;
                notch * 2
            } else {
                0 // 透明区：灰度给什么都行，反正 alpha = 0
            }
        },
        // ⚠ **块必须从 x = 0 起不透明**（见 `synthetic_png` 那段：削掉最左几列
        // 会让两个口径都跑偏）。只有块之外才是透明的。
        &|x, y| {
            let (x, y) = (x as usize, y as usize);
            x < BLOCK_W && (BLOCK_Y0..BLOCK_Y1).contains(&y)
        },
    );

    // 底图刻意走**调色板**形态（与实拍同形：底图是 8-bit 索引图）。
    let palette: Vec<u8> = (0..256u16)
        .flat_map(|i| [i as u8, i as u8, i as u8])
        .collect();
    let bg = indexed_png(BG_W as u32, BG_H as u32, &palette, &bg_px);
    (bg, pz)
}

/// 把 8-bit 索引图编码成 PNG（底图那一形态）。
fn indexed_png(width: u32, height: u32, palette: &[u8], pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((height * (1 + width)) as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: None
        raw.extend_from_slice(&pixels[y * width as usize..(y + 1) * width as usize]);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // depth 8、colortype 3（Indexed）
    ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
    out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&png_chunk(b"PLTE", palette));
    out.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&raw)));
    out.extend_from_slice(&png_chunk(b"IEND", &[]));
    out
}

/// zlib 容器 + stored（未压缩）DEFLATE 块 + Adler-32。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut zlib = vec![0x78, 0x01];
    let mut remaining = raw;
    while !remaining.is_empty() {
        let take = remaining.len().min(65_535);
        let last = take == remaining.len();
        zlib.push(if last { 1 } else { 0 });
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(&remaining[..take]);
        remaining = &remaining[take..];
    }
    let mut adler_a: u32 = 1;
    let mut adler_b: u32 = 0;
    for &byte in raw {
        adler_a = (adler_a + u32::from(byte)) % 65521;
        adler_b = (adler_b + adler_a) % 65521;
    }
    zlib.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());
    zlib
}

fn png_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 12);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// 一张合成的 **RGBA** PNG（拼图那一形态）。
///
/// `pixel` 给每个像素的灰度（r=g=b）；**alpha 由 `opaque` 决定**。
///
/// # ⚠ alpha 由调用方给，**不能**在这里按 `x < 4` 一刀切
///
/// 曾经的写法是"左侧 4 列透明"，那是照着"拼图是通高竖条、左边留白"的样子
/// 拍的 —— 但**块本身就必须从 x=0 起**（实拍与解算器都按这个假设：
/// 拼图块的左边缘就是块在底图里的落点）。把最左 4 列强行透明，等于
/// **削掉了块的左边缘**，于是：
///
/// - 口径① 少一份样本（拟合仍能过，落在 157 这种偏掉的值上）；
/// - 口径② 的轮廓点集整个变形（落在 28）。
///   实测分歧 129px ⇒ 解算器按设计回"不要提交"。
///
/// 那条错误的表面症状是"解算器说题面有问题"，真因却是**夹具把块切了**。
fn synthetic_png(
    width: u32,
    height: u32,
    pixel: &dyn Fn(u32, u32) -> u8,
    opaque: &dyn Fn(u32, u32) -> bool,
) -> Vec<u8> {
    let channels: u32 = 4; // RGBA
    let mut raw = Vec::with_capacity((height * (1 + width * channels)) as usize);
    for y in 0..height {
        raw.push(0); // filter: None
        for x in 0..width {
            let v = pixel(x, y);
            // RGBA：**恰好** 4 字节/像素（r,g,b,a）。
            //
            // ⚠ 这里曾经写成"先 push 1 个、再循环 push 3 个、最后 push alpha"
            // = **5 字节/像素**，比 IHDR 声明（colortype 6）多一个。
            // 每一行因此比声明的长 1 字节，行与行错位 ⇒ 解码器把**像素值**
            // 当成下一行的 filter 字节，报 `Unknown filter method 200`
            // （200 正是那第 5 个字节）。那条错误会把人引向"PNG 容器坏了"，
            // 而容器与 zlib 都是对的 —— 所以下面那行长度断言必须留着。
            raw.push(v); // r
            raw.push(v); // g
            raw.push(v); // b
            raw.push(if opaque(x, y) { 255 } else { 0 }); // a
        }
    }
    // 行宽必须与 IHDR 声明**逐字**一致：这个编码器是手写的，
    // 多一个字节不会报"格式错"，只会让解码器在下一行报一个**看起来无关**的错。
    assert_eq!(
        raw.len(),
        (height * (1 + width * channels)) as usize,
        "raw 长度必须与 IHDR 声明的一致（每行 1 字节 filter + width × channels）"
    );
    encode_png(width, height, 6, &raw)
}

/// 最小 PNG 编码器（只在测试里用，避免为测试引一个新依赖）。
fn encode_png(width: u32, height: u32, color_type: u8, raw: &[u8]) -> Vec<u8> {
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(raw)));
    out.extend_from_slice(&png_chunk(b"IEND", &[]));
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

/// `getSlide` 响应：两张真 PNG 的 base64。
fn slide_body() -> String {
    let (bg, pz) = slide_pngs();
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "picture": b64(&bg),
            "puzzle": b64(&pz),
            "picWidth": "200",
            "picHeight": "60",
            "puzzleWidth": "20",
            "puzzleLeft": serde_json::Value::Null
        }
    })
    .to_string()
}

/// `getSlide` 响应，但图是**坏**的（不是 PNG）⇒ 解算必然失败。
fn slide_body_broken() -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": { "picture": b64(b"not a png"), "puzzle": b64(b"neither") }
    })
    .to_string()
}

/// 最小 base64 编码（本 crate 的 dev-dependencies 里没有 base64，只有 hex）。
fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ── 时间工具（测试侧的**独立**实现，用来与生产实现对照）──────────────

/// 东八区某个时刻的 unix 毫秒。
///
/// # 为什么**不**用 `chrono`
///
/// `mcloud-tasks` 的 `Cargo.toml` 没有 `chrono` 依赖（引不到），
/// 而这里刻意走**独立**的一套算法（逐月累加 + 闰年判断），
/// 与模块里那套 `days_from_civil` 写法完全不同 ——
/// 两边都从同一个实现抄就没有对照意义了。
fn beijing_ms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> i64 {
    days_since_epoch(year, month, day) * 86_400_000
        + i64::from(hour) * 3_600_000
        + i64::from(min) * 60_000
        + i64::from(sec) * 1_000
        - 8 * 3_600_000
}

/// 1970-01-01 起的天数（与模块里那个算术版本写法不同）。
fn days_since_epoch(year: i32, month: u32, day: u32) -> i64 {
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let lengths = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for len in lengths.iter().take(month as usize - 1) {
        days += i64::from(*len);
    }
    days + i64::from(day) - 1
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 把 `"2006-01-02 15-04-05"` 拆开交给 [`beijing_ms`]。
fn beijing_ms_of(raw: &str) -> i64 {
    let (date, time) = raw.split_once(' ').unwrap();
    let d: Vec<i64> = date.split('-').map(|s| s.parse().unwrap()).collect();
    let t: Vec<i64> = time.split('-').map(|s| s.parse().unwrap()).collect();
    beijing_ms(
        d[0] as i32,
        d[1] as u32,
        d[2] as u32,
        t[0] as u32,
        t[1] as u32,
        t[2] as u32,
    )
}

// ══════════════════════════════ 判定层：逐档 ══════════════════════════════

/// 一个"全放行"的基线奖品：改哪一项，就测哪一档。
fn baseline() -> ExchangePrize {
    ExchangePrize {
        prize_id: 251230053,
        name: Some("移动云盘100G云空间月卡".into()),
        price: Some(1800),
        limit: Some(LIMIT_AVAILABLE),
        count: Some(839),
        daily_remainder_count: Some(12),
        min_remainder_count: Some(1),
        on_line: Some(1),
        ..ExchangePrize::default()
    }
}

fn parse_list(body: &str) -> ExchangeList {
    let env = SignInEnvelope::parse(body).unwrap();
    mcloud_upstream::exchange::parse_exchange_list(&env).unwrap()
}

/// 基线本身必须判 `Go` —— 否则下面每一条"改一项"的断言都测不出东西。
#[test]
fn baseline_prize_is_go() {
    assert_eq!(
        plan_exchange(&baseline(), Some(10496), BASE_NOW_MS),
        ExchangePlan::Go
    );
}

/// 1/17 `Go`：判据都有值且都放行。
#[test]
fn tier_go_when_everything_passes() {
    let plan = plan_exchange(&baseline(), Some(1800), BASE_NOW_MS);
    assert_eq!(
        plan,
        ExchangePlan::Go,
        "余额恰好等于价格应当放行（判据是严格大于）"
    );
    assert!(!plan.blocks(), "Go 是不拦的那一档");
}

/// 2/17 `NotFound`：清单里没有这一件。
#[test]
fn tier_not_found_when_list_has_no_such_prize() {
    let list = parse_list(&list_body(prize_json(0, 1800, 839)));

    assert_eq!(
        locate_exchange_prize(&list, "251230999").1,
        ExchangePlan::NotFound
    );
    assert_eq!(
        locate_exchange_prize(&list, "不存在的商品").1,
        ExchangePlan::NotFound
    );
    assert_eq!(
        locate_exchange_prize(&list, "   ").1,
        ExchangePlan::NotFound
    );

    let (prize, plan) = locate_exchange_prize(&list, "251230053");
    assert_eq!(plan, ExchangePlan::Go);
    assert_eq!(prize.unwrap().prize_id, 251230053);
}

/// 3/17 `Ambiguous`：同名多件 —— 不许凭猜下单。
#[test]
fn tier_ambiguous_when_two_prizes_share_a_name() {
    let mut a = prize_json(0, 1800, 839);
    let mut b = prize_json(0, 1800, 839);
    a["prizeId"] = serde_json::json!(251230053);
    b["prizeId"] = serde_json::json!(251230054);
    let list = parse_list(&list_body_many(vec![a, b]));

    let (prize, plan) = locate_exchange_prize(&list, "移动云盘100G云空间月卡");
    assert_eq!(plan, ExchangePlan::Ambiguous);
    assert!(prize.is_none(), "同名多件时不能挑一件出来");
    assert_eq!(
        locate_exchange_prize(&list, "251230054").1,
        ExchangePlan::Go,
        "按 id 定位不受同名影响"
    );
}

/// 3b/17 名称匹配是**精确相等**，不是包含。
///
/// 实拍就有"月卡 / 年卡 / 季卡"这一类同前缀商品：包含匹配会把
/// "我要月卡"打到别的**一件**上 —— 而这是真扣豆。
#[test]
fn name_matching_is_exact_not_substring() {
    let mut month = prize_json(0, 1800, 839);
    month["prizeName"] = serde_json::json!("移动云盘100G云空间月卡");
    let mut year = prize_json(0, 1800, 839);
    year["prizeId"] = serde_json::json!(251230054);
    year["prizeName"] = serde_json::json!("移动云盘100G云空间年卡");
    let list = parse_list(&list_body_many(vec![month, year]));

    assert_eq!(
        locate_exchange_prize(&list, "云空间").1,
        ExchangePlan::NotFound,
        "包含匹配会打到别的商品上，宁可说没找到"
    );
    assert_eq!(
        locate_exchange_prize(&list, "移动云盘100G云空间年卡").1,
        ExchangePlan::Go
    );
}

/// 4/17 `OffShelf`：`onLine` 明确不是 1。
#[test]
fn tier_off_shelf_when_on_line_is_not_one() {
    let mut prize = baseline();
    prize.on_line = Some(0);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::OffShelf
    );
    // ⚠ 非 1 的取值零样本，但"不是 1"就该挡 —— 这里用一个别的值钉住语义。
    prize.on_line = Some(2);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::OffShelf
    );
    // `None` = 上游没给 ⇒ 不挡（判据是"明确不是 1"）。
    prize.on_line = None;
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Go
    );
}

/// 5/17 `LimitMissing`：上游没给 `limit` ⇒ 判不了，不发。
#[test]
fn tier_limit_missing_when_upstream_omits_it() {
    let mut prize = baseline();
    prize.limit = None;
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::LimitMissing
    );
}

/// 6/17 `LimitTomorrow`：`limit == 1`（H5「明日再来」）。**零样本**。
#[test]
fn tier_limit_tomorrow() {
    let mut prize = baseline();
    prize.limit = Some(LIMIT_TOMORROW);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::LimitTomorrow
    );
}

/// 7/17 `ExchangedThisPrize`：`limit == 2`。⭐ **有字段依据**的一档。
#[test]
fn tier_exchanged_this_prize() {
    let mut prize = baseline();
    prize.limit = Some(LIMIT_EXCHANGED);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::ExchangedThisPrize
    );
}

/// 8/17 `LimitThree`：两处文案冲突 ⇒ 不猜。
#[test]
fn tier_limit_three_is_not_guessed() {
    let mut prize = baseline();
    prize.limit = Some(LIMIT_UNKNOWN3);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::LimitThree
    );
}

/// 9/17 `ExchangedThisGroup`：`limit == 4`（组内互斥）。⭐ **有字段依据**。
#[test]
fn tier_exchanged_this_group() {
    let mut prize = baseline();
    prize.limit = Some(LIMIT_GROUP_EXCHANGED);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::ExchangedThisGroup
    );
}

/// 10/17 `LimitUnknown`：不认识的 `limit` ⇒ 别猜。
#[test]
fn tier_limit_unknown_for_any_other_value() {
    let mut prize = baseline();
    for weird in [5, 9, 99, -1] {
        prize.limit = Some(weird);
        assert_eq!(
            plan_exchange(&prize, Some(10496), BASE_NOW_MS),
            ExchangePlan::LimitUnknown,
            "limit={weird} 不认识就不认，绝不落回可兑"
        );
    }
}

/// 11/17 `MinRemainderZero`：`minRemainderCount == 0`。
#[test]
fn tier_min_remainder_zero() {
    let mut prize = baseline();
    prize.min_remainder_count = Some(0);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::MinRemainderZero
    );
    // ⚠ 判据是 `== 0`，`< 0`（不该出现）**不**落这一档 —— 照参考实现钉住。
    prize.min_remainder_count = Some(-1);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Go,
        "minRemainderCount 的判据是 == 0，不是 <= 0"
    );
}

/// 12/17 `SoldOut`：`count <= 0`。
#[test]
fn tier_sold_out() {
    let mut prize = baseline();
    prize.count = Some(0);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::SoldOut
    );
    prize.count = Some(-3);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::SoldOut
    );
}

/// 13/17 `DailyStockEmpty`：`dailyRemainderCount <= 0`。
#[test]
fn tier_daily_stock_empty() {
    let mut prize = baseline();
    prize.daily_remainder_count = Some(0);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::DailyStockEmpty
    );
    prize.daily_remainder_count = Some(-1);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::DailyStockEmpty
    );
}

/// 14/17 `PriceMissing`：上游没给价格 ⇒ "够不够"判不了。
#[test]
fn tier_price_missing() {
    let mut prize = baseline();
    prize.price = None;
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::PriceMissing
    );
    // 没有余额读数时**同样**挡 —— 缺判据只有停下来这一种处置。
    assert_eq!(
        plan_exchange(&prize, None, BASE_NOW_MS),
        ExchangePlan::PriceMissing
    );
}

/// 15/17 `InsufficientCloud`：余额低于价格。
#[test]
fn tier_insufficient_cloud() {
    let prize = baseline(); // 价 1800
    assert_eq!(
        plan_exchange(&prize, Some(1799), BASE_NOW_MS),
        ExchangePlan::InsufficientCloud
    );
    assert_eq!(
        plan_exchange(&prize, Some(1800), BASE_NOW_MS),
        ExchangePlan::Go,
        "恰好够就必须放行（判据是严格大于）"
    );
    // ⚠ 没有余额读数 ⇒ 这一档**判不出**（界面上的选品器就是这条路）。
    assert_eq!(
        plan_exchange(&prize, None, BASE_NOW_MS),
        ExchangePlan::Go,
        "没有余额读数时不判「够不够」，这是刻意的（口径比下单前宽）"
    );
}

/// 16/17 `NotStarted`：`startTime` 解得出、且还没到。
#[test]
fn tier_not_started() {
    let mut prize = baseline();
    // 实拍形态：时分秒之间也是 `-`。
    prize.start_time = Some("2026-10-01 00-00-00".into());
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::NotStarted
    );
    // 到了那一刻就该放行（判据是 `now < start`，边界属于"已开始"）。
    let at_start = beijing_ms(2026, 10, 1, 0, 0, 0);
    assert_eq!(
        plan_exchange(&prize, Some(10496), at_start),
        ExchangePlan::Go
    );
}

/// 17/17 `Ended`：`endTime` 解得出、且已经过了。
#[test]
fn tier_ended() {
    let mut prize = baseline();
    prize.end_time = Some("2026-09-01 00-00-00".into());
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Ended
    );
    // 边界：恰好等于 endTime 不算过（判据是 `now > end`）。
    let at_end = beijing_ms(2026, 9, 1, 0, 0, 0);
    assert_eq!(plan_exchange(&prize, Some(10496), at_end), ExchangePlan::Go);
}

/// ⭐ **十七档逐一可达** —— 这条用例盯着"将来加了一档却没人测"。
#[test]
fn each_plan_tier_is_reachable() {
    assert_eq!(ExchangePlan::ALL.len(), 17, "档位数是契约的一部分");

    let mut reached: Vec<ExchangePlan> = Vec::new();
    let mut push = |plan: ExchangePlan| {
        if !reached.contains(&plan) {
            reached.push(plan);
        }
    };

    let base = baseline();
    push(plan_exchange(&base, Some(10496), BASE_NOW_MS));
    for limit in [
        None,
        Some(LIMIT_TOMORROW),
        Some(LIMIT_EXCHANGED),
        Some(LIMIT_UNKNOWN3),
        Some(LIMIT_GROUP_EXCHANGED),
        Some(7),
    ] {
        let mut prize = base.clone();
        prize.limit = limit;
        push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    }
    let mut prize = base.clone();
    prize.on_line = Some(0);
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.min_remainder_count = Some(0);
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.count = Some(0);
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.daily_remainder_count = Some(0);
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.price = None;
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    push(plan_exchange(&base, Some(1), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.start_time = Some("2026-10-01 00-00-00".into());
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));
    let mut prize = base.clone();
    prize.end_time = Some("2026-09-01 00-00-00".into());
    push(plan_exchange(&prize, Some(10496), BASE_NOW_MS));

    // 前两档只能由定位层造出来。
    let list = parse_list(&list_body(prize_json(0, 1800, 839)));
    push(locate_exchange_prize(&list, "没有这件").1);
    let mut a = prize_json(0, 1800, 839);
    let mut b = prize_json(0, 1800, 839);
    a["prizeId"] = serde_json::json!(251230053);
    b["prizeId"] = serde_json::json!(251230054);
    let dup = parse_list(&list_body_many(vec![a, b]));
    push(locate_exchange_prize(&dup, "移动云盘100G云空间月卡").1);

    for tier in ExchangePlan::ALL {
        assert!(
            reached.contains(&tier),
            "档位 {} 没有任何用例构造出来过 —— 加档必须同时加用例",
            tier.slug()
        );
    }
    assert_eq!(reached.len(), 17, "十七档都该被构造到");
}

/// 十七档**逐一**有 slug、有 reason，且 slug 互不相同。
#[test]
fn every_exchange_plan_has_a_distinct_slug_and_a_reason() {
    let mut slugs = std::collections::BTreeSet::new();
    for tier in ExchangePlan::ALL {
        assert!(!tier.slug().is_empty(), "{} 缺 slug", tier as i32);
        assert!(!tier.reason().is_empty(), "{} 缺 reason", tier.slug());
        assert!(
            slugs.insert(tier.slug()),
            "slug 重复：{} —— 前端颜色表会静默错配",
            tier.slug()
        );
        // reason 是给人看的中文，不能是 slug 那种机器名。
        assert!(
            tier.reason()
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "{} 的 reason 应当是中文",
            tier.slug()
        );
    }
    assert_eq!(slugs.len(), 17);
    // 零值必须是 Go（参考实现同形），且**只有** Go 不拦。
    assert_eq!(ExchangePlan::default(), ExchangePlan::Go);
    for tier in ExchangePlan::ALL {
        assert_eq!(
            tier.blocks(),
            tier != ExchangePlan::Go,
            "{} 的 blocks() 只能是「不是 Go」",
            tier.slug()
        );
    }
}

// ══════════════════════════════ 判定顺序 ══════════════════════════════

/// ⭐ **顺序**也是契约：`onLine` → `limit` → 三个余量 → 价格 → 余额 → 时间窗。
///
/// 同时违反多条时，报出来的必须是**前面**那一条 —— 否则用户看的那一句
/// 与"该等明天还是该换一件"对不上。
#[test]
fn judgement_order_is_a_contract() {
    // 全违反：报的应当是 onLine（第一档）。
    let mut prize = baseline();
    prize.on_line = Some(0);
    prize.limit = None;
    prize.min_remainder_count = Some(0);
    prize.count = Some(0);
    prize.daily_remainder_count = Some(0);
    prize.price = None;
    prize.start_time = Some("2030-01-01 00-00-00".into());
    assert_eq!(
        plan_exchange(&prize, Some(0), BASE_NOW_MS),
        ExchangePlan::OffShelf
    );

    // 去掉 onLine ⇒ 报 limit 缺失。
    let mut next = prize.clone();
    next.on_line = Some(1);
    assert_eq!(
        plan_exchange(&next, Some(0), BASE_NOW_MS),
        ExchangePlan::LimitMissing
    );

    // limit 放行 ⇒ 报余量阈值（三个余量里第一个）。
    next.limit = Some(LIMIT_AVAILABLE);
    assert_eq!(
        plan_exchange(&next, Some(0), BASE_NOW_MS),
        ExchangePlan::MinRemainderZero
    );

    // 余量阈值放行 ⇒ 报 count。
    next.min_remainder_count = Some(5);
    assert_eq!(
        plan_exchange(&next, Some(0), BASE_NOW_MS),
        ExchangePlan::SoldOut
    );

    // count 放行 ⇒ 报今日余量。
    next.count = Some(5);
    assert_eq!(
        plan_exchange(&next, Some(0), BASE_NOW_MS),
        ExchangePlan::DailyStockEmpty
    );

    // 余量都放行 ⇒ 报价格缺失（**先于**余额与时间窗）。
    next.daily_remainder_count = Some(5);
    assert_eq!(
        plan_exchange(&next, Some(0), BASE_NOW_MS),
        ExchangePlan::PriceMissing
    );

    // 有价格但余额不够 ⇒ 报余额（**先于**时间窗）。
    next.price = Some(1800);
    assert_eq!(
        plan_exchange(&next, Some(10), BASE_NOW_MS),
        ExchangePlan::InsufficientCloud
    );

    // 余额够 ⇒ 才轮到时间窗。
    assert_eq!(
        plan_exchange(&next, Some(99999), BASE_NOW_MS),
        ExchangePlan::NotStarted
    );
}

// ══════════════════════════════ 时间窗三态 ══════════════════════════════

/// ⭐ 时间窗三态：**两边都解不出 ⇒ 判不了 ⇒ 放行**。
///
/// ⚠ 与趣玩AI那一族**刻意相反**（那里判不了就不发）。理由照搬参考实现：
/// 这一族的硬判据是 `limit` 与余额，而 `startTime` / `endTime` 更像
/// "这一件挂在页面上的时间"；挡住一侧的代价是**所有**没给窗口的商品都兑不了。
#[test]
fn window_is_unknown_and_lets_through_when_neither_side_parses() {
    let prize = baseline();
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Go,
        "两边都没给 ⇒ 判不了 ⇒ 放行"
    );
    // 给了但形态不认识，一样是"这一边判不了"。
    let mut prize = baseline();
    prize.start_time = Some("不是时间".into());
    prize.end_time = Some("2026/09/01 00:00:00".into());
    assert_eq!(
        exchange_window_of(&prize, BASE_NOW_MS),
        ExchangeWindow::Unknown
    );
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Go
    );
}

/// 解不出的那一边**不参与**判定；另一边解得出来就按它判。
#[test]
fn unparsable_side_does_not_participate() {
    // start 解不出，end 解得出且已过 ⇒ 按 end 判 Ended。
    let mut prize = baseline();
    prize.start_time = Some("垃圾".into());
    prize.end_time = Some("2026-09-01 00-00-00".into());
    assert_eq!(exchange_window_of(&prize, BASE_NOW_MS), ExchangeWindow::Out);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Ended
    );

    // 反过来：end 解不出，start 解得出且还没到 ⇒ NotStarted。
    let mut prize = baseline();
    prize.start_time = Some("2026-10-01 00-00-00".into());
    prize.end_time = Some("垃圾".into());
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::NotStarted
    );

    // 一边解不出、另一边在期内 ⇒ 放行。
    let mut prize = baseline();
    prize.start_time = Some("垃圾".into());
    prize.end_time = Some("2026-10-01 00-00-00".into());
    assert_eq!(exchange_window_of(&prize, BASE_NOW_MS), ExchangeWindow::In);
    assert_eq!(
        plan_exchange(&prize, Some(10496), BASE_NOW_MS),
        ExchangePlan::Go
    );
}

/// ⚠⚠ 时分秒之间是 **`-`**，不是 `:`。
///
/// 按 `:` 解会让所有带窗口的商品判成"解不出" ⇒ 三态放行 ⇒
/// **整条时间窗规则静默失效**。这条用例就是防止有人"顺手改成标准形态"。
#[test]
fn time_layout_uses_dashes_between_hms() {
    assert_eq!(TIME_LAYOUT, "%Y-%m-%d %H-%M-%S");

    let mut prize = baseline();
    prize.end_time = Some("2026-09-01 00:00:00".into()); // 冒号形态
    assert_eq!(
        exchange_window_of(&prize, BASE_NOW_MS),
        ExchangeWindow::Unknown,
        "冒号形态**解不出**（实拍是短横线）—— 解不出就放行，这是刻意的三态"
    );

    prize.end_time = Some("2026-09-01 00-00-00".into());
    assert_eq!(
        exchange_window_of(&prize, BASE_NOW_MS),
        ExchangeWindow::Out,
        "短横线形态必须解得出"
    );
}

/// 时间串解出来的时刻必须是**公认的那一个**（东八区）。
///
/// # 对照基准是写死的常量 + 一套独立的算法
///
/// 四个时刻是公开可查的固定值（东八区墙钟 → unix 秒）：
///
/// | 东八区墙钟 | unix 秒 | 为什么选它 |
/// |---|---|---|
/// | `1970-01-01 08-00-00` | `0` | 纪元对齐点：东八区在这个时刻才是 0 |
/// | `2000-03-01 00-00-00` | `951_840_000` | 世纪闰年（2000 是闰年）之后 |
/// | `2026-09-01 00-00-00` | `1_788_192_000` | 实拍那条 `startTime` 的月份 |
/// | `2026-09-30 23-59-59` | `1_790_783_999` | 实拍那条 `endTime` |
///
/// ⚠ 这四个值只在东八区成立 —— 换了时区立刻错 8 小时。
#[test]
fn time_parsing_matches_known_epoch_values() {
    for (raw, expected_secs) in [
        ("1970-01-01 08-00-00", 0i64),
        ("2000-03-01 00-00-00", 951_840_000),
        ("2026-09-01 00-00-00", 1_788_192_000),
        ("2026-09-30 23-59-59", 1_790_783_999),
    ] {
        assert_eq!(
            parse_exchange_time(raw),
            Some(expected_secs * 1_000),
            "{raw} 应当解成 {expected_secs} 秒（东八区）"
        );
        assert_eq!(
            parse_exchange_time(raw),
            Some(beijing_ms_of(raw)),
            "{raw}：两套独立算法必须给出同一个时刻"
        );
    }
}

/// 非法时间串一律解不出（不被"规范化"成别的时刻）。
#[test]
fn malformed_times_are_rejected_not_normalised() {
    for bad in [
        "2026-13-01 00-00-00",   // 13 月
        "2026-09-32 00-00-00",   // 32 日
        "2026-02-30 00-00-00",   // 2 月没有 30 号
        "2026-09-01 24-00-00",   // 24 点
        "2026-09-01 00-60-00",   // 60 分
        "2026-09-01 00-00-60",   // 60 秒
        "2026-09-01",            // 没有时刻
        "2026-09-01 00-00",      // 缺秒
        "2026-09-01 00-00-00-1", // 多一段
        "2026-09-01-00-00-00",   // 日期与时刻之间没有空格
        "",
        "   ",
    ] {
        let mut prize = baseline();
        prize.start_time = Some(bad.to_string());
        assert_eq!(
            exchange_window_of(&prize, BASE_NOW_MS),
            ExchangeWindow::Unknown,
            "{bad:?} 不该被解出来"
        );
    }
    // 闰年那一条要正面钉住：2028-02-29 合法、2027-02-29 不合法。
    assert!(parse_exchange_time("2028-02-29 00-00-00").is_some());
    assert!(parse_exchange_time("2027-02-29 00-00-00").is_none());
    // 世纪闰年规则：2000 是闰年、1900 不是。
    assert!(parse_exchange_time("2000-02-29 00-00-00").is_some());
    assert!(parse_exchange_time("1900-02-29 00-00-00").is_none());
}

/// 负的时分秒不是时刻。
#[test]
fn negative_clock_parts_are_rejected() {
    assert!(parse_exchange_time("2026-09-01 -1-00-00").is_none());
    assert!(parse_exchange_time("2026-09-01 00--1-00").is_none());
}

/// `plan_exchange_wait`：整点后 20 秒内不等，否则等到下一个整点。
#[test]
fn wait_plan_waits_for_the_next_hour() {
    // 13:00:05 ⇒ 还在上一场尾巴里（< 20s）⇒ 不等。
    assert!(!plan_exchange_wait(beijing_ms(2026, 9, 19, 13, 0, 5)).needed);

    // 13:00:20 ⇒ 恰好达到阈值 ⇒ 等（判据是 `< 20s`）。
    let wait = plan_exchange_wait(beijing_ms(2026, 9, 19, 13, 0, 20));
    assert!(wait.needed);
    assert_eq!(
        wait.until_ms,
        beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS
    );

    // 13:30 ⇒ 等到 14:00。
    let wait = plan_exchange_wait(beijing_ms(2026, 9, 19, 13, 30, 0));
    assert_eq!(
        wait.until_ms,
        beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS
    );

    // 13:59:59 ⇒ 等到 14:00（只差一秒也要等）。
    let wait = plan_exchange_wait(beijing_ms(2026, 9, 19, 13, 59, 59));
    assert_eq!(
        wait.until_ms,
        beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS
    );

    // ⚠ 目标**不是**整点整，而是"整点 − 提前量"（TS 的 `offset`，默认 10ms）——
    // 这条断言就是那个差异的钉子：抢兑压着整点先手，不是等到整点才动。
    assert_eq!(
        wait.until_ms,
        beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS
    );
    assert!(
        wait.until_ms < beijing_ms(2026, 9, 19, 14, 0, 0),
        "必须早于整点"
    );
}
/// `offset`（**传输延迟补偿**）是**可输入的** ⇒ 它必须真的改变目标时刻。
///
/// 用户口径 2026-10-02：「10ms 是本地与云端的传输时间差」—— 提前 `offset` 发出，
/// 让请求正好在整点抵达云端。缺省 = 参考实现那个 `10`（`standalone.ts:213`）。
#[test]
fn wait_target_follows_the_inputtable_offset() {
    let now = beijing_ms(2026, 9, 19, 13, 30, 0);
    let hour = beijing_ms(2026, 9, 19, 14, 0, 0);
    assert_eq!(plan_exchange_wait_with(now, 10).until_ms, hour - 10);
    assert_eq!(plan_exchange_wait_with(now, 500).until_ms, hour - 500);
    // 旧签名 = 缺省补偿，行为必须与参考实现一致（10）。
    assert_eq!(plan_exchange_wait(now).until_ms, hour - EXCHANGE_LEAD_MS);
}

/// `plan_exchange_window_wait`：等到 **hh:59:40**；已经在窗口里 ⇒ 不等。
///
/// 边界逐条照 TS `utils.py`… 准确说 `utils.ts:57-65` 抄 —— 两条 `0` 的规则
/// （已过 59:40、整点后 15 秒内）是"上一场尾巴"的处理；写错就会在一场刚结束时
/// 又去等一个整小时。⚠ 本函数目前**只被测试用到**，接链在下一轮。
#[test]
fn window_plan_waits_for_hh_59_40() {
    // 13:30 ⇒ 等到 13:59:40。
    let w = plan_exchange_window_wait(beijing_ms(2026, 9, 19, 13, 30, 0));
    assert!(w.needed);
    assert_eq!(w.until_ms, beijing_ms(2026, 9, 19, 13, 59, 40));

    // 13:59:39 ⇒ 只差一秒也等（判据是"`>=` 59:40 才不等"）。
    let w = plan_exchange_window_wait(beijing_ms(2026, 9, 19, 13, 59, 39));
    assert_eq!(w.until_ms, beijing_ms(2026, 9, 19, 13, 59, 40));

    // 13:59:40 / 13:59:59 ⇒ 窗口已开 ⇒ 不等。
    assert!(!plan_exchange_window_wait(beijing_ms(2026, 9, 19, 13, 59, 40)).needed);
    assert!(!plan_exchange_window_wait(beijing_ms(2026, 9, 19, 13, 59, 59)).needed);

    // 14:00:00 / 14:00:14 ⇒ **上一场的尾巴**（整点后 15 秒内）⇒ 不等。
    assert!(!plan_exchange_window_wait(beijing_ms(2026, 9, 19, 14, 0, 0)).needed);
    assert!(!plan_exchange_window_wait(beijing_ms(2026, 9, 19, 14, 0, 14)).needed);

    // 14:00:15 ⇒ 尾巴结束 ⇒ 又要等到 14:59:40。
    let w = plan_exchange_window_wait(beijing_ms(2026, 9, 19, 14, 0, 15));
    assert!(w.needed);
    assert_eq!(w.until_ms, beijing_ms(2026, 9, 19, 14, 59, 40));
}

/// 等待的判据在**负时间戳**上也不该错（`%` 会对负输入给出负余数）。
#[test]
fn wait_plan_is_correct_for_negative_timestamps() {
    let at = -3_600_000 + 5_000; // -1 小时又 5 秒
    assert!(
        !plan_exchange_wait(at).needed,
        "负时间戳下也该判成「还在上一场尾巴里」，而不是把整点算到未来"
    );
}

// ══════════════════════════════ 链：不变式 ══════════════════════════════

/// ⭐ `ExchangeStatus` 的零值必须是 `NotAttempted`。
///
/// 理由（照搬参考实现）：本链唯一的写请求花的是真豆，而"漏填 status 的结果
/// 结构体"如果读成 `Ordered`，界面上就是一条绿色的"已兑到"。
#[test]
fn status_zero_value_is_not_attempted() {
    assert_eq!(ExchangeStatus::default(), ExchangeStatus::NotAttempted);
    assert_eq!(
        ExchangeOutcome::default().status,
        ExchangeStatus::NotAttempted
    );
    assert_eq!(ExchangeOutcome::default().attempts, 0);

    // 而且它必须落**失败**，不能静默成功。
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome::default(),
    };
    assert!(
        result.run_report().is_failure(),
        "零值结构体必须报失败 —— 读成成功会渲染成一条绿的假成交"
    );
    assert!(result.summary_line().contains("兑换没判成"));
}

/// ⭐ 清单读失败 ⇒ 一枪都不发，且不去换 deviceId。
#[tokio::test]
async fn a_failed_list_read_sends_nothing() {
    let device = DeviceId::ok("B-device");
    let fx = Fixture::new(
        &[r#"{"code":500,"msg":"boom"}"#],
        BASE_NOW_MS,
        Some(Arc::clone(&device)),
    );
    let opts = ready_opts(&fx);
    let err = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap_err();
    assert!(err.to_string().contains("读取兑换清单"), "{err}");
    assert_eq!(fx.http().attempts(), 0, "一条 exchangeV3 都不许发");
    assert_eq!(device.calls(), 0, "没放行之前不该去换 deviceId");
}

/// ⭐ 判据没过 ⇒ **一枪不发**（`attempts == 0` 是本模块最重要的不变式）。
#[tokio::test]
async fn a_rejected_plan_sends_nothing() {
    // limit=2（本月已兑）⇒ 判据在定位之后立刻挡住，连余额都不读。
    let device = DeviceId::ok("B-device");
    let list = list_body(prize_json(LIMIT_EXCHANGED, 1800, 839));
    let info = info_body(10496);
    let fx = Fixture::new(&[&list, &info], BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(outcome.plan, ExchangePlan::ExchangedThisPrize);
    assert_eq!(outcome.attempts, 0);
    assert_eq!(fx.http().attempts(), 0);
    // ⚠ 清单 + 余额**都要读**：余额是 `plan_exchange` 的一条判据（够不够豆），
    // 所以链是"先读两样、再判"，判不过就不取题、不换 deviceId。
    assert_eq!(
        fx.http().count(),
        2,
        "判据没过时读清单 + 余额，但不取题、不下单"
    );
    assert!(fx.http().url_at(0).contains("exchangeList"));
    assert_eq!(
        device.calls(),
        0,
        "没放行之前不该去换 deviceId（那要上报指纹）"
    );
}

/// 定位失败（查无此件）⇒ 一枪不发，且**不读余额**。
#[tokio::test]
async fn a_missing_prize_sends_nothing_and_skips_the_balance_read() {
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let fx = Fixture::new(&[&list], BASE_NOW_MS, None);
    let opts = ExchangeOptions {
        prize: "不存在的商品",
        skip_wait: true,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: None,
        wait: None,
    };
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.plan, ExchangePlan::NotFound);
    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(
        fx.http().count(),
        1,
        "查无此件时那一枪只是白打（infoV3 也是账号级请求）"
    );
}

/// 余额不够 ⇒ 一枪不发，且**不取滑块题**、不换 deviceId。
#[tokio::test]
async fn insufficient_balance_sends_nothing() {
    let device = DeviceId::ok("B-device");
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let info = info_body(100); // 只有 100 豆
    let fx = Fixture::new(&[&list, &info], BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.plan, ExchangePlan::InsufficientCloud);
    assert_eq!(outcome.attempts, 0);
    assert_eq!(outcome.balance, 100, "余额读数要留在结论里");
    assert_eq!(fx.http().count(), 2, "只读清单 + 余额");
    assert_eq!(device.calls(), 0);
}

/// 空入参 ⇒ `Config` 错误（没走到能报结论的那一步）。
#[tokio::test]
async fn empty_prize_is_a_config_error() {
    let fx = Fixture::new(&[], BASE_NOW_MS, None);
    for bad in ["", "   "] {
        let opts = ExchangeOptions {
            prize: bad,
            skip_wait: true,
            offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
            force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
            device_id: None,
            wait: None,
        };
        let err = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap_err();
        assert!(matches!(err, Error::Config(_)), "空奖品应当是配置错误");
    }
    assert_eq!(fx.http().count(), 0, "入参不成立时一个请求都不发");
}

/// ⭐ 换不到 `deviceId` ⇒ **一枪不发**（下单体里那个键必须有值）。
#[tokio::test]
async fn no_device_id_means_no_write_request() {
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let info = info_body(10496);
    let replies = [list.as_str(), info.as_str()];

    // 一分支：没有注入点。
    let fx = Fixture::new(&replies, BASE_NOW_MS, None);
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env_without_device_id(), BASE_NOW_MS, &opts)
        .await
        .unwrap();
    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(outcome.attempts, 0);
    assert_eq!(outcome.plan, ExchangePlan::Go, "判据本身是放行的");
    assert!(
        outcome.device_id_failed.contains("没接上 deviceId 来源"),
        "原因要说清：{}",
        outcome.device_id_failed
    );

    // 二分支：注入点报错。
    let bad = DeviceId::fails("指纹上报被拒");
    let fx = Fixture::new(&replies, BASE_NOW_MS, Some(Arc::clone(&bad)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();
    assert_eq!(outcome.attempts, 0);
    assert_eq!(fx.http().attempts(), 0);
    assert!(outcome.device_id_failed.contains("指纹上报被拒"));

    // 三分支：换回一个**空串**（比报错更危险：看起来成功了）。
    let empty = DeviceId::ok("   ");
    let fx = Fixture::new(&replies, BASE_NOW_MS, Some(Arc::clone(&empty)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();
    assert_eq!(outcome.attempts, 0);
    assert!(
        outcome.device_id_failed.contains("空串"),
        "空 id 必须单独说：{}",
        outcome.device_id_failed
    );
}

/// ⭐ **算不出 offset ⇒ 一枪不发**（拿 `0` 去凑就是拿真豆去赌）。
#[tokio::test]
async fn an_unresolvable_slider_sends_nothing() {
    let device = DeviceId::ok("B-device-1");
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let info = info_body(10496);
    let broken = slide_body_broken(); // 题面取到了，但图不是 PNG
    let fx = Fixture::new(
        &[&list, &info, &broken],
        BASE_NOW_MS,
        Some(Arc::clone(&device)),
    );
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::SliderUnresolved);
    assert_eq!(outcome.attempts, 0, "算不出来就不发");
    assert_eq!(fx.http().attempts(), 0);
    assert_eq!(outcome.rounds, 1, "取过一轮题");
    assert!(outcome.offset.is_none(), "没算出来就不该有 offset");
    assert!(!outcome.error.is_empty());
    assert_eq!(outcome.plan, ExchangePlan::Go, "判据本身是放行的");
}

/// 第一次取题就失败 ⇒ 一枪未发且交回错误（"没走到能报结论的那一步"）。
#[tokio::test]
async fn a_failed_puzzle_fetch_is_an_error_not_a_verdict() {
    let device = DeviceId::ok("B-device-1");
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let info = info_body(10496);
    let fail = v3_fail(500, "取题失败");
    let fx = Fixture::new(
        &[&list, &info, &fail],
        BASE_NOW_MS,
        Some(Arc::clone(&device)),
    );
    let opts = ready_opts(&fx);
    let err = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap_err();
    assert!(
        err.to_string().contains("取滑块题面"),
        "错误要说清是哪一枪：{err}"
    );
    assert_eq!(fx.http().attempts(), 0);
}

// ══════════════════════════════ 链：成交与余额回查 ══════════════════════════════

/// ⭐ 成交判据是**两条**：`code 0` **且** 余额差 == 价格。
#[tokio::test]
async fn ordered_requires_the_balance_delta_to_match_the_price() {
    let device = DeviceId::ok("B-device-1");
    let replies = replies_ordered();
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(
        outcome.status,
        ExchangeStatus::Ordered,
        "{:?}",
        outcome.error
    );
    assert_eq!(outcome.attempts, 1);
    assert_eq!(outcome.rounds, 1);
    assert_eq!(outcome.code, CODE_OK);
    assert_eq!(outcome.balance, 10496);
    assert_eq!(outcome.after_balance, Some(8696));
    assert_eq!(outcome.spent(), Some(1800));
    assert!(outcome.order.is_some(), "成交那份订单要留着");
    assert!(outcome.offset.is_some(), "发出去的那个 offset 要留着");
    assert_eq!(fx.http().attempts(), 1, "成交了只发一枪");
    assert_eq!(
        fx.http().count(),
        7,
        "清单 → 余额 → **预热两枪** → 题面 → 下单 → 复查（2026-10-02 起链上补了 TS 那个预热序列）"
    );
    assert_eq!(device.calls(), 1, "deviceId 只换一次（它要上报指纹）");

    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(result.run_report().is_success());
    let line = result.summary_line();
    assert!(line.contains("兑到「移动云盘100G云空间月卡」"), "{line}");
    assert!(line.contains("余额 10496 → 8696（差 1800）"), "{line}");
}

/// 没有价格读数时余额差**核不了**，但 `code 0` + `oid` 仍算成交并说出来。
#[test]
fn ordered_records_that_the_delta_could_not_be_checked() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::Ordered,
            plan: ExchangePlan::Go,
            prize: ExchangePrize {
                name: Some("移动云盘100G云空间月卡".into()),
                ..ExchangePrize::default()
            },
            balance: 10496,
            after_balance: Some(8696),
            error: "这一件没有价格读数".into(),
            ..ExchangeOutcome::default()
        },
    };
    assert!(result.run_report().is_success());
    let line = result.summary_line();
    assert!(line.contains("余额差没核到"), "{line}");
    assert!(line.contains("这一件没有价格读数"), "{line}");
    assert!(result.outcome.spent().is_none(), "缺价格就核不出差值");
}

/// ⭐ `code 0` 但**余额差对不上** ⇒ `Unknown`（不能默认成功）。
///
/// 这一档是刻意与旧实现分道的地方：旧实现只看 `code === 0`，
/// 那种写法在"上游说成功却没扣豆"时会报出一条**绿的假成功**。
#[tokio::test]
async fn code_ok_with_a_wrong_delta_is_unknown_not_success() {
    for after in [
        10496, // 一分没扣
        8697,  // 差 1799
        8695,  // 差 1801
        0,     // 全扣光了
    ] {
        let device = DeviceId::ok("B-device-1");
        let replies = vec![
            list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
            info_body(10496),
            visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
            visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
            slide_body(),
            order_body(1800),
            info_body(after),
        ];
        let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
        let opts = ready_opts(&fx);
        let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

        assert_eq!(
            outcome.status,
            ExchangeStatus::Unknown,
            "余额差对不上就不能默认成功（after={after}）"
        );
        assert_eq!(
            outcome.attempts, 1,
            "对不上的时候**绝不重发** —— 那一笔可能已经成交了"
        );
        assert_eq!(fx.http().attempts(), 1, "after={after} 时也不许重发");
        let result = ExchangeResult {
            account: "19****01".into(),
            outcome,
        };
        assert!(!result.run_report().is_success(), "Unknown 不能报成功");
        assert!(
            result.summary_line().contains("对不上"),
            "{}",
            result.summary_line()
        );
    }
}

/// 上游说成功、但余额**复查那一枪**失败 ⇒ 仍报成交，并发一句"没核到"。
#[tokio::test]
async fn ordered_even_if_the_balance_recheck_fails() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        order_body(1800),
    ];
    // 第 7 个请求（复查）传输层失败 —— 预热两枪都插在题面之前，所以整体后移两位。
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device))).failing_at(7);
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(
        outcome.status,
        ExchangeStatus::Ordered,
        "code 0 + oid 就是证据，复查失败不该推翻它"
    );
    assert!(outcome.after_balance.is_none());
    assert!(outcome.error.contains("余额复查失败"));
    assert_eq!(fx.http().attempts(), 1, "复查失败也不重发那一单");

    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(result.run_report().is_success());
    assert!(result.summary_line().contains("余额差没核到"));
}

// ══════════════════════════════ 链：514 重试口径 ══════════════════════════════

/// ⭐ `514` 消耗一轮，且第二轮是"**重新取题** + 再下一单"。
#[tokio::test]
async fn slider_failure_retries_with_a_fresh_puzzle() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),    // 第 1 轮取题
        v3_fail(514, "验证失败"),
        slide_body(), // 第 2 轮**重新**取题
        order_body(1800),
        info_body(8696),
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(
        outcome.status,
        ExchangeStatus::Ordered,
        "{:?}",
        outcome.error
    );
    assert_eq!(outcome.rounds, 2);
    assert_eq!(outcome.attempts, 2);
    assert_eq!(fx.http().attempts(), 2);
    assert_eq!(
        fx.http().slides(),
        2,
        "第二轮必须是**重新取题**，不是拿旧 offset 重发"
    );
}

/// ⭐ `SLIDER_ROUNDS` 轮都被判错 ⇒ `SliderUnresolved`，**不再发第三枪**。
#[tokio::test]
async fn both_rounds_rejected_stops_at_two_attempts() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        v3_fail(514, "验证失败"),
        slide_body(),
        v3_fail(514, "验证失败"),
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(SLIDER_ROUNDS, 2, "上限是契约的一部分");
    assert_eq!(outcome.status, ExchangeStatus::SliderUnresolved);
    assert_eq!(outcome.attempts, 2, "第三枪不许发");
    assert_eq!(fx.http().attempts(), 2);
    assert_eq!(fx.http().slides(), 2);
    assert_eq!(outcome.code, CODE_SLIDER_FAILED);

    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(!result.run_report().is_success());
    // 这一档是"要人去 App 手动点一下"那一类，小结必须说出来。
    assert!(
        result.summary_line().contains("滑块验证 2 次都被上游判错"),
        "{}",
        result.summary_line()
    );
}

/// 第二轮**取题**失败不推翻第一轮的 `514`：落回"滑块没过"，不发第三枪。
#[tokio::test]
async fn second_round_puzzle_failure_keeps_the_slider_verdict() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        v3_fail(514, "验证失败"),
        v3_fail(500, "取题服务抖了"), // 第 5 枪是第二轮取题，回非 0 ⇒ 报错
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(
        outcome.status,
        ExchangeStatus::SliderUnresolved,
        "第一轮那一枪确实被上游判错了，重试机会也用完了"
    );
    assert_eq!(outcome.attempts, 1, "第二轮连题都没取到，一枪都没发");
    assert_eq!(fx.http().attempts(), 1);
}

/// ⭐ **传输层错误 ⇒ `Unknown` 且绝不重发**（写操作零自动重试）。
///
/// 发出去超时的这一笔**可能已经成交**，重发的表现是同一件扣两次豆。
#[tokio::test]
async fn a_transport_error_is_unknown_and_is_never_resent() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
    ];
    // 第 4 枪 = 下单。
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device))).failing_at(6);
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::Unknown);
    assert_eq!(outcome.attempts, 1, "超时的一枪算发出去过");
    assert_eq!(fx.http().attempts(), 1, "⚠ 绝不重发 —— 那一笔可能已经成交");
    assert_eq!(fx.http().slides(), 1, "也不去重取题（重取 = 准备重发）");
    assert!(outcome.error.contains("超时"));
    // ⚠ 不能把它折成"滑块没过"（那会触发一次重试口径的误用）。
    assert_ne!(outcome.status, ExchangeStatus::SliderUnresolved);

    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(!result.run_report().is_success());
    assert!(
        result.summary_line().contains("需要去 App 核对"),
        "{}",
        result.summary_line()
    );
}

// ══════════════════════════════ 链：其余业务码 ══════════════════════════════

/// `2301 重复兑奖` ⇒ `AlreadyExchanged`（目标已达成，不是失败），且**不重发**。
#[tokio::test]
async fn duplicate_prize_code_is_already_exchanged() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        v3_fail(2301, "重复兑奖"),
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::AlreadyExchanged);
    assert_eq!(outcome.attempts, 1, "2301 再怎么试也不会成交，不重发");
    assert_eq!(outcome.code, 2301);
    assert!(outcome.after_balance.is_none(), "没成交就没有余额差可查");

    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(result.run_report().is_success(), "本来就有了 = 目标已达成");
    assert!(result.summary_line().contains("此前已兑过"));
}

/// `407 非移动用户不可领奖` ⇒ `NotTargetUser`（账号级闸门），落失败。
#[tokio::test]
async fn not_target_user_code_is_a_failure() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        v3_fail(407, "非移动用户不可领奖"),
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::NotTargetUser);
    assert_eq!(outcome.attempts, 1, "账号级闸门重试没有意义");
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    assert!(result.run_report().is_failure());
    assert!(result.summary_line().contains("非移动用户不可领奖"));
}

/// 其余业务码（`415`/`420`/`421`/`501`/`610`/`614`/`615`…）⇒ 明确失败，不重发。
#[tokio::test]
async fn other_business_codes_are_failures_without_retry() {
    for (code, msg) in [
        (415, "奖品单日已耗尽"),
        (420, "该奖品每日限兑，您今日份额已用完"),
        (421, "该奖品每月限兑，您本月份额已用完"),
        (501, "滑块验证参数不能为空"),
        (610, "兑换失败，请稍后再试或联系客服~"),
        (614, "当前兑换人数较多"),
        (615, "当前兑换人数较多"),
    ] {
        let device = DeviceId::ok("B-device-1");
        let replies = vec![
            list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
            info_body(10496),
            visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
            visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
            slide_body(),
            v3_fail(code, msg),
        ];
        let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
        let opts = ready_opts(&fx);
        let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();

        assert_eq!(
            outcome.status,
            ExchangeStatus::Failed,
            "code {code} 是明确失败"
        );
        assert_eq!(fx.http().attempts(), 1, "code {code} 不许重发");
        let result = ExchangeResult {
            account: "19****01".into(),
            outcome,
        };
        assert!(result.run_report().is_failure(), "code {code} 应当失败");
        let line = result.summary_line();
        assert!(line.contains(&code.to_string()), "{line}");
        assert!(line.contains(msg), "上游那句原文要带出去：{line}");
    }
}

/// 上游没给 `msg` ⇒ 不渲染成一对空引号。
#[tokio::test]
async fn empty_upstream_message_falls_back() {
    let device = DeviceId::ok("B-device-1");
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        visitlog_body(), // 预热那一枪（链上在下单前**每趟一次**）
        slide_body(),
        serde_json::json!({"code": 610}).to_string(),
    ];
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome,
    };
    let line = result.summary_line();
    assert!(line.contains("没给原因"), "{line}");
    assert!(!line.contains("「」"), "不许渲染空引号：{line}");
}

/// 下单请求体里必须带上**算出来的** offset 与 `deviceId`。
#[tokio::test]
async fn the_order_body_carries_the_offset_and_device_id() {
    let device = DeviceId::ok("B-device-xyz");
    let replies = replies_ordered();
    let fx = Fixture::new(&as_refs(&replies), BASE_NOW_MS, Some(Arc::clone(&device)));
    let opts = ready_opts(&fx);
    let outcome = exchange(&fx.env(), BASE_NOW_MS, &opts).await.unwrap();
    assert_eq!(outcome.status, ExchangeStatus::Ordered);

    let body = fx
        .http()
        .requests()
        .into_iter()
        .find(|r| r.url.contains("exchangeV3"))
        .and_then(|r| r.body_str().map(str::to_string))
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["prizeId"], 251230053);
    assert_eq!(json["deviceId"], "B-device-xyz");
    assert_eq!(
        json["puzzleOffset"].as_i64(),
        outcome.offset,
        "体里的 offset 必须就是算出来那一个"
    );
    assert_eq!(json["client"], "app");
    // ⚠ 实拍的 clientVersion 是 `12.6.0`（三段），与头里的 `12.6.0.0` 不同。
    assert_eq!(json["clientVersion"], "12.6.0");
}

// ══════════════════════════════ 链：等待与重判 ══════════════════════════════

/// `skip_wait == false` 且离整点还远 ⇒ 先等，等完**重读重判**。
#[tokio::test]
async fn waiting_rereads_and_rejudges() {
    // 第一轮：可兑；重判那一轮：别人已经兑掉了（限兑状态变了）。
    //
    // ⚠ 回复要按**枪序**排满两轮：每轮都是"清单 → 余额"两枪
    // （`judge_exchange` 先读两样再判），所以第二轮的余额那一枪也要给。
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        // ⚠ 预热两枪的位置在**整点等待之前**（与 TS 同序：窗口等待 → 预热 → 整点等待）⇒
        // 它排在"重判那一轮"的清单/余额**之前**。原先这条用例走不到预热（第一轮判完
        // 就等到整点、重判被挡下），预热挪到等待之间以后就必须给它两枪，否则后面的
        // 重判会读到错位的响应（症状是 `groupId=beforeTotal 那一组不是数组`）。
        visitlog_body(),
        visitlog_body(),
        list_body(prize_json(LIMIT_EXCHANGED, 1800, 839)),
        info_body(10496),
    ];
    let device = DeviceId::ok("B-device-1");
    let fx = Fixture::advancing(&as_refs(&replies), Some(Arc::clone(&device)));

    let waited = Arc::new(Mutex::new(Vec::<i64>::new()));
    let recorder = Arc::clone(&waited);
    let wait_fn = move |until: i64| {
        recorder.lock().unwrap().push(until);
        Ok(())
    };
    // 从 13:30 出发（离整点还远 ⇒ 要等），等完之后时钟往前跳。
    let base = beijing_ms(2026, 9, 19, 13, 30, 0);
    let opts = ExchangeOptions {
        prize: "251230053",
        skip_wait: false,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: fx.env().device_id,
        wait: Some(&wait_fn),
    };
    let outcome = exchange(&fx.env(), base, &opts).await.unwrap();

    assert_eq!(
        waited.lock().unwrap().as_slice(),
        &[
            // 第一段：先等到窗口打开（hh:59:40）—— 预热就发生在这一段之后。
            beijing_ms(2026, 9, 19, 13, 59, 40),
            // 第二段：再压着整点发单（整点 − 提前量，TS 的 offset 默认 10ms）。
            beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS,
        ],
        "先等到 hh:59:40，再等到「下一个整点 − 提前量」（两段等待，TS 同款）"
    );
    assert_eq!(
        outcome.waited_until_ms,
        Some(beijing_ms(2026, 9, 19, 14, 0, 0) - EXCHANGE_LEAD_MS)
    );
    assert_eq!(
        outcome.plan,
        ExchangePlan::ExchangedThisPrize,
        "等完之后**重判**那一份才是生效的判据"
    );
    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(outcome.attempts, 0);
    assert_eq!(
        fx.http().count(),
        6,
        "两轮各两次（清单 + 余额＝4 枪）**再加预热两枪**＝6。\
         ⚠ 预热在「窗口等待之后、整点等待之前」（与 TS 同序），\
         所以它排在这两轮只读请求之间 —— 这不是「多读了一轮」，是那两枪埋点。\
         早先写 4 的时候预热还在下单循环里，这条用例走不到它。\
         ⚠ 重判那一轮**也要读余额** —— `limit` 不是定位阶段判的，\
         而定位成功之后 `judge_exchange` 一律读余额（参考实现 `judgeExchange` 同形）。\
         写成 3 会把「重判时因 limit 挡住而少读一次余额」当成正确行为，\
         那正是这条测试要排除的偏差。"
    );
    // 到这一步为止只花了只读请求 + 预热那两枪（没发单）。⚠ `deviceId` 现在在两段等待**之前**就取好，所以这里不代表「没碰过设备指纹」。
    assert_eq!(device.calls(), 0);
}

/// 整点后 20 秒内（还在上一场尾巴里）⇒ **不等**，直接判。
#[tokio::test]
async fn no_wait_just_after_the_hour() {
    let device = DeviceId::ok("B-device-1");
    let replies = replies_ordered();
    let at = beijing_ms(2026, 9, 19, 14, 0, 5);
    let fx = Fixture::new(&as_refs(&replies), at, Some(Arc::clone(&device)));

    let called = AtomicUsize::new(0);
    let wait_fn = |_until: i64| -> Result<()> {
        called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let opts = ExchangeOptions {
        prize: "251230053",
        skip_wait: false,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: fx.env().device_id,
        wait: Some(&wait_fn),
    };
    let outcome = exchange(&fx.env(), at, &opts).await.unwrap();

    assert_eq!(called.load(Ordering::SeqCst), 0, "整点尾巴里不该等");
    assert!(outcome.waited_until_ms.is_none());
    assert_eq!(outcome.status, ExchangeStatus::Ordered);
}

/// `skip_wait == false` 但**没接上等待实现** ⇒ 如实挡下，绝不静默往下走。
#[tokio::test]
async fn missing_wait_impl_is_refused_not_silently_skipped() {
    let device = DeviceId::ok("B-device-1");
    let replies = replies_ready();
    let at = beijing_ms(2026, 9, 19, 13, 30, 0);
    let fx = Fixture::new(&as_refs(&replies), at, Some(Arc::clone(&device)));
    let opts = ExchangeOptions {
        prize: "251230053",
        skip_wait: false,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: fx.env().device_id,
        wait: None, // 没注入
    };
    let outcome = exchange(&fx.env(), at, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(outcome.attempts, 0);
    assert!(
        outcome.error.contains("没接上等待实现"),
        "要如实说清为什么没等：{}",
        outcome.error
    );
    assert_eq!(fx.http().attempts(), 0);
    assert_eq!(device.calls(), 0);
}

/// 等待被取消（返回 `Err`）⇒ 折成 `Skipped` + 原因，一枪不发。
#[tokio::test]
async fn cancelled_wait_sends_nothing() {
    let device = DeviceId::ok("B-device-1");
    let replies = replies_ready();
    let at = beijing_ms(2026, 9, 19, 13, 30, 0);
    let fx = Fixture::new(&as_refs(&replies), at, Some(Arc::clone(&device)));
    let wait_fn = |_until: i64| -> Result<()> { Err(Error::Cancelled) };
    let opts = ExchangeOptions {
        prize: "251230053",
        skip_wait: false,
        offset_ms: EXCHANGE_LEAD_MS, // 缺省提前量（传输延迟补偿 10ms）
        force: false,                // 既有用例一律走"本地判定照旧挡"这条老路
        device_id: fx.env().device_id,
        wait: Some(&wait_fn),
    };
    let outcome = exchange(&fx.env(), at, &opts).await.unwrap();

    assert_eq!(outcome.status, ExchangeStatus::Skipped);
    assert_eq!(outcome.attempts, 0);
    assert_eq!(fx.http().attempts(), 0);
    assert!(outcome.error.contains("中止"), "{}", outcome.error);
    assert_eq!(device.calls(), 0);
}

// ══════════════════════════════ RunReport 口径 ══════════════════════════════

/// ⭐ `RunReport` 分流：**用户要的那件到底兑到没有**。
#[test]
fn run_report_routes_by_whether_the_target_was_reached() {
    let make = |status: ExchangeStatus, plan: ExchangePlan| ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status,
            plan,
            prize: baseline(),
            balance: 10496,
            ..ExchangeOutcome::default()
        },
    };

    // 兑到了 / 本来就有 ⇒ 成功
    assert!(make(ExchangeStatus::Ordered, ExchangePlan::Go)
        .run_report()
        .is_success());
    assert!(make(ExchangeStatus::AlreadyExchanged, ExchangePlan::Go)
        .run_report()
        .is_success());

    // 判据没过、且没过的是"本月已兑"（limit 2 / 4）⇒ **也算成功**
    for plan in [
        ExchangePlan::ExchangedThisPrize,
        ExchangePlan::ExchangedThisGroup,
    ] {
        assert!(
            make(ExchangeStatus::Skipped, plan)
                .run_report()
                .is_success(),
            "{plan:?} 说的是「目标已达成」，该落成功"
        );
    }

    // 其余"判据没过" ⇒ 跳过（不是失败，也不是成功）
    for plan in [
        ExchangePlan::NotFound,
        ExchangePlan::Ambiguous,
        ExchangePlan::OffShelf,
        ExchangePlan::LimitMissing,
        ExchangePlan::LimitTomorrow,
        ExchangePlan::LimitThree,
        ExchangePlan::LimitUnknown,
        ExchangePlan::MinRemainderZero,
        ExchangePlan::SoldOut,
        ExchangePlan::DailyStockEmpty,
        ExchangePlan::PriceMissing,
        ExchangePlan::InsufficientCloud,
        ExchangePlan::NotStarted,
        ExchangePlan::Ended,
    ] {
        let report = make(ExchangeStatus::Skipped, plan).run_report();
        assert!(
            !report.is_success() && !report.is_failure(),
            "{plan:?} 该落跳过（目标没达成，但没有必须人去做的事）"
        );
    }

    // 滑块没过 / 结果判不了 / 账号级闸门 / 明确失败 / 零值 ⇒ 失败
    for status in [
        ExchangeStatus::SliderUnresolved,
        ExchangeStatus::Unknown,
        ExchangeStatus::NotTargetUser,
        ExchangeStatus::Failed,
        ExchangeStatus::NotAttempted,
    ] {
        let report = make(status, ExchangePlan::Go).run_report();
        assert!(report.is_failure(), "{status:?} 该落失败：{report:?}");
    }
}

/// 小结口径：每一档都有了断的一句话，且**都不为空**。
#[test]
fn every_status_has_a_non_empty_summary() {
    for status in [
        ExchangeStatus::NotAttempted,
        ExchangeStatus::Ordered,
        ExchangeStatus::Skipped,
        ExchangeStatus::AlreadyExchanged,
        ExchangeStatus::SliderUnresolved,
        ExchangeStatus::NotTargetUser,
        ExchangeStatus::Unknown,
        ExchangeStatus::Failed,
    ] {
        let result = ExchangeResult {
            account: "19****01".into(),
            outcome: ExchangeOutcome {
                status,
                prize: ExchangePrize {
                    name: Some("移动云盘100G云空间月卡".into()),
                    price: Some(1800),
                    ..ExchangePrize::default()
                },
                balance: 10496,
                ..ExchangeOutcome::default()
            },
        };
        assert!(
            !result.summary_line().trim().is_empty(),
            "{status:?} 的小结不能是空的"
        );
    }
}

/// 商品名缺失（或只有空白）时小结里说"未知商品"，不是空引号。
#[test]
fn missing_prize_name_falls_back_to_unknown() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::AlreadyExchanged,
            prize: ExchangePrize {
                name: Some("   ".into()),
                ..ExchangePrize::default()
            },
            ..ExchangeOutcome::default()
        },
    };
    let line = result.summary_line();
    assert!(line.contains("未知商品"), "{line}");
    assert!(!line.contains("「」"), "{line}");
}

/// `Skipped` 那一档，`device_id_failed` 有值时小结说的是它，而不是 `plan.reason()`。
#[test]
fn device_id_failure_takes_priority_in_the_summary() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::Skipped,
            plan: ExchangePlan::Go,
            device_id_failed: "换不到 deviceId".into(),
            ..ExchangeOutcome::default()
        },
    };
    assert!(result.summary_line().contains("换不到 deviceId"));
    assert_eq!(
        result.run_report(),
        TaskOutcome::skipped("没兑换：换不到 deviceId"),
        "deviceId 换不到时 plan 是 Go，不能落成功"
    );
}

/// `Skipped` 且没有 deviceId 失败原因时，小结念的是 `plan.reason()`。
#[test]
fn skipped_without_device_id_failure_quotes_the_plan_reason() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::Skipped,
            plan: ExchangePlan::InsufficientCloud,
            ..ExchangeOutcome::default()
        },
    };
    assert_eq!(
        result.summary_line(),
        format!("没兑换：{}", ExchangePlan::InsufficientCloud.reason())
    );
}

/// `Attempts == 0` 的滑块那一档，措辞要说"一枪都没发"（与"两轮都被判错"分开）。
#[test]
fn slider_summary_distinguishes_unresolved_from_rejected() {
    let unsent = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::SliderUnresolved,
            error: "PNG 解不开".into(),
            attempts: 0,
            ..ExchangeOutcome::default()
        },
    };
    assert!(unsent.summary_line().contains("一枪都没发"));
    assert!(unsent.summary_line().contains("PNG 解不开"));

    let rejected = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::SliderUnresolved,
            attempts: 2,
            ..ExchangeOutcome::default()
        },
    };
    assert!(rejected.summary_line().contains("2 次都被上游判错"));
}

/// `Unknown` 且**余额差算得出来**时，小结要点明"差与价格对不上"。
#[test]
fn unknown_summary_names_the_mismatch() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::Unknown,
            code: CODE_OK,
            prize: baseline(),
            balance: 10496,
            after_balance: Some(10496), // 一分没扣
            ..ExchangeOutcome::default()
        },
    };
    let line = result.summary_line();
    assert!(line.contains("余额差 0 与价格 1800 对不上"), "{line}");
    // `code 0` 时不该把"上游回 0"念出来（那是成功码，念出来反而误导）。
    assert!(!line.contains("上游回 0"), "{line}");
}

/// `Unknown` 且差值核不出时，小结念的是本地原因。
#[test]
fn unknown_summary_falls_back_to_the_local_reason() {
    let result = ExchangeResult {
        account: "19****01".into(),
        outcome: ExchangeOutcome {
            status: ExchangeStatus::Unknown,
            error: "下单那一枪超时".into(),
            ..ExchangeOutcome::default()
        },
    };
    let line = result.summary_line();
    assert!(line.contains("兑换结果判不了"), "{line}");
    assert!(line.contains("下单那一枪超时"), "{line}");
    assert!(line.contains("需要去 App 核对"), "{line}");
}

// ══════════════════════════════ 任务壳 ══════════════════════════════

fn task_ctx(http: Arc<dyn HttpClient>, clock: Arc<dyn Clock>) -> TaskContext {
    use mcloud_core::{Account, AccountId, AccountSettings, AuthInfo, DeviceId};
    use mcloud_engine::{ArtifactLedger, ArtifactRecorder};
    TaskContext {
        account: Account {
            id: AccountId::new("19****01"),
            nickname: None,
            auth: AuthInfo {
                raw: "Basic eA==".into(),
                platform: "pc".into(),
                phone: "19900000001".into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("DEV+1/base=".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: None,
            enabled: true,
            settings: AccountSettings::default(),
        },
        http,
        store: Arc::new(NoStore),
        clock,
        sink: Arc::new(NoSink),
        run_id: mcloud_core::RunId(1),
        cancel: mcloud_core::CancelToken::new(),
        chain_cancel: None,
        artifacts: ArtifactRecorder::new(
            Arc::new(ArtifactLedger::new()),
            mcloud_core::RunId(1),
            AccountId::new("19****01"),
        ),
        // 这一格测的是兑换**任务体**（它自报明细），所以必须给真的记录器 ——
        // 给个假的就等于把"明细真的会落进记录"这件事测成一句空话。
        details: mcloud_engine::DetailsRecorder::new(
            mcloud_engine::DetailsLedger::new(),
            mcloud_core::RunId(1),
        ),
    }
}

/// 任务元信息：名字是 wire 契约。
#[test]
fn task_meta_is_stable() {
    let meta = ExchangeTask.meta();
    assert_eq!(meta.name, "exchange");
    assert_eq!(meta.title, "兑换奖品");
    assert!(!meta.description.is_empty());
    let names: Vec<&str> = meta.params.iter().map(|p| p.name.as_str()).collect();
    // ⚠ 这条哨兵红了正是它的用途：提醒"元信息变了，是有意改的吗"。
    // 2026-10-02 是有意的 —— 加了 `force`（用户口径：「所有档都放行、点击就是真发请求」，
    // 与"下单前再确认一次"同款，做成开关、默认关）。
    // ⚠ 2026-10-02 又是有意的：加了 `offset`（用户口径：「10ms 是本地与云端的传输
    // 时间差」⇒ 它成为**可输入项**），排在 `force` 前面。
    assert_eq!(names, vec!["prizeIdOrName", "skipWait", "offset", "force"]);
}

/// ⚠ **`body` 里没有 `deviceId`** ⇒ 任务壳**一枪不发**并如实说原因。
///
/// 注入点**已经接上**了（`deviceId` 走请求体；路由那层用 `SmidService::device_id`
/// 取一个可用的 id，见 `routes/exchange.rs`）⇒ 这条用例钉的不再是"没有注入点"，
/// 而是**缺 id 时那条安全阀**：宁可不发，也不拿空 id 去赌真豆。
///
/// 另一面（`body` **带** id ⇒ 真的往下走到下单）由正向用例覆盖 —— 见
/// `task_shell_orders_when_the_body_carries_a_device_id`（下一轮补）。
#[tokio::test]
async fn task_shell_refuses_when_the_body_has_no_device_id() {
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let info = info_body(10496);
    let http = Arc::new(Scripted::new(&[&list, &info])) as Arc<dyn HttpClient>;
    let ctx = task_ctx(http, Arc::new(FixedClock(BASE_NOW_MS)));

    // ⚠ 显式 `skipWait: true`：任务层缺省已改为"等"（与 TS 一致），而这条测的是
    // **deviceId 缺口**，不显式跳过就会先撞上等待那条路径，测到别的东西。
    let body = serde_json::json!({"prizeIdOrName": "251230053", "skipWait": true});
    let outcome = ExchangeTask.run(&ctx, &body).await.unwrap();
    assert!(
        !outcome.is_success(),
        "没有 deviceId 来源时不能报成功：{outcome:?}"
    );
    assert!(
        outcome.summary().contains("没接上 deviceId 来源"),
        "要如实说清缺口：{}",
        outcome.summary()
    );
}
/// `body` **带** `deviceId` ⇒ 任务壳真的一路走到下单（与上一条正好是一对）。
///
/// ⚠ 这条用例是补上的：上一条的注释早就指向了它，但测试**并不存在** ——
/// 「注释指向一个不存在的东西」正是这次清理一直在挑的那类问题。
///
/// 枪序按链上实际顺序排：清单 → 余额 → **预热两枪** → 取题 → 下单 → 复查。
/// `deviceId` 由 `body` 提供（生产上由路由用 `SmidService::device_id` 取好塞进来），
/// 所以这里**不需要** fixture 的设备源。
#[tokio::test]
async fn task_shell_orders_when_the_body_carries_a_device_id() {
    let replies = vec![
        list_body(prize_json(LIMIT_AVAILABLE, 1800, 839)),
        info_body(10496),
        visitlog_body(),
        visitlog_body(),
        slide_body(),
        order_body(1800),
        info_body(8696),
    ];
    let http = Arc::new(Scripted::new(&as_refs(&replies))) as Arc<dyn HttpClient>;
    let ctx = task_ctx(http, Arc::new(FixedClock(BASE_NOW_MS)));

    let body = serde_json::json!({
        "prizeIdOrName": "251230053",
        "skipWait": true,
        "deviceId": "B-from-body-1",
    });
    let outcome = ExchangeTask.run(&ctx, &body).await.unwrap();
    assert!(
        outcome.is_success(),
        "带了 deviceId 就该真的下单成功：{outcome:?}"
    );
}
/// `force: true` ⇒ **本地判成「抢光」也照样真发单**（用户口径 2026-10-02：
/// 「所有档都放行、点击就是真发请求」）。
///
/// 与上一条成对：上一条钉"带 `deviceId` 就下单" ✓，这条钉"**本地挡着也下单**" ✓
/// —— 后者是那个开关存在的**唯一理由** ✗，没有它，开关就只是个装饰 ✓。
///
/// 枪序同上（清单 → 余额 → 预热两枪 → 取题 → 下单 → 复查）；
/// 差别只在 `minRemainderCount: 0`（本地判据会落 `min_remainder_zero` ✗）
/// 与 `"force": true` ✓ ⇒ 判定闸被放行 ⇒ 那一枪真发出去 ✓。
#[tokio::test]
async fn force_sends_the_order_even_when_the_judge_says_sold_out() {
    let mut prize = prize_json(LIMIT_AVAILABLE, 1800, 839);
    // 11/17 `MinRemainderZero`：判据是 `minRemainderCount == 0`。
    prize["minRemainderCount"] = serde_json::json!(0);

    let replies = vec![
        list_body(prize),
        info_body(10496),
        visitlog_body(),
        visitlog_body(),
        slide_body(),
        order_body(1800),
        info_body(8696),
    ];
    let http = Arc::new(Scripted::new(&as_refs(&replies))) as Arc<dyn HttpClient>;
    let ctx = task_ctx(http, Arc::new(FixedClock(BASE_NOW_MS)));

    let body = serde_json::json!({
        "prizeIdOrName": "251230053",
        "skipWait": true,
        "force": true,
        "deviceId": "B-from-body-force",
    });
    let outcome = ExchangeTask.run(&ctx, &body).await.unwrap();
    assert!(
        outcome.is_success(),
        "开了 force 就该真发单（哪怕本地判成抢光）：{outcome:?}"
    );
}

/// 任务壳：缺 `prizeIdOrName` ⇒ 配置错误（不是"跳过"）。
#[tokio::test]
async fn task_shell_requires_the_prize_parameter() {
    let ctx = task_ctx(
        Arc::new(Scripted::new(&[])) as Arc<dyn HttpClient>,
        Arc::new(FixedClock(BASE_NOW_MS)),
    );
    for body in [
        serde_json::json!({}),
        serde_json::json!({"prizeIdOrName": "  "}),
        serde_json::json!({"prizeIdOrName": 123}),
    ] {
        let err = ExchangeTask.run(&ctx, &body).await.unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
    }
}

// ── 最小替身 ────────────────────────────────────────────────────────────

/// 兑换链**只读** HTTP，不碰 store：给一个必然报错的替身。
///
/// 于是"哪天有人给兑换链加了一句读 store"会立刻炸出来。
struct NoStore;

fn refuse() -> Error {
    Error::Other("兑换链不该碰 store".into())
}

#[async_trait]
impl mcloud_core::AccountStore for NoStore {
    async fn accounts(&self) -> Result<Vec<mcloud_core::Account>> {
        Err(refuse())
    }
    async fn account(&self, _id: &mcloud_core::AccountId) -> Result<Option<mcloud_core::Account>> {
        Err(refuse())
    }
    async fn save_account(&self, _a: &mcloud_core::Account) -> Result<()> {
        Err(refuse())
    }
    async fn remove_account(&self, _id: &mcloud_core::AccountId) -> Result<()> {
        Err(refuse())
    }
    async fn account_state(&self, _id: &mcloud_core::AccountId) -> Result<serde_json::Value> {
        Err(refuse())
    }
    async fn save_account_state(
        &self,
        _id: &mcloud_core::AccountId,
        _s: &serde_json::Value,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::RunStore for NoStore {
    async fn upsert_run(&self, _r: &mcloud_core::RunRecord) -> Result<()> {
        Err(refuse())
    }
    async fn runs(&self, _limit: usize) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn run(&self, _id: mcloud_core::RunId) -> Result<Option<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn running_runs(&self) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::LogStore for NoStore {
    async fn append_logs(
        &self,
        _id: mcloud_core::RunId,
        _lines: &[mcloud_core::LogLine],
    ) -> Result<mcloud_core::AppendOutcome> {
        Err(refuse())
    }
    async fn logs(
        &self,
        _id: mcloud_core::RunId,
        _after: Option<mcloud_core::Seq>,
    ) -> Result<Vec<mcloud_core::LogLine>> {
        Err(refuse())
    }
    async fn clear_logs(&self, _id: mcloud_core::RunId) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::ScheduleStore for NoStore {
    async fn schedules(&self) -> Result<Vec<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn schedule(
        &self,
        _id: &mcloud_core::ScheduleId,
    ) -> Result<Option<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn save_schedule(&self, _s: &mcloud_core::Schedule) -> Result<()> {
        Err(refuse())
    }
    async fn remove_schedule(&self, _id: &mcloud_core::ScheduleId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_schedule_fired(&self, _id: &mcloud_core::ScheduleId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
    async fn record_schedule_run(
        &self,
        _id: &mcloud_core::ScheduleId,
        _ms: Option<i64>,
        _status: &str,
        _err: Option<&str>,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::PipelineStore for NoStore {
    async fn pipelines(&self) -> Result<Vec<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn pipeline(
        &self,
        _id: &mcloud_core::PipelineId,
    ) -> Result<Option<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn save_pipeline(&self, _p: &mcloud_core::Pipeline) -> Result<()> {
        Err(refuse())
    }
    async fn remove_pipeline(&self, _id: &mcloud_core::PipelineId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_pipeline_ran(&self, _id: &mcloud_core::PipelineId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::UserStore for NoStore {
    async fn user_count(&self) -> Result<usize> {
        Err(refuse())
    }
    async fn users(&self) -> Result<Vec<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user_by_name(&self, _name: &str) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user(&self, _id: &mcloud_core::UserId) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn save_user(&self, _u: &mcloud_core::User) -> Result<()> {
        Err(refuse())
    }
    async fn session(&self, _h: &str) -> Result<Option<mcloud_core::Session>> {
        Err(refuse())
    }
    async fn save_session(&self, _s: &mcloud_core::Session) -> Result<()> {
        Err(refuse())
    }
    async fn remove_session(&self, _h: &str) -> Result<()> {
        Err(refuse())
    }
    async fn remove_user_sessions(&self, _id: &mcloud_core::UserId) -> Result<usize> {
        Err(refuse())
    }
    async fn purge_expired_sessions(&self, _now_ms: i64) -> Result<usize> {
        Err(refuse())
    }
}

/// 事件汇：只收不发。
#[derive(Default)]
struct NoSink;

impl mcloud_core::EventSink for NoSink {
    fn emit(&self, _event: mcloud_core::RunEvent) {}
}

// ── 只读清单为什么要多打一枪余额（2026-10-02 用户口径）─────────────────────

/// 余额读数**参与**只读清单的判据 ⇒「余额不足」这一档判得出来。
///
/// 用户口径（2026-10-02）：「这是为啥这么说，无余额你就后端请求啊」。
///
/// 原先这条路刻意不读余额（"省一枪"、与参考实现给选品器的那一档一致），
/// 代价是**判据准确性**：余额不够的商品在界面上显示成"可兑"，
/// 用户点下去才被挡 —— 挡下那次确实有错误码，但已经是**白点一次**了。
/// 而代价只是一次**只读**请求（这条路本来就不碰滑块、不下单）。
///
/// 这条用例是那个改动的看门人：**两枪都发、且余额真的进了判定**。
#[tokio::test]
async fn listed_prizes_are_judged_with_balance_when_it_is_readable() {
    // ⚠ 清单条目要和 `baseline()` 同形：价格 1800、`limit = LIMIT_AVAILABLE`。
    // 前面那些档（未开始/已结束/今日份额空了/明天再来…）都得先过掉，
    // 才轮得到余额这一档 —— 随便造一件会先撞上别的档，测出来的就不是余额。
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    let fx = Fixture::new(&[&list, &info_body(50)], BASE_NOW_MS, None);

    let (judged, balance) = read_judged_prizes_with_balance(&fx.env(), BASE_NOW_MS)
        .await
        .expect("清单与余额都应该读到");

    assert_eq!(balance, Some(50), "余额要回给前端（界面上要能显示它）");
    assert_eq!(judged.len(), 1);
    assert_eq!(
        judged[0].plan,
        ExchangePlan::InsufficientCloud,
        "余额 50 < 价格 1800 ⇒ 必须判成余额不足（这条路以前判不出来）"
    );
    assert!(judged[0].plan.blocks(), "这一档要挡住按钮");
    assert_eq!(fx.http().count(), 2, "清单 + 余额：正好两枪");
}

/// 余额那一枪**失败**时：清单照常给出，退回"无余额读数"口径。
///
/// ⚠ 这是**刻意**的降级，不是将就：余额读不到不该把整份清单也带塌
/// （那样用户连"有哪些东西"都看不到），但要如实回 `None`，
/// 由调用方把"这一份不含余额读数"显示出来 —— 判据宽一点，但界面不撒谎。
#[tokio::test]
async fn listed_prizes_degrade_to_no_balance_when_status_fails() {
    let list = list_body(prize_json(LIMIT_AVAILABLE, 1800, 839));
    // 第 2 枪（`infoV3`，也就是余额那一枪）在传输层失败。
    let fx = Fixture::new(&[&list, &info_body(50)], BASE_NOW_MS, None).failing_at(2);

    let (judged, balance) = read_judged_prizes_with_balance(&fx.env(), BASE_NOW_MS)
        .await
        .expect("余额读失败不该把清单一起带塌");

    assert_eq!(balance, None, "没读到就是 None（前端据此显示说明）");
    assert_eq!(judged.len(), 1, "清单照常给");
    assert_ne!(
        judged[0].plan,
        ExchangePlan::InsufficientCloud,
        "没有余额读数时判不出这一档 —— 这正是要如实告诉用户的那件事"
    );
}
