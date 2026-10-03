/**
 * lite 的编译器：TSX → 目标形态。结构**参考 Solid 的 `babel-plugin-jsx-dom-expressions`**
 * （把它的真实输出调出来逐条对过，见 docs/web/lite-framework.md §4）。
 *
 * ```tsx
 * const Row = (props: { label: string; n: number }) => (
 *   <li class={props.n % 2 ? 'odd' : 'even'}>{props.label}:{props.n}</li>
 * )
 * ```
 * 编译成（就是 Solid 那套形状）：
 *
 * ```js
 * const _t0 = template('<li>:')
 * const Row = (props) => (() => {
 *   const _n0 = _t0(), _n1 = _n0.childNodes[1]
 *   setNodes(_n0, () => props.label, _n1)   // 插在静态的 ":" 之前
 *   setNodes(_n0, () => props.n, null)      // 末尾 ⇒ 锚点 null
 *   effect(() => setClass(_n0, props.n % 2 ? 'odd' : 'even'))
 *   return _n0
 * })()
 * ```
 *
 * 从 Solid 那里学到的四条（都是实打实省字节/省 DOM 操作的）：
 *
 * 1. **模板里折进静态 HTML**（`template()` + 克隆）—— 这部分照 Solid。
 *    ⚠ **锚点不是 Solid 那一套**：Solid 拿"后面那个静态兄弟"当锚点、末尾则 `null`；
 *    这里给**每个动态子节点各配一个 `<!---->` 占位注释**并拿它当锚点（见上面的产物）。
 *    理由是实测出来的坑（docs/web/lite-postmortem.md §11.2 第 6 条）：两个相邻的动态子节点
 *    若都没有静态兄弟可当锚点，就都退化成"追加到末尾"，**顺序取决于谁的 effect 后跑** ——
 *    而其中一个的数据是异步来的 ⇒ 界面顺序随机。占位注释把位置钉死，重跑多少次都在同一处。
 *    ⚠ 也正因为锚点是**我们自己的节点引用**，它会被别处摘走 ⇒ `insert()` 里有护栏，
 *    见 docs/web/lite-postmortem.md §12.3 / §12.5。
 * 2. **每个 JSX 表达式编译成一个立即执行的箭头函数** ⇒ 语句都待在自己的块里，
 *    编译器不必往宿主函数里插语句。
 * 3. **动态 prop 用 getter**（Solid 与 Vapor 在这里不同：Solid 用 getter、Vapor 用函数）。
 *    getter 的好处是运行期零魔法 —— 不用去猜"这个值是 prop 还是回调函数"。
 * 4. **条件分支不需要专门的原语**：`{c ? <A/> : null}` 就是
 *    `setNodes(parent, () => c ? A() : null, anchor)`。所以运行时里没有 `createIf`。
 *
 * 与 Solid 的**两处有意不同**：
 * * `.map()` 编译成键控的 `createFor`（Solid 的 `.map` 是朴素数组 diff，键控要写 `<For>`）；
 *   本项目的源码写的是 `.map` + `key=`。这里**不要求回调返回 JSX 字面量**：
 *   `(item) => navLink(item.id)`、带 `return` 的块体一样进 `createFor`（拿不到 `key`
 *   就退回索引键）。少了这一步，导航那一列每次切页都会整块重建，而它本来只需改 9 个 class。
 * * 事件不委托（Solid 用 `$$click` + `delegateEvents`）：见 docs §8，那是下一步的候选。
 *
 * ⚠ 只支持 `docs/web/lite-framework.md` §2 普查到的语法子集。**拦不拦分两类，别看错**（都是实测）：
 *
 * * **直接抛错**：指令式属性（`v-if` / `vIf` 这种形状）、`.map` 之外的 `key`、
 *   组件上的 spread、空元素带子节点、块体 map 拿不到返回值那类写法 —— 见
 *   `lite/regress/compiler.mjs` 的 12 条负例；
 * * **静默编错**（不抛，产物是错的）：`class` 数组 / `class` 对象 / `style` 对象 /
 *   `ref=` / 小写 `onclick=`。例如 `class={[a,b]}` 原样进 `setClass` ⇒ `String(数组)`，
 *   而 `ref={el}` 变成 `setAttr(el, "ref", fn)`。
 *   ⚠ **这五条不检测**：全仓 0 处使用，加拦截等于为"不存在的需求"付字节，
 *   还会误伤同名的普通属性（`ref` 在 HTML 里本来就是合法属性名）。
 *   别指望类型检查兜住 —— jsx 属性是 `any`（`lite/jsx.d.ts`）。
 *
 * ⚠⚠ **`key` 写在 `.map` 之外的位置也直接抛错。** 它是从 React/Vue 带来的肌肉记忆，
 * 而这里没有"按 key 决定复不复用"的那一次 diff —— `key` 只有一个消费者：`.map` 那一列的
 * `createFor`。写在别处（条件分支、普通元素）会被**静默丢掉**，于是攒出三层错位：
 * 代码里写着 `key`、注释解释"key 不能省"、界面上它其实什么都没做。
 * 编排页的步骤列表曾因此"点了不刷新"（docs/review/review-qwen3.8f-260929.md §1.2）。
 * 宁可编译不过，也别让人带着一个假的安全走动。
 */

import * as ast from './ast.ts'

export interface CompileResult {
  code: string
  helpers: Set<string>
}

/**
 * 这些属性改 property 比改标签更对（布尔类）。
 *
 * ⚠ `value` **不在**这里：它走 `setValue`，因为 Vapor 的 `setValue` 是
 * **property 与 attribute 都写**（见 `dom.ts` 里那段注释），只写 property 的话
 * 只读展示框（`<input value={x} readonly>`）在 DOM 里看不到值。
 */
const PROPS = new Set(['checked', 'selected', 'disabled', 'open', 'multiple', 'readonly', 'required', 'muted'])

