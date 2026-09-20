//! App 端（手机客户端）档案：iOS App 的请求头与设备串。
//!
//! # 与 PC 端的差别不是"多几个头"，而是**另一套设备串语法**
//!
//! PC 端（[`crate::client::ClientProfile`]）的设备串是 14 段、第 5 段是 `PC`、
//! 第 6 段是 `QkYtMjAyMDAzMTAxNjQ3`（base64 的机型）；
//! App 端是 **14 段**、第 5 段是 `APPLE`、第 6 段是 `iPad8,6`（机型**明文**），
//! 而它的 `x-yun-client-info` 在设备串之后再追加两段（`x-DeviceName` 的 base64、`0`）
//! 并以 `;` 收尾 —— 所以那个头是 **16 段**。
//!
//! 两套串的字段表见 `docs/client-profiles.md`；本模块只实现**已抓到的 iOS 形态**。
//!
//! # 为什么没有 `Android`
//!
//! `clientType` / `cpid` / 设备串语法在 Android 上是另一组值，**没有实测**。
//! 按项目铁律（`docs/rust-rewrite-plan.md` §7.0：抓包前不写猜测性接口代码），
//! 这里不预先造一个 `Android` 变体 —— 那只会让"传错端"变成一个可以静默通过的选项。
//!
//! # 实测来源
//!
//! `docs/capture/app-sms-login-samples.txt`（FXTrace 网络日志，2026-09-18 22:54-22:56）。

/// iOS App 的固定业务参数。
///
/// 这几个值是**按端**变的（见 `docs/client-profiles.md` 的端对照表），
/// 混用会被上游当成另一个客户端。做成常量而不是散在构造点，
/// 是为了让"改版本号"只发生在一处。
pub mod ios {
    /// `x-yun-app-channel` 与 `x-huawei-channelSrc` 的实测值。
    pub const CHANNEL: &str = "10000020";
    /// 请求体 `clienttype`。
    pub const CLIENT_TYPE: &str = "412";
    /// 请求体 `cpid`。
    pub const CPID: &str = "55";
    /// 请求体 `version` 与设备串第 4 段。
    pub const APP_VERSION: &str = "12.6.0";
    /// `x-UserAgent` 的第 4 段（版本 + 构建号）。
    pub const UA_BUILD: &str = "12.6.0-200";
    /// `User-Agent` 的产品段。
    pub const UA_PRODUCT: &str = "mCloud_iPhone/12.6.0";
    /// `service_identifier_key` 的实测值。
    ///
    /// PC 端**没有**这个头；它是 App 端用来把请求路由到用户域的关键，
    /// 少了它 `/user/thirdlogin` 会落到别的服务上。
    ///
    /// **它是按服务变的，不是一个全局常量**：同一份日志里出现过 4 个取值
    /// （`service_identifier_caixun` / `_cloudDisk` / `_recommend` / `_userDomain`）。
    /// 这里给的是**登录链路**要用的那个；其它接口各自的值在接那些接口时再逐个确认。
    pub const SERVICE_IDENTIFIER: &str = "service_identifier_userDomain";

    /// `Authorization` 头的第一段（`Basic base64("<prefix>:<account>:<authToken>")`）。
    ///
    /// **PC 端是 `pc`，App 端是 `mobile`** —— 这是两端登录态最容易被忽略的差异：
    /// 两端的 `authToken` 形状完全一样（`前缀|1|RCS|13 位毫秒|载荷`），
    /// 只有这一段的字面量不同。
    ///
    /// 证据（`docs/capture/app-sms-login-samples.txt` §3.5）：App 自身日志里
    /// 1076 处 `Authorization`，解码后全部是 `mobile:<手机号>:<authToken>`。
    pub const AUTH_PREFIX: &str = "mobile";
}

/// 未登录时 App 发的 `Authorization` 占位值。
///
/// 实测原样是 `Basic base64("mobile:(null):(null)")` ——
/// 字面量 `(null)`（不是 JSON `null`、不是空串）。
/// 也就是说 App **即使没登录也会带这个头**，只是三段都是占位。
///
/// 记下来是为了两件事：① 解析上游日志时认得它，别当成一个坏 token；
/// ② 将来若要逐字节复刻 App 的未登录请求，这里就是那个值。
pub const ANONYMOUS_AUTHORIZATION: &str = "Basic bW9iaWxlOihudWxsKToobnVsbCk=";

