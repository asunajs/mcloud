//! smid 设备指纹 —— 请求头 `deviceid` 是怎么来的。
//!
//! # 它不是"我们本地算出来的一个串"
//!
//! `deviceid` 是**中国移动统一认证（cmpassport）的风控服务发下来的**：
//! 客户端把一份"浏览器 / WebView 特征"加密上报，服务端回一个 `deviceId`，
//! 客户端在它前面**拼一个字母 `B`** 当作自己的设备 id 用。
//!
//! ```text
//! uid = randomUUID()
//!   ├─ ep     = RSA(uid)                 公钥见 mcloud_core::SKLAND_PUBLIC_KEY_B64
//!   └─ priId  = md5(uid)[0..16]          下面那份特征的 AES 密钥
//! 特征体（约 30 个字段，含一个当场随机出来的 smid）
//!   → tn = md5(getTn(特征体))             ← 先算摘要，再写回 tn 字段
//!   → 按 DES 规则逐字段加密 + 改名（appId→um、ua→pu、…）
//!   → JSON → gzip（OS 字节写 19）→ base64
//!   → AES-CBC/PKCS7 固定 IV，密钥 priId → hex      = data
//! POST https://slw.h5cmpassport.com:9090/deviceprofile/v4
//!   { appId, organization, ep, data, os:"web", encode:5, compress:2 }
//!   → code == 1100 → **deviceId = "B" + detail.deviceId**
//! ```
//!
//! 来源：旧项目 `packages/shared/src/smid/index.ts`（`getSmId` / `getTn` /
//! `encryptDes` / `encryptObjectByDesRules` / `getSmData` / `getDeviceId`）。
//!
//! # ⚠ 绑定账号的是**特征**，不是 smid，也不是 deviceId
//!
//! 这三样东西的生命周期**完全不同**，混起来就会把该变的东西钉死：
//!
//! | | 会不会变 | 本项目的处理 |
//! | --- | --- | --- |
//! | **设备特征**（屏幕、dpr、platform、canvas、cpu 数、ua…） | 真机上**不变**（那是同一台设备） | **绑定到账号**：首次生成一份，落进 `account_state.smid_fingerprint`，之后一直复用 |
//! | **smid 字段** | **每次都变** —— 它内嵌 `yyyyMMddHHmmss` 与一个现生成的 uuid | 每次上报现算，不落库 |
//! | **deviceId**（`B…`） | 服务端发的，会变（换指纹、过期都会换） | 每次上报重新取；库里的 `smid_device_id` 只是"最近拿到的那一个" |
//!
//! 旧项目**每跑一次任务就重新随机一份特征**（`core/src/utils/init-account.ts:33`
//! 调 `getDeviceId()`，而 `getSmData` 里那些字段全是当场 `Math.random`），
//! 等于同一台机器每次上报都长成另一台机器的样子。本项目不跟：
//! 特征生成一次、按账号存下来复用，**只在主动更换指纹时**才重生成。
//!
//! 于是本模块被拆成"稳定的一半 + 每次现算的一半"：
//! [`SmFingerprint`]（可序列化、进库）与 [`SmUpload`] + 那几个 `rng` 参数。
//!
//! # 这套特征是 **web / H5 侧**的（`os: "web"`）
//!
//! `platform: "Linux armv8l"`、`plugins`（Chrome PDF 查看器那一串）、`ua`
//! 都是**网页**特征，不是原生 App 特征。旧项目用它是为了给自己所有请求补一个
//! `deviceid` 头（`shared/src/http/base.ts:94`）。
//!
//! [`SmFingerprint::caiyun_android_web`] 逐字沿用旧项目那套池子，不"顺手改成 iOS"：
//! 我们手上没有原生侧 smid 的抓包，改了等于凭空发明一份特征。

