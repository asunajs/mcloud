//! 凭据续期编排。
//!
//! # 为什么是一个 service 而不是 CLI 里的一个函数
//!
//! 这条链路有**四个触发面**：CLI 单账号、CLI 批量、WebUI 那枚按钮、
//! 以及（将来的）服务内调度。四个面都做同一件事：
//!
//! 1. 读库里那串 `Authorization` 的过期时刻
//! 2. 按**同一张**三档判据决定要不要打上游
//! 3. 打 `POST /user/auth/refreshToken`
//! 4. **窄写**回库（只改 auth 相关项，其余列不动）
//!
//! 把这段放在某一层里写一遍，另外三层就会各自"抄一遍"——
//! 而抄出来的必然漂移（判据差一档、忘了窄写、单位换算不同）。
//! 收敛到一处之后，四个面只是**入口不同**。
//!
//! # ⚠ 不自动重试
//!
//! 实测最慢一次 **28.4 秒**才返回。重试的代价不是慢，是**再轮换一次真凭据**。

use std::sync::Arc;

use mcloud_core::{Account, Clock, Error, Result, Store};
use mcloud_upstream::auth_refresh;

/// 默认提前量：7 天。
///
/// 参考实现有两处值（代码兜底 7、给用户配置的默认 10）—— 本仓取 7，
/// 并把它做成可调参数。
pub const DEFAULT_LEAD_DAYS: f64 = 7.0;

/// 判据的三种结果。
///
/// 它们是**三档**而不是"刷/不刷"两档：`ManualOnly` 与 `NotYet` 都不打上游，
/// 但**处置完全不同** —— 前者要重新登录，后者什么都不用做。
/// 合成一档，用户看到的就是"没刷"而不知道要不要管。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDecision {
    /// 该刷：剩余 ≤ 提前量，或 `expire == 0`。
    Needed,
    /// 还没到点 —— **一个请求都不发**。
    NotYet,
    /// 库里没有有效的过期时刻，只能重新登录。
    ManualOnly,
}

/// 判据纯函数（零 IO，可直接对拍）。
///
/// # 单位陷阱
///
/// `expire_ms` 与 `now_ms` 都是 **epoch 毫秒**。写成秒的后果**不是报错
/// 而是静默算错**：`1893456000000` 会被当成公元 61968 年，
/// "永远不需要刷新"，直到上游回 `90001` 才发现。
///
/// # 边界
///
/// - `expire < 0` ⇒ `ManualOnly`（**只有**这一种情况）
/// - `expire == 0` ⇒ `Needed`。参考实现两处不一致（一处当"跳过"、
///   一处当"该刷"）；本仓取"该刷"，因为 0 更可能是"解析不出"
///   而不是"确实没有"—— 打一次不亏，漏刷才是问题。
/// - 其余按剩余时间比
pub fn decide(expire_ms: i64, now_ms: i64, lead_days: f64) -> RefreshDecision {
    if expire_ms < 0 {
        return RefreshDecision::ManualOnly;
    }
    if expire_ms == 0 {
        return RefreshDecision::Needed;
    }
    let lead_ms = (lead_days * 86_400_000.0) as i64;
    if expire_ms - now_ms <= lead_ms {
        RefreshDecision::Needed
    } else {
        RefreshDecision::NotYet
    }
}

/// 一次刷新的结果。
/// ⚠ 不能 derive `Eq`：`Skipped` 带一个 `f64`（剩余天数）。
/// 天数只用于展示，不参与相等判定 —— 所以给 `PartialEq` 就够了。
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshOutcome {
    /// 换了新凭据。带新的过期时刻（毫秒）。
    Rotated {
        new_expire_ms: i64,
        /// 这次是不是 `--force` 越过了 `not-yet`。
        ///
        /// 单独一位是必要的：同一句"已换新凭据"不区分这两件事，
        /// 批量跑完人会以为提前量生效过。
        forced: bool,
    },
    /// 没到点，**没打上游**。
    Skipped { remaining_days: f64 },
    /// 需人工（`expire < 0`）。
    ManualOnly { expire_ms: i64 },
}

/// 从**刷新回来的整串凭据**里取过期时刻；取不到或非正数一律 `None`。
///
/// ⚠ 这个函数单独存在，是为了让"该喂哪一串"这件事**能被测试钉住**：
///
/// * 入参必须是 `rebuild_authorization`（`mcloud-upstream`）拼好的**整串** ——
///   `Basic ` + base64("端别:手机号:token")，也就是 `apply_refresh` 的第二个返回值；
/// * **不能**喂 `format!("{platform}:{phone}:{token}")` 这种明文。`parse_auth` 里
///   走的是 `node_like_b64_decode`（复刻 Node 的宽容解码），它会把字母表以外的
///   字符**直接丢掉** —— `:` 正是其中之一。明文进去 ⇒ 解出垃圾 ⇒ 拿不到 expire。
///
/// 代价不对等：拿不到 expire 不会报错，只会让 `updated.auth.expire` 停在旧值，
/// 于是 `decide()` 每轮都判"该刷"，**反复真轮换凭据**（对上游是不可逆动作）。
fn expire_from_new_auth(new_raw: &str) -> Option<i64> {
    mcloud_upstream::parse_auth(new_raw)
        .map(|parsed| parsed.expire)
        .filter(|expire| *expire > 0)
}

