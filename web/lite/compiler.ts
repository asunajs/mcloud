/**
 * lite 的编译器：TSX → 目标形态。结构**参考 Solid 的 `babel-plugin-jsx-dom-expressions`**
 * （把它的真实输出调出来逐条对过，见 docs/lite-framework.md §4）。
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
 * 1. **锚点 = 后面那个静态兄弟节点**（末尾则 `null`）。不需要占位节点、不需要注释锚点。
 * 2. **每个 JSX 表达式编译成一个立即执行的箭头函数** ⇒ 语句都待在自己的块里，
 *    编译器不必往宿主函数里插语句。
 * 3. **动态 prop 用 getter**（Solid 与 Vapor 在这里不同：Solid 用 getter、Vapor 用函数）。
 *    getter 的好处是运行期零魔法 —— 不用去猜"这个值是 prop 还是回调函数"。
 * 4. **条件分支不需要专门的原语**：`{c ? <A/> : null}` 就是
 *    `setNodes(parent, () => c ? A() : null, anchor)`。所以运行时里没有 `createIf`。
 *
 * 与 Solid 的**两处有意不同**：
 * * `.map()` 编译成键控的 `createFor`（Solid 的 `.map` 是朴素数组 diff，键控要写 `<For>`）；
 *   本项目的源码写的是 `.map` + `key=`，我不想为了框架去改 41 处业务代码。
 * * 事件不委托（Solid 用 `$$click` + `delegateEvents`）：见 docs §8，那是下一步的候选。
 *
 * ⚠ 只支持 `docs/lite-framework.md` §2 普查到的语法子集；子集外的写法（`v-*` 指令、
 * `class` 数组/对象、`style={}`、模板引用、组件上的 spread）**直接抛错**，不静默编错。
 */

import ts from 'typescript'

export interface CompileResult {
  code: string
  helpers: Set<string>
}

/** 这些属性改 property 比改标签更对（布尔/值类）。 */
const PROPS = new Set(['value', 'checked', 'selected', 'disabled', 'open', 'multiple', 'readonly', 'required', 'muted'])

const VOID = new Set(['img', 'br', 'hr', 'input', 'meta', 'link', 'source', 'area', 'base', 'col', 'embed', 'track', 'wbr', 'path', 'circle', 'rect', 'line', 'polyline', 'polygon', 'use', 'stop', 'ellipse'])

const escText = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
const escAttr = (s: string) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;')

/** JSX 文本的空白语义：纯空白且跨行的丢，行内空白折叠成一个空格。 */
function jsxText(raw: string): string {
  if (!raw.includes('\n')) return raw.replace(/[ \t]+/g, ' ')
  const lines = raw.split('\n')
  const out = lines.map((l, i) => {
    let s = l.replace(/[ \t]+/g, ' ')
    if (i > 0) s = s.replace(/^ /, '')
    if (i < lines.length - 1) s = s.replace(/ $/, '')
    return s
  })
  const joined = out.join('')
  return joined.trim() === '' ? '' : joined.replace(/^\s+|\s+$/g, '')
}

/** 到某个节点的索引路径（相对本次编译的根）。 */
type Path = number[]

type Binding =
  | { kind: 'attr'; at: Path; name: string; expr: string; hoist: boolean }
  | { kind: 'event'; at: Path; name: string; expr: string }
  | { kind: 'spread'; at: Path; expr: string }
  | { kind: 'nodes'; parent: Path; anchor: Path | null; expr: string }
  | { kind: 'for'; parent: Path; anchor: Path | null; list: string; params: [string, string]; item: string; key?: string }

interface Built {
  html: string
  bindings: Binding[]
}

class Compiler {
  readonly helpers = new Set<string>()
  private templates: string[] = []

  constructor(
    private readonly sf: ts.SourceFile,
    private readonly src: string,
    private readonly runtime: string,
  ) {}

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