const VOID = new Set(['img', 'br', 'hr', 'input', 'meta', 'link', 'source', 'area', 'base', 'col', 'embed', 'track', 'wbr', 'path', 'circle', 'rect', 'line', 'polyline', 'polygon', 'use', 'stop', 'ellipse'])

const escText = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
const escAttr = (s: string) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;')

/** 属性值可以**折进模板串**的字面量：字符串 / 数字（`false`/`null` 不行，见 `html()` 里的注释）。 */
function foldLiteral(e: ast.Expression): string | undefined {
  if (ast.isStringLiteral(e) || ast.isNoSubstitutionTemplateLiteral(e)) return e.text
  if (ast.isNumericLiteral(e)) return e.text
  return undefined
}

/**
 * JSX 文本的空白语义 —— **与 `vue-jsx-vapor` 的实际产物逐字符对齐**（不是照 Babel）。
 *
 * 这两条是从**真实插件的输出**反推出来的，不是猜的：把各种形状的 JSX 过一遍
 * `vue-jsx-vapor/vite`，看它生成的 `template("…")` 字符串：
 *
 * | 源码文本 | Vue 生成 |
 * |---|---|
 * | `\n    第一行\n    第二行\n  ` | `第一行\n    第二行`（**行间换行与缩进原样保留**） |
 * | `\n    正文\n  ` | `正文` |
 * | `，\n    它们…\n  ` | `，\n    它们…` |
 * | `a  <b>c</b>  d`（单行） | `a  <b>c</b>  d`（**单行原样，不折叠**） |
 * | `  `（单行纯空白） | ` `（折叠成一个空格） |
 * | `\n    <b>x</b>\n    <i>y</i>\n  ` | `<b>x</b><i>y</i>`（纯空白跨行**整段丢掉**） |
 *
 * ⇒ 规则：
 * 1. **整段纯空白**：不含换行 ⇒ 一个空格（Vue 把 `  ` 折成 ` `）；含换行 ⇒ 整段丢掉；
 * 2. 否则**只裁"含换行的"首尾空白段**：不含换行的那点空格是**行内空格**，Vue 原样保留
 *    （`\n  耗时 {x}` 里的 `耗时 ` 后面那个空格就属于这种 —— 一开始按 Babel 的算法
 *    把它 trim 掉了，才在这里露馅）。
 *
 * ⚠ 第一版按 Babel 的 `cleanJSXElementLiteralChild` 写（行间补空格、整体 trim），
 * 与 Vue 差一两个空格、甚至丢掉换行 —— 逐字符比对（当时用 `lite/regress/compare.mjs`，
 * 该脚本已随拆桥删除）抓出来的。
 */
function jsxText(raw: string): string {
  // 纯空白（含跨行）：跨行的整段丢掉，单行的折成一个空格
  if (!/[^ \t\r\n]/.test(raw)) return raw.includes('\n') ? '' : ' '
  let s = raw
  const head = /^[ \t\r\n]+/.exec(s)?.[0] ?? ''
  if (head.includes('\n')) s = s.slice(head.length)
  const tail = /[ \t\r\n]+$/.exec(s)?.[0] ?? ''
  if (tail.includes('\n')) s = s.slice(0, s.length - tail.length)
  return s
}

/** 到某个节点的索引路径（相对本次编译的根）。 */
type Path = number[]

type Binding =
  | { kind: 'attr'; at: Path; name: string; expr: string; hoist: boolean }
  | { kind: 'event'; at: Path; name: string; expr: string }
  | { kind: 'spread'; at: Path; expr: string }
  | { kind: 'nodes'; parent: Path; anchor: Path | null; expr: string }
  | {
      kind: 'for'
      parent: Path
      anchor: Path | null
      list: string
      params: [string, string]
      item: string
      key?: string
      /** 渲染体读到了索引参数 ⇒ 位置是内容的一部分，重排时必须重建那一行（见 `createFor`）。 */
      positional: boolean
    }

interface Built {
  html: string
  bindings: Binding[]
}

class Compiler {
  readonly helpers = new Set<string>()
  private templates: string[] = []
  /**
   * 被 `.map()` 认领的 `key` 属性节点。
   *
   * 只有在这里登记过的才算数：登记发生在 `.map` 那条分支，消费发生在 `html()`/`component()`
   * 走到那个元素时。没登记过的 `key` 一律抛错（见文件头 ⚠⚠ 那段）。
   */
  private readonly mapKeys = new Set<ast.JsxAttribute>()

  constructor(
    private readonly sf: ast.SourceFile,
    private readonly src: string,
    private readonly runtime: string,
  ) {}

  /** 带位置的抛错。没有位置的编译错误等于让人回去 grep 一遍文件。 */
  private fail(msg: string, node?: ast.Node): never {
    // ⚠ 取"文件名"只能用字符串切：原来是 `ts.pathBasename`（tsc 的**内部** API，
    // 编译器包里 import 到的是 undefined，会抛 "pathBasename is not a function"，
    // 把真正要报的那句话顶掉）；换到 oxc 之后同样没有现成的 basename 可用 ⇒ 切法原样留着。
    const at = node
      ? `${this.sf.fileName.replace(/^.*[\\/]/, '')}:${ast.lineOf(this.sf, ast.getStart(node)) + 1} `
      : ''
    throw new Error(`[lite] ${at}${msg}`)
  }

  /**
   * Vue 的模板指令在 JSX 里**不会报错，只会变成一个没人理的属性**：
   * `v-if="ok"` 走静态属性那条路 ⇒ 条件根本没生效，而类型检查与构建全绿。
   * 所以在认属性名的两个入口（组件 / 元素）各拦一次。
   *
   * ⚠ 只管 `vIf`（驼峰）与 `v-if`（连字符）——`@click` 那种带 `@` 的属性名 **TSX 本身就解析不过**，
   * 由 `parseError` 拦。也**不带** `:xxx`：`xmlns:xlink` 那类带冒号的命名空间属性是真 SVG 属性，
   * 而 `data-*` / `aria-*` 更不该管。
   */
  private checkDirective(attr: ast.JsxAttribute, name: string): void {
    if (/^v[A-Z]/.test(name) || /^v-/.test(name)) {
      this.fail(`不支持指令式属性：${name} —— 本框架没有模板指令。条件渲染写 \`cond ? <…/> : null\`，列表写 \`list.value.map(…)\`，事件写 \`onClick={…}\``, attr)
    }
  }