/// 凭据续期服务。
pub struct AuthRefreshService {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    /// 客户端工厂：刷新要按账号注入 `Authorization`。
    http_for: mcloud_engine::HttpClientFactory,
}

impl AuthRefreshService {
    pub fn new(
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
        http_for: mcloud_engine::HttpClientFactory,
    ) -> Self {
        Self {
            store,
            clock,
            http_for,
        }
    }

    /// 按判据刷一个账号。
    ///
    /// `force = true` 越过 `not-yet`，但**越不过** `ManualOnly` ——
    /// 那一档打了上游也注定失败。
    pub async fn refresh(
        &self,
        account: &Account,
        lead_days: f64,
        force: bool,
    ) -> Result<RefreshOutcome> {
        let now = self.clock.now_ms();
        let decision = decide(account.auth.expire, now, lead_days);

        match decision {
            RefreshDecision::ManualOnly => {
                return Ok(RefreshOutcome::ManualOnly {
                    expire_ms: account.auth.expire,
                });
            }
            RefreshDecision::NotYet if !force => {
                let days = (account.auth.expire - now) as f64 / 86_400_000.0;
                return Ok(RefreshOutcome::Skipped {
                    remaining_days: days,
                });
            }
            _ => {}
        }

        let forced = decision == RefreshDecision::NotYet;
        let new_expire_ms = self.rotate(account).await?;
        Ok(RefreshOutcome::Rotated {
            new_expire_ms,
            forced,
        })
    }

    /// 真打上游并窄写回库。
    async fn rotate(&self, account: &Account) -> Result<i64> {
        let http = (self.http_for)(account);
        let req = auth_refresh::refresh_request(account)?;
        let resp = http.request(req).await?;
        let (result, new_raw) = auth_refresh::apply_refresh(account, &resp)?;

        // ⚠ **窄写**：只改 auth 那几项，设备指纹与设置原样不动。
        //
        // 上游失败时库里**一点不动** —— `apply_refresh` 已经先返回了 Err，
        // 走到这里的都是拿到了新串。
        let mut updated = account.clone();

        // 过期时刻从**新凭据自己**里取（与登录路径同一个来源）。
        //
        // 用响应里的 `expireTime` 去算的话（秒、且是相对时长），
        // 两个来源迟早分叉 —— 那时"界面显示已过期、刷的时候却判 not-yet"。
        //
        // ⚠⚠ **必须喂 `new_raw`（整串），不能自己拼 `platform:phone:token` 明文。**
        // `parse_auth` 的契约是"`Basic ` + base64 整串"：它要**宽容解码**
        // （`:` 属于字母表以外的字符、会被直接丢掉）。喂明文 ⇒ 解出来是垃圾 ⇒
        // `expire > 0` **恒不成立** ⇒ `updated.auth.expire` 停在旧值 ⇒
        // 下次 `decide()` 仍判"该刷" ⇒ **每轮都真轮换凭据**（对上游是不可逆动作）。
        // 这是 2026-09-30 复核 qwen review 时查实的真 bug，靠 `expire_from_new_auth` 钉住。
        if let Some(expire) = expire_from_new_auth(&new_raw) {
            updated.auth.expire = expire;
        }

        updated.auth.raw = new_raw;
        updated.auth.token = result.token;

        self.store.save_account(&updated).await?;

        if let Some(secs) = result.expire_seconds {
            tracing::info!(
                account = %account.id,
                upstream_says_seconds = secs,
                "上游说新凭据还能用这么久（供排障，不落库）"
            );
        }
        Ok(updated.auth.expire)
    }

    /// 批量刷**所有**账号。
    ///
    /// # ⚠ 上游参考实现这里会过滤掉"停用中"的账号，我们**没有那个概念**
    ///
    /// mcloud 有账号级的启用/停用，所以它的 `--all` 只碰启用中的：
    /// 停用是用户主动的决定，把它悄悄刷成"看起来还能用"是抹掉那个决定。
    ///
    /// 本项目的 `AccountSettings` **没有** `enabled` 这一位（账号列表页
    /// 也没有启停开关）。所以这里不发明一个 —— 编一个默认 `true` 的字段，
    /// 表现是"过滤永远通过"，即一段**看起来在做事、其实什么也没做**的代码。
    ///
    /// 将来真加了启停，过滤要加在这里（**一处**），而不是让四个触发面
    /// 各写一遍。
    pub async fn refresh_all(
        &self,
        lead_days: f64,
        force: bool,
    ) -> Result<Vec<(Account, Result<RefreshOutcome>)>> {
        let all = self.store.accounts().await?;
        let mut out = Vec::new();
        for acct in all {
            let r = self.refresh(&acct, lead_days, force).await;
            out.push((acct, r));
        }
        Ok(out)
    }
}

/// 落库前的兜底校验：新串必须是能解析回来的。
///
/// 单独一条是因为"拼错了但看起来有值"是最难查的一类 —— 它在库里、
/// 在界面上都正常显示，只是每次请求都鉴权失败。
pub fn assert_usable(auth_raw: &str) -> Result<()> {
    mcloud_upstream::parse_auth(auth_raw)
        .map(|_| ())
        .ok_or_else(|| Error::Parse("刷新得到的新凭据解析不回来，拒绝落库".into()))
}

#[cfg(test)]
mod tests;
