//! 直播侧（`live-room` / 小红花）的**常量与公共头** —— 全族的单一事实来源。
//!
//! # 这一族为什么单独一个文件
//!
//! `live-room` 一条链上跨了**三个不同的服务与三套响应约定**，常量散在四处时
//! 最容易出的一类错是"**把 A 侧的常量用在 B 侧**" —— 它不报错，只是查不到东西：
//!
//! | 侧 | 基址 | 响应约定 |
//! |---|---|---|
//! | 小红花（`/ycloud`） | `https://m.mcloud.139.com/ycloud` | `{code, msg, result}` 数字码 |
//! | 直播（`/cmlive`） | `https://v.cmcc-cs.cn/cmlive` | `{rtnCode:"0", rtnMsg, object}` **字符串**码 |
//! | IM（`videoim`） | `https://videoim.cmcc-cs.cn:20103` | `{code, msg, data, timestamp}` 数字码 |
//!
//! 实测踩过的两个具体例子都写进了常量注释里：
//! `targetSourceId` **两侧不同值**（小红花 `001005` / 直播 `017027`），
//! 以及 IM 的 HTTPS 端口 **20103** 与 WS 端口 **20102** 不是一个。
//!
//! # 分层
//!
//! 本模块只放**常量与纯拼装**，零 IO；请求构造与解析在各子模块里。

/// 小红花族（`/ycloud` 下，与签到同域）的接口根。
pub const BASE_LIVE_ROOM_FEEDBACK: &str = "https://m.mcloud.139.com/ycloud";

/// 直播侧 `/cmlive/api/...` 与直播 H5 页的入口。
pub const BASE_CMLIVE: &str = "https://v.cmcc-cs.cn/cmlive";

/// IM 的 HTTPS 入口（`member/apply` / `member/quit`）。
///
/// ⚠ 端口 **20103**，与 WS 的 20102 **不是同一个** —— 写错端口拿到的是超时，
/// 不是错误码，很难往"端口写错了"上想。
pub const BASE_VIDEO_IM: &str = "https://videoim.cmcc-cs.cn:20103";

/// IM 的 WebSocket 入口（SockJS）。
pub const BASE_VIDEO_IM_WS: &str = "wss://videoim.cmcc-cs.cn:20102";

// ── 小红花族 ────────────────────────────────────────────────────────────

/// **区分活动**的键。
///
/// ⚠ 它出现在两个地方：`visitlog/journaling` 的 form 体里，以及页面 URL 的 `path`。
/// 同一个 `visitlog/journaling` 接口也服务别的活动（实拍另见 `National_Activity`）
/// ⇒ **`marketName` 才是键，路径不是**。
pub const LIVE_ROOM_MARKET_NAME: &str = "National_LiveRoomFeedback";

/// 埋点的 `sourceid`（实拍 `1007`）。
pub const LIVE_ROOM_JOURNALING_SOURCE_ID: &str = "1007";

/// 抽小红花的路径（口令走 **query string**，不在 body 里）。
pub const LIVE_ROOM_DRAW_PATH: &str = "/liveRoomFeedback/redFlower/draw";

/// 小红花活动页（H5）的 UA。
///
/// ⚠ 与 [`LIVE_USER_AGENT`]（直播侧那个）**不是一个值**：这里的系统版本是 `18_7`
/// （2026-09-22 实拍），那边是 `18_5`（2026-09-17 夹具）。同一台设备在两个时间点
/// 抓到的 UA 会随系统升级而变，所以它**不是端常量，是抓包时刻的取值**
/// —— 照抄最近一次实拍，别去"统一"成一份。
pub const LIVE_ROOM_USER_AGENT: &str = "Mozilla/5.0 (iPad; CPU OS 18_7 like Mac OS X) \
AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN";

/// 签到族的 `activityid`。
///
/// ⚠ 小红花族的请求头里带的是**这个**（实拍），不是直播专属值 ——
/// 与签到同族。别"顺手改成直播侧的某个 id"。
pub const LIVE_ROOM_ACTIVITY_ID: &str = "sign_in_3";

