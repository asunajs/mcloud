package cli

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
)

// runMigrate 把旧项目（mcloud-sign）的数据迁进 SQLite。
//
// 退出码口径沿用 Rust：**迁移本身跑完就是成功**。"有 3 条历史记录找不到账号"
// 这类需要人看一眼的问题只是**提示**，不能因此判失败 —— 那会让脚本
// 在完全正常的情况下退非零，然后有人开始加 `|| true`。
func runMigrate(deps *Deps, out *output, a *MigrateArgs) error {
	plan := app.MigrationPlanFromRepoRoot(a.RepoRoot, a.DB, a.BackupDir)

	// 显式给的路径覆盖推导结果：有人把 asign.json 挪到别处时，
	// 不该被迫把整个仓库根指到一个不对的地方。
	if a.given("config") {
		plan.Source.ConfigPath = a.Config
	}
	if a.given("data-dir") {
		plan.Source.DataDir = a.DataDir
	}

	ctx := context.Background()

	// 导入的账号归"库里的首个用户"；库里还没有用户就保持无主，
	// 等 `mcloud user init` 时认领 —— 所以两种顺序都能得到正确结果。
	var owner *core.UserID
	if !a.DryRun {
		// 预演不建库，所以只有真跑时才需要打开它拿归属。
		store, err := openLocal(a.DB)
		if err != nil {
			return err
		}
		defer store.Close()

		owner, err = preferredOwner(ctx, store)
		if err != nil {
			return err
		}
		if owner == nil {
			out.note("ℹ 库里还没有用户，导入的账号先保持无主。" +
				"跑一次 `mcloud user init` 建管理员时会把它们一并认领。")
		}
	}

	outcome, err := app.RunMigration(ctx, plan, a.DryRun, owner, deps.Clock)
	if err != nil {
		return err
	}

	if a.JSON {
		encoded, err := app.OutcomeToJSON(outcome)
		if err != nil {
			return fmt.Errorf("序列化迁移报告失败: %w", err)
		}
		return out.rawJSON(encoded)
	}

	for _, line := range outcome.HumanLines() {
		out.human("%s", line)
	}
	return nil
}