  /** 处理 `key`：被 `.map` 认领过就放过（它不进 DOM），否则抛错。 */
  private claimKey(attr: ast.JsxAttribute, name: string): void {
    if (name !== 'key') return
    if (!this.mapKeys.delete(attr)) {
      this.fail('这里的 `key` 什么都不做：它只对 `.map()` 返回的那个元素有意义（交给 createFor 当复用键）。条件分支/普通元素上请直接删掉 —— 本框架没有"按 key 决定复不复用"的那一次 diff，不会因为它换实例', attr)
    }
  }

  /**
   * 记下一个 helper，返回**生成代码里用的名字**（带 `_$` 前缀）。
   *
   * 前缀是必须的：业务文件自己也会 `import { batch } from 'lite'`（比如事件处理器里
   * 手写 `batch(...)`），如果生成代码直接用 `batch`，那两行 import 就会撞名报
   * `Identifier 'batch' has already been declared`。所以生成代码用别名、import 也写别名。
   */
  private h(name: string): string {
    this.helpers.add(name)
    return `_$${name}`
  }

  private srcOf(node: ast.Node): string {
    return this.src.slice(ast.getStart(node), ast.getEnd(node))
  }

  private tpl(html: string): string {
    const name = `_t${this.templates.length}`
    this.templates.push(`const ${name} = ${this.h('template')}(${JSON.stringify(html)})`)
    return name
  }

  // ── 根 ───────────────────────────────────────────────────────────────────
  /** 编译一个 JSX 表达式：语句封在自己的块里，返回它的值。 */
  root(node: ast.JsxElement | ast.JsxSelfClosingElement | ast.JsxFragment): string {
    const stmts: string[] = []
    const value = this.value(node, stmts)
    return `(() => {\n${stmts.map((s) => '  ' + s).join('\n')}\n  return ${value}\n})()`
  }

  /**
   * 与 `value` 相同，但**语句封在自己的块里**。
   *
   * 嵌套 JSX（组件插槽里的元素、列表项、条件分支）必须走这个入口：否则内层元素
   * 生成的 `const _n0 = …` 会和外层撞名（实测报 `Identifier '_n0' has already been declared`）。
   */
  private valueIsolated(node: ast.JsxElement | ast.JsxSelfClosingElement | ast.JsxFragment): string {
    const local: string[] = []
    const v = this.value(node, local)
    if (!local.length) return v
    return `(() => {\n${local.map((l) => '  ' + l).join('\n')}\n  return ${v}\n})()`
  }

  /** 任意 JSX 节点 → 一个表达式的值（元素 / 组件 / 片段）。 */
  private value(node: ast.JsxElement | ast.JsxSelfClosingElement | ast.JsxFragment, stmts: string[]): string {
    if (ast.isJsxFragment(node)) {
      // ⚠ 片段成员的动态部分必须包成惰性槽：片段没有父节点，绑定只能等插入后再建
      const parts = node.children.map((c) => this.childValue(c, true)).filter((v): v is string => !!v)
      return parts.length === 0 ? 'null' : parts.length === 1 ? parts[0] : `[${parts.join(', ')}]`
    }
    const tag = this.tagOf(node)
    return /^[A-Z]/.test(tag) || tag.includes('.') ? this.component(node) : this.element(node, stmts)
  }

  private tagOf(node: ast.JsxElement | ast.JsxSelfClosingElement): string {
    return this.srcOf(ast.isJsxElement(node) ? node.openingElement.tagName : node.tagName)
  }

  // ── 组件 ─────────────────────────────────────────────────────────────────
  private component(node: ast.JsxElement | ast.JsxSelfClosingElement): string {
    const opening = ast.isJsxElement(node) ? node.openingElement : node
    const props: string[] = []
    for (const attr of opening.attributes.properties) {
      if (ast.isJsxSpreadAttribute(attr)) throw new Error('组件上的 {...spread} 未支持（项目里没有这种写法）')
      const name = this.srcOf(attr.name)
      this.checkDirective(attr, name)
      this.claimKey(attr, name) // `.map` 认领过的 key 由 createFor 取走；没认领过的直接抛错
      if (name === 'key') continue
      const init = attr.initializer
      if (!init) {
        props.push(`${name}: true`)
        continue
      }
      if (ast.isStringLiteral(init)) {
        props.push(`${JSON.stringify(name)}: ${JSON.stringify(init.text)}`)
        continue
      }
      if (ast.isJsxExpression(init) && init.expression) {
        // 动态 prop = getter（同 Solid）。⚠ 值里可能直接是 JSX
        // （`<EmptyState action={<button …/>} />` 在本项目里就有），必须递归编译掉
        props.push(`get ${JSON.stringify(name)}() { return ${this.exprWithJsx(init.expression)} }`)
        continue
      }
      throw new Error(`不支持的组件属性：${name}`)
    }
    const slots: string[] = []
    if (ast.isJsxElement(node) && node.children.length) {
      const parts = node.children.map((c) => this.childValue(c)).filter((v): v is string => !!v)
      slots.push(`default: () => ${parts.length === 0 ? 'null' : parts.length === 1 ? parts[0] : `[${parts.join(', ')}]`}`)
    }
    return `${this.h('createComponent')}(${this.tagOf(node)}, { ${props.join(', ')} }${slots.length ? `, { ${slots.join(', ')} }` : ''})`
  }

