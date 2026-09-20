#!/usr/bin/env bash
# pre-commit / pre-push 共用的门禁执行器。
#
# **不要单独跑它** —— 它是被两个 hook 调用的。想手动跑门禁用：
#   bash scripts/gates.sh
#
# 本文件只做一件 `scripts/gates.sh` 做不到的事：**按 git 暂存区/待推送范围
# 缩小检查范围**，以及在没有相关改动时直接放过（见下面"跳过"一节）。
#
# ── 为什么 hook 不直接调 gates.sh ──────────────────────────────────
#
# `gates.sh` 跑的是 `cargo test --workspace` 这种全量命令。它适合"从零验证
# 整棵树"这种一次性场景，但放在提交路径上会带来两个问题：
#
#   1. 慢。改一个错别字也要等 clippy + 全量测试，于是人开始用 `--no-verify`。
#   2. 不知道自己在查什么。它没有任何"这次改动碰了哪些文件"的概念，
#      所以没法说出"你这次只改了文档，却挂了 filenames"这种有用的信息。
#
# 这里保持与 gates.sh **同一组命令**（一致性是刻意的：本地与手动跑必须得出
# 同一个结论），但按 hook 的职责切分集合，并在可判定时跳过。
#
# ── 耗时：瓶颈是"编译次数"，不是"门禁条数" ──────────────────────────
#
# 实测（6 核，热 target）：
#   fmt ~1s ／ deps ~1s ／ filenames 跳过或 ~1s ／ idents ~3s
#   clippy --all-targets  ~2s      ← 被 test 带热的，不是它自己快
#   test --workspace     ~20s
#
# 七条全跑的热态总耗时约 30–50s。但**冷态**（改动触发重新编译）可以达到
# 8–15 分钟 —— 因为 `clippy --all-targets` 与 `test` 要各编一遍整棵树。
#
# 三个反直觉的点，都实测过，别再"优化"回去：
#
#   1. `clippy` 去掉 `--all-targets` 会**更慢**（39s vs 2s）。不带它时
#      clippy 只 lint 库目标，走的是与 `test` 不同的 fingerprint，
#      等于凭空多编一遍。"少检查"不等于"更快"——关键是产物能否复用。
#   2. `clippy` 的 2s 是 `test` 挣来的。把 `clippy` 单独跑（前面没有 test）
#      要整树重编。所以**调整这两条的先后顺序会显著改变总耗时**。
#   3. 真正贵的是"改一个 .rs 之后的第一次推送"。改 `.md` / 只改注释
#      不会触发重编，代价可以忽略。
#
# 因此：不要为了"快"去砍门禁条数，那省不下多少；要省就省整树重编
# （见 docs/dev-gates.md「耗时」一节）。

set -uo pipefail

cd "$(dirname "$0")/.."

STAGE="${1:-pre-commit}"

case "$STAGE" in
  pre-commit)
    GATES=(fmt deps filenames idents)
    TITLE="提交前门禁（快）"
    ;;
  pre-push)
    GATES=(fmt clippy deps filenames idents test golden)
    TITLE="推送前门禁（全）"
    ;;
  *)
    echo "!! 未知阶段: $STAGE（pre-commit | pre-push）"
    exit 2
    ;;
esac

# ── 改动范围 ────────────────────────────────────────────────────────
#
# pre-commit 看**暂存区**（`--cached`），pre-push 看"本地领先远程的那些提交"。
# 两者都是"这次动作会送出去的东西"，而不是工作区里所有未提交的东西 ——
# 拿工作区当判据会让"暂存了 A、又改了 B"这种最常见的操作误判。
changed_files() {
  if [ "$STAGE" = "pre-commit" ]; then
    git diff --cached --name-only --diff-filter=ACMR 2>/dev/null
    return
  fi
  # pre-push：@{push} 可能不存在（新分支、没有上游）——那时 `rev-parse`
  # 返回非零，退回 main..HEAD。两个都取不到（比如刚 init 的库）就退到"最后一个提交"。
  local base upstream
  upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{push}' 2>/dev/null)
  if [ -n "$upstream" ]; then
    base="$upstream"
  elif git rev-parse --verify --quiet origin/main >/dev/null 2>&1; then
    base="origin/main"
  else
    base="HEAD~1"
  fi
  if git rev-parse --verify --quiet "$base" >/dev/null 2>&1; then
    git diff --name-only "$base...HEAD" 2>/dev/null
  else
    # 兜底：真正拿不到比较基准时，只查本次推送的一批提交里的改动
    git log --name-only --pretty=format: -1 2>/dev/null
  fi
}

CHANGED=$(changed_files | sed '/^$/d' | sort -u)

if [ -z "$CHANGED" ]; then
  printf '\033[32m%s：没有相关改动，跳过\033[0m\n' "$TITLE"
  exit 0
fi

# 把"这次到底在查什么"打出来。看上去是废话，实际是让 hook 的输出**可被信任**：
# 没有这行时，人看到一片绿不知道它查了几个文件；有了它，"
# 我明明改了两个文件它却说 1 个"这种问题当场就能发现。
CHANGED_COUNT=$(printf '%s\n' "$CHANGED" | wc -l | tr -d ' ')

# ── 按改动挑门禁 ────────────────────────────────────────────────────
#
# 只做**单方向**的收窄：`filenames` 只在真有文件增删改名时才跑。
# 其余一律跑 —— 因为它们的失效方式都是"安静的"（见 docs/dev-gates.md
# 的"两条不写下来就悄悄失效的约束"），少跑一次就少一次发现的机会。
#
# ⚠ 特别**不**按扩展名去过滤门禁：改一个 `.rs` 文件也可能破坏分层铁律
# （`deps`）与黄金向量（`golden`），改一个 `.md` 也可能带进真标识（`idents`）。

