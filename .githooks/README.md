# Git hooks

这个目录是**入库的** hooks 实现。Git 默认不看它，要跑一次安装：

```bash
bash scripts/install-hooks.sh
```

那一步做的事就是把 `core.hooksPath` 指到这里：

```bash
git config core.hooksPath .githooks
```

## 为什么不用 `.git/hooks/`

`.git/` 不进版本库。把 hook 放在那里等于**只有当前这台机器、当前这个克隆**有——
换个目录克隆一份就没有了。放到 `.githooks/` + `core.hooksPath`，
hook 就跟代码一起被 review、被同步，也不需要谁记得"上一台机器上是怎么配的"。

## 两道闸，各拦各的

| hook | 跑什么 | 为什么是这些 |
|---|---|---|
| `pre-commit` | `fmt` `deps` `filenames` `idents` | 秒级、纯静态。**每条都能在一次编辑之后立刻修掉** |
| `pre-push` | 再加 `clippy` `test` `golden` | 分钟级、要编译。**跑了才有意义，但不必每次保存都跑** |

这个分法不是随手切的，判据是"**发现问题的时刻与修复成本**"：

- `fmt` / `filenames` / `deps` / `idents` 这四条，问题一旦产生就**立刻可修且必然要修**。
  让它们过夜再修，成本只是白等一次 CI。放 pre-commit。
- `clippy` / `test` / `golden` 需要完整编译，`test` 是测试执行本身。
  每次 `git commit` 都跑会让"改一行注释提交一下"变成两分钟——
  而**被拖慢的检查会被 `--no-verify` 绕过**，那是最坏的结果。
  放 pre-push：推之前一定跑得到，而推才是真正影响别人的动作。

## 与 GitHub Actions 的关系（不是"两道防线"）

`.github/workflows/ci.yml` 跑的是**同一组命令**。同一组命令在两个地方执行
**不构成两次独立的检查** —— 真正独立的第二道防线必须能发现第一道查不出来的问题，
而重复执行同一组断言只是赌"环境差异会碰巧暴露什么"，那是巧合不是设计。

两者的真实分工是：

| | 覆盖 | 什么时候起作用 |
|---|---|---|
| 本地 hook | **日常**。改完就能知道，不用等 | 你在这台机器上提交/推送时 |
| GitHub Actions | **对外**。公开仓库里任何人可在 Actions 页面核对 | 推送之后，作为一份可外证的历史 |

CI 的价值在于"**对外可核**"，而不是"兜底"。它不兜任何 hook 漏掉的东西，
因为它的检查面并不更宽。

## 绕过

```bash
git commit --no-verify      # 跳过 pre-commit
git push --no-verify        # 跳过 pre-push
```

门禁真的坏在环境上时可以用（本机 Windows 上 `target/` 会偶发被写盘拦截，
见 `scripts/gates.sh` 里的重试逻辑）。

⚠ **绕过之后没有任何东西会替你补上。** CI 跑的是同样的命令，
所以它查不出你用 `--no-verify` 跳过的那次检查——除非那次改动本身
在干净环境里也会失败。绕过的正确姿势是：**修好环境问题再提交**，
而不是把 `--no-verify` 当成常规操作。