  /**
   * 复制一段表达式的源码，但把它内部的 JSX 全部递归编译掉。
   *
   * ⚠ 少了这一步会**静默漏掉 JSX**：`<>…{cond ? null : <div>…</div>}…</>` 这种
   * 片段成员、或数组字面量里的三元分支，走到"普通表达式"路径时若原样复制，
   * 产物里就留着 JSX ⇒ 下游解析器直接报 `Unexpected JSX expression`
   * （实测 app.tsx 的根返回就是这个形状）。
   */
  private exprWithJsx(node: ast.Node): string {
    const base = ast.getStart(node)
    const edits: { start: number; end: number; text: string }[] = []
    const walk = (n: ast.Node, inJsx: boolean) => {
      const isJsx = ast.isJsxElement(n) || ast.isJsxSelfClosingElement(n) || ast.isJsxFragment(n)
      if (isJsx && !inJsx) {
        edits.push({ start: ast.getStart(n) - base, end: ast.getEnd(n) - base, text: this.valueIsolated(n) })
        return
      }
      ast.forEachChild(n, (c) => walk(c, inJsx || isJsx))
    }
    walk(node, false)
    if (!edits.length) return this.srcOf(node)
    let out = this.srcOf(node)
    for (const e of edits.sort((a, b) => b.start - a.start)) out = out.slice(0, e.start) + e.text + out.slice(e.end)
    return out
  }

  /**
   * 组件子节点 / 片段成员：拿一个"值"（文本要自己造节点）。
   * `slot = true` 时把动态成员包成惰性槽（只有片段需要，见 runtime 的 `lazySlot`）。
   */
  private childValue(child: ast.JsxChild, slot = false): string | undefined {
    if (ast.isJsxText(child)) {
      const t = jsxText(child.text)
      return t ? `document.createTextNode(${JSON.stringify(t)})` : undefined
    }
    if (ast.isJsxExpression(child)) {
      if (!child.expression) return undefined
      if (isNullish(child.expression)) return undefined
      const e = this.exprWithJsx(child.expression)
      return slot ? `${this.h('lazySlot')}(() => ${e})` : e
    }
    return this.valueIsolated(child)
  }

  // ── 内置元素（静态 HTML + 绑定表）─────────────────────────────────────────
  private element(node: ast.JsxElement | ast.JsxSelfClosingElement, stmts: string[]): string {
    const built = this.html(node, stmts, [])
    const tpl = this.tpl(built.html)
    const rootVar = `_n0`
    stmts.push(...this.materialize(built.bindings, tpl, rootVar))
    stmts.push(...this.emit(built.bindings, rootVar))
    return rootVar
  }