use crate::{Upstream, error_from_status};
use mcloud_core::{
    Error, HttpClient, HttpRequest, Random, SKLAND_PUBLIC_KEY_B64, aes_cbc_fixed_iv_encrypt_hex,
    des_ecb_no_padding_encrypt_base64, get_tn_md5, gzip_with_os19_base64, md5_hex,
    rsa_pkcs1v15_encrypt_base64_with_b64_key,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// cmpassport 风控服务（**不是**云盘的域，所以不在 [`crate::base`] 里）。
///
/// 刻意与 `base::USER_YUN` 那批分开：这台机器不认云盘凭据，
/// 把 `Authorization` 发过去等于把凭据交给第三方风控服务。
pub const API_HOST: &str = "https://slw.h5cmpassport.com:9090";
/// 上报路径（旧 `SKLAND_SM_CONFIG.apiPath`）。
pub const API_PATH: &str = "/deviceprofile/v4";

/// 组织标识（旧 `SKLAND_SM_CONFIG.organization`）。
pub const ORGANIZATION: &str = "FXlyfmWg2AzwbrxDKSv5";
/// 固定 `appId`（旧 `SKLAND_SM_CONFIG.appId`）。
pub const APP_ID: &str = "default";
/// 成功码 —— **数字**，与云盘那批字符串码（`"0000"`）不是一类，所以不走 [`crate::Envelope`]。
pub const OK_CODE: i64 = 1100;
/// `deviceId` 的前缀：旧 `getDeviceId()` 拿到 `detail.deviceId` 后硬拼上去的那一个字母。
pub const DEVICE_ID_PREFIX: &str = "B";

/// 上报里"这次指纹发生在哪个页面上"（旧 `desTarget.url`）。
pub const CAIYUN_PAGE_URL: &str =
    "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin&sourceid=1002&token=";
/// 旧项目的 `CAIYUN_BASE_UA`（Android WebView）。
pub const CAIYUN_ANDROID_WEB_UA: &str = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36";
/// 特征体里 `plugins` 的取值（旧实现写死的一条，不是随机池）。
pub const PLUGINS: &str = "ChromePDFViewerPortableDocumentFormatinternal-pdf-viewer2,ChromiumPDFViewerPortableDocumentFormatinternal-pdf-viewer2,MicrosoftEdgePDFViewerPortableDocumentFormatinternal-pdf-viewer2,PDFViewerPortableDocumentFormatinternal-pdf-viewer2,WebKitbuilt-inPDFPortableDocumentFormatinternal-pdf-viewer2";
/// 请求头 `origin`（旧 `getDeviceId` 里写死的那个）。
pub const REQUEST_ORIGIN: &str = "https://m.mcloud.139.com";
/// 请求头 `referer`。
///
/// ⚠ 与特征体里的 `referer` **不是一回事**：体里那个是空串，头里这个是页面地址。
/// 旧代码就是这么分的，别合并。
pub const REQUEST_REFERER: &str = "https://m.mcloud.139.com/portal/mobilecloud/index.html";

/// 随机池：旧 `generateRandomScreenSize()`。
pub const SCREENS: &[(u32, u32)] = &[
    (375, 667),
    (390, 844),
    (414, 896),
    (360, 640),
    (360, 740),
    (412, 915),
    (1920, 1080),
    (1366, 768),
];
/// 随机池：旧 `platforms`。
pub const PLATFORMS: &[&str] = &["Linux armv8l", "Linux aarch64"];
/// 随机池：`dpr`。
pub const DPRS: &[i64] = &[2, 3];
/// 随机池：`maxTouchPoints`。
pub const MAX_TOUCH_POINTS: &[i64] = &[1, 5];
/// 随机池：`connectionRtt`。
pub const CONNECTION_RTT: &[i64] = &[50, 100, 150];
/// 随机池：`cpucount`。
pub const CPU_COUNTS: &[i64] = &[4, 6, 8, 10, 12];

// ============================ 稳定的一半：绑定账号的设备特征 ============================

/// **绑定到账号**的那一份设备特征。
///
/// 这就是"这台设备长什么样"：真机上它是不会变的，所以本项目生成一次就存下来。
/// 旧项目把它每次运行重掷一遍 —— 那是同一个 `deviceId` 反复拿不到、
/// 或者风控侧看到"一台机器天天换屏幕分辨率"的根源。
///
/// 序列化后进 `account_state.smid_fingerprint`（JSON 文本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmFingerprint {
    /// 特征体 `ua`。
    pub ua: String,
    /// 特征体 `url`。
    pub page_url: String,
    /// 特征体 `platform`。
    pub platform: String,
    /// 屏幕宽（进 `clientSize` 与 `res`）。
    pub screen_width: u32,
    /// 屏幕高。
    pub screen_height: u32,
    /// 设备像素比（进 `res` 第 3 段）。
    pub dpr: i64,
    /// 触屏点数（不进 DES，原名带出去）。
    pub max_touch_points: i64,
    /// 网络 rtt（同上）。
    pub connection_rtt: i64,
    /// cpu 数（同上）。
    pub cpucount: i64,
    /// canvas 指纹：`md5(uuid)` 的前 8 位。真机上由渲染结果决定，**稳定**。
    pub canvas: String,
}