/// App 端设备信息（一次登录里不变的部分）。
///
/// # 为什么把 `os_version` 拆出来
///
/// 同一个系统版本在三个地方有**三种写法**，实测原样如下：
///
/// | 出现位置 | 写法 |
/// | --- | --- |
/// | `x-yun-client-info` / `x-DeviceInfo` 第 9 段 | `ipados 26.6`（小写、iPad 上是 `ipados`） |
/// | `x-UserAgent` / `x-yun-user-agent` | `ios 26.6` |
/// | `User-Agent` 括号内 | `iOS 26.6` |
///
/// 统一成一个 `os_version` 再按位置拼，是为了让"升系统版本"只改一个字段；
/// 但它**不是**可以随便归一化的 —— 三个写法的差异是上游按正则解析的，
/// 所以这里保留三段拼接函数而不是把它们塞进一个模板常量。
#[derive(Debug, Clone)]
pub struct AppDevice {
    /// 设备 UUID：32 位**大写** hex。对应 PC 端的 `windowMachine`（设备指纹）。
    device_id: String,
    /// 设备短标识：12 位大写 hex。服务端按它 + `device_id` 定位。
    ///
    /// # 它**不是**每安装稳定值（实测）
    ///
    /// 同一台设备（`device_id` 完全相同）在两个时间点的这一段的取值**不一样**：
    ///
    /// | 观测时间 | `device_id` | 这一段 |
    /// | --- | --- | --- |
    /// | 2026-08-12 | `67F4C56A…315C` | `8A708867DBC3` |
    /// | 2026-09-18 | `67F4C56A…315C` | `EAB9BF94B8DE` |
    ///
    /// 所以它**不能**和 `device_id` 一起当"每安装指纹"持久化。
    /// 到底按什么变（每次启动 / 每天 / 某个事件）只有两个观测点，定不下来 ——
    /// 实现侧按"每次运行现生成"处理（见 `mcloud_app::device::new_app_device_short`）。
    device_short: String,
    /// `x-DeviceName` 的 base64（实测 `aVBhZA==` = `iPad`）。
    device_name_b64: String,
    /// 机型，实测 `iPad8,6`（**明文**，PC 端那一段是 base64）。
    model: String,
    /// 系统版本号，实测 `26.6`。
    os_version: String,
    /// 分辨率段，实测 `375 * 667`（**带空格**，别 trim）。
    screen: String,
    /// 语言段，实测 `zh-Hans-CN`。
    locale: String,
}

