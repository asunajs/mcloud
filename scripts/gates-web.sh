#!/usr/bin/env bash
# 前端（web/）门禁一键跑 —— `scripts/gates-go.sh` 的 web 版。
#
# 为什么要有这个文件：2026-09-22 之前**整个仓库对 web 侧一条自动检查都没有**。
# `package.json` 里有 `typecheck` 与 `build` 两个脚本，但没有任何 hook / 脚本调它们，
# `.githooks/` 的两个 hook 全是 `exec bash scripts/gates-go.sh …`。
# ⇒ 改坏一个 `.tsx` 不会让任何东西变红：它既不过类型检查，也不过打包，
#   直到下一次有人手跑 `npm run build` 才撞上 —— 而那时早就说不清是哪一笔改坏的。
#
# # 为什么不复用 gates-go.sh 那套循环
#
# 那 25 行 runner（`GATE_NAMES` / 逐条落盘 / 末尾"本次 N/M 条"横幅）是**通用**的，
# 但它外面包着全是 Go 专有的东西：`GO_PKGS`、`-race` 探测、`gofmt -l ./cmd ./internal`。
# 把那层抽出来 = 改动一份今天有并行会话正在提交的脚本（近三笔提交都在动门禁），
# 而这里只有两条门禁 —— **不值得**。
# 所以：runner 的形状照抄（同一套日志落盘与横幅约定，排障时只有一套肌肉记忆），
# 内容各自一份。⚠ 代价就是"两份 runner 会长歪"，看这段注释的人请一起检查。
#
# # 两条各拦什么
#
#   typecheck  `tsmc --noEmit`（ts-macro 的 CLI：先展开 `.tsx` 里的宏再查类型）
#   build      `vite build`    （Tailwind/daisyUI 的 CSS 管线、模块解析、产物落盘）
#
# ⚠ `typecheck` 通过 **不等于** 页面是对的：web 侧没有测试运行器
# （方案 §6.1 L1' 要的 vitest 至今未装，见 `docs/webui-code-guide.md` §12 未决项 3），
# 所以这两条只保证"编得出来"，不保证"跑得对、长得对"。
# 那条缺口只能靠真机核验补，别把这里当成功能正确性的证明。
#
# ⚠ bash 3.2 兼容（macOS 自带的 /bin/bash）：不用 declare -A / mapfile / ${v,,}。
#   与 scripts/gates-go.sh 同一条约束，理由见 docs/dev-gates.md。
set -uo pipefail

cd "$(dirname "$0")/.."

GATE_NAMES="typecheck build"
ALL=($GATE_NAMES)

WEB_DIR="web"

# ── 环境探测：先说清"这台机器缺什么"，再谈门禁红不红 ──────────────────
#
# ⚠ 判据来自 gates-go.sh 里 `-race` 那一节的同一条：
#   环境问题被翻译成"你的代码有问题"，会让人去查一行没错的代码。
#   但这里**不降级**：缺 node_modules 时 `tsc` 报的是几百条"找不到模块"，
#   那既不是门禁想说的话，也不是能一眼修掉的错 ⇒ 直接拦住并给出那条命令。
#   （降级只适合"少了它其余仍然成立"的检查，竞态检测是那一类，编译不是。）
env_ready() {
  if ! command -v node >/dev/null 2>&1; then
    echo "!! 这台机器没有 node，web 门禁跑不了（不是代码问题）。" >&2
    return 1
  fi
  if [ ! -d "$WEB_DIR/node_modules" ]; then
    echo "!! 缺 $WEB_DIR/node_modules：先跑一次「cd $WEB_DIR && npm install」再提交。" >&2
    echo "   （依赖没装时 typecheck 会吐几百条「找不到模块」，那不是本门禁想报的错。）" >&2
    return 1
  fi
  return 0
}

gate_run() {
  case "$1" in
    typecheck) npm --prefix "$WEB_DIR" run typecheck ;;
    build)     npm --prefix "$WEB_DIR" run build ;;
    *)
      # 与 gates-go.sh 同一条：登记了却没实现分支，得说清问题在脚本这张表上，
      # 而不是让人去查这条门禁本身。
      printf '!! 门禁 "%s" 登记在 GATE_NAMES 里，但 gate_run 没有它的实现分支\n' "$1" >&2
      return 127
      ;;
  esac
}

is_known_gate() {
  case " $GATE_NAMES " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

ORDER=($GATE_NAMES)
if [ $# -gt 0 ]; then
  ORDER=("$@")
fi

for name in "${ORDER[@]}"; do
  if ! is_known_gate "$name"; then
    echo "!! 未知门禁: $name（可选：$GATE_NAMES）"
    exit 2
  fi
done

if ! env_ready; then
  printf '\033[31mweb 门禁没跑起来：环境缺东西（上面那句就是原因）\033[0m\n'
  exit 1
fi

FAILED=()
LOG="target/gates-web-last.log"
mkdir -p target 2>/dev/null
: > "$LOG"

for name in "${ORDER[@]}"; do
  printf '\n\033[1m──── web:%s ────\033[0m\n' "$name"

  RUN_LOG="target/gates-web-${name}.log"

  if gate_run "$name" >"$RUN_LOG" 2>&1; then
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    printf '\033[32m✓ %s 通过\033[0m\n' "$name"
  else
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    FAILED+=("$name")
  fi
done

if [ ${#FAILED[@]} -gt 0 ]; then
  printf '\033[31mweb 门禁未通过: %s\033[0m\n' "${FAILED[*]}"
  echo "完整输出: $LOG"
  exit 1
fi

# ── 横幅：与 gates-go.sh 同一条判据 ──────────────────────────────────
#
# 跑子集时**不许**打"全绿"。这条在 Go 侧是修过的真事故（早先 `gates-go.sh fmt`
# 也报"九项门禁全绿"），这里从一开始就按修好的写法来。
N_ALL=${#ALL[@]}
RAN="${ORDER[*]}"

if [ "${#ORDER[@]}" -eq "$N_ALL" ]; then
  printf '\033[32mweb 门禁 %d 条全绿\033[0m\n' "$N_ALL"
else
  printf '\033[32mweb 门禁通过：%s（本次 %d/%d 条）\033[0m\n' \
    "$RAN" "${#ORDER[@]}" "$N_ALL"
  for g in "${ALL[@]}"; do
    case " $RAN " in *" $g "*) ;; *) printf '\033[33m⚠ 未跑：%s\033[0m\n' "$g" ;; esac
  done
  printf '\033[33m  ⇒ 别把这当成"全绿"：上面这几条**这次没验**。\033[0m\n'
fi

# ⚠ 与 Go 侧的一处**故意**不同：这里不清空 $LOG。
#   gates-go.sh 末尾那句 `: > "$LOG"` 让"成功那次的输出"不留在盘上，
#   于是 target/gates-go-last.log 实际是"上一次**失败**的现场"（名字没说这件事）。
#   这里留着，是为了让"这次 web 门禁验了什么"在提交之后还能回看 ——
#   要跟着 Go 侧统一成哪一个，是另一个话题，别顺手改齐。
