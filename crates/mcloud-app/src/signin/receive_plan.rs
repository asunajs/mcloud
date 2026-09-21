use super::SignInCredential;
use super::session::open_session;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{ReceiveItem, ReceiveResult, ReceiveTaskExpansion};
// ─────────────────────────────────────────────────────────────────────────────
// 领取的分流：`receiveList[]` → 该发哪个请求
// ─────────────────────────────────────────────────────────────────────────────

/// `receiveList[]` 里一条**该怎么处理** —— 这就是 H5 的点击分流，只是不依赖 UI。
///
/// # 两条完全不同的线（这是最容易搞混的地方）
///
/// | | **月度任务奖励** | **立即领取** |
/// | --- | --- | --- |
/// | `cloudType` | `1`（**已到期、现在能领**）/ `2`（**还没到期**，下月才可领）| 其它值 |
/// | 来源 | 签到/做任务攒下的记录，**跨月结算** | 当期挂在账上的 |
/// | 接口 | `receiveTaskExpansion(acceptDate)` | `receiveV3(client, cloudId, cloudType)` |
/// | 门槛 | **要开启自动备份**（否则 `1302`）| 无 |
/// | 生效 | 按 `acceptDate` 结算，**不是立即** | **直接生效** |
///
/// ⇒ 所以「`receiveList` 里有个数」**推不出**「现在能领到」：
/// 那个数可能是**下个月**才作数的那一类（[`Self::NextMonth`]）。
///
/// ⚠ `1` 与 `2` 是**同一个特性的两个时点**，不是两种奖励：
/// `2` 是"本月攒的、下月才能领"的预告，等它到期就变成 `1`（那时才带 `acceptDate`）。
/// 这也解释了"为什么 `acceptDate` 只在 `1` 上出现" —— 只有到期的那条才需要结算日。
///
/// # 判据出处
///
/// H5 `newsignin.336f18c3.js` 里那三行（逐字见
/// [`mcloud_upstream::app_signin::ReceiveItem::cloud_type`]）：
///
/// ```js
/// 1 === t.cloudType ? A("2", t.acceptDate, e)   // receiveTaskExpansion
/// : 2 === t.cloudType ? s.$toast("下月可领取")  // 纯提示，不发请求
/// : n("receivePoints", t, "", …)                // receiveV3
/// ```
///
/// 旧项目 `mcloud-sign` 是同一套逻辑，只是拆成两个任务，而且它走的是**老路径**
/// （`mrp.mcloud.139.com/market/signin/page/taskExpansion`，新 H5 已经没有这个"查询"接口，
/// `acceptDate` 改由 `infoV3.receiveList[]` 直接带下来）：
///
/// | 老路径字段 | 对应 | 老项目怎么用 |
/// | --- | --- | --- |
/// | `curMonthTaskRecordCount` | `cloudType == 1` | `> 0` 就 `receiveTaskExpansion(acceptDate)` |
/// | `nextMonthTaskRecordCount` | `cloudType == 2` | **只打印**「下月可领取 N 个云朵」 |
/// | `curMonthBackup` | `infoV3.curMonthBackup` | `false` 就先上传个文件触发备份再重试 |
///
/// `receiveTask` 单独负责立即领取那一条（老路径是无参的
/// `GET …/market/signin/page/receiveV2?client=app`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveAction {
    /// **月度任务奖励已到期**（`cloudType == 1`）—— 走 `receiveTaskExpansion`。
    ///
    /// `accept_date` 是上游在**同一条条目**上给的结算日，不是我们编的日期。
    ///
    /// ⚠ `None` = 上游没给 —— 这时**照样发**，只是不带查询参数。理由：H5 的调用点
    /// 逐字是 `A("2", t.acceptDate, e)` → `receiveTaskExpansion({acceptDate: t})`，
    /// 而 `t.acceptDate` 缺席时是 `undefined`，axios 的默认序列化器会把
    /// `undefined` **整个键丢掉** ⇒ 真实客户端发出去的就是**裸 GET**。
    ///
    /// 这里曾经做成"缺了就跳过不发"，那会与真实客户端**行为分叉**：
    /// 上游明明受理，我们却什么都没发，还看不出为什么。要"可见"就该把
    /// `accept_date: None` 这件事**打进结果**（见 [`ReceiveOutcome::MonthlyExpansion`]），
    /// 而不是替上游决定不发。
    MonthlyExpansion {
        accept_date: Option<String>,
        cloud_num: i64,
    },
    /// **下月才能领**（`cloudType == 2`）—— 现在**什么都不发**。
    NextMonth { cloud_num: i64 },
    /// **立即领取、直接生效** —— 走 `receiveV3`。
    Immediate {
        cloud_id: String,
        cloud_type: i64,
        cloud_num: i64,
    },
}

