// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::account::*;
use crate::args::*;
use crate::cli_args::*;
use clap::Parser;
use mcloud_core::{Account, AccountId, StateStore, UserId, account_label};
use mcloud_io::SqliteStore;
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signin_run::*;
    use mcloud_core::Scope;

    #[test]
    fn new_accounts_go_to_the_first_user_when_one_exists() {
        // CLI 不要求"你是谁"，但新建的账号得有主人 —— 否则网页端看不到它
        let owner = Some(UserId::new("admin"));
        assert!(matches!(
            scope_for_new_accounts(&owner),
            Scope::User(id) if id.as_str() == "admin"
        ));
    }

    #[test]
    fn new_accounts_stay_unowned_when_no_user_exists_yet() {
        // 库里还没有用户时不能凭空造一个主人：账号保持无主，
        // 等 `mcloud user init` 时被认领（那条路径在 io 层有用例）
        assert!(matches!(scope_for_new_accounts(&None), Scope::Local));
    }

    // ── 账号选择（下标语义）─────────────────────────────────────────────
    //
    // 这几条盯的是**用户直接感受到的那一层**：旧项目里账号就是"账号 1 / 账号 2"，
    // 所以"下标几指哪个号"必须稳、错了必须说清楚。用 clap 真解析而不是手搓
    // `SigninArgs`，是为了连"参数之间的约束"一起验掉。

    /// 造一个只有账号、没有用户的临时库。
    ///
    /// 用 [`Scope::Local`] 是刻意的：这里要验的是**下标选择**，与归属无关；
    /// 归属那套（首个管理员认领无主数据）在 io 层有自己的用例。
    async fn store_with_accounts(accounts: &[(&str, bool)]) -> (tempfile::TempDir, SqliteStore) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = SqliteStore::open(dir.path().join("t.db")).expect("建库");
        for (position, (id, enabled)) in accounts.iter().enumerate() {
            let account = Account {
                id: AccountId::new(*id),
                enabled: *enabled,
                sort_order: position as i64,
                ..Account::default()
            };
            store
                .upsert_account(Scope::Local, &account)
                .await
                .expect("写入账号");
        }
        (dir, store)
    }

    fn signin_args(argv: &[&str]) -> SigninArgs {
        // 走**完整的**命令行解析与 [`Cli::into_command`] 合流，而不是只喂给
        // `SigninArgs`：这样子命令名、`conflicts_with_all`、"不带子命令"这条路
        // 全都在这一个入口上被验到。
        //
        // 这里刻意用**顶层写法**（`mcloud --account 2`），因为那正是用户
        // "直接运行"时会敲的形态；`mcloud signin --account 2` 由下面那条
        // 等价性用例覆盖。
        let command =
            Cli::parse_from(std::iter::once("mcloud").chain(argv.iter().copied())).into_command();
        let Command::Signin(args) = command else {
            panic!("解析出来必须是签到");
        };
        args
    }

    /// 顶层写法与 `mcloud signin` 写法**必须等价** ——
    /// 它们共用一个 `SigninArgs`，所以任何差异都说明有人把参数抄成了两份。
    #[test]
    fn top_level_and_subcommand_spellings_agree() {
        let bare =
            Cli::parse_from(["mcloud", "--account", "3", "--status-only", "--json"]).into_command();
        let named = Cli::parse_from([
            "mcloud",
            "signin",
            "--account",
            "3",
            "--status-only",
            "--json",
        ])
        .into_command();
        let (Command::Signin(bare), Command::Signin(named)) = (bare, named) else {
            panic!("两边都该是签到");
        };
        assert_eq!(bare.account, named.account);
        assert_eq!(bare.status_only, named.status_only);
        assert_eq!(bare.json, named.json);
        assert_eq!(bare.db, named.db, "默认库也要一致");
    }

    /// 不带 `--account` 就是全部**启用**的账号，且下标用的是**全列表位置**。
    ///
    /// 停用的那个占掉 2 号位、但不参与执行 —— 这两件事必须同时成立：
    /// 一旦执行时重新编号，同一个数字在不同时刻就会指到不同账号上，
    /// 而 `--account 2` 又走的是全列表位置，两套编号会打架。
    #[tokio::test]
    async fn signin_without_account_runs_every_enabled_account() {
        let (_dir, store) =
            store_with_accounts(&[("19****01", true), ("19****02", false), ("19****03", true)])
                .await;

        let targets = select_signin_targets(&store, Scope::Local, &signin_args(&[]))
            .await
            .expect("空参数应当能选出账号");
        let picked: Vec<(usize, &str)> = targets
            .iter()
            .map(|t| (t.index, t.account_id.as_str()))
            .collect();
        assert_eq!(
            picked,
            vec![(1, "19****01"), (3, "19****03")],
            "停用的 2 号位既不执行、也不被顶替"
        );
    }

    /// `--account N` 用的是**同一个**全列表位置，停用的账号也数进去 ——
    /// 显式指定就照做，否则"下标 = 清单里的行号"这条规则会有例外。
    #[tokio::test]
    async fn signin_account_index_counts_disabled_accounts_too() {
        let (_dir, store) =
            store_with_accounts(&[("19****01", true), ("19****02", false), ("19****03", true)])
                .await;

        let targets =
            select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "2"]))
                .await
                .expect("显式指定停用账号也应当照做");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].account_id.as_str(), "19****02");
        assert_eq!(targets[0].index, 2);
    }

    /// 下标从 1 开始。`--account 0` 一定是搞错了，且要**明确说是从 1 开始** ——
    /// 只回一句"没有这个账号"会让人以为库里是空的。
    #[tokio::test]
    async fn signin_account_index_is_one_based() {
        let (_dir, store) = store_with_accounts(&[("19****01", true)]).await;

        let err = select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "0"]))
            .await
            .expect_err("0 不是合法下标");
        let text = format!("{err}");
        assert!(text.contains("从 1 开始"), "{text}");
    }

    /// 越界时要说清"库里一共几个"，否则用户只能靠猜。
    #[tokio::test]
    async fn signin_account_index_out_of_range_reports_the_total() {
        let (_dir, store) = store_with_accounts(&[("19****01", true), ("19****02", true)]).await;

        let err = select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "9"]))
            .await
            .expect_err("越界");
        let text = format!("{err}");
        assert!(text.contains("超出范围"), "{text}");
        assert!(text.contains('2'), "要报出总数: {text}");
        assert!(text.contains("--list"), "要指出查清单的办法: {text}");
    }

    /// `--phone` 是**逃生口**：它按号码推 id，不查清单，所以空库也能选中 ——
    /// 那正是它的用途（老账号可能压根没进过清单）。下标记 0 表示"没有下标语义"。
    #[tokio::test]
    async fn signin_phone_needs_no_account_row() {
        let (_dir, store) = store_with_accounts(&[]).await;

        let targets = select_signin_targets(
            &store,
            Scope::Local,
            &signin_args(&["--phone", "19900000001"]),
        )
        .await
        .expect("按号码定位不需要库里有账号");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].index, 0, "没有下标语义就别编一个");
        assert_eq!(targets[0].phone_override.as_deref(), Some("19900000001"));
        assert_eq!(
            targets[0].account_id.as_str(),
            account_label(None, "19900000001")
        );
    }

    /// `--list` 是"查了就走"，与"指定跑谁"互斥 —— 让 clap 在解析期就拒掉，
    /// 而不是进到函数里再判一次（那种判法迟早会漏掉一条组合）。
    /// 两种写法都要拒，否则"顶层能用、子命令不能用"这种不一致会留一条口子。
    #[test]
    fn signin_list_conflicts_with_account_and_phone() {
        /// 拼一条 `… --list <extra>` 的命令行（`head` 是"顶层写法"或"子命令写法"）。
        fn list_argv(head: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
            let mut v = head.to_vec();
            v.push("--list");
            v.extend_from_slice(extra);
            v
        }

        for head in [&["mcloud"][..], &["mcloud", "signin"][..]] {
            assert!(Cli::try_parse_from(list_argv(head, &["--account", "1"])).is_err());
            assert!(Cli::try_parse_from(list_argv(head, &["--phone", "19900000001"])).is_err());
            assert!(Cli::try_parse_from(list_argv(head, &[])).is_ok());
        }
    }

    /// 不带子命令直接 `mcloud` 时走的就是签到那条路，且**顶层收全套签到参数**。
    ///
    /// 后者是这条用例真正的价值：早先顶层只认子命令，`mcloud --list` /
    /// `mcloud --status-only` / `mcloud --db X` 会被 clap 判成"未知参数"并报
    /// `Usage: mcloud [COMMAND]` —— 一个主打"直接运行"的命令行却要用户先知道
    /// 子命令叫 `signin` 才能查编号。
    #[test]
    fn bare_invocation_falls_through_to_signin() {
        let cli = Cli::try_parse_from(["mcloud"]).expect("不带子命令也要能解析");
        assert!(cli.command.is_none(), "顶层不该有默认子命令");

        let Command::Signin(args) = Cli::parse_from(["mcloud"]).into_command() else {
            panic!("不带子命令必须落到签到");
        };
        assert_eq!(args.db, PathBuf::from("data/mcloud.db"));
        assert!(
            args.account.is_none() && args.phone.is_none(),
            "零参数 = 全部启用账号"
        );
        assert!(!args.receive && !args.revive, "两个写开关默认都得关");
        // 新增的两个同理 —— 尤其 `--backup-gift` 是**每月一次**的写请求，
        // 默认打开等于每次跑签到都去消耗一次名额。
        assert!(
            !args.multiple && !args.backup_gift && !args.prizes,
            "`--multiple` / `--backup-gift` / `--prizes` 默认都得关"
        );

        // 顶层直接收参数（这一条以前是红的）
        for argv in [
            vec!["mcloud", "--list"],
            vec!["mcloud", "--status-only"],
            vec!["mcloud", "--json"],
            vec!["mcloud", "--db", "/tmp/other.db"],
        ] {
            let cli = Cli::try_parse_from(argv.clone())
                .unwrap_or_else(|err| panic!("{argv:?} 该被接受，却报：{err}"));
            assert!(cli.command.is_none(), "{argv:?} 不该被当成子命令");
        }
    }

    /// 写开关之间**可以叠加**，但它们与只读开关**必须互斥**。
    ///
    /// 为什么这条值得一个测试：`--prizes` 是只读、`--backup-gift` 是写，
    /// 两者都在同一次会话里发请求 —— 叠加起来不会报错，只会让"这次到底干了什么"
    /// 变得说不清。互斥关系是**声明式**的（`conflicts_with_all`），
    /// 写错一个名字 clap 不会报错，只会静默不互斥 —— 所以钉一条。
    #[test]
    fn write_and_read_switches_are_mutually_exclusive() {
        // 写开关之间叠加：允许（与旧项目把它们拆成不同任务是同一回事）。
        for argv in [
            vec!["mcloud", "--receive", "--multiple"],
            vec!["mcloud", "--receive", "--backup-gift"],
            vec!["mcloud", "--multiple", "--backup-gift"],
            vec![
                "mcloud",
                "--receive",
                "--revive",
                "--multiple",
                "--backup-gift",
            ],
        ] {
            Cli::try_parse_from(argv.clone())
                .unwrap_or_else(|err| panic!("{argv:?} 该被接受，却报：{err}"));
        }

        // 只读开关之间、以及只读与写之间：都不许叠。
        for argv in [
            vec!["mcloud", "--prizes", "--receive"],
            vec!["mcloud", "--prizes", "--backup-gift"],
            vec!["mcloud", "--prizes", "--raw"],
            vec!["mcloud", "--prizes", "--records"],
            vec!["mcloud", "--raw", "--backup-gift"],
            vec!["mcloud", "--records", "--backup-gift"],
            vec!["mcloud", "--status-only", "--backup-gift"],
        ] {
            assert!(
                Cli::try_parse_from(argv.clone()).is_err(),
                "{argv:?} 该被拒绝，却通过了"
            );
        }
    }
}
