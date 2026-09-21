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

> ⚠ **2026-09-20 起跑的是 Go 侧门禁**（Rust 侧已冻结到 `legacy/`，不再自动跑）。
> 分工没变，只是命令从 `cargo` 那套换成了 `go` 那套 —— 两个 hook 都只是
> `exec bash scripts/gates-go.sh <几条>`，逻辑不重复。

## ⚠⚠ 这个 hook 有**两层**各自都足以让它静默失效的东西

2026-09-21 实测发现：**这两层同时成立**，于是门禁**一次都没自动跑过**，
而任何地方都不报错 —— 提交照样成功，只是"以为有门禁"。

| 层 | 现象 | 为什么静默 |
|---|---|---|
| **1. `core.hooksPath` 没设** | git 默认看 `.git/hooks/`，那里是空的 | 没配就是没配，git 不认为这是异常 |
| **2. hook 没有 x 位** | 本目录的文件一度以 **`100644`** 入库（`git ls-files -s .githooks/` 可验） | **git 对没有 x 位的 hook 直接跳过，不报错** |

⇒ 装的时候**两件都要做**，`scripts/install-hooks.sh` 已经都做了
（`chmod +x` + `git config core.hooksPath`），并把 mode 变更提交进库。

⚠ 还有**第三层**，同样是"静默跳过"：hook 里若写成 `exec scripts/gates-go.sh`
（不带 `bash`），而 `scripts/*.sh` 在库里也是 **100644** ⇒ `Permission denied`，
**每一次提交都被拦住**。反过来（`exec bash …`）则完全不依赖那个 x 位。
**判据：路径上任何一环缺 x 位，"跳过"与"拦住"都不报出真正的原因** ——
所以要么让 x 位成为契约的一部分（全部补上并提交），要么让路径不依赖它。
本仓库选后者（文档与用法一律写 `bash scripts/gates-go.sh`）。

| hook | 跑什么 | 为什么是这些 |
|---|---|---|
| `pre-commit` | `fmt` `deps` `filenames` `idents` `jsontags` | 秒级、纯静态。**每条都能在一次编辑之后立刻修掉** |
| `pre-push` | 再加 `build` `vet` `test` `golden` | 要编译、要跑测试。**跑了才有意义，但不必每次保存都跑** |

这个分法不是随手切的，判据是"**发现问题的时刻与修复成本**"：

- `fmt` / `filenames` / `deps` / `idents` / `jsontags` 这五条，问题一旦产生就**立刻可修且必然要修**。
  让它们过夜再修，代价只是下次改到那儿时又看到一次。放 pre-commit。
- `build` / `vet` / `test` / `golden` 需要编译，`test` 是测试执行本身。
  每次 `git commit` 都跑会让"改一行注释提交一下"变慢——
  而**被拖慢的检查会被 `--no-verify` 绕过**，那是最坏的结果。
  放 pre-push：推之前一定跑得到，而推才是真正影响别人的动作。

> Go 编译比 Rust 快得多，所以"要不要把 `build` `vet` 也挪进 pre-commit"
> 是个可以讨论的问题。**当前刻意不挪** —— 一旦两道闸的内容开始按"快慢"
> 而不是按"发现问题的时刻"来切，这条判据就模糊了，而模糊的判据迟早
> 变成"看心情"。

## 没有第二道防线

这个仓库**没有 CI**。曾经有过（`.github/workflows/ci.yml`），2026-09-20 删掉了。

删它的理由见 `docs/dev-gates.md` 的「在哪里跑」一节。一句话版本：
**它存在期间推了 9 个提交，从未产生过任何影响** —— 没有一个提交提到过它的结果，
没有任何改动是因为它而做的。没人看的检查不是检查。

所以：

- **hook 是你唯一的门禁**。它拦不住的东西，就没有东西能拦住。
- 没有"绕过之后 CI 还会兜底"这回事。**用 `--no-verify` 就是真的跳过了。**

## 绕过

```bash
git commit --no-verify      # 跳过 pre-commit
git push --no-verify        # 跳过 pre-push
```

门禁真的坏在环境上时可以用（本机 Windows 上 `target/` 会偶发被写盘拦截，
见 `scripts/gates.sh` 里的重试逻辑）。

但既然没有兜底，绕过的正确姿势就只剩一条：**修好环境问题再提交**。
