#!/usr/bin/env bash
# Go 侧门禁一键跑 —— `scripts/gates.sh`（Rust 侧）的 Go 版。
#
# 九条，与 Rust 侧七条的对应关系：
#
#   build      ≈ cargo build（"能编译"的最直接判据）
#   vet        ≈ cargo clippy --all-targets -- -D warnings（**覆盖窄得多**，见下）
#   fmt        ≈ cargo fmt --check
#   test       ≈ cargo test --workspace（**多一个 -race**，见下）
#   deps       ≈ bash scripts/check-deps.sh   →  Go 版：scripts/check-go-deps.sh
#   filenames  = 同一个脚本，**复用**
#   idents     = 同一个脚本，**复用**
#   golden     = 同一个脚本，**复用**
#   jsontags   = Go 独有（字段名那个洞，见「第 9 条」那节）
#
# ⚠ filenames / idents / golden 三条刻意复用 Rust 侧那三个脚本，不重写。
#   它们与语言无关（扫文件名 / 扫文本里的真实标识 / 独立复算向量），
#   重写一遍等于给自己造第二份真相 —— 而两份真相迟早会分叉，
#   分叉的那天没有人知道该信哪一份。
#
# 用法：
#   bash scripts/gates-go.sh              # 全跑
#   bash scripts/gates-go.sh test deps    # 只跑指定几条
#
# ⚠ 为什么 test 必须带 -race（Rust 侧没有对应物）：
#
#   Rust 里 `&mut R rng` 保证同一随机源不被并发共享 —— 那是**编译期保证**。
#   Go 的 io.Reader 是引用语义，并发用同一个源是 data race，而它**不一定**
#   让测试变红：可能只是偶发地产生错误数据（签名对不上、deviceId 错一位），
#   于是被当成"上游抽风"。所以 -race 是那条保证在 Go 侧的替代品，不是可选项。
#
# ⚠ 为什么 vet 不等于 clippy：
#
#   `go vet` 只覆盖"几乎肯定是错"的一小撮检查（printf 格式串、锁被复制、
#   无用的赋值…），比 clippy 窄得多。补齐要引 golangci-lint，而那是**外部
#   依赖** —— 骨架阶段刻意一个第三方依赖都不引（见 docs/go-migration-plan.md）。
#   **在那之前，不要把 vet 当成 clippy 用**：它过了不代表静态检查这一关过了。
#
# ⚠ bash 3.2 兼容（macOS 自带的 /bin/bash 永远是 3.2.57，且不会升）：
#   不用 declare -A / mapfile / ${v,,}。理由见 docs/dev-gates.md。
set -uo pipefail

cd "$(dirname "$0")/.."

# ⚠⚠ 这是**唯一**一份清单。下面的 `ORDER` 与末尾横幅都必须从它派生，别再手写第二份 ——
#   早先 `ORDER=(build vet fmt test …)` 是把它逐字重抄的一遍，于是
#   **加了门禁却忘了加进 `ORDER` ⇒ 它默认永远不跑，而且什么都不报**
#   （`is_known_gate` 只保证"名字合法"，不保证"它会被跑到"）。
GATE_NAMES="build vet fmt test deps filenames idents golden jsontags"
ALL=($GATE_NAMES)

# ⚠ 为什么用显式包路径，而不是惯用的 `./...`：
#   `./...` 曾把 `legacy/target/`（7 GB 的 cargo 构建产物）也遍历一遍
#   —— legacy/ 已于 2026-09-21 剥离为同级仓库 mcloud-rust，该遍历问题不复存在；
#   显式包范围**保留**：它还顺带把"只测交付物"写进了定义。
#   两个后果，都不是理论上的：
#     ① 慢 —— 几万个与 Go 无关的文件；
#     ② **偶发失败** —— 当时 cargo 正在改写那个目录，Go 会撞上刚被删掉的临时目录，
#        报 `pattern ./...: open legacy/target/debug/deps/xxxx: no such file or directory`，
#        门禁于是红了，而代码一行问题都没有。
#        （2026-09-20 冻结后第一次跑就撞上：那轮 Go 门禁与 Rust 门禁是同时在跑的。）
#   代价是"新加一个顶层目录要记得改这里"。这个代价由 check-go-deps.sh 的
#   「所有 .go 文件都必须在 cmd/ 或 internal/ 下」那条断言兜住 —— 漏不掉。
GO_PKGS="./cmd/... ./internal/..."

