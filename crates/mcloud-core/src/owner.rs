//! 归属（owner）判定。
//!
//! 本仓是**多用户**服务：同一条数据（云盘账号、定时、编排、运行记录）属于
//! 某个系统用户。判定必须只有一份实现 —— 路由层、存储层、测试都引用这里，
//! 否则「列表看不到但直取看得到」这类不一致迟早出现。
//!
//! # 为什么 owner 是 Option
//!
//! 归属是**后加的**：老数据的 JSON 里没有这一列，直接要求 UserId 会让
//! 整份 accounts.json 读不出来 —— 一个历史字段不该有那种破坏力。
//! 所以落盘用 Option<UserId> + serde(default, skip_serializing_if)，
//! 读得到就 Some，读不到就是 None，由**加载时回填**把它补成
//! Some(唯一的管理员)（见 mcloud_store::JsonStore 的回填）。
//!
//! # None 的可见性：失败关闭
//!
//! 回填之后，正常路径上不应再出现 None；真出现（进程内造的测试替身、
//! 或 users.json 被清空）时**不能**理解成「谁都能看」—— 那正是要修的漏洞。
//! 这里的规则是：None 只对**实例唯一的那个用户**可见。
//! 单用户时不存在「别人的东西」，所以不构成泄漏；多用户时谁都看不见
//! （宁可暂时看不到，也不把无主数据摊给所有人）。

use crate::user::{User, UserId};

/// 一条**已归属**或**未归属**的数据，对某个观察者是否可见。
///
/// - owner = Some(o)：只有 o 自己看得见；
/// - owner = None：只有 sole_user（实例唯一的用户）看得见；
///   实例有多个用户时 sole_user 传 None ⇒ 谁都看不见（失败关闭）。
///
/// 调用方负责算 sole_user（存储层看 users 表只有一个用户时才给）。
pub fn visible_to(viewer: &UserId, owner: Option<&UserId>, sole_user: Option<&UserId>) -> bool {
    match owner {
        Some(o) => o == viewer,
        None => sole_user == Some(viewer),
    }
}

/// 回填老数据时，把归属补在谁名下。
///
/// - 有用户 ⇒ **创建时间最早**的那个（并列时按 id 定序）。
///
/// # 为什么是「最早的那个」而不是「随便一个」
///
/// 老数据是在本功能存在**之前**建的，那时实例只可能有管理员一个人
/// （/api/setup 只能建一个）。后来多出来的用户只可能是手改
/// users.json 加进来的 —— 他们不该继承那份老数据。
/// 因此取最早的（也就是最初的管理员）是唯一有依据的选法。
///
/// 没有用户（全新库 / 用户表被清空）⇒ None：没有可归属的对象，
/// 此时数据保持未归属，由 visible_to 的单用户规则处置。
pub fn legacy_owner(users: &[User]) -> Option<UserId> {
    users
        .iter()
        .min_by(|a, b| (a.created_at_ms, &a.id).cmp(&(b.created_at_ms, &b.id)))
        .map(|u| u.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::PasswordHash;

    fn user(id: &str, created_at_ms: i64) -> User {
        User {
            id: UserId::new(id),
            name: id.into(),
            password_hash: PasswordHash("hashed".into()),
            created_at_ms,
        }
    }

    #[test]
    fn owned_data_is_visible_only_to_its_owner() {
        let a = UserId::new("a");
        let b = UserId::new("b");
        assert!(visible_to(&a, Some(&a), None));
        assert!(!visible_to(&b, Some(&a), None), "别人的东西不给看");
        // 单用户规则不影响已归属的数据
        assert!(!visible_to(&b, Some(&a), Some(&b)));
    }

    /// 未归属数据：单用户看得见，多用户谁都看不见（失败关闭）。
    #[test]
    fn ownerless_data_is_visible_only_to_the_sole_user() {
        let a = UserId::new("a");
        let b = UserId::new("b");
        assert!(visible_to(&a, None, Some(&a)), "唯一用户能看到无主数据");
        assert!(
            !visible_to(&b, None, Some(&a)),
            "多用户实例里无主数据不能对谁都可见"
        );
        assert!(
            !visible_to(&a, None, None),
            "实例有多个用户时无主数据对谁都不可见（失败关闭）"
        );
    }

    #[test]
    fn legacy_owner_is_the_earliest_user() {
        let users = vec![user("late", 200), user("early", 100)];
        assert_eq!(legacy_owner(&users), Some(UserId::new("early")));
    }

    /// 同一时刻创建时按 id 定序，保证可复现（不依赖表的先后）。
    #[test]
    fn legacy_owner_is_deterministic_on_ties() {
        let users = vec![user("b", 100), user("a", 100)];
        assert_eq!(legacy_owner(&users), Some(UserId::new("a")));
    }

    #[test]
    fn legacy_owner_is_none_without_users() {
        assert_eq!(legacy_owner(&[]), None);
    }
}
