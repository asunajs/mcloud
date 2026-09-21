package cli

// runSignin 是"不带子命令"与 `mcloud signin` 的**合流点**。
//
// 根命令与子命令各持一份 [SigninArgs]，但都走这个函数 ——
// 于是校验、退出码、后端缺口三条路径两边天然一致，
// 对位 Rust 的 `Cli::into_command()`（"两种写法在这里合流，往下只有一个入口"）。
func runSignin(deps *Deps, out *output, a *SigninArgs) error {
	// 先跑解析期之后、动手之前的用法校验：`--records-all` 单独给是**用法**错误，
	// 不该被"后端未迁移"这条**运行**错误盖掉（那会把退出码从 2 变成 1）。
	if err := a.validate(); err != nil {
		return err
	}
	return notMigrated("signin")
}