impl SmFingerprint {
    /// 掷一份新特征（每个字段从旧项目那个池子里等概率取）。
    ///
    /// 顺序固定为：屏幕 → dpr → platform → 触屏点数 → rtt → cpu 数 → canvas。
    /// `Random` 一旦注入，这个顺序就是内容本身 —— 测试钉它。
    pub fn generate(rng: &mut impl Random) -> Self {
        let (width, height) = SCREENS[two_bytes(rng) as usize % SCREENS.len()];
        Self {
            ua: CAIYUN_ANDROID_WEB_UA.to_string(),
            page_url: CAIYUN_PAGE_URL.to_string(),
            platform: pick_str(rng, PLATFORMS).to_string(),
            screen_width: width,
            screen_height: height,
            dpr: pick_num(rng, DPRS),
            max_touch_points: pick_num(rng, MAX_TOUCH_POINTS),
            connection_rtt: pick_num(rng, CONNECTION_RTT),
            cpucount: pick_num(rng, CPU_COUNTS),
            canvas: canvas_from_uuid(rng),
        }
    }

    /// 逐字沿用旧项目那套池子里的**默认值**（屏幕 375×667、dpr 2、Linux armv8l…）。
    ///
    /// 用途是"不要随机也要有一份能用的"：夹具回放里想让整条链路字节级稳定时。
    pub fn caiyun_android_web() -> Self {
        Self {
            ua: CAIYUN_ANDROID_WEB_UA.to_string(),
            page_url: CAIYUN_PAGE_URL.to_string(),
            platform: PLATFORMS[0].to_string(),
            screen_width: 375,
            screen_height: 667,
            dpr: 2,
            max_touch_points: 5,
            connection_rtt: 50,
            cpucount: 8,
            canvas: "00000000".to_string(),
        }
    }

    /// `clientSize`：`0_0_0_0_宽_高_可用宽_可用高`（旧代码里可用尺寸就等于屏幕尺寸）。
    pub fn client_size(&self) -> String {
        format!(
            "0_0_0_0_{}_{}_{}_{}",
            self.screen_width, self.screen_height, self.screen_width, self.screen_height
        )
    }

    /// `res`：`宽_高_dpr_2`。
    pub fn res(&self) -> String {
        format!(
            "{}_{}_{}_2",
            self.screen_width, self.screen_height, self.dpr
        )
    }

    /// 换掉 canvas 那一项（其余不动）。
    ///
    /// 存在的理由：`canvas` 是唯一一个"我们冒充不了真实渲染结果"的字段，
    /// 万一哪天要对拍发现它可疑，可以只换它而不是整份重掷。
    pub fn with_canvas(mut self, canvas: impl Into<String>) -> Self {
        self.canvas = canvas.into();
        self
    }

    /// 存进库时的形态（JSON 文本）。
    pub fn to_json_string(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(|e| Error::Internal(format!("特征体序列化失败: {e}")))
    }

    /// 从库里读出来。
    ///
    /// 读失败**不回落到"重新掷一份"**：那等于静默换掉这台设备的长相，
    /// 而换指纹在这个服务侧是可观测事件，必须让调用方看见错误。
    pub fn from_json_str(raw: &str) -> Result<Self, Error> {
        serde_json::from_str(raw)
            .map_err(|e| Error::Parse(format!("库里的 smid 特征体读不出来: {e}")))
    }
}

fn canvas_from_uuid(rng: &mut impl Random) -> String {
    md5_hex(rng.uuid_v4().as_bytes())[..8].to_string()
}

// ============================ 每次现算的一半 ============================

