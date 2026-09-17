# 第三方内容声明

本仓库包含**非本项目自研**的内容。凡 vendor 进来的部分，其许可与版权声明随附于此 ——
MIT 要求"许可与版权声明须随软件的所有副本或实质性部分一同提供"，所以这份文件不是可选项。

## daisyUI 官方 agent skill

**位置**：`.workbuddy/skills/daisyui/`（73 个文件）

**来源**：[`saadeghi/daisyui`](https://github.com/saadeghi/daisyui) → `skills/daisyui/`

**固定版本**：`01d6dc2b2f96983180ddcc0b3e03f5cca155571e`（`master`）

**内容**：daisyUI 5 的组件与用法文档（`SKILL.md` + `colors/` `config/` `install/` `usage/` + 68 个组件说明）。
文件与上游**逐字节一致**，未做任何本地修改；这样做是为了日后能直接用 `diff` 比对上游、判断是否需要更新。

**为什么放进仓库**：这是**项目级**技能，服务于 `web/` 的 daisyUI 使用。
放在仓库里，`git clone` 之后技能即可用，不必依赖某台机器的本地安装。

**更新方式**：上游发新版时，用稀疏检出只取该目录再整体替换（不要逐文件手改，会丢失"与上游一致"这个性质）：

```bash
git clone --depth 1 --filter=blob:none --sparse https://github.com/saadeghi/daisyui.git /tmp/daisyui
cd /tmp/daisyui && git sparse-checkout set skills/daisyui
git rev-parse HEAD                       # 记下新的 SHA
diff -r skills/daisyui <本仓库>/.workbuddy/skills/daisyui   # 先看清差异
cp -R skills/daisyui <本仓库>/.workbuddy/skills/daisyui
```

更新后请同步本文件里的"固定版本"SHA。

### 许可

```
MIT License

Copyright (c) 2020 Pouya Saadeghi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## 关于 Rust 依赖

`Cargo.lock` 里各依赖的许可随其 crate 自身分发，不在此重复列举。
如需汇总，可用 `cargo deny` 或 `cargo about` 生成。
