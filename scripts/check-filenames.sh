#!/usr/bin/env bash
# 文件名门禁：非 Rust 文件一律 kebab-case。
#
# 规则出处：`docs/rust-rewrite-plan.md` §3.1 与 `README.md` §文件命名。
#
# # 为什么值得单独一条门禁
#
# 这条规则以前**只写在文档里**，于是 `web/src/pages/` 下三个 `AuthPage.tsx`
# 一路活到被用户点名。规则写在文档里没人执行，就会反复被违反 ——
# 而违反的表现极其安静：编译能过、测试全绿、界面正常，
# 只有下一个人加文件时"照着旁边那个写"，于是错误自我复制。
#
# Rust 那条（snake_case）不在本脚本范围：文件名就是模块名，
# `mod my-mod;` 不合法，编译器已经强制了。
#
# # 判据
#
# kebab-case = 全小写 + 数字 + `-` 分隔。按 `.` 把文件名拆成段，**每段**都判：
#   `^[a-z0-9]+(-[a-z0-9]+)*$`
# 这样 `gen-auth-vectors.cjs`（段 `gen-auth-vectors` / `cjs`）与
# `foo.d.ts`（段 `foo` / `d` / `ts`）都自然通过，不必为扩展名写特例。
#
# # 豁免：三类，各有各的理由，**不允许"顺手加一个"**
#
# 1. **生态固定名**：`Cargo.toml` / `README.md` / `SKILL.md` 等 ——
#    工具按字面找它们，改名即失效。
# 2. **上游客户端源码**（`docs/capture/*.js`）—— 它们是从官方 App 里**原样取出**的
#    素材，是"协议对照物"。改名会让"文件里能搜到的符号"与"文件名"对不上，
#    而这份素材的价值恰恰在于**可逐字对照**。改名等于伪造证据。
# 3. **生成物 / 依赖目录** —— 不是人写的，不在管辖范围。
set -uo pipefail

cd "$(dirname "$0")/.."