/// 这一次上报的时间与"上一次的 deviceId"。
///
/// # 为什么时间是外面给的
///
/// 旧代码用 `new Date()` 取**本地时间**拼 `yyyyMMddHHmmss`，而特征体里
/// `timezone: -480` 是**写死**的东八区 —— 两者必须一致，否则等于自报
/// "我在东八区，可我的时间戳是别处来的"。本项目统一按东八区算，
/// 偏移由调用方（`mcloud_app::smid::beijing_time_digits`）给出。
#[derive(Debug, Clone, Copy)]
pub struct SmUpload<'a> {
    /// 14 位 `yyyyMMddHHmmss`（东八区）。
    pub time_digits: &'a str,
    /// `Date.now()`：特征体里 `svm` 与 `pmf` 用它。
    pub now_millis: i64,
    /// 上一次拿到的 `deviceId`（含 `B` 前缀）；**首次上报给空串**。
    ///
    /// 它进的是特征体的 `box` 字段：换 id 时把旧 id 带上，让风控服务看出
    /// "还是那台机器在换 id"，而不是一台凭空出现的设备。
    pub old_device_id: &'a str,
}

/// 特征体里的 `smid` 字段（对齐旧 `getSmId()`）。
///
/// ```text
/// v       = time_digits + md5(uid) + "00"
/// smskWeb = md5("smsk_web_" + v)[0..14]
/// smid    = v + smskWeb + "0"
/// ```
///
/// 长度固定为 `14 + 32 + 2 + 14 + 1 = 63`。
///
/// **注意这个 uid 与 [`build_sm_data`] 那个 uid 不是同一个**：旧 `getSmId()`
/// 内部自己又 `randomUUID()` 了一次。照搬，不"顺手统一"。
pub fn smid_string(time_digits: &str, uid: &str) -> String {
    let v = format!("{}{}00", time_digits, md5_hex(uid.as_bytes()));
    let smsk_web = md5_hex(format!("smsk_web_{v}").as_bytes())[..14].to_string();
    format!("{v}{smsk_web}0")
}

/// `priId`：AES 密钥，`md5(uid)` 的前 16 个字符（旧 `getSmData` 开头两行）。
pub fn private_id(uid: &str) -> String {
    md5_hex(uid.as_bytes())[..16].to_string()
}

/// `ep`：用 skland 公钥包住 `uid`（旧 `encryptRsa(uid, publicKey)`）。
///
/// ⚠ 输出**不可对拍**：PKCS#1 v1.5 的填充本身含随机字节（见
/// [`mcloud_core::rsa_pkcs1v15_encrypt_base64_with_b64_key`] 的说明），
/// 所以只验形状与可解性。
pub fn ep(uid: &str, rng: &mut impl Random) -> Result<String, Error> {
    rsa_pkcs1v15_encrypt_base64_with_b64_key(SKLAND_PUBLIC_KEY_B64, uid.as_bytes(), rng)
}

/// 组一份**加密前**的特征体：稳定的 [`SmFingerprint`] + 现掷的 smid / vpw / trees / 电量。
///
/// 除了那四个 volatile 字段，其余每一项都完全由入参决定 —— 这就是把特征拆出来
/// 换来的东西：**给定同一份指纹、同一个时间、同一个随机源，整份特征体可复现**。
pub fn fingerprint_target(
    fingerprint: &SmFingerprint,
    upload: &SmUpload,
    rng: &mut impl Random,
) -> Value {
    // 旧代码的求值顺序：getSmId() 在字面量里排第 9 个，vpw / trees 在它后面
    let smid_uid = rng.uuid_v4();
    let vpw = rng.uuid_v4();
    let trees = rng.uuid_v4();
    // 电池是真·易变量：旧项目每次现掷，本项目也每次现掷（它不属于"设备长相"）
    let charging = pick_num(rng, &[0, 1]);
    // 旧：Math.round(Math.random() * 100) / 100 —— 两位小数
    let level = (fraction(rng) * 100.0).round() / 100.0;

    json!({
        "protocol": 4,
        "organization": ORGANIZATION,
        "appId": APP_ID,
        "os": "web",
        "version": "3.0.0",
        "sdkver": "3.0.0",
        // 旧代码把这个位置叫 box，装的是"上一次的 deviceId"
        "box": upload.old_device_id,
        "rtype": "all",
        "smid": smid_string(upload.time_digits, &smid_uid),
        "subVersion": "1.0.0",
        "time": 7,
        "platform": fingerprint.platform,
        "clientSize": fingerprint.client_size(),
        "res": fingerprint.res(),
        "status": "0010",
        "timezone": -480,
        "vpw": vpw,
        "svm": upload.now_millis,
        "trees": trees,
        "pmf": upload.now_millis,
        "cdp": 0,
        "maxTouchPoints": fingerprint.max_touch_points,
        "connectionRtt": fingerprint.connection_rtt,
        "cpucount": fingerprint.cpucount,
        "battery": { "charging": charging, "level": level },
        "canvas": fingerprint.canvas,
        "plugins": PLUGINS,
        // 旧项目这里就是空串：referer 在**请求头**上，不在特征体里
        "referer": "",
        "ua": fingerprint.ua,
        "url": fingerprint.page_url,
        // 先占位：算 tn 时它必须还是空串（见 [`encode_target`]）
        "tn": "",
    })
}

