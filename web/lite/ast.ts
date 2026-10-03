/**
 * AST 适配层：把 **oxc-parser** 的 ESTree 形状，适配成 lite 编译器要用的那一小撮接口。
 *
 * # 为什么单独一层
 *
 * `compiler.ts` 799 行里**只有 95 行碰 AST**，而且只做三类事：
 * "这是哪种节点" / "取字段" / "按位置切源码"。把这三类收进本文件后，
 * 换解析器就只动这里 + 一次机械改名（`ts.` → `ast.`）。
 *
 * 关键事实（决定了这件事有多小）：**编译器的代码生成完全没用 TS 的 printer**
 * ——`createPrinter` / `printNode` / `getText` 在 `compiler.ts` 里命中数为 0，
 * 产物是自写的字符串拼接。所以换解析器**不改产物形状**，只改"怎么读源码"。
 *
 * 2026-10-01：用户要求去掉 `typescript` 包（那一句"ts 就是为了编译"是对的：
 * 全仓只有 `compiler.ts` 一处 import 它）。本文件是**唯一** import `oxc-parser` 的地方。
 *
 * # 为什么是"归一化"而不是逐处改写字段
 *
 * oxc 的字段名与 TS 不同（`Literal.value` vs `.text`、`ConditionalExpression.consequent`
 * vs `.whenTrue`、`JSXOpeningElement.name` vs `.tagName`…），另有一处**结构**差异：
 * TS 把自闭合元素当作独立的 `JsxSelfClosingElement`，而 oxc 里它也是 `JSXElement`，
 * 靠 `openingElement.selfClosing` 区分。
 *
 * 与其在编译器里逐处改写（95 行会变成几百行 diff），这里做**一次 normalize 遍历**：
 * 把别名与 `kind` 直接挂到节点上，于是 `ts.` 改成 `ast.` 就能跑。
 *
 * ⚠ 代价：别名是**拷贝**（同一个子节点会被两个键指向），所以 `forEachChild` 必须跳过
 * 别名键，否则同一节点被访问两次 —— `compile()` 里的 `exprWithJsx` 正是靠
 * `forEachChild` 收集替换区间，重复访问会产出**重复编辑**（产物直接坏掉）。
 * 于是每个挂过别名的节点都记一份 `__alias` 名单，遍历时跳过。
 *
 * ⚠ 本层的类型是**宽松**的（`[key: string]: any`）：它要描述一棵来源不断变化的树，
 * 逐字段强类型只会让 95 行的移植变成 500 行的类型体操。行为不靠类型保证，靠两样东西：
 * `lite/regress/compiler.mjs` 的 12 条负例，以及"18 个 tsx 的产物与 TS 版逐字节一致"
 * （黄金样本在移植时对过，见 `docs/web/lite-framework.md`）。
 */
import { parseSync } from 'oxc-parser'

/** 一个 AST 节点。`kind` 是归一化后的种类名（多数与 `type` 相同，见下面的特例）。 */
export interface Node {
  type: string
  kind: string
  start: number
  end: number
  /**
   * 少数**结构性**字段给出具体类型（比索引签名更具体，于是赢）：
   * 有它们，`node.children.map((c) => …)` 里的回调参数才是 `Node` 而不是隐式 any。
   * ⚠ 这三个声明成**必有**是刻意的：并不是每个节点都有 `children`（Identifier 就没有），
   * 但编译器只在"确定有"的地方读它们 —— 声明成可选只会换来十几处 `?? []` 与 `!`，
   * 把这次移植的 diff 搅浑。行为不靠类型保证（见文件头最后一段）。
   * 其余字段一律走索引签名。
   */
  children: Node[]
  parameters: Node[]
  body: Node
  [key: string]: any
}