  /**
   * 生成静态 HTML 与绑定表。动态位置**不产出任何节点** —— 插入时以"后面那个静态兄弟"
   * 为锚点（Solid 的做法），所以 HTML 里不会多出占位节点。
   */
  private html(node: ast.JsxElement | ast.JsxSelfClosingElement, stmts: string[], base: Path): Built {
    const opening = ast.isJsxElement(node) ? node.openingElement : node
    const tag = this.tagOf(node)
    if (/^[A-Z]/.test(tag) || tag.includes('.')) throw new Error('组件只能出现在动态子节点位置')
    const attrs: string[] = []
    const own: Binding[] = []
    for (const attr of opening.attributes.properties) {
      if (ast.isJsxSpreadAttribute(attr)) {
        own.push({ kind: 'spread', at: [], expr: this.srcOf(attr.expression) })
        continue
      }
      const name = this.srcOf(attr.name)
      this.checkDirective(attr, name)
      // `key` 是框架的东西，**不能落到 DOM 上**：它由外层 `createFor` 取走（`.map()` 那条路）。
      // 写在别的位置会被静默丢掉 ⇒ 直接抛错，理由见文件头 ⚠⚠。
      this.claimKey(attr, name)
      if (name === 'key') continue
      const init = attr.initializer
      if (!init) {
        attrs.push(name)
        continue
      }
      if (ast.isStringLiteral(init)) {
        attrs.push(`${name}="${escAttr(init.text)}"`)
        continue
      }
      if (ast.isJsxExpression(init) && init.expression) {
        /**
         * 字符串/数字字面量**直接折进模板串**（Vapor 也这么做）：既少一次运行期写入，
         * 又让属性顺序与 Vue 一致 —— 静态属性在模板里按源码顺序排，动态属性由 setter
         * 在之后追加，`rows={3}` 折不折决定它排第 2 还是排最后（回归脚本能逐字符看出来）。
         *
         * ⚠ `false`/`null` 不折：布尔属性写进 HTML 是"存在即为真"，
         * `disabled={false}` 折成 `disabled="false"` 会把它**打开**（语义反了）。
         */
        const folded = foldLiteral(init.expression)
        if (folded !== undefined) {
          attrs.push(`${name}="${escAttr(folded)}"`)
          continue
        }
        const expr = this.exprWithJsx(init.expression)
        if (name.length > 2 && name.startsWith('on') && /^[A-Z]/.test(name[2])) {
          own.push({ kind: 'event', at: [], name: name.slice(2).toLowerCase(), expr })
        } else {
          own.push({ kind: 'attr', at: [], name, expr, hoist: isLiteral(init.expression) })
        }
        continue
      }
      throw new Error(`不支持的属性：${name}`)
    }

    const children = ast.isJsxElement(node) ? node.children : []
    const parts: string[] = []
    const childBindings: Binding[] = []
    /** 本元素里的动态子节点（占位注释 + 铺节点），登记完统一追加到 `childBindings`。 */
    const dyns: Extract<Binding, { kind: 'nodes' | 'for' }>[] = []
    /**
     * `dom` 是**真正产出节点的下标** —— 路径 `childNodes[dom]` 用的是它。
     *
     * ⚠ 第一版把"动态槽"也当成一个序列位置去数，于是 `<li>{label}:{n}</li>` 里静态的 ":"
     * 被标成 `childNodes[1]`（DOM 里它其实是 `childNodes[0]`）⇒ 锚点错位，渲染出 `:a1`。
     */
    let dom = 0
    /**
     * 上一个进模板的节点**是不是文本**。
     *
     * 相邻两段文本在 HTML 解析后**合成一个节点**，所以 `childNodes` 下标要按合并后的数：
     * `<span>将结束{' '}<b>…</b></span>` 里两段文本只有一个文本节点，`<b>` 是
     * `childNodes[1]`。按"每段文本各占一位"数就会算成 `[2]` —— 运行期拿一个文本节点
     * 当父节点去 `insertBefore`，抛 `HierarchyRequestError: This node type does not
     * support this method`（设置页的 `confirm-logout` 弹窗就是这么炸的）。
     */
    let textTail = false

    /**
     * 登记一个动态子节点：**它自己带一个 `<!---->` 占位注释**，这个注释就是它的锚点。
     *
     * ⚠⚠ 占位是**必须**的，不能靠"后面那个静态兄弟"当锚点（第一版就是那么做的）：
     * 动态兄弟**后面还有动态兄弟**时，后者没有静态兄弟可指，两者的锚点都是 `null`
     * （= 追加到末尾），于是最终顺序取决于**谁的 effect 后重跑**，而不是文档顺序 ——
     * 抽屉页脚那两行（"最近执行 —" 与 "以 admin 的身份"）就是这么对调的：
     * 一个先渲染、另一个晚一步（`backend` 是异步来的）后追加，跑到前面去了。
     * 占位注释把这个槽的位置**钉死**，重跑多少次都插在同一个地方。
     *
     * 顺带解决文本合并：`<span>a{dyn}b</span>` 的两段文本被注释分开，不再是同一个节点。
     * 抓 DOM 时本来就去注释（当时由 `regress/compare.mjs` 做，已删），所以不影响逐字符比对。
     */
    const dynChild = (binding: Extract<Binding, { kind: 'nodes' | 'for' }>): void => {
      parts.push('<!---->')
      binding.anchor = [...base, dom]
      dom++
      dyns.push(binding)
      textTail = false
    }

    for (const child of children) {
      if (ast.isJsxText(child)) {
        const t = jsxText(child.text)
        if (!t) continue
        parts.push(escText(t))
        // 与上一段文本合并 ⇒ 不再占一个下标
        if (!textTail) dom++
        textTail = true
        continue
      }
      if (ast.isJsxExpression(child)) {
        if (!child.expression) continue
        const lit = literalHtml(child.expression)
        if (lit !== undefined) {
          if (lit) {
            parts.push(lit)
            if (!textTail) dom++
            textTail = true
          }
          continue
        }
        dynChild(this.dynamic(child, stmts, [...base, dom]))
        continue
      }
      // 组件 / 片段：不是静态结构 ⇒ 走"动态插入"（Solid 也是 insert(parent, createComponent(…), anchor)）。
      // 这是本项目最常见的写法之一：`<Panel>…</Panel>` 直接放在元素里。
      if (ast.isJsxFragment(child) || /^[A-Z]/.test(this.tagOf(child)) || this.tagOf(child).includes('.')) {
        dynChild({ kind: 'nodes', parent: [...base], anchor: null, expr: `() => ${this.valueIsolated(child)}` })
        continue
      }
      // 静态元素：递归（它的绑定路径以本元素的 DOM 下标为前缀）
      const sub = this.html(child, stmts, [...base, dom])
      parts.push(sub.html)
      childBindings.push(...sub.bindings)
      dom++
      textTail = false
    }

    childBindings.push(...dyns)

    const inner = parts.join('')
    const open = `<${tag}${attrs.length ? ' ' + attrs.join(' ') : ''}>`
    /**
     * ⚠⚠ **HTML 里没有"自闭合"这回事**（除了空元素）。
     *
     * JSX 允许把任意元素写成 `<label ... />`，但 `<label>` 不是空元素 —— 生成
     * `<label ...>` 交给 HTML 解析器，它会把**后面的兄弟节点吞成 label 的子节点**。
     * 后果不是"报错看得见"：编译期算好的 `childNodes[i]` 路径全部错位一格，
     * 运行期在 effect 里抛 `Cannot read properties of undefined (reading 'firstChild')`，
     * 而应用那边（`authState` 的写入）正好在 `try/catch` 里 ⇒ 错误被吞掉、
     * 页面整个空白、`window.onerror` 一声不响。（`drawer-side` 里的
     * `<label ... />` 就是这么把整个应用打黑的。）
     */
    if (VOID.has(tag) && children.length) {
      throw new Error(`lite 编译器不支持的写法：空元素 <${tag}> 不能带子节点（生成 HTML 无法表达）`)
    }
    const html = VOID.has(tag) ? open : `${open}${inner}</${tag}>`
    // 自身绑定：路径为 base（相对整棵树的根，由调用方补前缀）
    const ownShifted = own.map((b) => ({ ...b, at: [...base] }) as Binding)
    /**
     * ⚠⚠ `<select>` 的 `value` 必须**等选项建出来之后再写**。
     *
     * `<select>` 没有"哪个选项被选中"的独立状态：`value` 是**在选项集合上算出来的**。
     * 先写 `value=''`（没有任何选项匹配空串）时属性的值确实是 `''`，但随后插入 `<option>`
     * 会触发浏览器的**自动选中**：Chrome 实测「反序插两个 option」的结果是**最后一个被选中**
     * （`append(b); insertBefore(a, b)` ⇒ `value === 'b'`），于是下拉框自己跳到了别处。
     *
     * Vapor 也是这么排的 —— 任务页那个下拉框它生成的是
     * `Z(t, () => r.value.map(…))`（先建选项）然后才 `R(() => H(e, i.value))`（再写值）。
     * 交互回归（`regress/interact.mjs`）拿"切到任务页之后下拉框的 property"比出来的。
     */
    const isValueAttr = (b: Binding) => b.kind === 'attr' && b.name === 'value'
    const late = tag === 'select' ? ownShifted.filter(isValueAttr) : []
    const early = ownShifted.filter((b) => !late.includes(b))
    return { html, bindings: [...early, ...childBindings, ...late] }
  }