impl AppDevice {
    /// 用实测形状的默认值构造，只要求设备指纹。
    ///
    /// 默认值全部来自抓包原值（脱敏后），不是编的：
    /// `iPad8,6` / `ipados 26.6` / `375 * 667` / `zh-Hans-CN` / `aVBhZA==`。
    pub fn ipad(device_id: impl Into<String>, device_short: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            device_short: device_short.into(),
            device_name_b64: "aVBhZA==".to_string(),
            model: "iPad8,6".to_string(),
            os_version: "26.6".to_string(),
            screen: "375 * 667".to_string(),
            locale: "zh-Hans-CN".to_string(),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// 覆盖机型（换设备形态时用）。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 覆盖系统版本。
    pub fn with_os_version(mut self, version: impl Into<String>) -> Self {
        self.os_version = version.into();
        self
    }

    /// 覆盖 `x-DeviceName` 的 base64。
    pub fn with_device_name_b64(mut self, name_b64: impl Into<String>) -> Self {
        self.device_name_b64 = name_b64.into();
        self
    }

    /// `x-DeviceInfo`：**14 段**（末段为空，因为原串以 `|` 结尾）。
    ///
    /// 实测原串（脱敏）：
    ///
    /// ```text
    /// 1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|<uuid>|<short>|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|
    /// ```
    pub fn device_info(&self) -> String {
        format!(
            "1|{ip}|2|{ver}|APPLE|{model}|{id}|{short}|ipados {os}|{screen}|{locale}|0.0000|0.0000|",
            ip = DEFAULT_IP,
            ver = ios::APP_VERSION,
            model = self.model,
            id = self.device_id,
            short = self.device_short,
            os = self.os_version,
            screen = self.screen,
            locale = self.locale,
        )
    }

    /// `x-yun-client-info`：在 [`AppDevice::device_info`] 之后**追加**两段
    /// （`x-DeviceName` 的 base64、`0`），并以 `;` 收尾 —— 合计 16 段。
    ///
    /// 注意 `x-DeviceInfo` 与 `x-yun-client-info` **不是同一个值**
    /// （PC 端那两个头才是同值双名，App 端不是 —— 这是两端最容易抄错的一处）。
    pub fn client_info(&self) -> String {
        format!("{}{}|0|;", self.device_info(), self.device_name_b64)
    }

    /// `x-DeviceName`。
    pub fn device_name(&self) -> &str {
        &self.device_name_b64
    }

    /// `x-UserAgent`：`412|iPad|ios 26.6|12.6.0-200`。
    pub fn x_user_agent(&self) -> String {
        format!(
            "{}|iPad|ios {}|{}",
            ios::CLIENT_TYPE,
            self.os_version,
            ios::UA_BUILD
        )
    }

    /// `x-yun-user-agent`：`iPad|iPad|ios 26.6|mCloud12.6.0-200`。
    pub fn yun_user_agent(&self) -> String {
        format!("iPad|iPad|ios {}|mCloud{}", self.os_version, ios::UA_BUILD)
    }

    /// `User-Agent`：`mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)`。
    pub fn user_agent(&self) -> String {
        format!(
            "{} (iPad; iOS {}; Scale/2.00)",
            ios::UA_PRODUCT,
            self.os_version
        )
    }
}

/// 设备串第 2 段上报的 IP。
///
/// 它是**客户端自己报的局域网地址**，服务端不据此鉴权（实测换网后同一账号
/// 只这一段的取值变化，登录不受影响），所以做成常量而不是必须注入的字段。
const DEFAULT_IP: &str = "192.168.3.179";

/// App 端档案：一次登录里不变的部分。
#[derive(Debug, Clone)]
pub struct AppProfile {
    device: AppDevice,
    /// `x-yun-uni`。登录前就已经带上（实测值 = 响应里的 `userDomainId`），
    /// 但首次登录时调用方**没有**这个值 —— 所以它是可选的，
    /// 而不是"随便填一个"（填错会让上游定位到别人的用户域）。
    user_domain_id: Option<String>,
}

impl AppProfile {
    pub fn new(device: AppDevice) -> Self {
        Self {
            device,
            user_domain_id: None,
        }
    }

    /// 构造 App 的 `Authorization` 头：`Basic base64("mobile:<account>:<authToken>")`。
    ///
    /// 做成**关联函数**而不是 `&self` 的方法：这个头只依赖凭据，不依赖设备，
    /// 放在 `self` 上会暗示"换个 profile 就换个头"，而实际上不是。
    ///
    /// 与 PC 的 [`crate::client::ClientProfile`] 只有第一段不同（`mobile` vs `pc`），
    /// 但那一段错了服务端就认不出这是谁 —— 所以由本函数统一构造，
    /// 调用点不自己拼字符串。
    pub fn authorization(account: &str, auth_token: &str) -> String {
        format!(
            "Basic {}",
            mcloud_core::b64_encode(
                format!("{}:{account}:{auth_token}", ios::AUTH_PREFIX).as_bytes()
            )
        )
    }

    /// 带上 `x-yun-uni`（登录后 / 已登录态复用会话时）。
    pub fn with_user_domain_id(mut self, id: impl Into<String>) -> Self {
        self.user_domain_id = Some(id.into());
        self
    }

    pub fn device(&self) -> &AppDevice {
        &self.device
    }

    pub fn user_domain_id(&self) -> Option<&str> {
        self.user_domain_id.as_deref()
    }