export type Expression = Node
export type JsxChild = Node
export type JsxElement = Node
export type JsxSelfClosingElement = Node
export type JsxFragment = Node
export type JsxAttribute = Node
export type JsxSpreadAttribute = Node
export type JsxExpression = Node
export type JsxText = Node
export type ConditionalExpression = Node
export type PropertyAccessExpression = Node
export type PropertyAssignment = Node
export type Identifier = Node
export type StringLiteral = Node
export type NumericLiteral = Node
export type CallExpression = Node
export type ObjectExpression = Node
export type ArrowFunction = Node
export type FunctionExpression = Node
export type Block = Node
export type ParenthesizedExpression = Node

/**
 * 解析结果。`program` 是 ESTree 的 `Program`；`errors` 是 oxc 的语法错误
 * （`@click` 那类"TSX 本身就解析不过"的写法在这里被抓住，编译期报错靠它）。
 */
export interface SourceFile {
  fileName: string
  source: string
  program: Node
  errors: { message: string; labels: { start: number; end: number }[] }[]
}

/** 编译器里仅有的三处 `SyntaxKind` 比较（字面量真假与 null）。 */
export const SyntaxKind = {
  NullKeyword: 'NullKeyword',
  TrueKeyword: 'TrueKeyword',
  FalseKeyword: 'FalseKeyword',
} as const

/** 别名（拷贝出来的键）在这里登记，`forEachChild` 跳过 —— 否则子节点被访问两次。 */
const ALIAS = '__alias'

const alias = (n: Node, key: string, value: unknown): void => {
  n[key] = value
  const list = (n[ALIAS] as string[] | undefined) ?? ((n[ALIAS] = []) as string[])
  list.push(key)
}

/** 归一化后 `kind` 会变成 TS 口径的名字（只在这三处不同，其余 `kind === type`）。 */
const literalKind = (v: unknown): string | undefined =>
  v === null ? 'NullKeyword' : v === true ? 'TrueKeyword' : v === false ? 'FalseKeyword' : undefined

/**
 * 一次遍历，把 TS 口径的别名挂到节点上。用显式栈（不用递归）：源码是用户写的，
 * 嵌套深度不设上限，递归版本在深层 JSX 上会把栈吃穿。
 */
function normalize(program: Node): void {
  /**
   * ⚠ 两遍走，而且第二遍要**反向**：
   * JSXElement 的别名读的是它 `openingElement.attributes`（那要被上一条规则换成包装节点），
   * 父节点先处理就会读到**还没换的数组** ⇒ `opening.attributes.properties is not iterable`。
   * 收集顺序是父在前、子在**后**入栈，所以倒着处理正好是"子先父后"。
   */
  const all: Node[] = []
  const stack: Node[] = [program]
  while (stack.length) {
    const n = stack.pop()
    if (!n || typeof n !== 'object' || typeof n.type !== 'string') continue
    all.push(n)
    for (const key of Object.keys(n)) {
      if (key === ALIAS) continue
      const v = n[key]
      // ⚠ 花括号不能省：`if (…) for (…) if (…) push(c) else if (…) push(v)`
      // 里的 `else` 会绑到**内层** if 上 —— 表现是"数组子节点进得来、对象子节点永远进不来"，
      // 于是绝大多数节点没被归一化（`kind` 是 undefined、JSX 找不到、产物静默退回原文件）。
      // 2026-10-01 实测踩过：192 个节点只进来了 7 个。
      if (Array.isArray(v)) {
        for (const c of v) {
          if (c && typeof c === 'object' && typeof (c as Node).type === 'string') stack.push(c as Node)
        }
      } else if (v && typeof v === 'object' && typeof (v as Node).type === 'string') {
        stack.push(v as Node)
      }
    }
  }
  for (let i = all.length - 1; i >= 0; i--) apply(all[i])
}