  /**
   * 拆开 `.map()` 的回调：参数名 + **返回的那一行**。
   *
   * 只认**表达式体** `(x) => <Row …/>`。
   *
   * ⚠⚠ 块体（`xs.map((x) => { const a = …; return <Row/> })`）**故意不认**，走通用路径。
   * 理由是响应式的边界，不是"懒得做"：列表行只在建那一行的时候跑一次回调，
   * 而块体里 `return` 之前那几句（典型：`const active = page.value === id`）是在
   * **任何 effect 之外**读信号的 —— 信号变了不会重跑它。列表源又是常量
   * （`DOCK_IDS` 这种）时，那一行就**永久停在建出来那一刻的值**上。
   * 表达式体没这个问题：它的每个动态值都编译成绑定 effect，信号是**在 effect 里**读的。
   *
   * 其余形状（`xs.map(f).join(',')` 这种不返回 JSX 的、解构参数、参数多于两个）同样返回
   * `undefined` ⇒ 走通用路径（整表重建，语义仍然对）。
   */
  private mapCallback(fn: ast.Expression): { fn: ast.ArrowFunction | ast.FunctionExpression; expr: ast.Expression } | undefined {
    if (!(ast.isArrowFunction(fn) || ast.isFunctionExpression(fn))) return undefined
    const ps = fn.parameters
    if (ps.length < 1 || ps.length > 2 || ps.some((p) => !ast.isIdentifier(p.name))) return undefined
    // 块体一律不认（见上面那段）
    if (ast.isBlock(fn.body)) return undefined
    const e = ast.isParenthesizedExpression(fn.body) ? fn.body.expression : fn.body
    return { fn, expr: e }
  }

  /** 一个动态子节点的绑定。返回值只可能是 `nodes` / `for`（两者都是"往父节点里铺一批节点"）。 */
  private dynamic(child: ast.JsxExpression, stmts: string[], at: Path): Extract<Binding, { kind: 'nodes' | 'for' }> {
    const e = child.expression as ast.Expression
    const parent = at.slice(0, -1)
    // {cond ? <A/> : null} ⇒ setNodes(parent, () => cond ? A() : null, anchor)（Solid 同款，不需要 createIf）
    // ⚠ 只有**两个分支都是 JSX 或 null** 时才走这条精确路径；否则落到下面的通用路径
    // （分支是普通表达式的三元，项目里也有 —— 那里靠 exprWithJsx 递归处理 JSX）
    const branchOk = (x: ast.Expression) => isNullish(x) || ast.isJsxElement(x) || ast.isJsxSelfClosingElement(x) || ast.isJsxFragment(x)
    if (ast.isConditionalExpression(e) && hasJsx(e) && branchOk(e.whenTrue) && branchOk(e.whenFalse)) {
      const cond = this.srcOf(e.condition)
      const a = isNullish(e.whenTrue) ? 'null' : this.branch(e.whenTrue)
      const b = isNullish(e.whenFalse) ? 'null' : this.branch(e.whenFalse)
      return { kind: 'nodes', parent, anchor: null, expr: `() => ${cond} ? ${a} : ${b}` }
    }
    // {list.map((x) => <Row/>)} ⇒ createFor
    if (ast.isCallExpression(e) && ast.isPropertyAccessExpression(e.expression) && e.expression.name.text === 'map' && e.arguments.length === 1) {
      const cb = this.mapCallback(e.arguments[0])
      // 回调不返回 JSX 的 `.map`（例如 `{xs.map(f).join(',')}`）不是列表，交给普通表达式路径
      if (cb && isJsxNode(cb.expr)) {
        // 两个参数名：用户只写了一个就补一个 `_i`（createFor 会给 index，key/render 都能用）
        const a = this.srcOf(cb.fn.parameters[0].name)
        const b = cb.fn.parameters.length > 1 ? this.srcOf(cb.fn.parameters[1].name) : '_i'
        let key: string | undefined
        if (!ast.isJsxFragment(cb.expr)) {
          const opening = ast.isJsxElement(cb.expr) ? cb.expr.openingElement : cb.expr
          for (const attr of opening.attributes.properties) {
            if (ast.isJsxAttribute(attr) && this.srcOf(attr.name) === 'key' && attr.initializer && ast.isJsxExpression(attr.initializer) && attr.initializer.expression) {
              key = this.srcOf(attr.initializer.expression)
              // ⚠ 登记它：随后编译这一行时 `html()`/`component()` 会来认领这个 `key`，
              // 认领不到就抛错（见 `claimKey`）。没登记 = 这根本不是 `.map` 直接返回的那个元素。
              this.mapKeys.add(attr)
            }
          }
        }
        // 只编一次：`item` 与 `positional` 的判断共用同一棵返回的 JSX
        const item = this.root(cb.expr)
        return {
          kind: 'for',
          parent,
          anchor: null,
          list: this.srcOf((e.expression as ast.PropertyAccessExpression).expression),
          params: [a, b],
          item,
          key,
          // 渲染体（连 `onClick={() => move(i)}` 这种嵌套箭头）读了索引 ⇒ 那一行按位置重建
          positional: readsIdent(cb.expr, b),
        }
      }
    }
    void stmts
    return { kind: 'nodes', parent, anchor: null, expr: `() => ${this.exprWithJsx(e)}` }
  }

  private branch(e: ast.Expression): string {
    if (ast.isJsxElement(e) || ast.isJsxSelfClosingElement(e) || ast.isJsxFragment(e)) return this.root(e)
    throw new Error('条件分支只支持 JSX 或 null')
  }

