package cli

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// openLocal 打开库（本地运维视角，**不要求任何身份**）。
//
// 与 [preferredOwner] 一起构成 CLI 侧的"归属"两半：CLI 不问你是谁，
// 但新建的云盘账号**必须有主人** —— 网页端按隔离规则看不到无主账号，
// 否则会出现"扫码成功了、网页里却没有这个账号"这种最难解释的现象。
func openLocal(path string) (*mcloudio.SqliteStore, error) {
	store, err := mcloudio.Open(path)
	if err != nil {
		return nil, fmt.Errorf("打开数据库 %s 失败: %w", path, err)
	}
	return store, nil
}

// preferredOwner 取"新建云盘账号的默认归属"= 库里的首个用户（通常就是那个管理员）。
//
// 库里还没有用户时返回 nil，账号保持无主，等 `mcloud user init` 时被认领。
// 这不是权限检查，只是挑一个默认主人。
func preferredOwner(ctx context.Context, store *mcloudio.SqliteStore) (*core.UserID, error) {
	users, err := store.ListUsers(ctx)
	if err != nil {
		return nil, fmt.Errorf("读用户列表失败: %w", err)
	}
	if len(users) == 0 {
		return nil, nil
	}
	owner := users[0].ID
	return &owner, nil
}

// runUserInit 建首个管理员，并把无主的旧数据认领给它。
//
// 📌 口令只在栈上活一次：从 [Deps.readPasswordTwice] 直接交给
// [app.InitFirstAdmin]（那里由 core 的 argon2id 哈希），
// 全程不进参数、不进打印、不进错误串。
func runUserInit(deps *Deps, out *output, name, dbPath string) error {
	store, err := openLocal(dbPath)
	if err != nil {
		return err
	}
	defer store.Close()

	password, err := deps.readPasswordTwice()
	if err != nil {
		return err
	}

	ctx := context.Background()
	outcome, coreErr := app.InitFirstAdmin(
		ctx, store, name, password, deps.Random, deps.Clock.Now())
	if coreErr != nil {
		return coreErr
	}

	out.human("已创建管理员 %s（角色 %s）", outcome.User.Name, outcome.User.Role)
	if outcome.Claimed.Total() > 0 {
		// 三笔分开报：从旧项目升上来的人最想知道"我的东西还在不在"，
		// 只回一个总数等于把这件事糊成一句话。
		out.human("已认领旧数据里原本没有归属的数据：账号 %d 个、定时任务 %d 个、组合 %d 个",
			outcome.Claimed.Accounts, outcome.Claimed.Schedules, outcome.Claimed.Pipelines)
	}

	out.human("\n接下来：mcloud login qr    # 用手机扫码添加第一个云盘账号")
	return nil
}

// runUserList 列出系统用户。只读，且 CLI 持有库文件，所以不需要任何身份。
func runUserList(out *output, dbPath string) error {
	store, err := openLocal(dbPath)
	if err != nil {
		return err
	}
	defer store.Close()

	users, err := store.ListUsers(context.Background())
	if err != nil {
		return fmt.Errorf("读用户列表失败: %w", err)
	}
	if len(users) == 0 {
		out.human("还没有用户。先跑：mcloud user init")
		return nil
	}

	out.human("用户 %d 个：", len(users))
	for _, user := range users {
		// 逐个字段点名打 —— 不能顺手 `%+v` 整个结构体：User 里含 PasswordHash，
		// struct 级的 Stringer 挡不住 `%+v` 展开字段（那是 PasswordHash 自己接住的）。
		out.human("  %-20s %-6s 建于 %s",
			user.Name, user.Role, user.CreatedAt.Format(time.RFC3339))
	}
	return nil
}