# ── 豁免表 ───────────────────────────────────────────────────────────
#
# 用 `case` 通配**仓库相对路径**。新增一条必须同时满足：
#   ① 它是上面三类之一；② 在这里写清属于哪一类。
#
# 特别提醒：**不要**因为"这个文件不好改名"就加进来。第 2 类的判据是
# "它是从上游原样取出的证据"，而不是"改名麻烦"。
is_exempt() {
  case "$1" in
    # ① 生态固定名
    Cargo.toml|Cargo.lock|README.md|LICENSE|THIRD-PARTY-NOTICES.md|rust-toolchain.toml) return 0 ;;
    # 嵌套层级的生态固定名（docs/README.md 等）。豁免按"路径点名"，层级一变就得补。
    # （历史：Rust 曾冻结进 legacy/ 时漏了 Cargo.lock 与 rust-toolchain.toml，门禁当场抓到。）
    */Cargo.toml|*/Cargo.lock|*/rust-toolchain.toml|*/README.md|*/LICENSE) return 0 ;;
    */.npmrc|*/.gitignore|*/.gitattributes) return 0 ;;
    */index.html) return 0 ;;                 # Vite / 静态站点按字面找它
    */package.json|*/package-lock.json|*/tsconfig.json) return 0 ;;
    # GitHub Actions 按**路径**发现 workflow，文件名本身无语义。
    # 本仓库目前没有 CI（2026-09-20 删掉了 ci.yml，理由见 docs/dev-gates.md），
    # 这条豁免保留 —— 它约束的是"万一再加回来时该怎么写"，
    # 而且它的理由（按路径发现）与当前有没有 CI 无关。
    # 注意：这里**不豁免** workflow 的命名风格，只豁免 kebab-case 检查，
    # 因为生态惯例就是 `ci.yml` / `release.yml` 这类短横线名，本就合规。
    */.github/workflows/*) return 0 ;;
    # vendor 的第三方 skill：`SKILL.md` 是 agent skill 规范的固定名，
    # 且内容是上游原样文件 —— 改名等于改动 vendor 产物，
    # 会让"固定版本 + 校验和"这条来源声明失效（见 THIRD-PARTY-NOTICES.md）
    .workbuddy/skills/*/SKILL.md) return 0 ;;
    .workbuddy/skills/*/*/SKILL.md) return 0 ;;
    # ② 上游客户端源码素材（原样取出，不得改名 —— 见上面第 2 类）
    docs/capture/apiServer.js) return 0 ;;
    docs/capture/pcclient-api.js) return 0 ;;
    #
    # 旧项目 mcloud-sign 的**原样导出件**：`$meta.json` 这个名字本身就带 `$`，
    # 是旧项目的数据文件名。改名会破坏"新库这一份 ↔ 旧项目那一份"的一一对应，
    # 而这层对应正是迁移测试的价值所在（`internal/core/legacy_test.go` 按名断言；
    #  Rust 那份同名断言在剥离后的同级仓库 `../mcloud-rust/crates/mcloud-core/src/legacy/tests/fixtures.rs`）。
    test/fixtures/legacy/\$meta.json) return 0 ;;
    # ③ 生成物 / 依赖
    */node_modules/*|*/target/*|*/dist/*) return 0 ;;
    *) return 1 ;;
  esac
}

fail=0
checked=0
offenders=""

# 逐行读 `git ls-files`。
#
# 用管道而不是 here-string：`<<<"$(...)"` 会把整份清单先展开成一个巨型字符串，
# 与内层再套判断循环叠加后**实测会跑到超时**（几百个文件 × 每段一次子进程）。
# 管道是流式的，且这里不需要在循环里改外部变量以外的状态。
#
# 循环体**只用内建**（`[[ =~ ]]` / `case` / 参数展开），不起 `grep`/`sed`：
# 一旦每个文件 fork 一次，几百个文件的耗时是数量级差异。
while IFS= read -r path; do
  [ -z "$path" ] && continue
  # Rust 源走 snake_case，Go 源走 Go 自己的命名规则 —— 两者都由工具链赋予
  # 语义，改名即失效，所以都不在 kebab-case 的管辖范围：
  #
  #   *.rs        文件名就是模块名，`mod my-mod;` 不合法
  #   *_test.go   `go test` 只认 `_test.go` 后缀
  #   *_GOOS.go / *_GOARCH.go   构建约束按字面匹配后缀（foo_linux.go / foo_amd64.go）
  #
  # 这与上面 is_exempt() 的豁免表**不是**一回事：那是"按路径点名的例外"，
  # 这是"整类文件不适用这条规则"，理由与 Rust 那条完全相同。
  case "$path" in *.rs|*.go) continue ;; esac
  is_exempt "$path" && continue

  base="${path##*/}"
  # 按 `.` 拆段：`tr` 只调用一次（不是每文件一次），可接受
  segs="${base//./$'\n'}"

  offending=""
  while IFS= read -r seg; do
    [ -z "$seg" ] && continue
    if [[ ! "$seg" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
      offending="$seg"
      break
    fi
  done <<<"$segs"

  checked=$((checked + 1))
  if [ -n "$offending" ]; then
    offenders="${offenders}${path} :: 段 \"${offending}\" 不是 kebab-case"$'\n'
    fail=1
  fi
done < <(git ls-files)

if [ "$fail" -ne 0 ]; then
  printf '%s' "$offenders" | while IFS= read -r line; do
    [ -n "$line" ] && printf '!! %s\n' "$line"
  done
  echo
  echo "文件名门禁未通过。规则见 docs/rust-rewrite-plan.md §3.1："
  echo "  非 Rust 文件一律 kebab-case（全小写、- 分隔、不用空格与下划线）。"
  echo
  echo "重命名用 git mv 以保留历史："
  echo "  git mv path/OldName.tsx path/old-name.tsx"
  echo
  echo "若确实是「上游原样取出的素材」，把它加进 scripts/check-filenames.sh 的豁免表，"
  echo "并在注释里写明属于哪一类 —— 不允许因为「改名麻烦」而豁免。"
  exit 1
fi

echo "文件名门禁通过：${checked} 个文件均为 kebab-case（Rust 源文件除外）"