/// 组好整份上报体（旧 `getSmData` 的返回）。
pub fn build_sm_data(
    fingerprint: &SmFingerprint,
    upload: &SmUpload,
    rng: &mut impl Random,
) -> Result<Value, Error> {
    // 外层这个 uid 同时是 ep 包住的那一个和 priId 推出来的那一个：
    // 分两次生成会让服务端看到"包裹用的 uid"与"解密用的密钥"对不上，那直接是失败。
    let uid = rng.uuid_v4();
    let target = fingerprint_target(fingerprint, upload, rng);
    Ok(json!({
        "appId": APP_ID,
        "organization": ORGANIZATION,
        "ep": ep(&uid, rng)?,
        "data": encode_target(&target, &uid)?,
        "os": "web",
        "encode": 5,
        "compress": 2,
    }))
}

/// 特征体 → 上报体里那个 `data` 字段（`tn` → DES 改名 → gzip → AES）。
///
/// # `tn` 那一步的顺序不能换
///
/// 旧代码是 `desTarget.tn = md5Hash(getTn(desTarget))`：**求值时 `tn` 还是空串**。
/// 先写回再算，摘要里会多出一段 32 位十六进制，整份指纹当场作废。
pub fn encode_target(target: &Value, uid: &str) -> Result<String, Error> {
    let mut with_tn = target
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Parse("smid 特征体不是 JSON 对象".into()))?;
    let tn = get_tn_md5(&Value::Object(with_tn.clone()));
    with_tn.insert("tn".into(), Value::String(tn));

    let encoded = encrypt_by_des_rules(&Value::Object(with_tn))?;
    // 旧：gzip(JSON.stringify(特征体)) → base64 → AES-CBC(那个 base64 串, priId) → hex
    let plain = serde_json::to_vec(&encoded)
        .map_err(|e| Error::Internal(format!("特征体序列化失败: {e}")))?;
    let gzipped = gzip_with_os19_base64(&plain)?;
    aes_cbc_fixed_iv_encrypt_hex(&private_id(uid), &gzipped)
}

// ============================ DES 字段规则 ============================

/// 一个字段的上报规则。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    /// `None` = **不加密**（旧 `is_encrypt: 0`），但仍然改名。
    pub cipher_key: Option<&'static str>,
    /// 混淆后的键名（上报体里真正出现的那个）。
    pub wire_name: &'static str,
}