# gofmt -l 只**列出**未格式化的文件，退出码仍是 0 —— 所以不能直接当门禁用，
# 必须自己判"输出是否为空"。这一步很容易写错成 `gofmt -l . && echo ok`。
#
# ⚠ 范围与 GO_PKGS 对齐（不用 `.`）：历史上 `gofmt -l .` 会走进 legacy/target，
#   剥离后继续用显式范围，理由见上。
gate_fmt() {
  local out
  out="$(gofmt -l ./cmd ./internal 2>&1)"
  if [ -n "$out" ]; then
    echo "!! 以下文件未格式化（跑 'gofmt -w .' 可修）："
    echo "$out"
    return 1
  fi
  echo "ok gofmt：全部已格式化"
  return 0
}

# ── `-race` 跑不跑得了：先探测，再决定 ──────────────────────────────
#
# `go test -race` 在 **Windows** 上必须开 cgo，而 cgo 必须有一个 Go 能用的
# C 编译器（`go env CC`，默认 gcc）。两道缺一，go 报的是：
#     go: -race requires cgo; enable cgo by setting CGO_ENABLED=1
#     cgo: C compiler "gcc" not found: exec: "gcc": executable file not found
# 两条都**不是代码问题**，但它们会让 `test` 红 —— 而 `test` 一红，这台机器
# 一条提交都推不出去。
# （2026-09-20 实测：Windows 机器上没有 gcc，且本机网络取不到 mingw-w64。）
#
# ⚠ 这里是**降级**，不是静默放过：
#   · 降级会大声说 —— 横幅、该条门禁的结果行、末尾汇总行，三处都标出来；
#   · 理由是：一个"拦住一切"的检查，最后会被 `--no-verify` 整体绕过，
#     连带把其余七条一起废掉 —— 那样的实际价值是 **0**。
#     宁可少一条保证但**看得见**，不要"文档写着有、实际一次没跑"。
#
# 装上 mingw-w64 / w64devkit（或任何能让 `go env CC` 命中的编译器）后，
# 下面这个函数自己会恢复成带 -race，不用改脚本。
race_available() {
  local cc
  cc="$(go env CC 2>/dev/null)"
  [ -n "$cc" ] || return 1
  command -v "$cc" >/dev/null 2>&1
}

RACE_SKIPPED=0

gate_test() {
  if race_available; then
    # CGO_ENABLED 在没有 C 编译器时默认是 0，而 -race 要求 1 —— 显式设上。
    CGO_ENABLED=1 go test -race $GO_PKGS
    return $?
  fi
  RACE_SKIPPED=1
  printf '\033[33m⚠ -race 不可用：cgo 需要 C 编译器「%s」，本机没有。\033[0m\n' "$(go env CC)"
  printf '\033[33m  ⇒ 本轮**不**做竞态检测（Rust 侧由 `&mut R` 在编译期保证的那条，\033[0m\n'
  printf '\033[33m     在 Go 侧唯一替代品就是 -race —— 它现在没跑）。\033[0m\n'
  printf '\033[33m  ⇒ 装 mingw-w64 / w64devkit 后自动恢复，无需改脚本。\033[0m\n'
  go test $GO_PKGS
}

gate_run() {
  case "$1" in
    build)     go build $GO_PKGS ;;
    vet)       go vet $GO_PKGS ;;
    fmt)       gate_fmt ;;
    test)      gate_test ;;
    deps)      bash scripts/check-go-deps.sh ;;
    filenames) bash scripts/check-filenames.sh ;;
    idents)    node scripts/check-identifiers.cjs ;;
    golden)    node scripts/check-golden-vectors.cjs ;;
    jsontags)  node scripts/check-go-jsontags.cjs ;;
    *)
      # 名字已过 `is_known_gate`（它在 GATE_NAMES 里），却走到这里 ⇒ **本函数的
      # case 漏了它**，即"登记了门禁但没实现"。别让它以 `✗ <名字> 失败` 收场 ——
      # 那会让人去查这条门禁本身，而问题在脚本这张表上。
      printf '!! 门禁 "%s" 登记在 GATE_NAMES 里，但 gate_run 没有它的实现分支\n' "$1" >&2
      return 127
      ;;
  esac
}

