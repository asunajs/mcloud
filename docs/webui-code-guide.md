# WebUI 代码组织与验收（§九 代码约定 · §十 落地流程 · §十一 验收清单 · §十二 未决项）

> 状态：**规范** —— 与 [`webui-design-guide.md`](./webui-design-guide.md) 同一套判据，2026-09-22 从该文件逐字拆出。落地下一个页面前读这篇。

> **本规范 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 基调·色彩·布局](./webui-design-guide.md) ｜ [② 组件·交互·移动端·动效](./webui-components.md) ｜ [③ 代码组织·落地·验收·未决](./webui-code-guide.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§一 ~ §十二 连续），所以「§4.1」「§7.2」
> 这类引用仍然有效，只是落在不同的文件里。

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
    theme.ts           主题切换：nextTheme() 纯函数 + 读写 data-theme/localStorage
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

### 9.1 两个「静默失效」陷阱（都实际踩过）

这两个坑的共同点是：**不报错、类型检查也过、构建也成功，只是样式不对**。
所以只能靠"产物核对"发现，不要指望工具链报错。

**① 拼接类名**（Tailwind 只认字面量）

```tsx
class={`btn btn-${variant}`}      // ❌ 生产构建后样式消失
class={btnVariant[variant]}       // ✅ 查表，值必须是完整字面量
```

**② 主题变量写错插件块**

`--color-*` / `--radius-*` / `--depth` / `--noise` / `--size-*` / `--border`
是 **daisyUI** 的主题变量 → 写进 `@plugin "daisyui/theme"`。

`--font-*` / `--spacing` / `--text-*` / `--breakpoint-*` 等是
**Tailwind** 的 `@theme` 变量 → 写进 `@theme { }`，**写进 daisyUI 块会被静默吞掉**。

> **怎么核对**：改完主题相关的东西，**一定**要 grep 一遍产物：
> ```bash
> f=$(ls web/dist/assets/index-*.css)
> grep -o -e '--font-sans:[^;}]*' -e '--depth:[^;}]*' "$f" | sort -u
> ```
> 看到的值和源码里写的一致才算生效。§2.3④ 的字体坑就是这么发现的。

### 9.2 静默失效陷阱追加（2026-09-21 实测，都是真实踩过的）

§9.1 立了两条，这里再补两条同样「不报错、类型检查过、构建成功」的。
共同点还是那句：**只能靠产物核对 + 真机量一遍，不要指望工具链报错。**

**③ 移动端抽屉用 JS 挂 `drawer-open` —— 内容被挤瘪**

§4.3 的骨架是对的（根 div 只有 `drawer lg:drawer-open`，展开态由 `#nav-drawer` 这个 checkbox 驱动）。
实现时一度"顺手"加了 `drawerOpen ? 'drawer drawer-open lg:drawer-open' : ...`，
以为这样才是"打开抽屉"。**`drawer-open` 不是展开态，是常驻分栏变体**：

```css
.drawer { display: grid; grid-auto-columns: max-content auto }
.drawer-open > .drawer-toggle ~ .drawer-side { display: block; position: sticky; width: auto }
```

⇒ 窄屏挂上它，256px 侧栏占掉**一列真实栅格**，`.drawer-content` 只剩剩下的那条。
实测（493px 视口）：`grid-template-columns: 256px 237px`、内容区 **493 → 237px**、
`.drawer-side` 的 `position` 从 `fixed` 变 `sticky`。这就是"打开侧边栏右边一堆挤在一起的字符"。

正确形态：移动端靠 `.drawer-toggle:checked ~ .drawer-side` 走**浮层**（`fixed` + 半透明 overlay），
`drawer-open` 只以 `lg:` 前缀出现。JS 里的开关状态**只**绑 checkbox 的 `checked`，不碰类名。

**④ Vapor 组件在 setup 里读 `props.x` 的快照 —— 切页时正文不动**

Vapor 没有重渲染：函数体就是 setup，**一个实例只跑一次**。
空态页 `<PlaceholderPage id={page.value} />` 落在 `createIf` 的**同一个 else 分支**里，
页面之间切换（定时 → 调试台 → 设置）不会重建实例，于是 setup 里的
`const item = navItem(props.id)` 永远停在第一次进来的那个页面。
实测表现很迷惑人：**navbar 标题跟着变（那是 app.tsx 里的响应式插值），正文的标题/图标/说明纹丝不动**。

编译器（`vue-jsx-vapor`）对这两件事的处理不一样，可以直接看产物确认：

```bash
node -e "const t=require('./web/node_modules/vue-jsx-vapor/dist/api.cjs'),fs=require('fs');
console.log(t.transformVueJsxVapor(fs.readFileSync('web/src/app.tsx','utf8'),'app.tsx').code)"
```

- 文本插值 `{item.value.title}` → `_setNodes(x, () => item.value.title)`：**响应式**，会跟着变。
- 组件元素 `<item.value.icon />` → `_createComponent(item.value.icon, {...})`：setup 里**只求值一次**。
- props 传的是 getter（`id: () => page.value`），所以 `props.id` 本身是响应式的 ——
  问题出在 setup 把它抄成了常量。

⇒ 两种改法：**要么** setup 里一律用 `computed(() => ...props.x)`（只够救文本，救不了图标这类组件元素）；
**要么**在调用点给 `key`：`<PlaceholderPage key={page.value} id={page.value} />`，
它编译成 `_createKeyedFragment(() => page.value, () => _createComponent(PlaceholderPage, {...}))`，
key 变了整棵子树重建，标题/图标/说明一起跟上。**当前用的是 key**：一个 props 决定整页形态的组件，
重建比逐字段响应式更省心，也不会漏掉下一个新增字段。

> **怎么核对**：切页不能只看"有没有报错"，要逐跳量 `main h1` 的文本是否跟着 `location.hash` 变。
> 本次两条都是这么发现的：`#/schedules → #/debug → #/settings` 三跳，
> 修复前 `h1` 一直是「定时任务」，修复后逐跳对上。

### 产物大小怎么读（2026-09-21 实测，别再判成"没做 tree shaking"）

`web/dist/assets/index-*.css` 有 **80,213 字节**，看着像"什么都在往里塞"。实测不是：

- 逐个 grep 那 181 个类名，**没有**一个未被引用的组件混进来（carousel / diff / hover-3d /
  megamenu / otp / kbd / rating / timeline / countdown / textarea / modal 全是 0 命中）
  ⇒ 按需生成（Tailwind 4 + daisyUI 5 的 `@utility`）是**生效**的。
- 大头的真实原因：daisyUI 每个组件被拆成 **9 个 `@layer daisyui.lX.lY.lZ` 子层**
  （为了变体优先级），产物里 `@layer daisyui.*` 占 54,191 字节 —— 层包装是那个乘数，
  不是"没删干净"。
- **走线不是 80 KB**：`internal/server/static.go` 那层已按 `Accept-Encoding` 出 gzip，
  同一份 CSS 实测 **12,771 字节**；而 `/assets/*` 是内容哈希名 ⇒ `immutable` 一年，
  第二次访问**一个字节都不下**（走本地缓存）。

> **所以量体积要量哪一个**：判"这次改动给产物加了多少字节"用**磁盘上那份**（构建是确定性的，
> 哈希名会变，层数会涨）；判"用户多下多少"得走 HTTP 打一遍 `curl -sI -H 'Accept-Encoding: gzip'`，
> 磁盘数字在 gzip 那一层面前不成比例。别拿磁盘那份去论证"要减类名"。
> 细节与判据见 [`status.md`](./status.md) 服务端那一节的 2026-09-21 追加。

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

> 第 1 步之前还有一步：**激活 daisyUI 官方 skill 并读对应组件指南**（§1.1）。
> 顺序不能反 —— 先凭记忆写完类名再去核对，等于把"是否存在这个类"交给运气，
> 而 daisyUI 5 改过名的组件（`btm-nav` → `dock`）正是这么踩的。

---

## 十一、验收清单（整体）

一个页面上线前必须全部为「是」：

- [ ] 用的是 §4.3 的 `drawer` 骨架，`drawer-toggle` 是 checkbox 且靠 `label` 驱动
- [ ] 没有出现 `btn-${x}`、`badge-${x}` 这类**拼接类名**（§9.1①）
- [ ] 次要行动用 `-soft`，一屏只有一个实心 `btn-primary`（§2.3①、§3.4）
- [ ] 卡片用 `shadow-sm` 而非 `border`（§2.3②）
- [ ] 没有 `gray-*` / `slate-*` 等**具名色阶**（§3.4）
- [ ] 没有 `rounded-lg` 等**硬编码圆角**（§3.5）
- [ ] 没有 `tailwind.config.js`（§1）
- [ ] 涨跌用 `text-error` / `text-success`，且没和状态语义混用（§2.4）
- [ ] 危险动作有确认弹窗且显示了**具体影响面**（§6.4）
- [ ] `dropdown-content` 带 `z-50`（§5.7）
- [ ] 改过主题变量的话，**已 grep 产物核对**（§9.1②）
- [ ] 移动端清单（§7.4）逐条通过
- [ ] **主题切换可用**：navbar 有开关、点了生效、刷新后仍在（§3.2）
- [ ] **两个主题都试过**：`light` 与 `dark` 下各看一遍，`primary` 都是品牌色而非默认紫（§3.3）
- [ ] 深色模式下无对比度问题
- [ ] **浅色模式下也无对比度问题** —— 特别是 `badge-soft` + 颜色类（§5.4 补充：文字是原色、底色只掺 8%）
- [ ] **写码前激活了 daisyUI 官方 skill，且用到的组件指南读过**（§1.1）；
  类名对不上产物时以 `web/node_modules/daisyui/components/<name>.css` 为准
- [ ] 切页时**正文与 URL 对得上**：逐跳量 `main h1`（§9.2④，Vapor 的 props 快照坑）

---

## 十二、未决项（不猜，等实现时定）

| # | 事项 | 何时定 |
| --- | --- | --- |
| 1 | **图标库落地**：方案说 `unplugin-icons` + Iconify（inline SVG），但 `package.json` 里**尚未安装** | 写第一个真实页面时 |
| 2 | `openapi-typescript` 生成链路（方案 §4.3）—— 类型目前**仍是手写**的 `api.ts` | P5 接口冻结后 |
| 3 | vitest 尚未引入（方案 §6.1 L1' 需要） | 写 `ui/` 纯函数时 |
| 4 | 抽屉侧栏在**极窄屏（<320px）** 的宽度是否需要收窄为 `w-56` | 移动端首次自测后 |
| 5 | 是否要 vendored 一份 daisyUI 文档文本供 AI 参考（方案 §2.2.6 的建议） | 视 AI 写码的实际出错率。<br>**2026-09-21 追加**：已装 daisyUI 作者发布的官方 skill（`saadeghi/daisyui`，用户级 `~/.qoder/skills/daisyui/`），本条的"要不要再 vendored 一份"暂时不用答 —— 见 §1.1 |
| 6 | **字体落地**：§2.3④ 的 `--font-sans` 已在 `app.css` 生效，但**只写拉丁子集 + 中文走系统回落**，字体文件**尚未自托管**（目前实际渲染仍是 Segoe UI / 雅黑）。要自托管就选一个可变量重字体，**不要**链 Google Fonts | 有设计稿或首次做视觉验收时 |
| 7 | ~~`--depth: 0` 会不会让 `btn` 失去按下的反馈~~ **已核实不会**：`:hover` 的加深是硬编码 7%、`:active` 是 `translate:0 .5px`，两者都**不经过 `--depth`**。`--depth` 只管内阴影/浮雕，拿掉它正好。无需补偿 | — |
| 8 | `btn-soft` 会带一条 `border-style:solid`（边框色是主色 10% 混底）。这与 §2.3②「阴影而非边框」并存，视觉上可接受（很淡），但若实测偏"描边感"，要在 `app.css` 里统一清掉软按钮的 border | 首次视觉验收时 |
| 9 | **浅色主题下状态 badge 读不清**：`badge-soft badge-success / warning / error` 在 light 下实测 1.87 / 1.69 / 2.64:1（机制见 §5.4 补充；dark 下同一批类是 7.08 / 7.76 / 4.97:1，没问题）。候选解法：**(a)** 在 `app.css` 的 light 主题块里把这三个语义色压暗（与 §3.3「两个主题的品牌色必须都微调」同源，改动最小、不动类名）；**(b)** 状态标记改 `badge-outline` 或实心；**(c)** 接受现状。**未定，等用户拍** —— 这动的是 §2.4/§3.4 的状态色语义，属视觉决策不是技术决策。<br>**2026-09-21 已定：保留现状（=候选 (c)）**。用户原话"停，不用计算了，保留"，当场把注意力转到"左下角两个 admin"（→ §4.5）。`app.css` 的两个主题块与所有 `badge-soft badge-*` 类名**均未改动**；重新打开的时机仍是做视觉验收 | 下次做视觉验收时一并定 |
| 10 | 侧栏底部那一行现在报的是**后端自述**（`GET /api/status`，见 §4.5）。等调度循环真接上时，`schedulerRunning` 要改成**从运行时取**而不是常量，而那一行的「定时调度未启用」必须一起消失 —— 二者任漏其一就是说谎 | 接调度器那天（`docs/status.md` 运行时层那条） |

> 本文与 [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) 的分工：
> **方案定"用什么技术、有哪些页面"；本文定"长什么样、怎么写"。**
> 两者冲突时，技术选型以方案为准，视觉与交互以本文为准。