run_gate() {
  local name="$1"
  local cmd="$2"
  local log="target/gates-${name}.log"

  printf '\n\033[1m──── %s ────\033[0m\n' "$name"

  # 写盘被拦时**必须重试**，和 `scripts/gates.sh` 一致。
  #
  # 这里原来只跑一次：失败即判"你的代码没过门禁"。而在 Windows 上
  # `cargo` 会随机地写不进 target/（杀毒软件实时扫描），**同一次改动换个
  # 时刻重跑就过**。于是一次环境抖动被翻译成"你写错了代码"。
  #
  # 两个后果，都不是小事：
  #   1. 人开始怀疑门禁的结论 —— 而门禁的全部价值就在于"它说过了就是过了"。
  #   2. 更容易去用 `--no-verify`。而 hook 根本拦不住 `--no-verify`
  #      （它没有执行权限），于是**这个门禁实际是"自愿"的，却把人推去绕过它**。
  #
  # 更隐蔽的第三点：被杀掉的 `cargo test` 不留下任何缓存，下一次要**从头
  # 整树重编**。实测同一批文档改动，一次被拦的推送之后，下一次推送因为
  # 要重编整棵树而多花了几分钟 —— 故障的成本被转嫁到了下一次。
  #
  # 判据与 `gates.sh` 严格一致：**只对"写盘被拦"重试，且必须有 `error:` 开头的行**。
  # 测试失败、编译错误一律立即返回 —— 否则真问题会被掩盖成"重试五次都失败"。
  local attempt=1
  local max_attempts=5
  while :; do
    if eval "$cmd" >"$log" 2>&1; then
      cat "$log"
      printf '\033[32m✓ %s 通过\033[0m\n' "$name"
      return 0
    fi
    cat "$log"

    if [ "$attempt" -lt "$max_attempts" ] \
      && grep -qE "^(error|error\[)" "$log" \
      && grep -qE "拒绝访问|os error 5|LNK1104|permission denied" "$log"; then
      attempt=$((attempt + 1))
      printf '\033[33m· %s 写盘被拦，第 %d 次重试…\033[0m\n' "$name" "$attempt"
      sleep 3
      continue
    fi

    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    return 1
  done
}

# 工具链定位：**唯一实现在 `scripts/toolchain.sh`**，这里只 source。
#
# 绝不在这下面再写一份。历史上这里确实有一份手抄的简化版，而且已经和
# `gates.sh` 分叉了 —— 它少了 INCLUDE / LIB 的导出，于是 Windows 上
# `clippy` / `test` 在 hook 里链接失败、在 `gates.sh` 里却能过。
# 同一组命令两个结论，是最难排查的一类失效。
. scripts/toolchain.sh || exit 1

declare -A CMDS=(
  [test]="cargo test --workspace"
  [clippy]="cargo clippy --all-targets -- -D warnings"
  [fmt]="cargo fmt --check"
  [deps]="bash scripts/check-deps.sh"
  [filenames]="bash scripts/check-filenames.sh"
  [idents]="node scripts/check-identifiers.cjs"
  [golden]="node scripts/check-golden-vectors.cjs"
)

FAILED=()
printf '\n\033[1m%s\033[0m（本次涉及 %s 个文件）\n' "$TITLE" "$CHANGED_COUNT"
for name in "${GATES[@]}"; do
  # 名字门禁只在真有文件增删改名时有意义 —— 纯改内容时它必然通过
  # （文件名没变），跑一次只是白等。
  #
  # 两个阶段的判据不同，因为"这次动作会送出去的东西"不同：
  #   pre-commit → 暂存区的 A/D/R
  #   pre-push   → 待推送提交范围里的 A/D/R（那时暂存区通常是空的）
  #
  # 这里用 `head -1` 而不是 `grep -q`：管道给 `grep -q` 时上游会收到 SIGPIPE，
  # 在 `set -o pipefail` 下整个判断会变成"失败"——于是该跳过的时候不跳过，
  # 反向出错。取第一行再做字符串比较没有这个问题。
  if [ "$name" = "filenames" ]; then
    if [ "$STAGE" = "pre-commit" ]; then
      rename_line=$(git diff --cached --name-status --diff-filter=ADR 2>/dev/null | head -1)
    else
      rename_line=$(printf '%s\n' "$CHANGED" | head -1)
      # pre-push 时 `$CHANGED` 是"改过的文件"，不区分增删改名，
      # 所以退回更保守的判据：只要文件名里出现新面孔就跑。
      # 宁可多跑一次（它是纯静态的，很快），也别漏掉一次重命名。
      rename_line=$(git log --name-status --pretty=format: "$(git rev-parse origin/main 2>/dev/null || echo HEAD)"..HEAD 2>/dev/null \
        | grep -E '^[ADR]' | head -1)
    fi
    if [ -z "$rename_line" ]; then
      printf '\033[2m· filenames 跳过（本次没有文件增删或改名）\033[0m\n'
      continue
    fi
  fi

  run_gate "$name" "${CMDS[$name]}" || FAILED+=("$name")
done

if [ ${#FAILED[@]} -gt 0 ]; then
  echo
  printf '\033[31m%s 未通过: %s\033[0m\n' "$TITLE" "${FAILED[*]}"
  echo
  echo "修好再提交/推送。若确信是环境问题（Windows 上 target/ 偶发写盘被拦）："
  echo "  git commit --no-verify   /   git push --no-verify"
  echo
  echo "⚠ 绕过之后**没有东西会替你补上** —— 本仓库没有 CI（见 .githooks/README.md）。"
  exit 1
fi

printf '\n\033[32m%s 全绿\033[0m\n' "$TITLE"