    /// App 端请求要带的头。
    ///
    /// 逐个对应抓包实测的头集合（`docs/capture/app-sms-login-samples.txt` §3.2）。
    /// 每次请求会变的两个头（`x-yun-user-timestamp`、`Pragma`）也在这里 ——
    /// 前者由调用方传 `timestamp` 注入，这样请求构造函数仍是纯函数、可逐字段断言。
    pub fn headers(&self, timestamp: i64) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Pragma".to_string(), "no-cache".to_string()),
            ("x-yun-user-timestamp".to_string(), timestamp.to_string()),
            ("x-yun-app-channel".to_string(), ios::CHANNEL.to_string()),
            ("x-yun-net-type".to_string(), "1".to_string()),
            ("x-yun-svc-type".to_string(), "1".to_string()),
            ("x-huawei-channelSrc".to_string(), ios::CHANNEL.to_string()),
            ("x-yun-api-version".to_string(), "v1".to_string()),
            ("x-UserAgent".to_string(), self.device.x_user_agent()),
            ("x-yun-client-info".to_string(), self.device.client_info()),
            ("Accept-Language".to_string(), "zh-CN".to_string()),
            (
                "service_identifier_key".to_string(),
                ios::SERVICE_IDENTIFIER.to_string(),
            ),
            ("User-Agent".to_string(), self.device.user_agent()),
            (
                "x-DeviceName".to_string(),
                self.device.device_name().to_string(),
            ),
            ("x-DeviceInfo".to_string(), self.device.device_info()),
            ("x-yun-user-agent".to_string(), self.device.yun_user_agent()),
            ("x-SvcType".to_string(), "1".to_string()),
        ];
        if let Some(id) = &self.user_domain_id {
            headers.push(("x-yun-uni".to_string(), id.clone()));
        }
        headers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 抓包里那台设备的脱敏形态，全部断言都以它为准。
    fn device() -> AppDevice {
        AppDevice::ipad("AAA00000000000000000000000000000", "AAA000000000")
    }

    fn profile() -> AppProfile {
        AppProfile::new(device())
    }