  // ── 代码生成 ─────────────────────────────────────────────────────────────
  /** 给绑定表里出现过的每个路径分配变量，并按文档顺序物化。 */
  private materialize(bindings: Binding[], tpl: string, root: string): string[] {
    const paths = new Map<string, Path>()
    const add = (p: Path | null) => {
      if (p) paths.set(p.join('/'), p)
    }
    for (const b of bindings) {
      if (b.kind === 'attr' || b.kind === 'event' || b.kind === 'spread') add(b.at)
      else {
        add(b.parent)
        add(b.anchor)
      }
    }
    // ⚠ 必须把**所有前缀**也物化：绑定只引用"叶"路径，但中间那一跳（父节点）
    // 得先有变量才能取到它 —— 否则父变量会静默回退成根节点，
    // 生成 `_n0.childNodes[1]` 这种看着对、其实层次错的路径（实测就是这个症状）。
    for (const p of [...paths.values()]) {
      for (let i = 1; i < p.length; i++) {
        const prefix = p.slice(0, i)
        paths.set(prefix.join('/'), prefix)
      }
    }
    const sorted = [...paths.values()].sort((a, b) => a.length - b.length || a.join('/').localeCompare(b.join('/')))
    const lines: string[] = []
    const vars = new Map<string, string>()
    vars.set('', root)
    let n = 0
    const decls: string[] = [`const ${root} = ${tpl}()`]
    for (const p of sorted) {
      if (p.length === 0) continue
      const name = `_n${++n}`
      const parentPath = p.slice(0, -1)
      const parentVar = vars.get(parentPath.join('/')) ?? root
      const idx = p[p.length - 1]
      decls.push(`      ${name} = ${parentVar}${idx === 0 ? '.firstChild' : `.childNodes[${idx}]`}`)
      vars.set(p.join('/'), name)
    }
    this.varOf = (p) => (p.length === 0 ? root : vars.get(p.join('/')) ?? root)
    if (decls.length === 1) return decls
    // 物化语句合并成一条 const/var 声明（与 Solid 的输出同形）
    const merged = decls[0] + decls.slice(1).map((d) => ',' + d.trim()).join('')
    lines.push(merged)
    return lines
  }

  private varOf: (p: Path) => string = () => '_n0'

  private emit(bindings: Binding[], _root: string): string[] {
    const out: string[] = []
    const v = (p: Path) => this.varOf(p)
    for (const b of bindings) {
      switch (b.kind) {
        case 'attr': {
          // `class` / `value` 是 `(node, v)` 两参数，其余是 `(node, name, v)`
          const two = b.name === 'class' || b.name === 'value'
          const fn = this.h(b.name === 'class' ? 'setClass' : b.name === 'value' ? 'setValue' : PROPS.has(b.name) ? 'setProp' : 'setAttr')
          // ⚠ `(node, name, v)` 那三个参数里，**名字不能漏**。漏了不会报错：
          // `setAttr(el, true)` 里的 `true` 被当成**属性名**，而值成了 undefined
          // ⇒ 走 removeAttribute 分支，静默什么都不做（`aria-current`、`aria-expanded`
          // 这类全在这里，页面上"看着对、就是没这个属性"）。
          const call = two ? `${fn}(${v(b.at)}, ${b.expr})` : `${fn}(${v(b.at)}, ${JSON.stringify(b.name)}, ${b.expr})`
          // 字面量属性直接写一次；其余包 effect（值变了才写 DOM）
          out.push(b.hoist ? call : `${this.h('effect')}(() => ${call})`)
          break
        }
        case 'event':
          // 事件处理器自动包 batch：本实现是同步刷新，不包的话连续写多个信号会刷多遍
          out.push(`${this.h('on')}(${v(b.at)}, ${JSON.stringify(b.name)}, (e) => ${this.h('batch')}(() => (${b.expr})(e)))`)
          break
        case 'spread':
          out.push(`${this.h('spread')}(${v(b.at)}, ${b.expr})`)
          break
        case 'nodes':
          out.push(`${this.h('setNodes')}(${v(b.parent)}, ${b.expr}, ${b.anchor ? v(b.anchor) : 'null'})`)
          break
        case 'for': {
          const [a, i] = b.params
          // ⚠ key 的位置**必须占住**（没有 key 就显式写 `null`）：少了它，锚点会落到
          // createFor 的 key 形参上 —— `r ? r(i, t) : t` 于是去调一个**节点**，
          // 抛 `TypeError: r is not a function`（账号页那几个无 key 的 `.map()` 就这么炸的）。
          const key = b.key ? `(${a}, ${i}) => ${b.key}` : 'null'
          out.push(`${this.h('createFor')}(${v(b.parent)}, () => ${b.list}, (${a}, ${i}) => ${b.item}, ${key}, ${b.anchor ? v(b.anchor) : 'null'}, ${b.positional})`)
          break
        }
      }
    }
    return out
  }

  finish(code: string): CompileResult {
    /**
     * ⚠ 早退的判据**不能只看 `templates`**。
     *
     * `helpers` 里那些 `_$createComponent` / `_$setNodes` 是**代码里真的会调用**的
     * （第 297 行那个组件分支就会发 `createComponent`），漏注入 import 的后果是
     * 运行期 `ReferenceError: _$createComponent is not defined`，而**编译期一声不响**。
     *
     * 2026-10-02 实测踩中：`ui/account-picker.tsx` 改成"整份就是一个组件、
     * 没有任何静态元素"之后 `templates.length === 0`，于是 import 被这行早退吞掉，
     * 任务页整块渲染炸掉。判据补上 `helpers.size === 0` 即可 ——
     * 两个都空时才是真的没什么要插。
     */
    if (this.templates.length === 0 && this.helpers.size === 0) return { code, helpers: this.helpers }
    const lines = code.split('\n')
    let at = 0
    for (let i = 0; i < lines.length; i++) if (/^\s*import\s/.test(lines[i])) at = i + 1
    const importLine = this.helpers.size
      ? `import { ${[...this.helpers].sort().map((n) => `${n} as _$${n}`).join(', ')} } from '${this.runtime}'`
      : ''
    lines.splice(at, 0, [importLine, ...this.templates].filter(Boolean).join('\n'))
    return { code: lines.join('\n'), helpers: this.helpers }
  }
}

