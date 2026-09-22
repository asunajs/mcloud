#!/usr/bin/env bash
# 按**这次暂存了什么**决定该跑哪几条门禁 —— `.githooks/pre-commit` 的分类器。
#
# # 要解决的这件事是真的
#
# pre-commit 原先写死 `gates-go.sh fmt deps filenames idents jsontags` 五条。
# ⇒ 改一行 `docs/*.md` 也要过一遍要 Go 工具链的检查（`gofmt -l` / `deps` / `jsontags`）；
#   而改 `.tsx` 时**那五条一条都验不到前端**，前端那边至今没有任何自动检查。
# 两件事合起来是同一句话：**闸口的内容与被改的东西无关**。
#
# # 为什么"按范围跳过"在这家是真的安全
#
# ⚠ 这一条是整个设计的前提，不成立就不要改这个文件：
#
#   1. **pre-push 不跳过。** 它跑的是全套（Go 九条 + web 两条），
#      所以"提交时按范围省下来的"必须在推送前补齐 —— 推才是影响别人的动作。
#      两道闸的分工见 `.githooks/README.md`。
#   2. **两条与语言无关的守卫恒跑**：`filenames`（命名）与 `idents`（脱敏）。
#      脱敏那条要扫的是**文本**，`.md` / `.json` / `.py` 一样能泄漏真实标识 ——
#      把它按语言切掉，等于给"最不该漏的那一条"开一个永远存在的洞。
#      ⇒ 分类器的输出里这两条**不在可跳的集合里**，不是"这次没匹配上"。
#   3. **分类器不确定时是全跑，不是全跳。** 见下面 `fallback` 那条分支：
#      拿不到暂存清单（未诞生分支、git 报错）时给"什么都跑"，
#      因为"少验"在这里是不可接受的失效方向。
#
# # 分类规则就两条
#
#   *.go / go.mod / go.sum   → 追加 Go 的三条静态门禁（fmt deps jsontags）
#   web/** 下任何文件         → 追加 web 的 typecheck
#
# ⚠ **不**按"`web/*.ts` 才算、`.css` 不算"细分：typecheck 1.9 秒，
# 而细分要把"哪些扩展名影响打包"写成第三份清单（vite 的配置才是那份的真出处）。
# 同理 `build` 不在提交档 —— 它写 `web/dist/`，与 Go 侧 `build` 一样属"要产出物的那条"，
# 留到 pre-push。判据是"**发现问题的时刻与副作用**"，不是快慢（README 同一条）。
#
# 用法：
#   bash scripts/gates-scope.sh            # 读暂存区（hook 用这个）
#   bash scripts/gates-scope.sh staged     # 同上，写法展开一点
#   printf 'web/src/a.tsx\n' | bash scripts/gates-scope.sh stdin   # 自己测分类用
#
# 输出（stdout，三行 `KEY=`，值可为空）：
#   GATES_GO=<喂给 scripts/gates-go.sh 的门禁名>
#   GATES_WEB=<喂给 scripts/gates-web.sh 的门禁名>
#   NOTE=<这一行为什么这么分，给人看的>
#
# 退出码：0 分类完成；1 门禁脚本自己语法就不对（这条就是"给门禁的门禁"）；2 用法错误。
#
# ⚠ bash 3.2 兼容（macOS 自带 /bin/bash）：不用 declare -A / mapfile / ${v,,}。
set -uo pipefail

cd "$(dirname "$0")/.."

MODE="${1:-staged}"

# 恒跑的那两条（语言无关的守卫，见上面第 2 条前提）。
ALWAYS="filenames idents"
# 只有 Go 代码被碰到才加的三条静态门禁。
GO_STATIC="fmt deps jsontags"
# 只有 web 被碰到才加的那一条。
WEB_STATIC="typecheck"

# ── 给门禁自身的门禁：这几个文件坏了，整道闸就是一次"静默没跑" ──────────
#
# 为什么放在这里而不是做成一条独立门禁：这几个文件正是**接下来要被调用**的东西，
# 分类器又恰好是每次提交的第一环 —— 它先活着，才谈得上派发。
# `bash -n` 只查语法（不执行），所以它抓不到"逻辑写错"，
# 抓的是引号没闭合、`fi` 少了这种"整个文件根本读不进来"的错 ——
# 而那类错的**现象**恰好是"门禁跑了、而且绿了"，是最贵的一种假绿。
for f in scripts/gates-go.sh scripts/gates-web.sh scripts/gates-scope.sh \
         scripts/check-filenames.sh scripts/check-go-deps.sh \
         .githooks/pre-commit .githooks/pre-push; do
  if ! bash -n "$f" 2>/dev/null; then
    echo "!! 门禁脚本自身语法不通：$f" >&2
    bash -n "$f" 2>&1 | sed 's/^/   /' >&2
    exit 1
  fi
done

case "$MODE" in
  staged)
    # ⚠ `-c core.quotePath=false`：否则 git 会把含非 ASCII / 特殊字符的路径
    #   打成 `"web/\346\226\207…" ` 这种带引号的转义串，下面的 `^web/` 就匹配不上 ——
    #   现象是"改了前端的中文命名文件，前端门禁却没跑"，而且什么都不报。
    # ⚠ `--diff-filter=ACMRD`：**删除也要算**。删掉一个被 import 的模块，
    #   正是 typecheck 最该响的那一刻；漏掉 D 等于把它当成"没改"。
    if ! files="$(git -c core.quotePath=false diff --cached --name-only \
                    --diff-filter=ACMRD 2>&1)"; then
      files=""
      FALLBACK=1
    else
      FALLBACK=0
    fi
    ;;
  stdin)
    files="$(cat)"
    FALLBACK=0
    ;;
  *)
    echo "!! 未知模式: $MODE（可选：staged / stdin）" >&2
    exit 2
    ;;
esac

if [ "${FALLBACK:-0}" = "1" ]; then
  # 拿不到清单 ⇒ **全跑**。失效方向必须是"多验"，不能是"少验"。
  printf 'GATES_GO=%s %s\n' "$ALWAYS" "$GO_STATIC"
  printf 'GATES_WEB=%s\n' "$WEB_STATIC"
  printf 'NOTE=拿不到暂存清单（未诞生分支？git 报错），按最保守的一档全跑\n'
  exit 0
fi

GO="$ALWAYS"
WEB=""

# 只动 Go 源码与依赖清单才算。`internal/**` 里的非 .go 文件（几乎没有）不必跑 gofmt。
if printf '%s\n' "$files" | grep -qE '(\.go|go\.mod|go\.sum)$'; then
  GO="$GO $GO_STATIC"
fi
if printf '%s\n' "$files" | grep -qE '^web/'; then
  WEB="$WEB_STATIC"
fi

if [ -z "$files" ]; then
  NOTE="暂存区没有文件变更（空提交？），只跑恒过的两条"
elif [ -n "$WEB" ] && [ "$GO" != "$ALWAYS" ]; then
  NOTE="同时改了 Go 与前端 ⇒ 两侧静态门禁都跑"
elif [ -n "$WEB" ]; then
  NOTE="只触及 web/ ⇒ 跳过 Go 静态门禁（pre-push 全跑兜底）"
elif [ "$GO" != "$ALWAYS" ]; then
  NOTE="触及 *.go ⇒ 加跑 fmt deps jsontags"
else
  NOTE="只触及文档/素材 ⇒ 只跑恒过的两条（filenames idents）"
fi

printf 'GATES_GO=%s\n' "$GO"
printf 'GATES_WEB=%s\n' "$WEB"
printf 'NOTE=%s\n' "$NOTE"