/// 旧 `DES_RULE` 逐条照搬（25 条）。
///
/// 注意"没有规则"与"有规则但不加密"是两件事：前者**原名**输出（`smid`、`cdp`、
/// `battery` 那批），后者**改名**输出（`box`→`aw`、`protocol`、`version`）。
const RULES: &[(&str, Rule)] = &[
    (
        "appId",
        Rule {
            cipher_key: Some("a9kbown8"),
            wire_name: "um",
        },
    ),
    (
        "box",
        Rule {
            cipher_key: None,
            wire_name: "aw",
        },
    ),
    (
        "canvas",
        Rule {
            cipher_key: Some("ultuhs59"),
            wire_name: "iu",
        },
    ),
    (
        "clientSize",
        Rule {
            cipher_key: Some("7s9eixjh"),
            wire_name: "na",
        },
    ),
    (
        "organization",
        Rule {
            cipher_key: Some("14e30pp1"),
            wire_name: "nl",
        },
    ),
    (
        "os",
        Rule {
            cipher_key: Some("j862vr1f"),
            wire_name: "lz",
        },
    ),
    (
        "platform",
        Rule {
            cipher_key: Some("pgutfbo7"),
            wire_name: "lm",
        },
    ),
    (
        "plugins",
        Rule {
            cipher_key: Some("x98c92bm"),
            wire_name: "hy",
        },
    ),
    (
        "pmf",
        Rule {
            cipher_key: Some("gwvvsmr4"),
            wire_name: "yu",
        },
    ),
    (
        "protocol",
        Rule {
            cipher_key: None,
            wire_name: "protocol",
        },
    ),
    (
        "referer",
        Rule {
            cipher_key: Some("gka1ljjt"),
            wire_name: "mf",
        },
    ),
    (
        "res",
        Rule {
            cipher_key: Some("ht2n6dwf"),
            wire_name: "ut",
        },
    ),
    (
        "rtype",
        Rule {
            cipher_key: Some("k2p9ifij"),
            wire_name: "fm",
        },
    ),
    (
        "sdkver",
        Rule {
            cipher_key: Some("q4ij0mv5"),
            wire_name: "ir",
        },
    ),
    (
        "status",
        Rule {
            cipher_key: Some("moyspi7i"),
            wire_name: "kp",
        },
    ),
    (
        "subVersion",
        Rule {
            cipher_key: Some("egsyl21v"),
            wire_name: "dk",
        },
    ),
    (
        "svm",
        Rule {
            cipher_key: Some("yf780e9w"),
            wire_name: "ee",
        },
    ),
    (
        "time",
        Rule {
            cipher_key: Some("btb17oag"),
            wire_name: "jt",
        },
    ),
    (
        "timezone",
        Rule {
            cipher_key: Some("l94h8clc"),
            wire_name: "bl",
        },
    ),
    (
        "tn",
        Rule {
            cipher_key: Some("crnw0x7e"),
            wire_name: "as",
        },
    ),
    (
        "trees",
        Rule {
            cipher_key: Some("x6the9nq"),
            wire_name: "ij",
        },
    ),
    (
        "ua",
        Rule {
            cipher_key: Some("bewicpxu"),
            wire_name: "pu",
        },
    ),
    (
        "url",
        Rule {
            cipher_key: Some("llj8yd1x"),
            wire_name: "sb",
        },
    ),
    (
        "version",
        Rule {
            cipher_key: None,
            wire_name: "version",
        },
    ),
    (
        "vpw",
        Rule {
            cipher_key: Some("f1zlae4b"),
            wire_name: "qb",
        },
    ),
];

/// 查字段规则。
pub fn rule_for(field: &str) -> Option<Rule> {
    RULES
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, rule)| *rule)
}

/// `String(value)`（旧 `encryptDes(String(value), key)` 里的那次转换）。
///
/// 只有进 DES 的字段会经过它，而那些字段实际只有字符串与整数两种，
/// 所以这里只保证这两类与 JS 一致；其余按 JSON 文本兜底。
fn js_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return i.to_string();
            }
            if let Some(u) = n.as_u64() {
                return u.to_string();
            }
            // JS 的 `Number.toString()` 与 Rust 的 f64 Display 在小数上同为最简形式
            n.to_string()
        }
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// 按 [`RULES`] 把特征体改写成上报体（加密 + 换键名）。
///
/// 没有规则的字段**原名原值**带出去（旧 `encryptObjectByDesRules` 的 `else` 分支）。
pub fn encrypt_by_des_rules(target: &Value) -> Result<Value, Error> {
    let obj = target
        .as_object()
        .ok_or_else(|| Error::Parse("smid 特征体不是 JSON 对象".into()))?;
    let mut out = serde_json::Map::new();
    for (field, value) in obj {
        match rule_for(field) {
            Some(rule) => {
                let wire = match rule.cipher_key {
                    Some(key) => Value::String(des_ecb_no_padding_encrypt_base64(
                        key,
                        &js_to_string(value),
                    )?),
                    None => value.clone(),
                };
                out.insert(rule.wire_name.to_string(), wire);
            }
            None => {
                out.insert(field.clone(), value.clone());
            }
        }
    }
    Ok(Value::Object(out))
}

