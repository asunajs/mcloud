//! 任务中止。
//!
//! # 为什么不用 `tokio::select!` 直接砍掉 future
//!
//! 直接 drop 一个正在发 HTTP 请求的 future 会留下**已发出但结果没人处理**的
//! 请求：签到请求真的打到上游了，本地却记成"已中止"。用户再点一次就是重复签到。
//!
//! 所以这里的取消是**协作式**的：取消只置一个标志，
//! 由任务自己在**安全点**（请求之间、循环每轮）检查并主动退出。
//! 代价是取消不是瞬时的；换来的是"中止"不会造成半个副作用。
//!
//! 任务若不检查，取消就只是"不再开始新步骤"——这是刻意的默认，
//! 比留下不一致的状态好。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// 取消令牌。可克隆，克隆共享同一份状态。
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    /// 用于让"正在等待"的任务立刻醒来，而不是等下一个轮询周期。
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。幂等 —— 重复调用无副作用。
    pub fn cancel(&self) {
        // 先置位再唤醒：反过来的话被唤醒的任务可能看到 false 又睡回去
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 在安全点检查取消。已取消则返回 `Err`。
    ///
    /// 任务的写法：
    /// ```ignore
    /// ctx.check_cancelled()?;      // 每轮循环开头
    /// do_one_request().await?;
    /// ```
    pub fn check(&self) -> crate::Result<()> {
        if self.is_cancelled() {
            Err(crate::Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// 等待取消发生。用于"等待外部事件"的任务。
    ///
    /// 不用轮询：`Notify` 让取消一发生就立刻返回。
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        // 循环防丢唤醒：notify_waiters 只唤醒**已在等待**的一方，
        // 若在我们注册之前就 cancel 了，这里会看到已置位而直接返回。
        loop {
            let waiter = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            waiter.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_uncancelled() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        assert!(t.check().is_ok());
    }

    #[test]
    fn cancel_is_visible_and_idempotent() {
        let t = CancelToken::new();
        t.cancel();
        assert!(t.is_cancelled());
        t.cancel(); // 重复调用不应出问题
        assert!(t.is_cancelled());
    }

    #[test]
    fn check_returns_cancelled_error() {
        let t = CancelToken::new();
        t.cancel();
        match t.check() {
            Err(crate::Error::Cancelled) => {}
            other => panic!("应返回 Cancelled，实际 {other:?}"),
        }
    }

    #[test]
    fn clones_share_state() {
        let a = CancelToken::new();
        let b = a.clone();
        a.cancel();
        assert!(b.is_cancelled(), "克隆必须共享同一份状态");
        b.cancel();
        assert!(a.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_returns_immediately_if_already_cancelled() {
        let t = CancelToken::new();
        t.cancel();
        // 已取消 ⇒ 不该挂住
        tokio::time::timeout(std::time::Duration::from_millis(100), t.cancelled())
            .await
            .expect("已取消时 cancelled() 应立即返回");
    }

    #[tokio::test]
    async fn cancelled_wakes_up_on_cancel() {
        let t = CancelToken::new();
        let t2 = t.clone();
        let handle = tokio::spawn(async move { t2.cancelled().await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        t.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(500), handle)
            .await
            .expect("取消后应唤醒等待者")
            .unwrap();
    }

    /// 取消令牌在发出取消**之后**才被等待，也必须立刻返回。
    ///
    /// 这是 `notify_waiters` 的经典坑：它只唤醒已在等待的一方。
    /// 若实现里少了"先看标志位"那一步，这里会永久挂起。
    #[tokio::test]
    async fn late_waiter_is_not_missed() {
        let t = CancelToken::new();
        t.cancel();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        tokio::time::timeout(std::time::Duration::from_millis(100), t.cancelled())
            .await
            .expect("取消先于等待发生时也不能丢");
    }
}
