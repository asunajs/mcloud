/**
 * lite 的 JSX 类型声明 —— **只有类型，零运行时代码**。
 *
 * 为什么要有这个文件：lite 的编译器（`compiler.ts`）把 TSX 折成运行时调用，但**类型检查**
 * 是 tsc 的事，tsc 只认 JSX 命名空间、不认 lite 的编译器。没有它，`tsc --noEmit` 会对每个
 * 标签报：
 *
 * ```
 * error TS7026: JSX element implicitly has type 'any' because no interface 'JSX.IntrinsicElements' exists.
 * ```
 *
 * 覆盖面 = **编译器实际支持的子集**（`compiler.ts` 只支持内置元素、普通函数组件、片段、
 * `.map()` 上的 `key`）。属性**刻意宽松**：lite 的属性行为是运行期的（`class` 只接字符串、
 * `style` 只接字符串、`false`/`null` 在部分属性上有特殊语义……），用类型去假装能校验它
 * 只会造出一堆假错误。
 *
 * 三条使用前提（见 `docs/web/lite-guide.md` §3）：
 * 1. `"jsx": "preserve"` —— 否则 tsc 按该设置去找 JSX 运行时（例如 `react-jsx` 会直接报
 *    `TS2875: This JSX tag requires the module path 'react/jsx-runtime' to exist`）；
 * 2. 这个文件靠 tsconfig 的 `include` / `files` 进类型程序（`types` 字段与它无关，
 *    写 `"types": []` 也照样生效）；
 * 3. 不要另外写 `"jsxImportSource"`：那条路是给存量 `vue-jsx-vapor` 项目用的（二选一；
 *    两个同时存在也不报错，但这份声明就用不上了）。
 */

declare namespace JSX {
  /**
   * JSX 表达式的类型。
   *
   * lite 的 JSX 表达式可以求值成节点、节点数组、`null`（条件分支）……而且没人对它的返回值
   * 做运算（都交给 `mount` / 插槽 / 绑定）。所以这里**故意不收紧** —— 收紧了就会把
   * `{cond ? <A /> : null}` 这类写法卡成类型错误。
   *
   * 片段（`<>…</>`）也走这个类型，不需要额外声明。
   */
  type Element = unknown

  /**
   * 合法标签类型：内置元素（字符串）+ 普通函数组件。
   *
   * 声明了 `ElementType` 之后，tsc 不再要求"组件返回值必须满足 `JSX.Element`"
   * （lite 里返回值可以是节点数组 / `null`），但**函数签名上的 props 仍然照常校验**。
   */
  type ElementType = string | ((props: any, ctx: any) => unknown)

  /**
   * 内置元素（HTML / SVG / 自定义元素）。
   *
   * * 不按 `HTMLElementTagNameMap` 精确映射：lite 的属性写法与 DOM 属性并不一一对应
   *   （`class` 接字符串、`value` property 与 attribute 双写、`disabled` 走 property……），
   *   精确映射只会误报。自定义元素也因此天然放行。
   * * 那条 `on${string}` 是**必须**的：属性表若只有笼统的索引签名，`onClick={(e) => …}`
   *   里的 `e` 拿不到上下文类型，strict 下直接报 `TS7006: Parameter 'e' implicitly has an
   *   'any' type`。写了它，任何 `onXxx` 都能推断成 `(e: any) => void`。
   *
   * 事件参数故意是 `any`：lite 给处理器的是**原生事件对象**（`Event` / `MouseEvent` /
   * `InputEvent`……由事件名决定，类型系统看不出来）。要精确类型就自己标注参数：
   * `onInput={(e: InputEvent) => …}`。
   */
  interface IntrinsicElements {
    [tag: string]: {
      [attr: string]: any
      [attr: `on${string}`]: ((e: any) => void) | undefined
    }
  }

  /**
   * 所有标签都接受的通用属性。lite 只有 `key`，而且**只对 `.map()` 有意义**
   * （写在其它的元素/组件上会被编译器静默丢掉，见 `docs/web/lite-guide.md` §6）。
   */
  interface IntrinsicAttributes {
    key?: string | number | bigint | null | undefined
  }

  /**
   * 子节点由哪个属性接收 —— `children` 是 tsc 的默认名，写出来只是把意图摆明。
   *
   * ⚠ 注意：lite 的子节点**不进 props**。编译器把 `<Panel>…</Panel>` 的子节点折成
   * `createComponent` 的第三个参数 `{ default: () => … }`，子组件用 `useSlots()` 取
   * （`component.ts`）；`props.children` 在 lite 里**永远是 `undefined`**。
   */
  interface ElementChildrenAttribute {
    children: {}
  }

  /**
   * 这条是让上面那句成立的另一半：给**所有组件的 props 类型**补一个可选的 `children`。
   *
   * 少了它，`<Panel title="x">…</Panel>` 会报
   * `Property 'children' does not exist on type '{ title: string }'` ——
   * 而 lite 里你**不该**为了写子节点去 props 上声明 `children`。类型补上，语义上不用管它。
   */
  type LibraryManagedAttributes<Component, Props> = Props & { children?: unknown }
}