/// H5 的 `appversion` 头。
pub const LIVE_ROOM_APP_VERSION: &str = "12.6.0.0";

/// 换票用的 `toSourceId`（**小红花侧**）。
///
/// ⚠ 直播侧用的是 [`LIVE_SOURCE_ID`]（`017027`）。用 `017027` 去筛 `/ycloud` 侧
/// 会一条都筛不出来（坑表 #106）。
pub const TO_SOURCE_ID: &str = "001005";

// ── 直播侧 ──────────────────────────────────────────────────────────────

/// 移动云盘频道 id。
///
/// ⚠ 它**只是查询参数**，不是过滤条件：实拍（`app-live-redflower-samples.txt` §5.3）
/// 里 `?channelId=134` 拉回来的房间，`channelId` 有 `29 / 51 / 243 / 134` 四种 ——
/// 那是**房间的归属**，不是"我拉的是哪个频道"。拿它当过滤条件会一间都筛不出来。
pub const LIVE_CHANNEL_ID: &str = "134";

/// 直播房间页 URL 上的 `targetSourceId`（**只在直播侧**）。
pub const LIVE_SOURCE_ID: &str = "017027";

/// **会发口令**的那个主播的昵称。
///
/// ⚠ 判据是**精确相等**，不是包含：实拍那张表里还有「福建移动」「移动心选」
/// 「移动臻选」三个昵称，`status===1` 的房间里就有「福建移动」
/// ⇒ **只看 `status` 会误判**，真正干活的是这个名字。主播改名即失效（原项目如此）。
pub const KOULING_ANCHOR: &str = "中国移动云盘";

/// 「盘盘福利」页签。
pub const LIVE_TAB_ID_BENEFITS: i64 = 127;

/// 「精彩回放」页签。
///
/// ⚠ 原项目**只查 127**。这里两个都查，依据是 2026-09-22 的实拍：
/// 同一间房（147606）**同时**出现在 127 与 129 里，而下一场（147608，国庆场）
/// **只在 129 里**。⇒ 只查 127 会在"房间还没进福利页签"时判成"没开播"，
/// 而那正是"漏掉整场口令"这一侧的错误（代价远大于多打一个 GET）。
pub const LIVE_TAB_ID_REPLAY: i64 = 129;

/// `status = 0`：**未开播（预告）**。
///
/// 三档语义由 2026-09-22 实拍钉死：`1` 直播中 / `6` 已结束 / `0` 未开播。
/// 原项目只记了前两档，`0` 是这次补上的 —— 而它恰好就是"今天抓不到 WS"的原因。
pub const LIVE_ROOM_STATUS_NOT_STARTED: i64 = 0;
/// `status = 1`：直播中。
pub const LIVE_ROOM_STATUS_LIVE: i64 = 1;
/// `status = 6`：已结束（回放）。
pub const LIVE_ROOM_STATUS_ENDED: i64 = 6;

/// 直播 H5 的 UA（iPad App 内嵌 WebView 的原样值）。
///
/// ⚠ 与 [`LIVE_ROOM_USER_AGENT`] 不是一个值（系统版本 `18_5` vs `18_7`）。
pub const LIVE_USER_AGENT: &str = "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) \
AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN";

/// `/cmlive` 请求的 `Origin`。
pub const LIVE_ORIGIN: &str = "https://v.cmcc-cs.cn";

/// `member/apply` 的 `Version` 头。
///
/// ⚠ **未实拍**：抓包里这个头是 `<REDACTED>`（脱敏时连带把版本号一起遮了）。
/// 取值来自原项目 `apis/live-room-chat.ts` 的字面量 `v23.0.2`，与 [`PROVISION_VERSION`]
/// 的 `23.0.2` 对得上（只差 `v` 前缀）。**这是本文件里唯一一个"取自原项目而非实拍"
/// 的常量**，开播后第一次抓包要回来核对。
pub const LIVE_APPLY_VERSION: &str = "v23.0.2";