    fn get<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("缺少头 {name}"))
    }

    #[test]
    fn device_info_has_14_segments_and_ends_with_a_bar() {
        let info = device().device_info();
        assert_eq!(
            info,
            "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|"
        );
        let segments: Vec<&str> = info.split('|').collect();
        assert_eq!(
            segments.len(),
            14,
            "设备串是 13 个字段 + 一个空的尾段 = 14；段数变了说明抄错了模板"
        );
        assert_eq!(segments[4], "APPLE");
        assert_eq!(segments[5], "iPad8,6", "机型是明文，不是 base64");
        assert_eq!(segments[8], "ipados 26.6");
        assert_eq!(segments[9], "375 * 667", "分辨率段里的空格是原样");
        assert_eq!(*segments.last().unwrap(), "", "原串以 | 结尾");
    }

    #[test]
    fn client_info_is_16_segments_and_differs_from_device_info() {
        let info = device().client_info();
        assert_eq!(
            info,
            "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|aVBhZA==|0|;"
        );
        let segments: Vec<&str> = info.split('|').collect();
        assert_eq!(segments.len(), 16);
        assert_eq!(
            segments[13], "aVBhZA==",
            "第 14 段是 x-DeviceName 的 base64"
        );
        assert_eq!(segments[14], "0");
        assert_eq!(segments[15], ";");

        // 两端最容易抄错的一处：PC 端 x-yun-client-info 与 x-DeviceInfo 同值，
        // App 端**不同值**。这里钉住"不同"这件事本身。
        assert_ne!(
            device().client_info(),
            device().device_info(),
            "App 端这两个头不是同一个值（PC 端才是）"
        );
    }

    #[test]
    fn three_os_version_spellings_are_all_present() {
        let d = device();
        // 同一个系统版本在三处三种写法 —— 统一成一种会让上游的正则失配
        assert!(d.device_info().contains("|ipados 26.6|"));
        assert_eq!(d.x_user_agent(), "412|iPad|ios 26.6|12.6.0-200");
        assert_eq!(d.yun_user_agent(), "iPad|iPad|ios 26.6|mCloud12.6.0-200");
        assert_eq!(
            d.user_agent(),
            "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)"
        );
    }

    #[test]
    fn os_version_override_reaches_all_three_spellings() {
        let d = device().with_os_version("18.2");
        assert!(
            d.device_info().contains("|ipados 18.2|"),
            "{}",
            d.device_info()
        );
        assert!(d.x_user_agent().contains("|ios 18.2|"));
        assert!(d.user_agent().contains("(iPad; iOS 18.2;"));
    }

    #[test]
    fn headers_match_the_captured_set() {
        let p = profile();
        let h = p.headers(1_789_743_362_439_040);

        assert_eq!(get(&h, "x-yun-user-timestamp"), "1789743362439040");
        assert_eq!(get(&h, "Pragma"), "no-cache");
        assert_eq!(get(&h, "x-yun-app-channel"), "10000020");
        assert_eq!(get(&h, "x-huawei-channelSrc"), "10000020");
        assert_eq!(get(&h, "x-yun-net-type"), "1");
        assert_eq!(get(&h, "x-yun-svc-type"), "1");
        assert_eq!(get(&h, "x-SvcType"), "1");
        assert_eq!(get(&h, "x-yun-api-version"), "v1");
        assert_eq!(get(&h, "Accept-Language"), "zh-CN");
        assert_eq!(
            get(&h, "service_identifier_key"),
            "service_identifier_userDomain"
        );
        assert_eq!(get(&h, "x-DeviceName"), "aVBhZA==");
        assert_eq!(get(&h, "x-yun-client-info"), p.device().client_info());
        assert_eq!(get(&h, "x-DeviceInfo"), p.device().device_info());

        // x-yun-uni 在首次登录时**不该出现**：那时还没有 userDomainId，
        // 编一个值会让上游定位到别人的用户域。
        assert!(
            !h.iter().any(|(k, _)| k == "x-yun-uni"),
            "未登录时不应带 x-yun-uni"
        );
    }

    #[test]
    fn user_domain_id_is_optional_and_only_added_when_known() {
        let p = profile().with_user_domain_id("1990000000000000001");
        let h = p.headers(1);
        assert_eq!(get(&h, "x-yun-uni"), "1990000000000000001");
    }

    #[test]
    fn headers_do_not_carry_pc_only_names() {
        let h = profile().headers(1);
        for pc_only in ["x-MM-Source", "x-yun-tid"] {
            assert!(
                !h.iter().any(|(k, _)| k == pc_only),
                "{pc_only} 是 PC 端才有的头，App 端抓包里没有"
            );
        }
    }

    #[test]
    fn authorization_uses_the_mobile_prefix_not_pc() {
        const TOKEN: &str = "SYNTH001|1|RCS|1893456000000|AAAA";
        let auth = AppProfile::authorization("19900000000", TOKEN);

        // 逐字节钉住整个头。比"解回来看看"更强：前缀写错时 base64 仍然合法，
        // 只有整串比对才挡得住。
        assert_eq!(
            auth,
            "Basic bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAxfDF8UkNTfDE4OTM0NTYwMDAwMDB8QUFBQQ=="
        );

        // 反证：PC 的前缀会得到另一串（这一条防止有人把两端的构造合并成一个）
        assert_ne!(
            auth, "Basic cGM6MTk5MDAwMDAwMDA6U1lOVEgwMDF8MXxSQ1N8MTg5MzQ1NjAwMDAwMHxBQUFB",
            "pc 前缀是 PC 端的形状，App 不能用"
        );
    }

    #[test]
    fn anonymous_authorization_matches_the_captured_bytes() {
        // 字面量 `(null)` 而不是空串 —— 照抄实测，别"顺手清理"成 `mobile::`
        assert_eq!(
            ANONYMOUS_AUTHORIZATION,
            "Basic bW9iaWxlOihudWxsKToobnVsbCk="
        );
        // 与构造函数自洽：用 `(null)` 当凭据应当得到同一个头
        assert_eq!(
            AppProfile::authorization("(null)", "(null)"),
            ANONYMOUS_AUTHORIZATION
        );
    }
}