// ── 小工具 ────────────────────────────────────────────────────────────────
const isNullish = (e: ast.Expression) => e.kind === ast.SyntaxKind.NullKeyword || e.kind === ast.SyntaxKind.FalseKeyword || (ast.isIdentifier(e) && e.text === 'undefined')

const isLiteral = (e: ast.Expression) => ast.isStringLiteral(e) || ast.isNumericLiteral(e) || ast.isNoSubstitutionTemplateLiteral(e) || e.kind === ast.SyntaxKind.TrueKeyword || e.kind === ast.SyntaxKind.FalseKeyword || isNullish(e)

function hasJsx(e: ast.Expression): boolean {
  return ast.isJsxElement(e) || ast.isJsxSelfClosingElement(e) || ast.isJsxFragment(e) || (ast.isConditionalExpression(e) && (hasJsx(e.whenTrue) || hasJsx(e.whenFalse)))
}

const isJsxNode = (n: ast.Node) => ast.isJsxElement(n) || ast.isJsxSelfClosingElement(n) || ast.isJsxFragment(n)

/**
 * 这一段代码里**读了**某个变量名吗（用来决定列表行是否"位置敏感"）。
 *
 * ⚠ 判定要**按结构走**，不能"整段源码里搜一下这个名字"：`a.i` 的那截 `i` 是成员名、
 * `<div i={…}>` 的 `i` 是属性名、`{ i: 1 }` 的 `i` 是键 —— 都不是读那个变量。
 *
 * 反过来漏判的代价不对称：多判一次只是重排时重建一行（丢一次 CSS 过渡），
 * 漏判则会把「第 3 步」这种步号留在旧位置上。所以除了上面那三处**明确不算**，
 * 其余一律算读了（包括嵌套箭头里 `() => move(i)`）。
 */
function readsIdent(node: ast.Node, name: string): boolean {
  if (ast.isIdentifier(node)) return node.text === name
  if (ast.isPropertyAccessExpression(node)) return readsIdent(node.expression, name)
  if (ast.isJsxAttribute(node)) return !!node.initializer && readsIdent(node.initializer, name)
  if (ast.isPropertyAssignment(node)) return readsIdent(node.initializer, name)
  let found = false
  ast.forEachChild(node, (c) => {
    if (!found && readsIdent(c, name)) found = true
  })
  return found
}

function literalHtml(e: ast.Expression): string | undefined {
  if (ast.isStringLiteral(e) || ast.isNoSubstitutionTemplateLiteral(e)) return escText(e.text)
  if (ast.isNumericLiteral(e)) return escText(e.text)
  if (isNullish(e) || e.kind === ast.SyntaxKind.TrueKeyword || e.kind === ast.SyntaxKind.FalseKeyword) return ''
  return undefined
}

/**
 * 只在**源码本来就有语法错误**时才会命中的分支：`createSourceFile` 不抛，它把诊断
 * 攒在 `parseDiagnostics` 里，节点树则是"就着残文能认多少认多少"。
 *
 * 为什么要单独判一句：不判的话 `compile` 会照常产出一段**残缺的**代码 —— 实测
 * `<div @click={f}>x</div>`（Vue 的事件简写，TSX 里非法）编出来是
 * `const A = () => (() => { … })() @click={f}>x</div>`，也就是把元素吃掉、把余下的
 * 原文当尾巴留下。下游转换器确实会报错，但那句 `Unexpected token` 与本文件
 * 隔着好几层，看着像编译器的 bug。先在这里拦，报的是"你这一行写错了"。
 */
const parseError = (sf: ast.SourceFile): string | undefined => {
  // oxc 的 errors 是语法错误列表（`@click` 那类"TSX 本身就解析不过"的写法在这里）：
  // 每条带 message 与出错区间（labels），行号自己从偏移量数出来。
  const d = sf.errors[0]
  if (!d) return undefined
  const line = ast.lineOf(sf, d.labels?.[0]?.start ?? 0) + 1
  return `[lite] ${sf.fileName.replace(/^.*[\\/]/, '')}:${line} 源码解析失败：${d.message}（TSX 里没有 Vue 的 @click / v-if：事件写 onClick，条件用三元）`
}

/** 编译一个 TSX 源文件。 */
export function compile(source: string, options: { runtime?: string; filename?: string } = {}): CompileResult {
  const runtime = options.runtime ?? '../src/index'
  const sf = ast.parse(source, options.filename ?? 'x.tsx')
  const bad = parseError(sf)
  if (bad) throw new Error(bad)
  const c = new Compiler(sf, source, runtime)
  const edits: { start: number; end: number; text: string }[] = []

  const visit = (node: ast.Node, inJsx: boolean): void => {
    const isJsx = ast.isJsxElement(node) || ast.isJsxSelfClosingElement(node) || ast.isJsxFragment(node)
    if (isJsx && !inJsx) {
      edits.push({ start: ast.getStart(node), end: ast.getEnd(node), text: c.root(node) })
      return
    }
    // ⚠ 属性上的检查不放在这里：命中**根** JSX 节点就 `return` 了，子节点根本走不到这一层，
    // 放在这儿的那句 `v-if` 拦截从来没生效过（`regress/compiler.mjs` 的负例把它抓出来了）。
    // 指令式属性由 `Compiler.checkDirective` 在认属性名的两个入口（组件 / 元素）拦。
    ast.forEachChild(node, (child) => visit(child, inJsx || isJsx))
  }
  visit(sf.program, false)

  let out = source
  for (const e of edits.sort((a, b) => b.start - a.start)) out = out.slice(0, e.start) + e.text + out.slice(e.end)
  return c.finish(out)
}