// ============================ 随机源 ============================

fn two_bytes(rng: &mut impl Random) -> u16 {
    let mut b = [0u8; 2];
    rng.fill_bytes(&mut b);
    u16::from_be_bytes(b)
}

/// 旧 `randomChoice`：从池子里等概率取一个。
fn pick_str<'a>(rng: &mut impl Random, pool: &'a [&'a str]) -> &'a str {
    pool[two_bytes(rng) as usize % pool.len()]
}

fn pick_num(rng: &mut impl Random, pool: &[i64]) -> i64 {
    pool[two_bytes(rng) as usize % pool.len()]
}

/// `Math.random()` 的位置：换成注入的 `Random`。
///
/// 分布细节与 `Math.random` 不同（这里 16 位均匀，那边 53 位），
/// **不影响正确性**：这是设备特征不是签名校验 —— 旧项目每次运行现掷一份都能过，
/// 说明服务端不比对内容，只吃"像个浏览器"这个形状。
fn fraction(rng: &mut impl Random) -> f64 {
    two_bytes(rng) as f64 / u16::MAX as f64
}

// ============================ 请求与响应 ============================

/// POST 上报体（旧 `getDeviceId` 里那个 `fetch`）。
///
/// 这台主机不是云盘的，所以**不带**任何云盘凭据（`Authorization` / `jwttoken` /
/// `x-yun-uni` 一个都不发）—— 它既不需要，也不该看到。
pub fn device_profile_request(
    payload: &Value,
    fingerprint: &SmFingerprint,
) -> Result<HttpRequest, Error> {
    // `json_body` 顺手补上 content-type: application/json
    let request = HttpRequest::post(format!("{API_HOST}{API_PATH}"))
        .header("origin", REQUEST_ORIGIN)
        .header("referer", REQUEST_REFERER)
        .header("user-agent", fingerprint.ua.clone())
        .json_body(payload)?;
    Ok(request)
}

/// 从响应里取 `deviceId`（旧 `code === 1100 → "B" + detail.deviceId`）。
///
/// 返回的是**拼好前缀**的那个，也就是最终进 `deviceid` 头的值。
///
/// # 为什么失败时不回吐响应体
///
/// 成功响应里就带着 `deviceId`，失败响应可能带别的机器能认的标识；
/// 而错误串会被打日志、会被测试断言引用。所以这里只放**码值**。
pub fn parse_device_id(raw: &Value) -> Result<String, Error> {
    let code = raw.get("code").and_then(Value::as_i64);
    if code != Some(OK_CODE) {
        return Err(Error::Parse(format!(
            "smid 上报被拒：code={code:?}（期望 {OK_CODE}）"
        )));
    }
    let device = raw
        .get("detail")
        .and_then(|d| d.get("deviceId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Parse(format!("smid 响应 code={OK_CODE} 但没有 detail.deviceId")))?;
    Ok(format!("{DEVICE_ID_PREFIX}{device}"))
}

impl<C: HttpClient> Upstream<'_, C> {
    /// 上报一份特征，换回一个 `deviceId`（带 `B` 前缀）。
    ///
    /// # 与旧实现的一处**刻意**不同
    ///
    /// 旧 `getDeviceId()` 把一切异常 `catch` 掉返回**空串**，由调用方判空。
    /// 这里返回 `Err`：空串一旦被当成"成功"存进库，下次读出来是非空，
    /// 于是那个账号永远绑着一个空设备、再也不会触发重新上报。
    pub async fn smid_device_id(
        &self,
        fingerprint: &SmFingerprint,
        payload: &Value,
    ) -> Result<String, Error> {
        let response = self
            .http
            .send(device_profile_request(payload, fingerprint)?)
            .await?;
        if !response.is_success() {
            return Err(error_from_status(response.status));
        }
        parse_device_id(&response.json::<Value>()?)
    }
}

#[cfg(test)]
mod tests;
