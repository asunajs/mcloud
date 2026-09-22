# WebUI 设计规范（**后续所有前端美工设计的唯一依据**）

> 状态：**规范**（normative）。风格参照 [Nexus — daisyUI Dashboard](https://nexus.daisyui.com/dashboards/ecommerce)。
> 技术栈已定，不再讨论：**Vue Vapor（无虚拟 DOM）+ TSX + daisyUI 5 + Tailwind CSS 4**。
>
> - 生效范围：`web/` 下的一切界面代码（含 `src/ui/` 组件层）
> - 权威关系：**视觉与交互冲突时以本文为准**；功能范围以
>   [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) §4.1 的页面表为准；
>   技术选型理由见 §2.2，本文不重复论证，只落地
> - 写新页面/新组件前**先读本文**；本文没有覆盖到的形态，先补本文再写码
> - 最后核对：**2026-09-19**
> - 2026-09-21 补充：**daisyUI 官方 skill（`saadeghi/daisyui`）为写码前置条件**，
>   它和 Tailwind 的分工见新增的 §1.1；两条"不报错但样式不对"的陷阱见 §9.2。

---

## 一、为什么要有这份文件

前端最容易烂的地方不是"不会写"，而是**同一个东西在三个页面里长三个样**：
卡片圆角不一样、间距不一样、同一个「失败」一会儿红一会儿橙、移动端一处能用一处不能。

这类不一致**不会报错**，只会在页面多起来之后变成"整体看起来不专业"，而且**改不动** ——
因为没有一处是"权威的定义"。

所以本文的作用是：**把决定权收口**。凡是"哪种更好看"的问题，答案在这里；
写码时不需要再即兴决定。**新增形态请改本文，而不是就地发挥。**

配合两条已经写进方案的硬约束（见 [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) §2.2.2）：

1. **类名必须是字面量** —— Tailwind 只识别源码里字面出现的类名，
   `btn-${variant}` 这种拼接在生产构建后会**静默失效**（样式没了但不报错）。
   动态一律走查表：`btnTheme[variant]`。**这是本项目的头号样式 bug 来源。**
2. **不能创建 `tailwind.config.js`** —— Tailwind 4 是 CSS-first，已废弃该文件。

### 1.1 daisyUI 与 Tailwind CSS：谁管什么（2026-09-21 补，按官方 skill 核对）

本文通篇在用这两个东西，但它们是**两层**，混着用就会出 §9.1 那类静默失效：

| 层 | 版本（`web/package.json`） | 管什么 | 入口 |
| --- | --- | --- | --- |
| **Tailwind CSS** | `tailwindcss ^4.3.3` + `@tailwindcss/vite ^4.3.3` | 原子工具类：布局、间距、字号、响应式前缀（`flex`/`p-4`/`md:hidden`）；`@theme` 令牌命名空间 | `web/src/app.css:1` 的 `@import "tailwindcss"` |
| **daisyUI** | `daisyui ^5.7.38`（devDependency） | 组件类名（`drawer`/`dock`/`menu`/`card`…）、语义色（`base-100`/`primary`/`error`…）、主题档位（`--depth`/`--radius-*`） | `web/src/app.css:5` 的 `@plugin "daisyui"` |

三条容易踩的边界，都是实测口径：

- **daisyUI 是 Tailwind 的 CSS 插件，不是 Vite 插件。** 写进 `vite.config.ts` 的 `plugins`
  会报 `handler is not a function`；只能在 CSS 里 `@plugin "daisyui"`（该注释已在 `vite.config.ts` 顶部）。
- **两套主题变量各查各的注册表。** `--color-*` / `--radius-*` / `--size-*` / `--border` / `--depth` / `--noise`
  归 daisyUI 的 `@plugin "daisyui/theme"`；`--font-*` / `--spacing` / `--text-*` / `--breakpoint-*` 归 Tailwind 的 `@theme`。
  写错块**不报错、直接被吞**（§9.1②，字体那条就是这么发现的）。
- **语义色就是 Tailwind 颜色**，所以照常带透明度用：`text-base-content/60`、`border-base-300`。
  反过来 **daisyUI 语义色不要配 `dark:`** —— 主题切换走 `data-theme`，颜色自己会变；
  真需要"所有主题都不变"的颜色（图表、SVG）才用 Tailwind 具名色阶（§3.4 的禁令与此一致）。

**用类的优先序**（照官方 skill 的 usage 规则 1→4，和本文 §5 的口径相同）：
先用 daisyUI 组件类 → 不够再用 Tailwind 工具类补（`btn px-10`）→ 只有被特异性挡住才在末尾加 `!`
（`btn bg-red-500!`，不许常用）→ **daisyUI 没有对应组件就用 Tailwind 手搭，不要写自定义 CSS**。

#### 官方 skill 是"类名存在性"的权威，写码前必须读

本项目装的是 daisyUI 作者本人发布的官方 skill：`saadeghi/daisyui`（skills.sh 源），
用户级装在 `~/.qoder/skills/daisyui/`，会话里以 `daisyui` 这个 skill 名激活。规矩：

1. **写任何 HTML/JSX 之前**先激活它，并按它的"组件发现协议"走：
   先按**功能**找候选组件（不看名字是否对得上），再读候选的 `components/*.md`，然后才动手。
2. 用到哪个组件就读哪个：`components/drawer.md`、`components/dock.md`、`components/menu.md`…
   语法与 `#### Rules` 一节是硬约束。§4.3 的 `drawer` 骨架、§4.4 的 `dock` 都已照它复核过：
   `drawer-open` 是**常驻分栏 modifier**（"always visible on large screens"），
   展开态由 `drawer-toggle` 这个隐藏 checkbox + `label for` 驱动 —— 这正是 §9.2③ 那个 bug 的判据。
3. 记不准类名时**查产物**，不要凭记忆：组件源码就在 `web/node_modules/daisyui/components/<name>.css`，
   一行压缩 CSS 里能直接看到全部规则（§9.2③ 的 `position: sticky` 就是这么抓到的）。
   改名过的组件尤其要查 —— 例如 5 里 `btm-nav` 已废弃、现名 `dock`。

> **一处刻意的偏离**：skill 的 usage 规则 12 是"用户没点变体就用最朴素的 `btn`"。
> 本项目**不照这一条** —— §2.3① 与 §3.4 规定一屏只有一个实心 `btn-primary`（主行动），
> 次要一律 `-soft` / `-ghost`。这是产品纪律不是技术限制，权威关系见本文头部的声明。
> 除这一条外，skill 的所有规则优先于个人习惯。


---

> **本规范 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 基调·色彩·布局](./webui-design-guide.md) ｜ [② 组件·交互·移动端·动效](./webui-components.md) ｜ [③ 代码组织·落地·验收·未决](./webui-code-guide.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§一 ~ §十二 连续），所以「§4.1」「§7.2」
> 这类引用仍然有效，只是落在不同的文件里。

## 二、设计基调

### 2.1 一句话定位

**克制的数据面板**：信息密度高、装饰极少、状态一眼可辨。
不做营销页那种大留白与渐变，也不做"仪表盘炫技"（发光边框、玻璃拟态、大色块）。

判断标准：**这个像素有没有承载信息？** 没有就去掉。

### 2.2 从 Nexus 借鉴什么、不借鉴什么

Nexus 是一个电商后台（daisyUI 官方做的示例仪表盘）。我们的**用途不同**
（定时任务 + 账号运营，不是交易），所以照搬结构会别扭。

**先说清借鉴的性质**：Nexus 的参考价值**只在"布局怎么组织"这一层**。
它的**样式是 stock daisyUI，没有可借鉴的自定义实现**（见上方口径框）。
所以下表"借"的全是**结构用法**，不是视觉样式：

| 借（且必须借） | 理由 |
| --- | --- |
| **`drawer` + `navbar` + `dock` 三件套的骨架** | 这是 daisyUI 处理移动端最成熟的一套，自己造只会更差 |
| **指标卡的 `stats` 用法** | 顶部一排关键数字（云朵 / 今日增量 / 失败数 / 下次任务），结构直接对应 |
| **卡片 + 内嵌图表的节奏** | 大卡片承载趋势图，小卡片承载数字，层次清晰 |
| **时间范围切换的 `tabs`** | 它用 Day/Month/Year，我们映射为「7 天 / 30 天 / 全部」 |
| **表格右侧操作列用 `dropdown` 收口** | 行内按钮一多就挤，移动端尤其 |
| **状态用 `badge` 而不是彩色文字** | 可扫读性显著更好 |

| 不借 | 理由 |
| --- | --- |
| 商品图 / 头像 / 图片列 | 我们没有图片资源，塞占位图是纯噪音 |
| 大面积营销式留白 | 我们是运维工具，屏幕要装信息 |
| 多币种 / 汇率 / 预测曲线 | 无对应业务 |
| 侧栏多级嵌套菜单 | 我们只有 9 个页面，一级足够；嵌套反而难点 |
| 自定义多主题切换（它的 6 个主题） | **我们只做 `light` / `dark` 两个**，但**必须有手动切换**（见 §3.1） |

> **口径**：Nexus 是**布局与组件的参照**，不是**像素的参照**，更不是**样式的参照**。
> 它用什么间距我们就用什么，但它的商品卡、图表配色、动效节奏都不适用，不搬。

> **关于「扒 Nexus 的样式实现」这件事 —— 结论是没有可扒的。**
> 我把它编译后的 CSS bundle 下载下来逐条核对过：**它用的是完全未经修改的 stock daisyUI**。
> 没有任何自定义组件类（`nx-*` / `cx-*` 出现次数为 0），没有自定义样式层，
> 它的 `light` 主题块与 daisyUI 默认值逐字节一致（`--depth: 1`、`--radius-box: .5rem`）。
> 所以「它好看」**不是因为它做了什么，而是因为 daisyUI 5 本来就长这样** ——
> 差别在于它**用对了**（挑着用 `-soft`、用 `shadow` 不用 `border`），
> 而不是**改了**。**要参考就直接看 daisyUI 官方文档，不要绕道去反编译别人的站点。**

### 2.3 让它「看起来现代」的四个开关（**来自 daisyUI 官方文档，非第三方**）

"daisyUI 默认样子过时"这个说法要修正一下：**过时的是"什么都用默认、什么都上实心"的用法**，
而不是 daisyUI 本身。daisyUI 5 已经内置了现代观感所需的全部开关，
问题只在于**默认不开、要主动选**。下面四条**都是官方提供的正式能力**：

| 开关 | 怎么做 | 效果 |
| --- | --- | --- |
| ① **`-soft` 取代实心** | `btn-soft` / `badge-soft` / `alert-soft` | 背景是主色 8% 混 `base-100`，弱化但不失语义。**这是"现代感"的最大来源** |
| ② **阴影而非边框** | `shadow-sm` + **不加** `border` | 默认的 1px 硬边框是"表格软件"感；轻阴影才是"产品"感 |
| ③ **`--depth: 0`** | 关掉 daisyUI 的立体感 | 默认 `depth:1` 给按钮加了内阴影浮雕，扁平化后立刻现代 |
| ④ **品牌字体** | `--font-sans` 换成非系统字体 | 系统字体是"没设计过"的信号，成本最低、收益最高的一条 |

展开说，因为这四条都容易被做成"半吊子"：

**① `-soft` 系列（最重要）**

daisyUI 5 的 `-soft` 修饰符是独立于颜色的一层样式：

```tsx
// 主要行动仍用实心（一屏一个），其余一律 soft
<button class="btn btn-primary">确认兑换</button>          {/* 实心：唯一主行动 */}
<button class="btn btn-soft btn-primary">查看详情</button>   {/* soft：次要 */}
<span class="badge badge-soft badge-success">成功</span>
<div class="alert alert-soft alert-warning">…</div>
```

为什么它关键：实心 `btn-primary` 满屏都是 ⇒ 重点消失。`-soft` 让**语义色还在、
视觉重量减半**，界面立刻有层次。**默认从 `-soft` 起手，只有主行动才实心。**

> 已核验（**在本项目 `vite build` 产物里逐个确认**，不是看文档口径）：
> `.btn-soft { --btn-bg: color-mix(in oklab, var(--btn-color) 8%, var(--color-base-100)) }`
> —— 与上面写的一致。`badge-soft` / `alert-soft` 同为 8%，边框是 10%。
>
> `card` **没有** `-soft`，它只有 `card-border` / `card-dash` —— 所以卡片靠手法②。

**② 阴影代替边框**

```tsx
<div class="card bg-base-100 shadow-sm">   {/* ✅ */}
<div class="card bg-base-100 border">      {/* ❌ 硬边框 */}
<div class="card bg-base-100 shadow-sm border"> {/* ❌ 加戏 */}
```

深色模式下阴影几乎不可见 —— 这是正常的，靠 `base-100` 与 `base-200` 的明度差分层即可。
**不要**为了"深色下也有边界"就去加边框，那会让浅色模式变丑。**选一个，两个主题共用。**

**③ 全局扁平（`--depth: 0`）**

daisyUI 用 `--depth` 控制按钮等的"立体感"（内阴影 + 描边）。默认 `1`，
Nexus 用 `1`（stock 值，它没改）。**我们取 `0`**：

```css
@plugin "daisyui/theme" {
  name: "light";
  /* … */
  --depth: 0;        /* 关掉浮雕，扁平化 */
  --radius-box: 0.75rem;   /* 卡片圆角稍大一点，比默认 0.5rem 更"软" */
  --radius-field: 0.5rem;  /* 输入框/按钮同比例 */
}
```

`--radius-*` 一并稍调：默认 `0.25rem` 的输入框显得"紧"，`0.5rem` 更舒展。
**光调 `primary` 颜色是不够的** —— 形状变量（`--depth` / `--radius-*` / `--border`）
才是"整体观感"的来源。

> **已核实 `--depth: 0` 不会损坏交互反馈**（这点当初我猜错过，实测纠正）：
> daisyUI 的 `.btn:hover` 加深用的是**硬编码 7%**、`:active` 用的是
> **`translate: 0 .5px`** 的位移 —— 两者都**不经过 `--depth`**。
> `--depth` 只管内阴影浮雕（`--btn-inset`、`--btn-shadow` 里的 `calc(var(--depth) * …)`）。
> 所以拿掉它**只掉装饰，不掉反馈**，不需要补偿。

**④ 字体（性价比最高的一条）**

系统默认字体（Windows 的 Segoe UI / 中文回落到雅黑）是"没设计过"的典型信号。
换一个**拉丁子集 + 中文回落**的字体栈，观感提升立竿见影。

> ⚠️ **这里有个坑，我实测踩到过：不能写在 `@plugin "daisyui/theme"` 里。**
> `--font-sans` **不是 daisyUI 的主题变量**，它是 **Tailwind 自己的 `@theme` 命名空间变量**
> （见 `tailwindcss/theme.css` 顶部 `@theme default`）。
> 写进 daisyUI 插件块，Tailwind 会拿它去查自己的注册表 —— 查得到就返回注册值，
> 于是**你写的字体栈被静默替换成 `sans-serif`**，`vite build` 不报任何错。
>
> 正确写法是用 **Tailwind 的 `@theme`**，且**只写一次**（字体不随主题变）：

```css
/* web/src/app.css —— 放在 daisyUI 插件之后 */
@theme {
  --font-sans: "Inter", "PingFang SC", "Microsoft YaHei", system-ui, sans-serif;
}
```

> **落地待办**：字体文件当前**尚未引入**（见 §12 未决项 6）。
> 中文全量字体很大，建议只引**拉丁子集 + 中文走系统回落**，
> 或直接自托管一个精简子集 —— **不要**链 Google Fonts（国内访问不稳，且是外部依赖）。

### 2.4 「涨红跌绿」不可动

这是**业务约定，不是配色偏好**（见 [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) 与旧项目日志语义）：

- **涨 = 红**（`text-error`），**跌 = 绿**（`text-success`）
- 与 A 股习惯一致，**与 daisyUI 默认语义相反**（daisyUI 里 success 是绿、error 是红）

由此推出两条硬规则：

1. **禁止用 `text-success` / `text-error` 表达"涨跌"以外的语义**。
   成功/失败状态另见 §5.4 的 `status` 用法 —— 那里绿=正常、红=异常，
   与涨跌**恰好同向**，不会打架；但**不要**用同一个 badge 同时表达两件事。
2. 涨跌色只用于**数值本身**，不要给整行/整卡上色。

---

## 三、主题与色彩

### 3.1 两个主题 + **必须有的手动切换**

只启用 `light` / `dark` 两个主题，但**必须提供手动切换开关**。

```css
/* web/src/app.css —— 目前是旧版，按本节订正 */
@plugin "daisyui" {
  themes: light --default, dark;
}
```

**为什么给 `dark` 去掉 `--prefersdark`**：`--prefersdark` 只做一件事 ——
"系统偏好深色时**自动**用 `dark`"。一旦有了手动开关，它就从"贴心的默认值"变成**抢状态**的干扰源：
用户手动选了浅色，切到别的应用再回来（或系统主题变了），页面可能被 `--prefersdark` 顶回深色，
而开关还显示"浅色"——**界面与实际样式不一致**，这是最难查的一类 bug。

所以职责要分清：

| 职责 | 由谁负责 |
| --- | --- |
| **初始主题**（用户还没表态时用哪个） | 我们的 JS：读 localStorage → 没有则读 `prefers-color-scheme` |
| **主题状态** | 只有 `data-theme` 一个来源，**CSS 的 `--prefersdark` 不参与** |

即：**"跟随系统偏好"是初始值的计算逻辑，不是交给 CSS 的常驻规则。**
这样用户显式选择后，选择**永久生效**，不会被系统事件覆盖。

### 3.2 主题切换的实现规则

三条硬规则，缺一条就会出问题：

1. **`data-theme` 必须挂在 `<html>` 上**（不是 `body`、不是某个容器）。
   daisyUI 的所有主题变量都定义在 `:root` / `[data-theme=...]` 上，
   挂错层会出现"部分元素没变色"。
2. **必须持久化到 `localStorage`**。刷新页面、关掉再开，用户的选择要还在 ——
   否则"切了等于没切"，每次访问都要重切一遍。
3. **必须在首屏渲染前应用，且要防闪烁（FOUC）**。
   若等 JS 框架挂载（Vapor `createVaporApp`）之后再设 `data-theme`，
   用户会看到**先浅后深的一闪**。做法是 `index.html` 里一段**内联同步脚本**：

```html
<!-- web/index.html —— 必须内联、必须同步、必须在 <body> 之前 -->
<script>
  // 目的：在首次绘制前就把 data-theme 定下来，避免主题闪烁。
  try {
    var saved = localStorage.getItem('mcloud-theme')
    var theme = saved === 'light' || saved === 'dark'
      ? saved
      : (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
    document.documentElement.setAttribute('data-theme', theme)
  } catch (e) {
    // localStorage 可能被隐私模式禁用 —— 退化成默认浅色，不能让脚本报错白屏
    document.documentElement.setAttribute('data-theme', 'light')
  }
</script>
```

> 这段脚本**刻意不写成模块**，也**不放进 `main.ts`** —— 它是防闪烁的一部分，
> 一旦被异步加载就失去意义。`<html data-theme="light">` 上的静态值保留作**兜底**
> （脚本被 CSP 拦掉时至少不是无主题裸页面）。

**切换开关放哪**：**所有视图的右上角**都放一个 `btn btn-square btn-ghost btn-sm`。

> **登录页也要有** —— 否则用户得先登录才能把刺眼的浅色换成深色，本末倒置。
> 登录页没有 navbar，用绝对定位：外层容器 `relative`，按钮 `absolute right-3 top-3`。

点一下在 `light` / `dark` 间切换，写完 `data-theme` 立刻 `localStorage.setItem`。
图标随当前主题变（浅色下显示月亮=「切到深色」，深色下显示太阳），并带 `aria-label`。

实际实现见 `web/src/ui/theme.ts`（`nextTheme` 是纯函数，独立出来便于单测）：

- **不做"跟随系统"三态开关**（浅 / 深 / 自动）。理由：多一个状态就多一处"当前到底哪个"的歧义
  ——尤其和上面那条"防 `--prefersdark` 抢状态"的规则叠在一起时。
  系统偏好已经作为**初始值**生效了，够用。
- **禁用** Nexus 用的 `contrast` / `material` / `dim` 等多主题（它是演示站，需要 showcase）。
  我们**只有两个主题**，但**两个都能手动切**。

### 3.3 两个主题的品牌色必须都微调（**当前是漏的**）

`primary` 是品牌色，**两个主题都要覆盖**，否则切到深色会变成 daisyUI 默认紫，
与浅色下的品牌色不是同一个颜色 —— 这正是"切主题"最容易被忽略的破绽。

```css
/* light */
@plugin "daisyui/theme" {
  name: "light";
  default: true;
  color-scheme: light;

  --color-primary: oklch(52% 0.19 275);          /* 对齐 mcloud 既有品牌色 */
  --color-primary-content: oklch(98% 0.01 275);
}

/* dark —— 目前缺失，必须补上 */
@plugin "daisyui/theme" {
  name: "dark";
  color-scheme: dark;

  /* 深色底上同样的色相要提亮才够对比度（oklch 的 L 值调高） */
  --color-primary: oklch(72% 0.17 275);
  --color-primary-content: oklch(18% 0.04 275);
}
```

**做法与边界**：

- 只调 `primary` 一档，其余用 daisyUI 默认 —— 默认值经过对比度校验，随手改容易掉到 AA 以下。
- 深色的 `primary` **必须比浅色亮**（oklch 的 `L` 值更高）：
  深底上沿用浅色那个 `L=52%` 会糊在背景里，`btn-primary` 直接看不清。
- 新增主题变量时**两个主题一起加**，这是本节最容易被漏的一步。
- `data-theme` 挂在 `<html>` 上，见 `web/index.html`。

### 3.4 色彩的用法纪律

**总原则**（来自 §2.3①）：**默认从 `-soft` 起手，只有主行动才实心。**
下表左列是"强度档位"，不是"颜色"——同一屏里强度要有梯度，全实心就等于没有重点。

| 用途 | 用什么 | 禁止 |
| --- | --- | --- |
| 页面底色 | `bg-base-200` | 不要在 `base-300` 上放正文（对比度不足） |
| 卡片 / 容器 | `bg-base-100` + `shadow-sm`（§2.3②） | 不要给卡片加边框**又**加阴影（选一个） |
| 主行动（**每屏最多一个**） | `btn-primary`（**实心，唯一允许实心的地方**） | 不要一屏多个 `btn-primary`，会失去"主要"的含义 |
| 次要行动 | `btn-soft btn-primary` | 不要用 `btn-outline` —— 描边是"表单软件"感，已被 `-soft` 取代 |
| 纯导航/工具类按钮（无强调） | `btn-ghost` | 不要给工具按钮上语义色 |
| 危险动作（消耗云朵、删除） | `btn-soft btn-error`，**执行按钮必须二次确认**（§6.4） | 不要用红色做"取消"按钮 |
| 信息系统提示 | `alert-soft alert-info` | 不要用实心 `alert-info` —— 大面积色块会压过正文 |
| 成功 / 失败（操作结果） | `alert-soft alert-success` / `alert-soft alert-error` | 同屏不给两个 `alert`（见 §6.3） |
| 状态徽标 | `badge-soft badge-*`（§5.4） | `badge` 不要实心 —— 表格里一列实心 badge 会花 |
| 涨 | `text-error` | — |
| 跌 | `text-success` | — |
| 弱化文字 | `opacity-70` / `opacity-60` | 不要用 `text-gray-500` 这类写死色 —— 切主题就崩 |

> **最后一条是硬规则**：**禁止使用 `gray-*` / `slate-*` / `zinc-*` 等 Tailwind 具名色阶**。
> 它们不随 daisyUI 主题变，深色模式下必然出现"浅灰字压深灰底"。
> 一律用语义色（`base-content`）+ `opacity-*`。

> **`-soft` 的写法要点**：修饰符**必须与颜色类同时出现**（`btn-soft btn-primary`），
> 只写 `btn-soft` 会落到默认色（`base-content`），看起来像"没上色的灰按钮"。
> 另外 Tailwind 只认字面量：**不要**写 `btn-soft btn-${color}`（§9.2 的字符串拼接陷阱）。

### 3.5 间距与圆角

**间距**用 Tailwind 默认刻度，**不自定义**。常用组合固定下来：

| 场景 | 类名 |
| --- | --- |
| 页面主容器内边距 | `p-4 md:p-6` |
| 卡片之间 | `gap-3 md:gap-4` |
| 卡片内部 | `card-body` 默认（不要自己 `p-4` 覆盖，除非是紧凑卡片） |
| 表单字段之间 | `gap-3` |
| 分组标题与内容 | `mb-3` |

**圆角**走 daisyUI 主题变量，**不要写 `rounded-lg` 这类硬编码** —— 它绕过了主题，
换主题时"圆角不跟着变"就是这么来的。

形状档位在 §2.3③ 统一定义，**两个主题各写一遍**（这是最容易漏的一步）：

| 变量 | light | dark | 说明 |
| --- | --- | --- | --- |
| `--depth` | `0` | `0` | 关掉浮雕，全局扁平 |
| `--radius-box` | `0.75rem` | `0.75rem` | 卡片 / 模态框 / 下拉面板 |
| `--radius-field` | `0.5rem` | `0.5rem` | 按钮 / 输入框 / 标签页 |
| `--radius-selector` | `0.5rem` | `0.5rem` | 勾选框 / 开关 |
| `--border` | `1px` | `1px` | 保持默认，配合 §2.3② 只在必要时出现 |

> 圆角**不必两个主题取不同值**（它不涉及对比度），但**仍要写两遍** ——
> 主题块是独立的，light 里定义的不会继承给 dark。
> 若哪天只想改一处，正确的做法是抽成 CSS 变量在上层定义，**不是**指望它继承。

---

## 四、布局骨架

### 4.1 桌面端

```
┌──────────────────────────────────────────────┐
│ navbar（固定，bg-base-100, shadow-sm）        │
│  左：抽屉按钮(仅移动端) + 页面标题             │
│  右：主题切换按钮 + 用户 badge                  │
├────────────┬─────────────────────────────────┤
│ drawer-side│  main（p-4 md:p-6, bg-base-200）│
│ 侧栏菜单    │    ↑ 内容区                      │
│ (w-64)     │                                 │
└────────────┴─────────────────────────────────┘
```

- 侧栏在 `lg` 及以上是**常驻**的（`lg:drawer-open`），不是可折叠的。
- 导航项 **9 条封顶**，与 §4.1 页面表一一对应，**不设二级菜单**。
- **主题切换按钮常驻 `navbar-end`**（桌面上侧栏不可折叠，所以它只能放 navbar），
  移动端同理 —— 窄屏也要能切，不能因为位置不够就砍掉。

### 4.2 移动端（< 768px）

```
┌────────────────────────────┐
│ navbar（抽屉按钮 + 标题）    │
├────────────────────────────┤
│                            │
│  main（p-4, 单列）          │
│                            │
│                            │
├────────────────────────────┤
│ dock（固定底部，5 项）       │
└────────────────────────────┘
```

**移动端是本项目的硬要求**（用户明确要求"良好的移动端界面"），细则见 §7。

### 4.3 骨架代码（新页面照抄）

daisyUI 的 `drawer` 结构是**固定三层**，顺序错了就不工作：

```tsx
// 结构照抄，只改 <main> 里的内容
<div class="drawer lg:drawer-open">
  <input id="nav-drawer" type="checkbox" class="drawer-toggle" />

  <div class="drawer-content flex min-h-screen flex-col">
    <div class="navbar bg-base-100 shadow-sm">
      <div class="navbar-start">
        <label for="nav-drawer" class="btn btn-square btn-ghost lg:hidden">
          {/* 汉堡图标 */}
        </label>
        <span class="px-2 text-lg font-medium">{pageTitle}</span>
      </div>
      <div class="navbar-end gap-2">
        <button class="btn btn-square btn-ghost" aria-label="切换主题" onClick={toggleTheme}>
          {/* 图标随当前主题变：浅色下显示月亮、深色下显示太阳 */}
        </button>
        <span class="badge badge-ghost">{user.name}</span>
      </div>
    </div>

    <main class="flex-1 bg-base-200 p-4 md:p-6">{/* 页面内容 */}</main>
  </div>

  <div class="drawer-side z-40">
    <label for="nav-drawer" class="drawer-overlay" />
    <ul class="menu bg-base-100 min-h-full w-64 gap-1 p-3">
      {/* 导航项 */}
    </ul>
  </div>
</div>
```

要点（每条都被实践打过）：

- **`drawer-toggle` 是 `<input type="checkbox">`，靠 `<label for>` 驱动** ——
  不要用 JS 控制显隐，那是 Vapor 里没必要引入的状态。
- `drawer-side` 加 **`z-40`**：不加会被 navbar 的 `shadow-sm` 压住（z-index 同层时后写的赢）。
- `lg:drawer-open` 加在**根 `div`** 上，不是加在 `drawer-side` 上。
- `min-h-screen` 加在 `drawer-content` 上，且内容是 `flex-col` + `main` 用 `flex-1`
  —— 否则窄屏时底部 dock 会浮在半空。

> ⭐ **2026-09-21 按 daisyUI 官方 skill 复核骨架，三条补充（上面原文不改）**：
>
> 1. **开关抽屉的 `label` 要带部件类 `drawer-button`**（`class="btn btn-square btn-ghost drawer-button lg:hidden"`）。
>    它提供 `outline: 2px solid` + `outline-offset: 2px`，键盘 Tab 到汉堡包时才有焦点环。
>    上面的骨架里漏了这个类，`web/src/app.tsx` 的两个 `label`（打开 / 侧栏内关闭）已补上；新页面照抄时带上。
> 2. **`drawer-open` 是"常驻分栏"的 modifier，不是"当前展开"** —— 官方原文：
>    *"To make the sidebar visible on larger screens, use `lg:drawer-open`"*。
>    窄屏挂上它会把 256px 侧栏变成真实栅格列、压瘪内容，实测 493px 视口下内容区只剩 237px（§9.2③）。
> 3. **另有 `is-drawer-open:` / `is-drawer-close:` 两个 variant**，用来做"收起时只显示图标、展开时显示文字"的侧栏。
>    本项目当前**不做**这种形态（§4.1 定的是侧栏常驻 + 移动端抽屉），记在这里是为了将来要时不用重新翻文档。

### 4.4 底部导航（`dock`）

```tsx
<div class="dock dock-md border-t border-base-300">
  <button class={isActive('/') ? 'dock-active' : ''}>
    <IconDashboard class="size-[1.2em]" />
    <span class="dock-label">总览</span>
  </button>
  {/* … */}
</div>
```

- 类名是 **`dock`**，daisyUI 5 里**旧名 `btm-nav` 已废弃**，不要用旧的。
- **最关键的移动端细节**：`dock` 的 iOS 响应式依赖 `viewport-fit=cover`，
  而 `web/index.html` **已经配好了**（`content="width=device-width, initial-scale=1.0, viewport-fit=cover"`）。
  **不要删掉 `viewport-fit=cover`**，删了刘海里会出白条。
- 底部安全区：给 `dock` 所在容器加
  `padding-bottom: env(safe-area-inset-bottom)`（见 §7.3），否则 iPhone 小白条会盖住标签。
- **只放 5 项**：总览 / 任务 / 兑换 / 历史 / 更多（「更多」打开侧栏抽屉）。
  9 个页面全塞进 dock 会挤成不可点。
- dock 只在 `<768px` 显示：`md:hidden`。

> ⭐ **2026-09-21 更正（上面那条「更多」已作废）**：5 格**全部放页面** ——
> 总览 / 账号 / 任务 / 兑换 / 历史。
> **Why:** 「更多」点开的是侧栏抽屉，而抽屉在 navbar 左上角的汉堡包那里**已经有一个入口**
> （同一个 `label for="nav-drawer"`）⇒ 这一格不换来任何新的可达性，
> 只是把既有导航复制了一遍。用户原话："如果最底部的更多只是打开侧边栏，那就没有存在的意义"。
> **代价（明写出来，别当成无损）**：抽屉入口只剩左上角那一处，单手在大屏上够不着；
> 换来的是第 5 格是**真页面**（账号是仅次于总览的高频项，且是已落地的两个真页面之一）。
> 定时 / 调试台 / 设置 3 个页面仍只走抽屉。
> 实测（493px 视口）：5 格标签无一截断，`账号` 的激活态与 `aria-current="page"` 正常，
> 汉堡包仍能开抽屉且抽屉里 8 个页面齐全。

### 4.5 侧栏底部那一行：后端自述（2026-09-21 补）

```tsx
<div class="border-t border-base-300 p-3 text-xs leading-5">
  <div class="flex items-center gap-2 px-1">
    <span class={dot} aria-hidden="true" />   {/* status-success / -error / -neutral animate-pulse */}
    <span class="font-medium">{head}</span>   {/* mcloud v0.1.0 */}
  </div>
  <div class="px-1 opacity-70">{最近执行 …}</div>
</div>
```

这一格放什么，是被一轮反馈推出来的。三条边界都记在这里：

1. **不重复身份**。原先这里放「用户名 badge + 角色」，而初始用户的 `name` 与 `role`
   字面**都是 `admin`** ⇒ 左下角并排两个 admin（用户原话："显示两个用户名意义不明"）。
   现在身份**只在 navbar 一处**，`role` 收进那枚 badge 的 `title`。
2. **显示后端事实，不显示前端推断**。数据源是 `GET /api/status`
   （对位 `internal/server/routes.go` 的 `getStatus`），只有后端确实知道的三件事：
   产品名+版本（与 `--version`、`/api/version` **同源**，不是第二份真相）、
   `schedulerRunning`、最近一次 run。
   刻意**不**放账号数 / 任务数 —— 那是各自页面的内容，搬进常驻侧栏
   等于把"同一个数字出现在两个地方"再犯一遍。
3. **`schedulerRunning` 当前恒 `false`，界面就得跟着说「定时调度未启用」**。
   Go 侧还没有把 `schedules` 表变成"到点触发"的循环
   （`docs/status.md` 运行时层那条"调度接入中"）。只报任务数的话，
   用户会以为存进去的任务在跑 —— 那是**界面替后端编结论**。

三条实现细节（都是会被打的那种）：

- **红点只在真失败时红**。取数分三态 `pending / up / down`，`pending` 用
  `status-neutral animate-pulse`。把 pending 并进失败，首屏就闪一下红点，
  而红点的语义是"后端挂了"，看到的人当场去重启服务。
- **`lastRun.status` 只当文字，不当颜色**。它是存储层生命周期
  （`running` / `done` / `failed`），而 `done` 里含"部分成 / 需人工"三种业务结论 ——
  拿它决定红绿就是替后端编一个它没给过的结论（与 §2.4、与"云朵增减不是状态"同一类）。
- **不轮询**。这一行说的是"进页面时后端是什么样"，每分钟自己刷一次
  只会让人以为那个时间戳在动。取数时机就两个：会话恢复后、登录/初始化成功后。
  登出时把 `backend` 与状态一起清回 `pending` —— 这一行报的是**这个人**的数据，
  不清的话下一个人登录后先看到上一个人的"最近执行"。

---