/// 把 `infoV3` 的 `receiveList` 按上面的规则分流。**纯函数**，不碰网络。
///
/// 抽成纯函数是为了能单测：这条分流逻辑是"领错东西"的唯一防线，
/// 而它一旦错，表现只是"领到 0 朵"——**看不出来**。
pub fn plan_receive(receive_list: &[ReceiveItem]) -> Vec<ReceiveAction> {
    receive_list
        .iter()
        .map(|item| match item.cloud_type {
            // 月度任务奖励：`acceptDate` 有就用，没有就发裸请求（见变体的说明）。
            // 空串按"没有"处理 —— H5 那边 `""` 也会被当成有值发出去，但一个空日期
            // 对服务端没有意义，**发 `?acceptDate=` 反而多一个空参数**，不如不发。
            1 => ReceiveAction::MonthlyExpansion {
                accept_date: item.accept_date.clone().filter(|date| !date.is_empty()),
                cloud_num: item.cloud_num,
            },
            // 下月可领取：H5 连请求都不发。
            2 => ReceiveAction::NextMonth {
                cloud_num: item.cloud_num,
            },
            // 其余：立即领取。`cloudId` 照 H5 的 `e.recordId ?? ""` —— 缺了就是空串。
            cloud_type => ReceiveAction::Immediate {
                cloud_id: item.record_id.clone().unwrap_or_default(),
                cloud_type,
                cloud_num: item.cloud_num,
            },
        })
        .collect()
}

/// 一条 [`ReceiveAction`] 跑完之后的实况。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// 月度任务奖励领成功，`cloud_count` 见
    /// [`ReceiveTaskExpansion::cloud_count`]（`None` = 上游没说领到多少）。
    ///
    /// `accept_date` 是**发出去的那个值**：`None` = 没带查询参数（裸 GET）。
    /// 如实记下来，是为了让"这条到底发了什么"可查 —— 它不影响成功与否。
    MonthlyExpansion {
        accept_date: Option<String>,
        cloud_count: Option<i64>,
    },
    /// 下月才能领 —— 没发请求。
    NextMonth { cloud_num: i64 },
    /// 立即领取成功，`received` 是上游回的 `result.receive`。
    Immediate { cloud_type: i64, received: i64 },
    /// 请求发了但**失败了** —— 不中断其余条目，如实记下来。
    Failed { cloud_type: i64, error: String },
}

/// 按 [`plan_receive`] 的分流**把该领的都领一遍**。
///
/// # 与 [`receive_cloud`] 的区别
///
/// [`receive_cloud`] 是"**我指定**领哪一笔"（`--cloud-type` 那种自检用法）；
/// 本函数是"**按上游的账目自己判断**该领哪些" —— 这才是日常该走的路。
///
/// 之所以要有它：把"该领哪个"交给用户，等于让用户去猜 `cloudType` 的含义，
/// 而猜错的表现是"领到 0 朵"，**看起来像上游没发**。
///
/// # 会话只建一次
///
/// `infoV3` 与随后每个领取请求共用同一条会话 —— 见 [`open_session`]。
///
/// # 单条失败不中断
///
/// 一条失败（例如月度那条回 `1302` 未开备份）不影响其余条目：它们本来就是
/// 互不相干的账。失败进 [`ReceiveOutcome::Failed`]，由调用方决定怎么报。
pub async fn receive_all<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    device_id: &str,
    clock: &K,
    rng: &mut R,
) -> Result<Vec<ReceiveOutcome>, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let status = upstream.app_sign_in_status(&jwt, &referer).await?;

    let mut outcomes = Vec::new();
    for action in plan_receive(&status.receive_list) {
        let outcome = match action {
            ReceiveAction::NextMonth { cloud_num } => ReceiveOutcome::NextMonth { cloud_num },
            ReceiveAction::MonthlyExpansion {
                accept_date,
                cloud_num: _,
            } => match upstream
                .app_receive_task_expansion(&jwt, &referer, accept_date.as_deref())
                .await
            {
                Ok(ReceiveTaskExpansion { cloud_count }) => ReceiveOutcome::MonthlyExpansion {
                    accept_date,
                    cloud_count,
                },
                Err(err) => ReceiveOutcome::Failed {
                    cloud_type: 1,
                    error: err.to_string(),
                },
            },
            ReceiveAction::Immediate {
                cloud_id,
                cloud_type,
                cloud_num: _,
            } => match upstream
                .app_receive_v3(&jwt, &referer, &cloud_id, Some(cloud_type), device_id)
                .await
            {
                Ok(ReceiveResult { receive }) => ReceiveOutcome::Immediate {
                    cloud_type,
                    received: receive,
                },
                Err(err) => ReceiveOutcome::Failed {
                    cloud_type,
                    error: err.to_string(),
                },
            },
        };
        outcomes.push(outcome);
    }
    Ok(outcomes)
}
