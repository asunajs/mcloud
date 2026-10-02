//! 兑换页的「订阅」—— 用户钉在顶部的那几件奖品。
//!
//! # 为什么存**快照**（名字 / 组名），而不是只存 id
//!
//! 顶部那块要回答的是"我想要的东西现在能不能兑" ✓，而**上游清单是会变的**：
//! 奖品可能下架、这一场活动结束、或者换了账号之后那一件压根不在清单里。
//! 只存 id 的话，那几行只能显示成一串数字 ✗ —— 而这一页要解决的恰恰是
//! "别让我在长清单里翻找" ✓。⇒ 存一份订阅当时的名字，清单里还在时用**实时**
//! 那份覆盖它（见 `mcloud-server` 的兑换订阅路由）。
//!
//! # 为什么是"实例级"而不是"按云盘账号"
//!
//! 用户口径（2026-10-02）：「按单个奖品订阅」+「服务端（跟账号走、多设备一致）」。
//! 这里的"账号"是**登录 WebUI 的那个账号**（`mcloud` 用户），不是被操作的云盘账号 ——
//! 同一个人换个云盘账号，想看的东西还是那几件 ✓。落盘位置见 `JsonStore`。

use serde::{Deserialize, Serialize};

/// 一条订阅：用户钉在顶部的一件奖品。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeSubscription {
    /// 上游的奖品 id（对应 `ExchangePrizeView.prizeId`）。
    ///
    /// ⚠ 它是 `i64` 而不是字符串：上游给的就是数字，且**可能为负/为 0** ——
    /// 别拿"0 = 没给"当判据（`groupId` 才是那个有 `null` 的格子）。
    pub prize_id: i64,
    /// 订阅那一刻的名字快照。清单里还在时会被**实时**名字覆盖 ✓。
    pub name: String,
    /// 分组 id（`None` = 上游没给这一格）。
    pub group_id: Option<i64>,
    /// 分组名快照（顶部那行小字用）。
    pub group_title: Option<String>,
    /// 订阅时刻（毫秒）。
    ///
    /// 顶部按它**从早到晚**排：用户自己挑的顺序是稳定的，不会因为上游清单
    /// 换了顺序而让那几行跳来跳去 ✗（那正是"订阅"要消灭的东西之一）。
    pub subscribed_at_ms: i64,
}

/// 订阅 / 取消订阅一件，返回**新的**列表（不改入参）。
///
/// 放在 `mcloud-core` 而不是路由里：这是**领域规则**（同一个 `prize_id`
/// 只能出现一次、先订的在前），HTTP 那边只负责搬运 ✓。将来 CLI 也要按它走 ✓。
///
/// ⚠ 已订阅时**取消**，未订阅时**追加到末尾** —— 一次调用就是一次切换，
/// 前端不必先读再算（那会在两个标签页之间打架 ✗）。
pub fn toggled(
    list: &[ExchangeSubscription],
    prize_id: i64,
    name: &str,
    group_id: Option<i64>,
    group_title: Option<String>,
    now_ms: i64,
) -> Vec<ExchangeSubscription> {
    if list.iter().any(|s| s.prize_id == prize_id) {
        return list
            .iter()
            .filter(|s| s.prize_id != prize_id)
            .cloned()
            .collect();
    }
    let mut next: Vec<ExchangeSubscription> = list.to_vec();
    next.push(ExchangeSubscription {
        prize_id,
        name: name.to_string(),
        group_id,
        group_title,
        subscribed_at_ms: now_ms,
    });
    // 顺序按订阅时刻定，**不是**按插入顺序：两边（前端/服务端）各自算出来的
    // 列表顺序必须一样，否则同一次订阅在两个标签页里显示成两种顺序 ✗。
    next.sort_by_key(|s| (s.subscribed_at_ms, s.prize_id));
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: i64, at: i64) -> ExchangeSubscription {
        ExchangeSubscription {
            prize_id: id,
            name: format!("奖品{id}"),
            group_id: None,
            group_title: None,
            subscribed_at_ms: at,
        }
    }

    #[test]
    fn toggling_twice_returns_to_the_original_set() {
        let start = vec![sub(1, 100)];
        let on = toggled(
            &start,
            2,
            "云盘转存券",
            Some(9),
            Some("云盘转存券".into()),
            200,
        );
        assert_eq!(
            on.iter().map(|s| s.prize_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        let off = toggled(&on, 2, "云盘转存券", Some(9), None, 300);
        assert_eq!(off, start, "取消订阅必须回到原样（连时刻都不该被改）");
    }

    #[test]
    fn the_new_one_carries_the_name_snapshot() {
        let on = toggled(
            &[],
            42,
            "音乐会员月卡",
            Some(3),
            Some("音乐类会员".into()),
            500,
        );
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].name, "音乐会员月卡");
        assert_eq!(on[0].group_title.as_deref(), Some("音乐类会员"));
    }

    #[test]
    fn order_is_by_subscribe_time_not_by_insert_order() {
        // 先订 2、再订 1：时刻决定顺序 ⇒ 列表仍是 [1, 2]（按时间），
        // 与"谁先插进来"无关 —— 两个标签页各算一次也得到同一个顺序 ✓。
        let a = toggled(&[], 2, "乙", None, None, 200);
        let b = toggled(&a, 1, "甲", None, None, 100);
        assert_eq!(b.iter().map(|s| s.prize_id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn old_json_without_new_fields_still_loads() {
        // 订阅是**后来才有的**东西 ⇒ 老文件里没有它。这里钉住的是"多一个字段
        // 不该让已有文件读不出来"：未知字段被忽略，缺字段的用 `Option`/默认。
        let one: ExchangeSubscription =
            serde_json::from_str(r#"{"prize_id":7,"name":"旧件","subscribed_at_ms":1}"#)
                .expect("缺 group_id/group_title 必须能读");
        assert_eq!(one.group_id, None);
        assert_eq!(one.group_title, None);
        assert_eq!(one.prize_id, 7);
    }
}