/** 单节点归一化（别名与 `kind`）。 */
function apply(n: Node): void {
  {
    n.kind = n.type
    switch (n.type) {
      case 'Literal': {
        const k = literalKind(n.value)
        if (k) n.kind = k
        // TS 的 `.text`：字符串字面量给值本身，其余给**源码原文**（数字/大整数/正则）
        n.text = typeof n.value === 'string' ? n.value : n.value === null || n.value === true || n.value === false ? String(n.value) : n.raw
        break
      }
      case 'Identifier':
      case 'JSXIdentifier':
        n.text = n.name
        break
      case 'JSXText':
        // TS 的 `JsxText.text` 是**源码原文**（`&amp;` 这类实体不解码，转义由编译器自己做）⇒ 取 raw
        n.text = n.raw ?? n.value
        break
      case 'TemplateLiteral':
        if (!n.expressions?.length) {
          n.kind = 'NoSubstitutionTemplateLiteral'
          n.text = n.quasis?.[0]?.value?.cooked ?? n.quasis?.[0]?.value?.raw
        }
        break
      case 'JSXAttribute':
        // TS：`attr.initializer`；oxc：`attr.value`（无值属性为 null）
        alias(n, 'initializer', n.value)
        break
      case 'JSXOpeningElement': {
        // TS 的 `openingElement.attributes` 是一个 **JsxAttributes 节点**（属性数组在它的
        // `.properties` 上），而 oxc 直接给数组 ⇒ 编译器那三处 `opening.attributes.properties`
        // 会读到 undefined。这里把数组**换**成一个同形状的包装节点（不是别名：键名没变，
        // 遍历仍能走进去，属性节点只被访问一次）。
        const attrs = Array.isArray(n.attributes) ? n.attributes : []
        n.attributes = {
          type: 'JsxAttributes',
          kind: 'JsxAttributes',
          start: attrs[0]?.start ?? n.start,
          end: attrs[attrs.length - 1]?.end ?? n.end,
          properties: attrs,
        }
        alias(n, 'tagName', n.name)
        break
      }
      case 'ArrowFunctionExpression':
      case 'FunctionExpression':
      case 'FunctionDeclaration': {
        // TS 的参数是 **ParameterDeclaration 节点**（真正的模式挂在 `.name` 上），
        // oxc 直接把模式节点放进 `params` ⇒ 编译器那句 `isIdentifier(p.name)` 会拿到
        // 一个字符串而全部判否（`.map()` 的键控列表就退化成整表重建了：语义还对，但形状变了，
        // 黄金样本能一眼看出来）。这里补一层包装，与 TS 同形。
        alias(
          n,
          'parameters',
          (Array.isArray(n.params) ? n.params : []).map((q: Node) => ({
            type: 'Parameter',
            kind: 'Parameter',
            start: q.start,
            end: q.end,
            name: q,
          })),
        )
        break
      }
      case 'JSXElement': {
        // TS 把自闭合元素当成另一种节点；oxc 里它是 JSXElement + selfClosing
        const opening = n.openingElement
        if (opening?.selfClosing) n.kind = 'JsxSelfClosingElement'
        // TS：`node.tagName` / `node.attributes` 在自闭合节点上直接可取 ⇒ 两条路都铺平
        alias(n, 'tagName', opening?.name)
        alias(n, 'attributes', opening?.attributes ?? [])
        break
      }
      case 'ConditionalExpression':
        alias(n, 'condition', n.test)
        alias(n, 'whenTrue', n.consequent)
        alias(n, 'whenFalse', n.alternate)
        break
      case 'Property':
        // TS 的 PropertyAssignment：`.name` / `.initializer`
        alias(n, 'name', n.key)
        alias(n, 'initializer', n.value)
        break
      case 'CallExpression':
      case 'NewExpression':
        // TS 的 `CallExpression.expression` 是被调方；ESTree 叫 `.callee`
        alias(n, 'expression', n.callee)
        break
      case 'MemberExpression':
        // TS 的 PropertyAccessExpression：`.expression`（对象）与 `.name`（属性）
        alias(n, 'expression', n.object)
        alias(n, 'name', n.property)
        break
      case 'JSXSpreadAttribute':
        // TS：`.expression`；oxc：`.argument`
        alias(n, 'expression', n.argument)
        break
      case 'JSXExpressionContainer':
        /**
         * ⚠ TS 对**空表达式容器**（`{/* 注释 *\/}`、`{}`、`{ }`）一律给 `expression === undefined`，
         * 而 oxc 给一个 `JSXEmptyExpression` 节点 —— 实测三种写法都对过拍。
         * 这个差别会**改产物**：编译器靠 `if (!child.expression) continue` 跳过它们，
         * oxc 的节点是真值 ⇒ 会当成动态子节点多吐一个 `<!---->` 锚点（黄金样本就是这么抓出来的）。
         * 所以这里镜像 TS 的口径。
         */
        if (n.expression?.type === 'JSXEmptyExpression') n.expression = undefined
        break
      default:
        break
    }
  }
}

