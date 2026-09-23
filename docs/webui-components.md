# WebUI 组件与交互规范（§五 组件 · §六 交互 · §七 移动端 · §八 动效）

> 状态：**规范** —— 与 [`webui-design-guide.md`](./webui-design-guide.md) 同一套判据，2026-09-22 从该文件逐字拆出。写任何组件、任何移动端适配前先读这篇。

> **本规范 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 基调·色彩·布局](./webui-design-guide.md) ｜ [② 组件·交互·移动端·动效](./webui-components.md) ｜ [③ 代码组织·落地·验收·未决](./webui-code-guide.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§一 ~ §十二 连续），所以「§4.1」「§7.2」
> 这类引用仍然有效，只是落在不同的文件里。

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
| `badge` | 有**文字**的标记（余额档位、任务状态） | `<span class="badge badge-soft badge-success">成功</span>` |
| `status` | **纯色点**，无文字（账号健康度） | `<span class="status status-success" />` |

`badge` **一律 `badge-soft`**（§2.3①）—— 表格里一列实心 badge 会把整张表压花。
只有"必须一眼抓住"的单一异常标记才允许实心，且同屏只允许一个。

**余额档位查表**（沿用现有 `app.tsx` 的约定，不要另起一套）：

```ts
// 修饰符 badge-soft 单独写、颜色类查表 —— 两边都不能拼接（§9.1① 的字面量陷阱）
const balanceBadge = {
  high: 'badge-soft badge-success',   // ≥1000
  mid:  'badge-soft badge-warning',   // ≥500
  low:  'badge-soft badge-error',     // <500
} as const
```

**账号健康度**（`status` 点）：绿 = 正常，红 = 连续失败，黄 = 有告警但可用。

> ⭐ **2026-09-21 补：`badge-soft` + 颜色类是一条会咬人的规则**（按 daisyUI 官方 skill 复核产物得出）。
>
> `web/node_modules/daisyui/components/badge.css` 里 `badge-soft` 的实现是：
> ```css
> .badge-soft { color: var(--badge-color, var(--color-base-content));
>               background-color: color-mix(in oklab, var(--badge-color, …) 8%, var(--color-base-100)) }
> ```
> **文字用颜色的原色，底色只掺 8%** —— 于是可读性完全取决于"这个颜色在当前主题里离 `base-100` 有多远"。
> 而 daisyUI 的 `neutral` 定义就是 *"A dark neutral color"*，**两个主题里都是深色**：
>
> | 写法 | light | dark |
> | --- | --- | --- |
> | `badge-soft badge-neutral` | 16.2:1 ✓ | **1.23:1 ✗ 黑字压黑底** |
> | `badge-soft`（不给颜色类） | 14.7:1 ✓ | 12.2:1 ✓ |
> | `badge-ghost` | 16.7:1 ✓ | 15.6:1 ✓ |
>
> （比值来源：`getComputedStyle` 解析出的 `color` / `background-color` —— 这些值都是**不透明**的
> `lab()` / `oklab()`，按 WCAG 相对亮度算对比度；同一批颜色另用测试带截图肉眼复核过。）
>
> **本项目的规矩**：中性/次要标记一律写**裸 `badge badge-soft`**，不要 `badge-neutral`。
> 已按这条改掉 4 处：navbar 用户名、账号页计数、平台标签（`手机`）、总览行数计数。
> §4.3 骨架里用户名原本写的 `badge badge-ghost` 也仍然可用 —— 两者都随主题走，
> 只是 `badge-soft` 与其余状态标记同族，优先它。
>
> 反过来这条规则也咬**浅色主题**：`badge-soft badge-success/warning/error` 在 light 下曾只有
> 1.87 / 1.69 / 2.64:1（亮色文字压 8% 浅底）。
> **2026-09-22 已修**（= 代码指南 §十二 未决项 9 的候选 (a)）：`app.css` 的 **light 块**里把四个
> 语义色压暗成 `#0a7d55` / `#8a6100` / `#c0233f` / `#0f7396`，现在是 **4.61 / 4.96 / 5.28 / 4.80:1**，
> 均过 WCAG AA。（第四个是 `info` —— 原色 `#00bafc` 实测只有 2.11:1，而 `alert-soft alert-info`
> 是应用级提示条那档，同族一并收口。）
> ⚠ **dark 块刻意不跟** —— 同一组原色压在深底上本来就有 7.08 / 7.76 / 4.97 / 6.26:1，
> 抄过去反而会弄坏深色主题（与上面"中性标记写裸 `badge-soft`"那条同理：规矩要分主题看）。
> ⚠⚠ 这些比值**要在真页面上量**：`-soft` 的底色是「8% 该色 + `base-100`」而不是纯白，
> 按纯白算会高估（`info` 曾按纯白选中一个真机只有 4.41:1 的值）。
> 触发这次改动的是用户的一句报障："**启用和停用是同一个颜色，无区别**" —— 见 §十二 未决项 9。


### 5.5 趋势图（手写 SVG）

**不引图表库**（方案 §2.2 已定，理由是可单测 + 体积）。

- 数据 → 路径的转换必须是**纯函数** `buildTrendPath(points, width, height)`，放 `src/ui/` 下，**可被 vitest 直接测**。
- 颜色用 `currentColor` 或 `text-primary`，这样自动跟随主题。
- **必须留 `viewBox` 且宽高自适应**（`class="w-full h-auto"`），否则窄屏会被裁。
- 空数据不能画成一条直线 —— 显示 `暂无数据` 的弱化文案。
- 涨跌填充色遵循 §2.4（涨红跌绿）。

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
- 触发器那枚 `<div class="btn …">` **本身不可聚焦** ⇒ 菜单永远打不开（`.dropdown` 靠
  `:focus-within` 展开），且不报任何错。要么写 `<button>`，要么照 `schedules-page.tsx`
  那样补 `tabindex={0} role="button"`。
- ⚠ `.menu` 的条目默认只有 **32px** 高 ⇒ 移动端这一页唯一的入口就在点不中的高度上，
  两枚条目要补 `min-h-11 md:min-h-0`（2026-09-23 定时任务页量到并补；桌面那档保持 33px 密度）。
  上面那枚 `btn-sm` 触发器同一档（32px），移动端也别用 `btn-sm`、给 `min-h-11`。

### 5.8 弹窗与确认（`modal`）

用**原生 `<dialog>`** + daisyUI 的 `modal` 类（方案 §2.2.3 已定）。

- **危险动作**（`core.RiskHigh`）必须弹确认，且要**显示影响面**：
  「将对 **3 个账号** 各消耗 **50 云朵**，共 **150**」——
  只写「确定吗？」等于没确认。
- 确认按钮文案**写动词**（「确认兑换」），不要写「确定」。
- `modal` 的 `modal-box` 在窄屏要 `max-w-[calc(100vw-2rem)]`，否则会顶到屏幕边缘。

  > **补记（2026-09-22，写 `ui/dialog.tsx` 时实测）**：上面这条**只在自定义宽度时**才需要，
  > 别把它当成 `modal-box` 的默认类照抄。daisyUI 5.7 的 `.modal-box` 自带
  > `width:91.6667%; max-width:32rem`（产物 CSS 里可核），窄屏本来就不贴边；
  > 而 `max-w-[calc(100vw-2rem)]` 会把那个 `32rem` 整个顶掉，宽屏下弹窗撑满屏幕 ——
  > 2026-09-22 登录弹窗"太宽、太丑"就是这个来由。
  > 现在的口径：宽度由 `Dialog` 的 `size` 档给（`sm`/`md`/`lg` 三个字面量集中在
  > `web/src/ui/dialog.tsx` 的 `SIZE_CLASS` 表里，拼接式 Tailwind 扫不到），
  > 表单类弹窗用 `md`、确认类用 `sm`。窄屏实测 373px 视口下弹窗 342px、左右各留 16px。

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
| 错误 | `alert alert-soft alert-error`，**必须带可读原因**，不能只显示「请求失败」 |

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

> ⭐ **2026-09-22 补：这条与 daisyUI 的默认值有一档出入，账号页那一处按字面执行。**
>
> `.btn` 默认只有 **40px** 高（实测记录与"要不要全局改"的那两个选项在
> [`status.md`](./status.md) 末尾"移动端 375×667 的量法"那一段，**那条出入仍留给用户拍板**，
> 本节不把它改成 40px）。
> 本轮加第四枚行内动作（"刷新 auth"）时，账号页移动端那两枚（新加的"刷新 auth"与原有的"删除"）
> 都补了 `min-h-11` ⇒ 44px，量法与数值见上面 status.md 那一段的同源 iframe 办法。
> 若将来定成"全局给 `.btn` / `.input` 加 `min-h:2.75rem`"，这两处 `min-h-11` 就该跟着删掉。

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
| 直播小红花 | 首屏三格 `stats` **竖排**（余额 / 门 / 下一场）+ 表单卡片全宽；房间日程与结果/历史**桌面表格 → 移动卡片**（每项带标签）；每个账号一张卡（两条 SSE 各一张），日志区固定高度可滚动；主按钮写 `min-h-11 md:min-h-0`（窄屏 44px，桌面回到 daisyUI 尺寸，见 §7.4 那条补记） |
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
- [ ] **主题切换按钮在窄屏也够得着**（不能被挤掉或挤出屏幕）
- [ ] 深色模式下无"浅字压浅底"

> **补记（2026-09-22，写「直播小红花」页时逐条量过）**：这条清单与现实**差一档**，
> 且差额在**共享件**里，不在新页面里：
> - `ui/dialog.tsx` 那两个按钮（取消 / 确认）静止 **40px**
>   （此处原写 38px —— 那是**弹窗开场那几帧**读到的，见下面"量法"那两条；结论一样：够不到 44）；
> - daisyUI 的 `select` / `input[type=number]` 默认 **40px**（本项目表单里到处是）。
>
> 所以新页面自己那两颗主按钮按 `min-h-11 md:min-h-0` 补到了 44px（窄屏生效、桌面回落），
> ⚠ **没有**顺手改 `ConfirmDialog` 与整站输入框尺寸 —— 那会一次动到所有页面，
> 属于"规范 vs 现实"要用户拍板的那类（AGENTS.md §4），已连同数据报进 `docs/status.md`。
> 量法：内置浏览器改不了尺寸，用**同源 iframe**（375×740）载入同一 URL，
> 在里面读 `getBoundingClientRect().height` 与 `documentElement.scrollWidth`。
> 两条读数的坑（2026-09-23 各踩一次，数值都记在 [`status.md`](./status.md) 末尾那一轮）：
> ① `modal-box` 打开的头几帧带 **0.95** 倍缩放，当场读会把静止 44 读成 42、40 读成 38
> ⇒ 弹窗里的尺寸**等一下再读**；
> ② 点完立刻同步读文本会读到**改之前**的那一份（Vapor 的更新在微任务之后才落）
> ⇒ 隔一两百毫秒再断言，别拿"读出来是旧的"当"界面坏了"。
> ③ 顺带一条只有真机看得见的：badge 表里只给修饰类（`badge-soft badge-success`）会**丢掉基类 `badge`**，
> 渲染成无样式裸文字，而 typecheck 与 build **全绿** —— 基类要么写在模板里（`` `badge ${mod}` ``），要么整条写全。
- [ ] 长文本已 `break-words`
- [ ] **抽屉展开时内容区仍是满屏宽**（`.drawer-content` 宽度 == 视口宽；不是被 256px 侧栏挤成一条，见 §9.2③）
- [ ] **逐跳切页后 `main h1` 跟着 `location.hash` 变**（空态页尤其容易停在第一跳，见 §9.2④）

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