# 默认顺序 = 上面那份清单本身（派生，不重抄）。
ORDER=($GATE_NAMES)
if [ $# -gt 0 ]; then
  ORDER=("$@")
fi

# 校验门禁名：**在跑之前**按名字查表，不靠退出码。
#
# ⚠ 为什么不能沿用"退出码 127 = 未知门禁"（原写法）：
#   127 同时是 shell 的 "command not found" 退出码。于是
#   **Go 没装时**，`go build` 返回 127，脚本会打印
#       !! 未知门禁: build（可选：build vet fmt test deps filenames idents golden）
#   ——把"这台机器没有 Go"报成了"这个脚本写错了"，而且是**列着 build 自己
#   告诉你它不存在**，看上去自相矛盾，非常耗排查时间。
#   （2026-09-20 实测：Windows 机器上没装 Go，第一条门禁就报这个。）
#
#   这与 Rust 侧踩过的坑是同一类：把环境问题翻译成"你的东西有问题"。
#   按名字查表后，名字合法但命令失败就是正常的"✗ build 失败"，
#   终端会打出真实原因（`go: command not found`）。
is_known_gate() {
  case " $GATE_NAMES " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

for name in "${ORDER[@]}"; do
  if ! is_known_gate "$name"; then
    echo "!! 未知门禁: $name（可选：$GATE_NAMES）"
    exit 2
  fi
done

FAILED=()
# 日志放 target/（已在 .gitignore 里），用 gates-go- 前缀与 Rust 侧区分开 ——
# 两套门禁的日志混在一个文件里，排障时会看错那一条的输出。
LOG="target/gates-go-last.log"
mkdir -p target 2>/dev/null
: > "$LOG"

for name in "${ORDER[@]}"; do
  printf '\n\033[1m──── %s ────\033[0m\n' "$name"

  # 每条门禁的输出单独一个文件：诊断只看本次这条说了什么，
  # 不被前面通过的门禁留下的字样污染（这个坑 Rust 侧踩过，见 scripts/gates.sh）。
  RUN_LOG="target/gates-go-${name}.log"

  if gate_run "$name" >"$RUN_LOG" 2>&1; then
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    if [ "$name" = "test" ] && [ "$RACE_SKIPPED" = "1" ]; then
      printf '\033[33m⚠ %s 通过（降级：未跑 -race）\033[0m\n' "$name"
    else
      printf '\033[32m✓ %s 通过\033[0m\n' "$name"
    fi
  else
    # 名字的合法性已在上面按表校验过，所以走到这里就是**真的失败了** ——
    # 不要再拿退出码去猜"是不是未知门禁"（127 撞车，见上面 is_known_gate）。
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    FAILED+=("$name")
  fi
done

if [ ${#FAILED[@]} -gt 0 ]; then
  printf '\033[31m门禁未通过: %s\033[0m\n' "${FAILED[*]}"
  echo "完整输出: $LOG"
  exit 1
fi
# ── 横幅必须说清**这次到底跑了哪几条** ─────────────────────────────────
#
# ⚠ 早先这里无条件打"九项门禁全绿"，于是 `bash scripts/gates-go.sh fmt` 也打出
#   "九项门禁全绿" —— 把"跑了 1/9"报成"9/9"。而 **pre-commit 跑的正是 5 条子集**，
#   所以**每一次提交**都打印一句"九项门禁全绿"。
#   这条横幅是要被抄进 `docs/status.md` 的（"门禁实况：九项门禁全绿"），
#   抄错一次就再也说不清 —— 与"绿只对**跑的那个版本**成立"是同一条。
#
# ⚠ 条数从 `${#ALL[@]}` 派生（不写字面量）；中文数字查表是为了保住文档里
#   已经在用的"九项门禁全绿"这个说法，超出表长就退回阿拉伯数字。
CN_NUM=(零 一 二 三 四 五 六 七 八 九 十 十一 十二)
N_ALL=${#ALL[@]}
if [ "$N_ALL" -lt "${#CN_NUM[@]}" ]; then CN_ALL="${CN_NUM[$N_ALL]}"; else CN_ALL="$N_ALL"; fi

RACE_NOTE=""
if [ "$RACE_SKIPPED" = "1" ]; then
  RACE_NOTE=" —— ⚠ 但 test 未跑 -race（本机缺 C 编译器）"
fi

SKIPPED=""
for g in "${ALL[@]}"; do
  case " ${ORDER[*]} " in
    *" $g "*) ;;
    *) SKIPPED="$SKIPPED $g" ;;
  esac
done

if [ -z "$SKIPPED" ]; then
  printf '\033[32m%s项门禁全绿%s\033[0m\n' "$CN_ALL" "$RACE_NOTE"
else
  printf '\033[32m门禁通过：%s（本次 %d/%d 条）%s\033[0m\n' \
    "${ORDER[*]}" "${#ORDER[@]}" "$N_ALL" "$RACE_NOTE"
  printf '\033[33m⚠ 未跑：%s\033[0m\n' "${SKIPPED# }"
  printf '\033[33m  ⇒ 别把这当成"全绿"：上面这几条**这次没验**。\033[0m\n'
fi
: > "$LOG"
