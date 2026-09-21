package cli

import (
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

// newUserCommand 建 `user init|list`。
//
// # CLI **刻意不做登录**
//
// 数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，给他加一道
// "你是谁"既不增加安全性，又把本地运维变别扭。所以这里一律走
// `core.ScopeLocal()`（不过滤）。隔离是给**网络上的用户**准备的（WebUI）。
//
// 只保留两件事：init（建首个管理员，也是账号认领的触发点）与 list。
func newUserCommand(deps *Deps, out *output) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "user",
		Short: "系统用户管理（登录**本系统**的账号，与云盘账号是两回事）",
	}

	var (
		initName string
		initDB   string
	)
	init := &cobra.Command{
		Use:   "init",
		Short: "初始化首个管理员（顺手认领从旧库迁移过来、还没归属的账号）",
		RunE: func(*cobra.Command, []string) error {
			return runUserInit(deps, out, initName, initDB)
		},
	}
	init.Flags().StringVar(&initName, "name", defaultUserAdminName, "用户名")
	init.Flags().StringVar(&initDB, "db", defaultDB, "SQLite 文件")

	var listDB string
	list := &cobra.Command{
		Use:   "list",
		Short: "列出已有用户",
		RunE: func(*cobra.Command, []string) error {
			return runUserList(out, listDB)
		},
	}
	list.Flags().StringVar(&listDB, "db", defaultDB, "SQLite 文件")

	cmd.AddCommand(init, list)
	return cmd
}

// newMigrateCommand 建 `migrate`。
func newMigrateCommand(deps *Deps, out *output) *cobra.Command {
	a := &MigrateArgs{}

	cmd := &cobra.Command{
		Use:   "migrate",
		Short: "把旧项目（mcloud-sign）的数据迁移到 SQLite",
		RunE: func(*cobra.Command, []string) error {
			return runMigrate(deps, out, a)
		},
	}
	flags := cmd.Flags()
	flags.StringVar(&a.RepoRoot, "repo-root", defaultRepoRoot,
		"旧项目仓库根（据此推导 asign.json 与 packages/api/data/）")
	flags.StringVar(&a.Config, "config", "",
		"旧 asign.json 路径（覆盖 --repo-root 的推导结果）")
	flags.StringVar(&a.DataDir, "data-dir", "",
		"旧数据目录（覆盖 --repo-root 的推导结果）")
	flags.StringVar(&a.DB, "db", defaultDB, "目标 SQLite 文件")
	flags.BoolVar(&a.DryRun, "dry-run", false,
		"只预演：跑通身份映射与全部引用诊断并报出问题，**不写任何文件**")
	flags.StringVar(&a.BackupDir, "backup-dir", defaultLegacyBackup,
		"旧数据原样归档到这个目录（预演时不归档）")
	flags.BoolVar(&a.JSON, "json", false, "以 JSON 输出报告")
	a.flags = flags

	return cmd
}

// MigrateArgs 对位 `args.rs` 的 `MigrateArgs`。
//
// Config / DataDir 是"覆盖推导结果"的，所以留空必须与"没给"可分 —— 见 [MigrateArgs.given]。
type MigrateArgs struct {
	RepoRoot  string
	Config    string
	DataDir   string
	DB        string
	DryRun    bool
	BackupDir string
	JSON      bool

	flags *pflag.FlagSet
}

func (a *MigrateArgs) given(name string) bool { return a.flags != nil && a.flags.Changed(name) }

// newFixtureCommand 建 `fixture extract`。
func newFixtureCommand(deps *Deps, out *output) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "fixture",
		Short: "流量包 → 脱敏 fixture（离线回放测试用）",
	}

	var (
		har    string
		dir    string
		dryRun bool
	)
	extract := &cobra.Command{
		Use:   "extract",
		Short: "HAR → 脱敏 fixture",
		RunE: func(*cobra.Command, []string) error {
			return notMigrated("fixture extract")
		},
	}
	extract.Flags().StringVar(&har, "har", defaultFixtureHarDir, "单个 .har 文件或包含 .har 的目录")
	extract.Flags().StringVar(&dir, "out", defaultFixtureOutDir, "输出目录")
	extract.Flags().BoolVar(&dryRun, "dry-run", false, "只扫描与自检，不写文件")

	cmd.AddCommand(extract)
	return cmd
}