  private srcOf(node: ts.Node): string {
    return this.src.slice(node.getStart(this.sf), node.getEnd())
  }

  private tpl(html: string): string {
    const name = `_t${this.templates.length}`
    this.templates.push(`const ${name} = ${this.h('template')}(${JSON.stringify(html)})`)
    return name
  }

  // ── 根 ───────────────────────────────────────────────────────────────────
  /** 编译一个 JSX 表达式：语句封在自己的块里，返回它的值。 */
  root(node: ts.JsxElement | ts.JsxSelfClosingElement | ts.JsxFragment): string {
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
  private valueIsolated(node: ts.JsxElement | ts.JsxSelfClosingElement | ts.JsxFragment): string {
    const local: string[] = []
    const v = this.value(node, local)
    if (!local.length) return v
    return `(() => {\n${local.map((l) => '  ' + l).join('\n')}\n  return ${v}\n})()`
  }

  /** 任意 JSX 节点 → 一个表达式的值（元素 / 组件 / 片段）。 */
  private value(node: ts.JsxElement | ts.JsxSelfClosingElement | ts.JsxFragment, stmts: string[]): string {
    if (ts.isJsxFragment(node)) {
      const parts = node.children.map((c) => this.childValue(c)).filter((v): v is string => !!v)
      return parts.length === 0 ? 'null' : parts.length === 1 ? parts[0] : `[${parts.join(', ')}]`
    }
    const tag = this.tagOf(node)
    return /^[A-Z]/.test(tag) || tag.includes('.') ? this.component(node) : this.element(node, stmts)
  }

  private tagOf(node: ts.JsxElement | ts.JsxSelfClosingElement): string {
    return this.srcOf(ts.isJsxElement(node) ? node.openingElement.tagName : node.tagName)
  }

  // ── 组件 ─────────────────────────────────────────────────────────────────
  private component(node: ts.JsxElement | ts.JsxSelfClosingElement): string {
    const opening = ts.isJsxElement(node) ? node.openingElement : node
    const props: string[] = []
    for (const attr of opening.attributes.properties) {
      if (ts.isJsxSpreadAttribute(attr)) throw new Error('组件上的 {...spread} 未支持（项目里没有这种写法）')
      const name = this.srcOf(attr.name)
      if (name === 'key') continue // key 由外层的 createFor 取，不进 props
      const init = attr.initializer
      if (!init) {
        props.push(`${name}: true`)
        continue
      }
      if (ts.isStringLiteral(init)) {
        props.push(`${JSON.stringify(name)}: ${JSON.stringify(init.text)}`)
        continue
      }
      if (ts.isJsxExpression(init) && init.expression) {
        // 动态 prop = getter（同 Solid）。⚠ 值里可能直接是 JSX
        // （`<EmptyState action={<button …/>} />` 在本项目里就有），必须递归编译掉
        props.push(`get ${JSON.stringify(name)}() { return ${this.exprWithJsx(init.expression)} }`)
        continue
      }
      throw new Error(`不支持的组件属性：${name}`)
    }
    const slots: string[] = []
    if (ts.isJsxElement(node) && node.children.length) {
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
  private exprWithJsx(node: ts.Expression): string {
    const base = node.getStart(this.sf)
    const edits: { start: number; end: number; text: string }[] = []
    const walk = (n: ts.Node, inJsx: boolean) => {
      const isJsx = ts.isJsxElement(n) || ts.isJsxSelfClosingElement(n) || ts.isJsxFragment(n)
      if (isJsx && !inJsx) {
        edits.push({ start: n.getStart(this.sf) - base, end: n.getEnd() - base, text: this.valueIsolated(n) })
        return
      }
      ts.forEachChild(n, (c) => walk(c, inJsx || isJsx))
    }
    walk(node, false)
    if (!edits.length) return this.srcOf(node)
    let out = this.srcOf(node)
    for (const e of edits.sort((a, b) => b.start - a.start)) out = out.slice(0, e.start) + e.text + out.slice(e.end)
    return out
  }

  /** 组件子节点 / 片段成员：拿一个"值"（文本要自己造节点）。 */
  private childValue(child: ts.JsxChild): string | undefined {
    if (ts.isJsxText(child)) {
      const t = jsxText(child.text)
      return t ? `document.createTextNode(${JSON.stringify(t)})` : undefined
    }
    if (ts.isJsxExpression(child)) {
      if (!child.expression) return undefined
      if (isNullish(child.expression)) return undefined
      return this.exprWithJsx(child.expression)
    }
    return this.valueIsolated(child)
  }

  // ── 内置元素（静态 HTML + 绑定表）─────────────────────────────────────────
  private element(node: ts.JsxElement | ts.JsxSelfClosingElement, stmts: string[]): string {
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
  private html(node: ts.JsxElement | ts.JsxSelfClosingElement, stmts: string[], base: Path): Built {
    const opening = ts.isJsxElement(node) ? node.openingElement : node
    const tag = this.tagOf(node)
    if (/^[A-Z]/.test(tag) || tag.includes('.')) throw new Error('组件只能出现在动态子节点位置')
    const attrs: string[] = []
    const own: Binding[] = []
    for (const attr of opening.attributes.properties) {
      if (ts.isJsxSpreadAttribute(attr)) {
        own.push({ kind: 'spread', at: [], expr: this.srcOf(attr.expression) })
        continue
      }
      const name = this.srcOf(attr.name)
      const init = attr.initializer
      if (!init) {
        attrs.push(name)
        continue
      }
      if (ts.isStringLiteral(init)) {
        attrs.push(`${name}="${escAttr(init.text)}"`)
        continue
      }
      if (ts.isJsxExpression(init) && init.expression) {
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

    const children = ts.isJsxElement(node) ? node.children : []
    const parts: string[] = []
    const childBindings: Binding[] = []
    /**
     * ⚠ 这里必须用**两个**计数器，不能用同一个：
     *
     * * `seq` 是"子节点序列位置"（动态槽也占一位）—— 只用来判断"点后面那个静态兄弟是谁"；
     * * `dom` 是**真正产出节点的下标** —— 路径 `childNodes[dom]` 用的是它。
     *
     * 第一版只有一个计数器，于是 `<li>{label}:{n}</li>` 里那个静态的 ":" 被标成
     * `childNodes[1]`（DOM 里它其实是 `childNodes[0]`）⇒ 锚点错位，渲染出 `:a1`。
     */
    const statics: { seq: number; path: Path }[] = []
    const dyns: { seq: number; binding: Binding }[] = []
    let seq = 0
    let dom = 0

    for (const child of children) {
      if (ts.isJsxText(child)) {
        const t = jsxText(child.text)
        if (!t) continue
        parts.push(escText(t))
        statics.push({ seq, path: [...base, dom] })
        seq++
        dom++
        continue
      }
      if (ts.isJsxExpression(child)) {
        if (!child.expression) continue
        const lit = literalHtml(child.expression)
        if (lit !== undefined) {
          if (lit) {
            parts.push(lit)
            statics.push({ seq, path: [...base, dom] })
          }
          seq++
          if (lit) dom++
          continue
        }
        dyns.push({ seq, binding: this.dynamic(child, stmts, [...base, dom]) })
        seq++
        continue
      }
      // 组件 / 片段：不是静态结构 ⇒ 走"动态插入"（Solid 也是 insert(parent, createComponent(…), anchor)）。
      // 这是本项目最常见的写法之一：`<Panel>…</Panel>` 直接放在元素里。
      if (ts.isJsxFragment(child) || /^[A-Z]/.test(this.tagOf(child)) || this.tagOf(child).includes('.')) {
        dyns.push({
          seq,
          binding: { kind: 'nodes', parent: [...base], anchor: null, expr: `() => ${this.valueIsolated(child)}` },
        })
        seq++
        continue
      }
      // 静态元素：递归（它的绑定路径以本元素的 DOM 下标为前缀）
      const sub = this.html(child, stmts, [...base, dom])
      parts.push(sub.html)
      statics.push({ seq, path: [...base, dom] })
      childBindings.push(...sub.bindings)
      seq++
      dom++
    }

    // 动态位置的锚点 = 它后面第一个静态兄弟（没有 ⇒ null，追加到末尾）
    for (const { seq: at, binding } of dyns) {
      const anchor = statics.find((st) => st.seq > at)?.path ?? null
      if (binding.kind === 'nodes' || binding.kind === 'for') binding.anchor = anchor
      childBindings.push(binding)
    }

    const inner = parts.join('')
    const selfClose = VOID.has(tag) || children.length === 0
    const html = selfClose ? `<${tag}${attrs.length ? ' ' + attrs.join(' ') : ''}>` : `<${tag}${attrs.length ? ' ' + attrs.join(' ') : ''}>${inner}</${tag}>`
    // 自身绑定：路径为 base（相对整棵树的根，由调用方补前缀）
    const ownShifted = own.map((b) => ({ ...b, at: [...base] }) as Binding)
    return { html, bindings: [...ownShifted, ...childBindings] }
  }

  /** 一个动态子节点的绑定。 */
  private dynamic(child: ts.JsxExpression, stmts: string[], at: Path): Binding {
    const e = child.expression as ts.Expression
    const parent = at.slice(0, -1)
    // {cond ? <A/> : null} ⇒ setNodes(parent, () => cond ? A() : null, anchor)（Solid 同款，不需要 createIf）
    // ⚠ 只有**两个分支都是 JSX 或 null** 时才走这条精确路径；否则落到下面的通用路径
    // （分支是普通表达式的三元，项目里也有 —— 那里靠 exprWithJsx 递归处理 JSX）
    const branchOk = (x: ts.Expression) => isNullish(x) || ts.isJsxElement(x) || ts.isJsxSelfClosingElement(x) || ts.isJsxFragment(x)
    if (ts.isConditionalExpression(e) && hasJsx(e) && branchOk(e.whenTrue) && branchOk(e.whenFalse)) {
      const cond = this.srcOf(e.condition)
      const a = isNullish(e.whenTrue) ? 'null' : this.branch(e.whenTrue)
      const b = isNullish(e.whenFalse) ? 'null' : this.branch(e.whenFalse)
      return { kind: 'nodes', parent, anchor: null, expr: `() => ${cond} ? ${a} : ${b}` }
    }
    // {list.map((x) => <Row/>)} ⇒ createFor
    if (ts.isCallExpression(e) && ts.isPropertyAccessExpression(e.expression) && e.expression.name.text === 'map' && e.arguments.length === 1) {
      const fn = e.arguments[0]
      const body = ts.isArrowFunction(fn) ? (ts.isParenthesizedExpression(fn.body) ? fn.body.expression : fn.body) : undefined
      const isListOfJsx = !!body && (ts.isJsxElement(body) || ts.isJsxSelfClosingElement(body) || ts.isJsxFragment(body))
      // 回调不返回 JSX 的 `.map`（例如 `{xs.map(f).join(',')}`）不是列表，交给普通表达式路径
      if (isListOfJsx && ts.isArrowFunction(fn) && fn.parameters.length >= 1 && fn.parameters.length <= 2) {
        // 两个参数名：用户只写了一个就补一个 `_i`（createFor 会给 index，key/render 都能用）
        const a = this.srcOf(fn.parameters[0].name)
        const b = fn.parameters[1] ? this.srcOf(fn.parameters[1].name) : '_i'
        let key: string | undefined
        if (!ts.isJsxFragment(body)) {
          const opening = ts.isJsxElement(body) ? body.openingElement : body
          for (const attr of opening.attributes.properties) {
            if (ts.isJsxAttribute(attr) && this.srcOf(attr.name) === 'key' && attr.initializer && ts.isJsxExpression(attr.initializer) && attr.initializer.expression) {
              key = this.srcOf(attr.initializer.expression)
            }
          }
        }
        return {
          kind: 'for',
          parent,
          anchor: null,
          list: this.srcOf((e.expression as ts.PropertyAccessExpression).expression),
          params: [a, b],
          item: this.root(body),
          key,
        }
      }
    }
    void stmts
    return { kind: 'nodes', parent, anchor: null, expr: `() => ${this.exprWithJsx(e)}` }
  }

  private branch(e: ts.Expression): string {
    if (ts.isJsxElement(e) || ts.isJsxSelfClosingElement(e) || ts.isJsxFragment(e)) return this.root(e)
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
          const fn = this.h(b.name === 'class' ? 'setClass' : PROPS.has(b.name) ? 'setProp' : 'setAttr')
          const call = `${fn}(${v(b.at)}, ${b.expr})`
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
          const key = b.key ? `, (${a}, ${i}) => ${b.key}` : ''
          out.push(`${this.h('createFor')}(${v(b.parent)}, () => ${b.list}, (${a}, ${i}) => ${b.item}${key}, ${b.anchor ? v(b.anchor) : 'null'})`)
          break
        }
      }
    }
    return out
  }

  finish(code: string): CompileResult {
    if (this.templates.length === 0) return { code, helpers: this.helpers }
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
const isNullish = (e: ts.Expression) => e.kind === ts.SyntaxKind.NullKeyword || e.kind === ts.SyntaxKind.FalseKeyword || (ts.isIdentifier(e) && e.text === 'undefined')

const isLiteral = (e: ts.Expression) => ts.isStringLiteral(e) || ts.isNumericLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e) || e.kind === ts.SyntaxKind.TrueKeyword || e.kind === ts.SyntaxKind.FalseKeyword || isNullish(e)

function hasJsx(e: ts.Expression): boolean {
  return ts.isJsxElement(e) || ts.isJsxSelfClosingElement(e) || ts.isJsxFragment(e) || (ts.isConditionalExpression(e) && (hasJsx(e.whenTrue) || hasJsx(e.whenFalse)))
}

function literalHtml(e: ts.Expression): string | undefined {
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)) return escText(e.text)
  if (ts.isNumericLiteral(e)) return escText(e.text)
  if (isNullish(e) || e.kind === ts.SyntaxKind.TrueKeyword || e.kind === ts.SyntaxKind.FalseKeyword) return ''
  return undefined
}

/** 编译一个 TSX 源文件。 */
export function compile(source: string, options: { runtime?: string; filename?: string } = {}): CompileResult {
  const runtime = options.runtime ?? '../src/index'
  const sf = ts.createSourceFile(options.filename ?? 'x.tsx', source, ts.ScriptTarget.ESNext, true, ts.ScriptKind.TSX)
  const c = new Compiler(sf, source, runtime)
  const edits: { start: number; end: number; text: string }[] = []

  const visit = (node: ts.Node, inJsx: boolean): void => {
    const isJsx = ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node)
    if (isJsx && !inJsx) {
      edits.push({ start: node.getStart(sf), end: node.getEnd(), text: c.root(node) })
      return
    }
    if (ts.isJsxAttribute(node) && /^v[A-Z]/.test(node.name.getText(sf))) throw new Error(`不支持指令式属性：${node.name.getText(sf)}`)
    ts.forEachChild(node, (child) => visit(child, inJsx || isJsx))
  }
  visit(sf, false)

  let out = source
  for (const e of edits.sort((a, b) => b.start - a.start)) out = out.slice(0, e.start) + e.text + out.slice(e.end)
  return c.finish(out)
}