/** 解析 TSX。抛错交给调用方（编译器要把语法错误变成一条可读的失败信息）。 */
export function parse(source: string, fileName: string): SourceFile {
  const r = parseSync(fileName, source)
  const program = r.program as unknown as Node
  normalize(program)
  return { fileName, source, program, errors: (r.errors ?? []) as SourceFile['errors'] }
}

/** 遍历子节点。**跳过别名键**（见文件头：重复访问会让 `exprWithJsx` 产出重复编辑）。 */
export function forEachChild(node: Node, cb: (child: Node) => void): void {
  const skip = node[ALIAS] as string[] | undefined
  for (const key of Object.keys(node)) {
    if (key === ALIAS || skip?.includes(key)) continue
    const v = node[key]
    if (Array.isArray(v)) {
      for (const c of v) if (c && typeof c === 'object' && typeof (c as Node).type === 'string') cb(c as Node)
    } else if (v && typeof v === 'object' && typeof (v as Node).type === 'string') {
      cb(v as Node)
    }
  }
}

/** TS 的 `node.getStart(sf)`：跳过前导空白/注释后的起点 —— oxc 的 `start` 就是这个语义。 */
export const getStart = (node: Node): number => node.start

/** TS 的 `node.getEnd()`。 */
export const getEnd = (node: Node): number => node.end

/** 按位置取行号（0 基，与 TS 的 `getLineAndCharacterOfPosition().line` 同口径）。 */
export function lineOf(sf: SourceFile, pos: number): number {
  let line = 0
  for (let i = 0; i < pos && i < sf.source.length; i++) if (sf.source.charCodeAt(i) === 10) line++
  return line
}

export const isStringLiteral = (n: Node): boolean => n.kind === 'Literal' && typeof n.value === 'string'
export const isNumericLiteral = (n: Node): boolean => n.kind === 'Literal' && typeof n.value === 'number'
export const isNoSubstitutionTemplateLiteral = (n: Node): boolean => n.kind === 'NoSubstitutionTemplateLiteral'
export const isIdentifier = (n: Node): boolean => n.type === 'Identifier'
export const isJsxElement = (n: Node): boolean => n.kind === 'JSXElement'
export const isJsxSelfClosingElement = (n: Node): boolean => n.kind === 'JsxSelfClosingElement'
export const isJsxFragment = (n: Node): boolean => n.type === 'JSXFragment'
export const isJsxExpression = (n: Node): boolean => n.type === 'JSXExpressionContainer'
export const isJsxText = (n: Node): boolean => n.type === 'JSXText'
export const isJsxAttribute = (n: Node): boolean => n.type === 'JSXAttribute'
export const isJsxSpreadAttribute = (n: Node): boolean => n.type === 'JSXSpreadAttribute'
export const isConditionalExpression = (n: Node): boolean => n.type === 'ConditionalExpression'
export const isPropertyAccessExpression = (n: Node): boolean => n.type === 'MemberExpression'
export const isPropertyAssignment = (n: Node): boolean => n.type === 'Property'
export const isCallExpression = (n: Node): boolean => n.type === 'CallExpression'
export const isArrowFunction = (n: Node): boolean => n.type === 'ArrowFunctionExpression'
export const isFunctionExpression = (n: Node): boolean => n.type === 'FunctionExpression'
export const isBlock = (n: Node): boolean => n.type === 'BlockStatement'
// oxc 默认保留括号节点（与 TS 的 ParenthesizedExpression 同口径）⇒ 判定一致
export const isParenthesizedExpression = (n: Node): boolean => n.type === 'ParenthesizedExpression'
