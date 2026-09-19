# WebUI 设计规范（**后续所有前端美工设计的唯一依据**）

> 状态：**规范**（normative）。风格参照 [Nexus — daisyUI Dashboard](https://nexus.daisyui.com/dashboards/ecommerce)。
> 技术栈已定，不再讨论：**Vue Vapor（无虚拟 DOM）+ TSX + daisyUI 5 + Tailwind CSS 4**。
>
> - 生效范围：`web/` 下的一切界面代码（含 `src/ui/` 组件层）
> - 权威关系：**视觉与交互冲突时以本文为准**；功能范围以
>   [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) §4.1 的页面表为准；
>   技术选型理由见 §2.2，本文不重复论证，只落地
> - 写新页面/新组件前**先读本文**；本文没有覆盖到的形态，先补本文再写码
> - 最后核对：**2026-09-19**

---

## 一、为什么要有这份文件

前端最容易烂的地方不是"不会写"，而是**同一个东西在三个页面里长三个样**：
卡片圆角不一样、间距不一样、同一个「失败」一会儿红一会儿橙、移动端一处能用一处不能。

这类不一致**不会报错**，只会在页面多起来之后变成"整体看起来不专业"，而且**改不动** ——
因为没有一处是"权威的定义"。

所以本文的作用是：**把决定权收口**。凡是"哪种更好看"的问题，答案在这里；
写码时不需要再即兴决定。**新增形态请改本文，而不是就地发挥。**

配合两条已经写进方案的硬约束（见 [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) §2.2.2）：

1. **类名必须是字面量** —— Tailwind 只识别源码里字面出现的类名，
   `btn-${variant}` 这种拼接在生产构建后会**静默失效**（样式没了但不报错）。
   动态一律走查表：`btnTheme[variant]`。**这是本项目的头号样式 bug 来源。**
2. **不能创建 `tailwind.config.js`** —— Tailwind 4 是 CSS-first，已废弃该文件。

---

## 二、设计基调

### 2.1 一句话定位

**克制的数据面板**：信息密度高、装饰极少、状态一眼可辨。
不做营销页那种大留白与渐变，也不做"仪表盘炫技"（发光边框、玻璃拟态、大色块）。

判断标准：**这个像素有没有承载信息？** 没有就去掉。

### 2.2 从 Nexus 借鉴什么、不借鉴什么

Nexus 是一个电商后台。我们的**用途不同**（定时任务 + 账号运营，不是交易），
所以照搬结构会别扭。逐条划清：

| 借（且必须借） | 理由 |
| --- | --- |
| **`drawer` + `navbar` + `dock` 三件套的骨架** | 这是 daisyUI 处理移动端最成熟的一套，自己造只会更差 |
| **指标卡的 `stats` 用法** | 顶部一排关键数字（云朵 / 今日增量 / 失败数 / 下次任务），结构直接对应 |
| **卡片 + 内嵌图表的节奏** | 大卡片承载趋势图，小卡片承载数字，层次清晰 |
| **时间范围切换（Day/Month/Year）的 `tabs`** | 直接映射为「7 天 / 30 天 / 全部」 |
| **表格右侧操作列用 `dropdown` 收口** | 行内按钮一多就挤，移动端尤其 |
| **状态用 `badge` 而不是彩色文字** | 可扫读性显著更好 |

| 不借 | 理由 |
| --- | --- |
| 商品图 / 头像 / 图片列 | 我们没有图片资源，塞占位图是纯噪音 |
| 大面积营销式留白 | 我们是运维工具，屏幕要装信息 |
| 多币种 / 汇率 / 预测曲线 | 无对应业务 |
| 侧栏多级嵌套菜单 | 我们只有 9 个页面，一级足够；嵌套反而难点 |
| 自定义多主题切换（它的 6 个主题） | **我们只用 `light` + `dark`**（见 §3.1） |

> **口径**：Nexus 是**布局与组件的参照**，不是**像素的参照**。它用什么间距我们就用什么，
> 但它的商品卡、图表配色、动效节奏都不适用，不搬。

### 2.3 「涨红跌绿」不可动

这是**业务约定，不是配色偏好**（见 [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) 与旧项目日志语义）：

- **涨 = 红**（`text-error`），**跌 = 绿**（`text-success`）
- 与 A 股习惯一致，**与 daisyUI 默认语义相反**（daisyUI 里 success 是绿、error 是红）

由此推出两条硬规则：

1. **禁止用 `text-success` / `text-error` 表达"涨跌"以外的语义**。
   成功/失败状态另见 §4.5 的 `status` 用法 —— 那里绿=正常、红=异常，
   与涨跌**恰好同向**，不会打架；但**不要**用同一个 badge 同时表达两件事。
2. 涨跌色只用于**数值本身**，不要给整行/整卡上色。

---

## 三、主题与色彩

### 3.1 只启用两个主题

```css
/* web/src/app.css —— 现状保持，不要新增主题 */
@plugin "daisyui" {
  themes: light --default, dark --prefersdark;
}
```

- **浅色为默认**（`light --default`）：本机 IDE 与既有界面都是浅色，且它是打印/截图友好的那个。
- 深色跟随系统（`dark --prefersdark`），**不提供手动切换开关** —— 少一个状态就少一处 bug，
  且真正需要的人系统级设置已经解决。
- **禁用** Nexus 用的 `contrast` / `material` / `dim` 等多主题（它是演示站，需要showcase）。

> `data-theme` 挂在 `<html>` 上，见 `web/index.html`。

### 3.2 品牌主色已微调，不要再改

```css
@plugin "daisyui/theme" {
  name: "light";
  default: true;
  color-scheme: light;

  --color-primary: oklch(52% 0.19 275);        /* 对齐 mcloud 既有品牌色 */
  --color-primary-content: oklch(98% 0.01 275);
}
```

**只微调了 `primary` 一档**，其余全部用 daisyUI 默认 —— 这是刻意的：
默认值经过对比度校验，随手改容易掉到 AA 以下。

### 3.3 色彩的用法纪律

| 用途 | 用什么 | 禁止 |
| --- | --- | --- |
| 页面底色 | `bg-base-200` | 不要在 `base-300` 上放正文（对比度不足） |
| 卡片 / 容器 | `bg-base-100` | 不要给卡片加边框**又**加阴影（选一个） |
| 主行动（每屏最多一个） | `btn-primary` | 不要一屏多个 `btn-primary`，会失去"主要"的含义 |
| 次要行动 | `btn-ghost` / `btn-outline` | — |
| 危险动作（消耗云朵、删除） | `btn-error` **并且必须二次确认**（§6.4） | 不要用红色做"取消"按钮 |
| 信息系统提示 | `alert-info` | — |
| 成功 / 失败 | `alert-success` / `alert-error` | — |
| 涨 | `text-error` | — |
| 跌 | `text-success` | — |
| 弱化文字 | `opacity-70` / `opacity-60` | 不要用 `text-gray-500` 这类写死色 —— 切主题就崩 |

> **最后一条是硬规则**：**禁止使用 `gray-*` / `slate-*` / `zinc-*` 等 Tailwind 具名色阶**。
> 它们不随 daisyUI 主题变，深色模式下必然出现"浅灰字压深灰底"。
> 一律用语义色（`base-content`）+ `opacity-*`。

### 3.4 间距与圆角

用 Tailwind 默认刻度，**不自定义**。常用组合固定下来：

| 场景 | 类名 |
| --- | --- |
| 页面主容器内边距 | `p-4 md:p-6` |
| 卡片之间 | `gap-3 md:gap-4` |
| 卡片内部 | `card-body` 默认（不要自己 `p-4` 覆盖，除非是紧凑卡片） |
| 表单字段之间 | `gap-3` |
| 分组标题与内容 | `mb-3` |

圆角一律走 daisyUI 变量（`--radius-box` / `--radius-field`），
**不要写 `rounded-lg` 这类硬编码** —— 它绕过了主题。

---

## 四、布局骨架

### 4.1 桌面端

```
┌──────────────────────────────────────────────┐
│ navbar（固定，bg-base-100, shadow-sm）        │
│  左：抽屉按钮(仅移动端) + 页面标题             │
│  右：用户 badge + 主题无关的操作                │
├────────────┬─────────────────────────────────┤
│ drawer-side│  main（p-4 md:p-6, bg-base-200）│
│ 侧栏菜单    │    ↑ 内容区                      │
│ (w-64)     │                                 │
└────────────┴─────────────────────────────────┘
```

- 侧栏在 `lg` 及以上是**常驻**的（`lg:drawer-open`），不是可折叠的。
- 导航项 **9 条封顶**，与 §4.1 页面表一一对应，**不设二级菜单**。

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

---

## 五、组件规范

> 全部是 **daisyUI 类名**，不自造 CSS。这一节是"什么东西长什么样"的权威答案。
> 每一类的**业务逻辑**要自己写（这是选 daisyUI 的代价，见方案 §2.2.3），
> 但**样式类名**照抄本节。

### 5.1 指标卡（`stats`）

顶部一排关键数字。**这是每个页面的第一屏**。

```tsx
<div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
  <div class="stat">
    <div class="stat-title">云朵总数</div>
    <div class="stat-value text-2xl">{total}</div>
    <div class="stat-desc">今日 +{todayGain}</div>
  </div>
  {/* 更多 stat */}
</div>
```

- **移动端必须 `stats-vertical`**，桌面 `md:stats-horizontal` —— 窄屏横排会挤到不可读。
  注意写法是 `stats-vertical ... md:stats-horizontal`（先竖后横），不是反过来。
- 数值用 `text-2xl`，**不要用默认的 `text-4xl`** —— 我们的数字（云朵数）没有电商金额那么重要，
  4xl 一屏只能放两个。
- `stat-title` 用默认（小号弱化），不要再叠加 `opacity-*`。

### 5.2 卡片（`card`）

内容的基本容器。

```tsx
<div class="card bg-base-100 shadow-sm">
  <div class="card-body">
    <h2 class="card-title text-base">{title}</h2>
    {/* 内容 */}
  </div>
</div>
```

- **`card-title` 一律压到 `text-base`** —— daisyUI 默认约 `text-2xl`，做后台标题太大。
- 卡片用 **`shadow-sm`**（不是 `shadow` / `shadow-md`）：多层 `shadow` 叠起来很脏。
- 卡片**不再加 `border`**（`shadow-sm` 已经分界，两者同时上是典型的"加戏"）。
- 紧凑场景（列表项）可用 `card-body p-3`，此时才覆盖内边距。

### 5.3 表格（`table`）

**桌面表格 + 移动卡片双形态**，不用横向滚动。

```tsx
{/* 桌面：≥768px */}
<div class="hidden overflow-x-auto rounded-box bg-base-100 md:block">
  <table class="table table-sm">
    <thead><tr><th>账号</th><th>云朵</th><th>状态</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</div>

{/* 移动：<768px */}
<div class="grid gap-3 md:hidden">{cards}</div>
```

- **`table-sm`** 是默认选择：后台表格密度优先。
- **不做横向滚动**。列多的表在窄屏是**不可用**的，必须切卡片列表。
  这是 §7 的硬要求之一（方案 §4.1 已把每页的移动端形态定好了）。
- 行操作放**最右列**，且用 `dropdown` 收口（§5.7）—— 行内并排三个按钮在窄屏必崩。

### 5.4 状态与标记（`badge` / `status`）

两个组件分工明确，**不要混用**：

| 要用 | 场景 | 写法 |
| --- | --- | --- |
| `badge` | 有**文字**的标记（余额档位、任务状态） | `<span class="badge badge-success">成功</span>` |
| `status` | **纯色点**，无文字（账号健康度） | `<span class="status status-success" />` |

**余额档位查表**（沿用现有 `app.tsx` 的约定，不要另起一套）：

```ts
const balanceBadge = {
  high: 'badge-success',   // ≥1000
  mid:  'badge-warning',   // ≥500
  low:  'badge-error',     // <500
} as const   // ← 必须是字面量查表，不能拼接（见 §1）
```

**账号健康度**（`status` 点）：绿 = 正常，红 = 连续失败，黄 = 有告警但可用。

### 5.5 趋势图（手写 SVG）

**不引图表库**（方案 §2.2 已定，理由是可单测 + 体积）。

- 数据 → 路径的转换必须是**纯函数** `buildTrendPath(points, width, height)`，放 `src/ui/` 下，**可被 vitest 直接测**。
- 颜色用 `currentColor` 或 `text-primary`，这样自动跟随主题。
- **必须留 `viewBox` 且宽高自适应**（`class="w-full h-auto"`），否则窄屏会被裁。
- 空数据不能画成一条直线 —— 显示 `暂无数据` 的弱化文案。
- 涨跌填充色遵循 §2.3（涨红跌绿）。

### 5.6 表单（`input` / `select` / `fieldset`）

```tsx
<fieldset class="fieldset">
  <legend class="fieldset-legend">账号名</legend>
  <input class="input input-bordered w-full" />
</fieldset>
```

- **`label` 写法统一用 `fieldset` + `fieldset-legend`**（daisyUI 5 的新形态），
  不要混用旧的 `form-control` + `label-text`（现有 `app.tsx` 里是旧的，改页面时顺手统一）。
- 所有输入控件**必须 `w-full`** —— 窄屏下不写会缩成默认宽度。
- 参数表单由 `FieldSpec` 生成（方案 §4.2），**新增能力零前端改动**是硬指标。
- 保留「高级 JSON」折叠区（`collapse`）兜底。

### 5.7 下拉操作（`dropdown`）

行操作、卡片右上角的「⋯」菜单统一用：

```tsx
<div class="dropdown dropdown-end">
  <div tabindex={0} role="button" class="btn btn-sm btn-ghost">⋯</div>
  <ul tabindex={0} class="menu dropdown-content z-50 w-40 rounded-box bg-base-100 shadow">
    <li><button>编辑</button></li>
    <li><button class="text-error">删除</button></li>
  </ul>
</div>
```

- **`dropdown-content` 必须带 `z-50`**，否则会被 `table` 的行或 `card` 裁掉。
- `dropdown-end` 让菜单右对齐（行操作在最右列时必需，否则菜单会溢出屏幕）。

### 5.8 弹窗与确认（`modal`）

用**原生 `<dialog>`** + daisyUI 的 `modal` 类（方案 §2.2.3 已定）。

- **危险动作**（`RiskLevel::High`）必须弹确认，且要**显示影响面**：
  「将对 **3 个账号** 各消耗 **50 云朵**，共 **150**」——
  只写「确定吗？」等于没确认。
- 确认按钮文案**写动词**（「确认兑换」），不要写「确定」。
- `modal` 的 `modal-box` 在窄屏要 `max-w-[calc(100vw-2rem)]`，否则会顶到屏幕边缘。

### 5.9 倒计时（`countdown`）

抢兑时刻用 daisyUI 内置 `countdown`，不要自己写定时器算格子。

- **`countdown` 的数字必须包在 `<span>` 里**，这是它的实现要求（靠 CSS 变量取位）。
- T-1 小时内**改变颜色**提示（`text-warning`），T-1 分钟内 `text-error`。
- 距开抢时间是**服务端给的**，前端只做展示倒推，**不要用本地时钟算差值**（时区/漂移）。

### 5.10 加载与空态

| 状态 | 用什么 |
| --- | --- |
| 首屏加载 | 居中 `loading loading-spinner loading-lg` |
| 局部加载 | 容器内 `loading loading-spinner`，或 `skeleton` 占位 |
| 空列表 | 弱化文字 + 一个明确的下一步动作（如「还没有账号，去添加」） |
| 错误 | `alert alert-error`，**必须带可读原因**，不能只显示「请求失败」 |

> 空态**不要**只写「暂无数据」四个字 —— 它没告诉用户下一步做什么。

### 5.11 提示（`toast`）

用 daisyUI `toast` 样式 + 一个约 20 行的自建 store（方案 §2.2.3 已清点）。

- `toast-end toast-top`（右上角）—— **不要用底部**，会和移动端 `dock` 打架。
- 自动消失 **3 秒**；错误类提示**不自动消失**，要用户手动关。
- 一屏最多堆 **2 条**，多了直接替换最旧的。

---

## 六、交互与状态

### 6.1 触控目标 ≥44px

**移动端硬要求。** 推论：

- 移动端**禁用 `btn-xs`**（约 24px 高，点不中）。
- `btn-sm` 只在 `md:` 以上使用。
- 表格行如果是可点的，加 `min-h-11`。

### 6.2 禁用 hover-only 交互

**移动端没有 hover。** 所以：

- 表头操作、行内按钮**常驻显示**，或收进 `dropdown`（§5.7）。
- **禁止**用 `group-hover:` 显示关键操作 —— 那等于移动端没有这个功能。
- `tooltip` 只用于**补充说明**，不能承载必需信息。

### 6.3 加载态不要锁死界面

长任务（签到、兑换）走 SSE 实时日志（方案 §4.2）：

- 提交按钮进入 `disabled` + 文案改「处理中…」，但**页面其余部分保持可操作**。
- 进度用 `progress` 或 `radial-progress`，并显示**已用时长**。
- 显示「**可关闭页面**，任务在后台继续」—— 这是真实行为，不写用户会以为关了就没了。

### 6.4 危险动作的统一流程

```
点击 → 确认弹窗（显示影响面）→ 执行 → toast 反馈
```

- 影响面必须**具体到数字**（几条记录 / 几个账号 / 多少云朵）。
- 云朵消耗类动作：确认按钮文案 = 「确认消耗 N 云朵」。
- 二次确认**不做**"输入文字才算数"那种重流程 —— 本系统是自托管工具，用户就是管理员。

---

## 七、移动端细则（**本次的硬要求**）

> 方案 §2.2.5 已定"响应式足够，不做独立组件库"。本节把它落成可检查的条目。
> **验收方式**：Chrome DevTools 设成 iPhone SE（375×667），逐页面走一遍。

### 7.1 断点约定

| 断点 | 形态 |
| --- | --- |
| `< 768px`（Tailwind 默认，即 `md` 以下） | 移动：单列 + `dock` + 抽屉侧栏 |
| `≥ 768px` | 桌面：侧栏常驻 + 表格 |

**只用一个断点**（`md`）。多断点是复杂度的主要来源，而我们只有两种真正不同的形态。

### 7.2 逐页移动端形态（与方案 §4.1 对齐，不得偏离）

| 页面 | 移动端形态 |
| --- | --- |
| 登录 / 初始化 | 单列表单，卡片 `w-full max-w-sm`，页面垂直居中 |
| 仪表盘 | **单列卡片流**（`grid gap-3`），指标卡 `stats-vertical` |
| 账号详情 | **折叠分区**（`collapse`），趋势图全宽 |
| 任务中心 | `tabs` 横滑分组 + 卡片；参数表单全宽 |
| 兑换中心 | **抢兑优先布局**：倒计时在最上，其余依次下移 |
| 定时任务 | 列表 + **底部固定操作条** |
| 历史与日志 | 卡片流；SSE 日志区固定高度可滚动 |
| 调试台 | 简化版：请求/响应上下堆叠，不做左右分栏 |
| 设置 | 分区表单（`fieldset`） |

### 7.3 必做的技术细节

```css
/* 底部安全区（iPhone 小白条）：dock 所在容器 */
padding-bottom: env(safe-area-inset-bottom);

/* 抽屉在 iOS 上滚动到边界会带动整页 —— 侧栏需要独立滚动 */
overscroll-behavior: contain;
```

- **`viewport-fit=cover` 已在 `index.html` 配好，不要删。**
- 长文本（账号名、错误信息）一律 `break-words`，否则会撑破卡片。
- 表格切卡片后，**每条数据必须有标签**（「云朵 1286」而不是只显示 `1286`）。
- `dock` 用 `md:hidden` 只在移动端出现；同时给 `main` 加
  `pb-20`（或 `dock` 高度对应的值），否则内容会被 dock 盖住最后一行。

### 7.4 移动端验收清单

每加一个页面，逐条自检：

- [ ] 375px 宽下**无横向滚动条**
- [ ] 所有按钮可点中（≥44px 高）
- [ ] 不用 hover 也能完成全部操作
- [ ] `stats` 是竖排
- [ ] 表格已切卡片、且**每项带标签**
- [ ] 内容底部不被 `dock` 遮挡
- [ ] 刘海 / 小白条区域无内容被裁
- [ ] 深色模式下无"浅字压浅底"
- [ ] 长文本已 `break-words`

---

## 八、动效

**要克制。** 这是运维工具，用户一天看几十次，动效只增加等待感。

| 允许 | 禁止 |
| --- | --- |
| `transition-colors`（按钮/链接着色，150ms） | 元素入场动画（fade-in / slide-in） |
| `loading-spinner` | 骨架屏闪烁动画 |
| 抽屉 / 弹窗的默认过渡 | 数字滚动、图表生长动画 |
| — | 任何超过 300ms 的过渡 |

---

## 九、代码组织约定

```
web/src/
  app.tsx          应用外壳：会话门禁 → 决定渲染哪个页面（不长内容）
  api.ts           后端客户端（类型将由 openapi.json 生成，见方案 §4.3）
  ui/              ★ 自有组件层（本节的重点）
    TrendChart.tsx     手写 SVG 趋势图
    StatRow.tsx        指标卡组
    DataTable.tsx      表格 ↔ 卡片双形态
    Confirm.tsx        危险动作确认弹窗
    toast.ts           toast store（约 20 行）
    fields.tsx         FieldSpec → 表单控件
  pages/           9 个页面，一个文件一个页面
```

三条约定：

1. **页面不写重复的样式串** —— 出现第二次就抽进 `ui/`。
   判定标准：**同一串类名出现两次**就抽，不要等出现三次。
2. **`ui/` 里的逻辑全部是纯函数**，与 TSX 分离，这样能被 vitest 直接测（方案 §6.1 的 L1'）。
   典型：`buildTrendPath()`、排序/分页、`时刻 → cron` 映射、JSON 校验。
3. **组件名与属性一律 camelCase** —— `vue-jsx-vapor` **不把连字符转小驼峰**（方案 §2.2.4）。
   写成 `<trend-chart>` 会失效。

### Vapor 边界（写页面时会撞上的）

- **仅 Composition API**；没有 `getCurrentInstance()`、`app.config.globalProperties`。
- **不支持 `Suspense`** → 异步一律用显式 `loading` 状态。
- `v-model` 不支持数组表达式 → 用修饰符写法 `v-model:$value$_trim`。

> **兜底（当前不开）**：`interop: true` + `vaporInteropPlugin` 已在方案里写好。
> 真要引入依赖 VDOM 的库（如实时代码编辑器）时零成本开启。**当前默认不开。**

---

## 十、新页面的落地流程

1. **先读本文**，确认要用的形态在 §5 里有定义；没有就先补本文。
2. 从 §4.3 的骨架照抄开始，不重新发明布局。
3. 用 §5 的组件类名搭结构，**不写自定义 CSS**。
4. 逻辑抽成纯函数放 `ui/`，配 vitest 用例（方案 §6.1 的 L1'）。
5. 跑 §7.4 的移动端清单。
6. 桌面 + 移动、浅色 + 深色，**四种组合都过一遍**。

---

## 十一、验收清单（整体）

一个页面上线前必须全部为「是」：

- [ ] 用的是 §4.3 的 `drawer` 骨架，`drawer-toggle` 是 checkbox 且靠 `label` 驱动
- [ ] 没有出现 `btn-${x}`、`badge-${x}` 这类**拼接类名**（§1）
- [ ] 没有 `gray-*` / `slate-*` 等**具名色阶**（§3.3）
- [ ] 没有 `rounded-lg` 等**硬编码圆角**（§3.4）
- [ ] 没有 `tailwind.config.js`（§1）
- [ ] 涨跌用 `text-error` / `text-success`，且没和状态语义混用（§2.3）
- [ ] 一屏只有一个 `btn-primary`（§3.3）
- [ ] 危险动作有确认弹窗且显示了**具体影响面**（§6.4）
- [ ] `dropdown-content` 带 `z-50`（§5.7）
- [ ] 移动端清单（§7.4）逐条通过
- [ ] 深色模式下无对比度问题

---

## 十二、未决项（不猜，等实现时定）

| # | 事项 | 何时定 |
| --- | --- | --- |
| 1 | **图标库落地**：方案说 `unplugin-icons` + Iconify（inline SVG），但 `package.json` 里**尚未安装** | 写第一个真实页面时 |
| 2 | `openapi-typescript` 生成链路（方案 §4.3）—— 类型目前**仍是手写**的 `api.ts` | P5 接口冻结后 |
| 3 | vitest 尚未引入（方案 §6.1 L1' 需要） | 写 `ui/` 纯函数时 |
| 4 | 抽屉侧栏在**极窄屏（<320px）** 的宽度是否需要收窄为 `w-56` | 移动端首次自测后 |
| 5 | 是否要 vendored 一份 daisyUI 文档文本供 AI 参考（方案 §2.2.6 的建议） | 视 AI 写码的实际出错率 |

> 本文与 [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) 的分工：
> **方案定"用什么技术、有哪些页面"；本文定"长什么样、怎么写"。**
> 两者冲突时，技术选型以方案为准，视觉与交互以本文为准。