/// MSync `Provision.version`。
pub const PROVISION_VERSION: &str = "23.0.2";

/// 云盘门户基址（拼 `Referer` / `Origin` 用）。
pub const BASE_MCLOUD: &str = "https://m.mcloud.139.com";

// ── 业务码 ──────────────────────────────────────────────────────────────

/// 口令无效：**需当场直播获取**才有效。
pub const CODE_KOULING_INVALID: i64 = 200108;
/// 首次参与惊喜奖：已参与过，本次没有。
pub const CODE_FIRST_PARTICIPATE_ALREADY: i64 = 200110;
/// 口令已兑换过（每个口令只能兑换一次）。
pub const CODE_KOULING_ALREADY_DRAWN: i64 = 200112;
/// 口令已失效（**本场已结束**）。
pub const CODE_KOULING_EXPIRED: i64 = 200118;

// ── 拼装 ────────────────────────────────────────────────────────────────

/// 小红花活动页的 `Referer`。
pub fn live_room_page_referer() -> String {
    format!(
        "{BASE_MCLOUD}/portal/yunClound/index.html?path={LIVE_ROOM_MARKET_NAME}\
&sourceid={LIVE_ROOM_JOURNALING_SOURCE_ID}&targetSourceId={TO_SOURCE_ID}"
    )
}

/// 直播间页的 `Referer`。
///
/// 真实客户端的这一串里还有一个 `cmpassToken=<…>`（进房间时带的凭据）。
/// 纯协议实现不重复带它 —— `Referer` 在服务端不参与鉴权（鉴权走 `Authorization` 头），
/// 而把一个会过期的凭据拼进 URL 只会让日志里多一处凭据。
pub fn live_room_referer(room_id: &str) -> String {
    format!(
        "{BASE_MCLOUD}/portal/yunClound/index.html?path={LIVE_ROOM_MARKET_NAME}\
&roomId={room_id}&sourceid={LIVE_ROOM_JOURNALING_SOURCE_ID}&targetSourceId={LIVE_SOURCE_ID}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两侧的 `targetSourceId` 必须**不同** —— 这是最容易张冠李戴的一处。
    #[test]
    fn source_ids_differ_between_sides() {
        assert_eq!(TO_SOURCE_ID, "001005");
        assert_eq!(LIVE_SOURCE_ID, "017027");
        assert_ne!(TO_SOURCE_ID, LIVE_SOURCE_ID);
    }

    /// 两个 UA 不是同一个值（系统版本不同），别"统一"它们。
    #[test]
    fn user_agents_are_not_unified() {
        assert_ne!(LIVE_USER_AGENT, LIVE_ROOM_USER_AGENT);
        assert!(LIVE_USER_AGENT.contains("18_5"));
        assert!(LIVE_ROOM_USER_AGENT.contains("18_7"));
    }

    /// IM 的两个端口不是一个（HTTPS 20103 / WS 20102）。
    #[test]
    fn video_im_ports_differ() {
        assert!(BASE_VIDEO_IM.contains(":20103"));
        assert!(BASE_VIDEO_IM_WS.contains(":20102"));
    }

    /// `Referer` 的 path 就是 `marketName` —— 两处必须是同一个键。
    #[test]
    fn referer_carries_market_name_and_to_source() {
        let r = live_room_page_referer();
        assert!(r.contains(LIVE_ROOM_MARKET_NAME));
        assert!(r.contains(&format!("targetSourceId={TO_SOURCE_ID}")));
        assert!(r.starts_with(BASE_MCLOUD));
    }

    /// 直播间 `Referer` 用**直播侧**的 sourceId，且带上 roomId。
    #[test]
    fn room_referer_uses_live_source() {
        let r = live_room_referer("147606");
        assert!(r.contains("roomId=147606"));
        assert!(r.contains(&format!("targetSourceId={LIVE_SOURCE_ID}")));
        assert!(!r.contains(TO_SOURCE_ID));
    }
}
