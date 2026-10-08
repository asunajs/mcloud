# lite（已剥离到独立仓库）

> **2026-10-07**：lite 从本仓的 `web/lite/` **剥离**成一个独立的公有仓库
> —— [`asunajs/lite`](https://github.com/asunajs/lite)（npm 包名 `@asunajs/lite`）。
> 这份文件只讲**本仓怎么用它**；框架本身的设计、教程与踩坑史都在框架仓里。

| 原来在哪 | 现在在哪 |
|---|---|
| `web/lite/`（源码 + 编译器 + 插件 + 夹具 + 体积脚本） | 框架仓**根目录** |
| `docs/web/lite-guide.md` | [框架仓 `docs/guide.md`](https://github.com/asunajs/lite/blob/main/docs/guide.md) |
| `docs/web/lite-framework.md` | [框架仓 `docs/design.md`](https://github.com/asunajs/lite/blob/main/docs/design.md) |
| `docs/web/lite-postmortem.md` | [框架仓 `docs/pitfalls.md`](https://github.com/asunajs/lite/blob/main/docs/pitfalls.md)（2026-10-07 从 `postmortem.md` 重写改名：按技术主题组织） |
| `web/lite/regress/compiler.mjs`、`demo/`、`bench/`、`size.mjs` | 框架仓（框架自己的门） |
| `web/lite/regress/{lib,interact,bulk,search,narrow,leak,live-logs,preview}.mjs` | **本仓** `web/regress/`（页面级验收，见下） |
| `web/lite/chrome-flags.mjs` | **本仓** `web/scripts/chrome-flags.mjs` |
| （新）**写页面前先看的那一屏** | [框架仓 `docs/agent.md`](https://github.com/asunajs/lite/blob/main/docs/agent.md) —— 硬约束 + 静默编错速查 + 归属/销毁；**本仓/框架仓的作业须知**见 [框架仓 `AGENTS.md`](https://github.com/asunajs/lite/blob/main/AGENTS.md) |

框架仓的历史是**接着本仓走的**：用 `git subtree split -P web/lite` 切出来的 57 个提交，
不是"从零开始的新仓" ⇒ 想知道"某段代码为什么长这样"，在框架仓里 `git log` 一样查得到 ✓。

## 本仓怎么用它

依赖在 `web/package.json`：

```jsonc
"devDependencies": {
  "@asunajs/lite": "git+https://github.com/asunajs/lite.git#v0.1.2",   // git 依赖，tag 即版本
  ...
}
```

⚠ 是 **git 依赖**（还没发布到 npm registry）⇒ `npm install` / `npm ci` 需要能访问 GitHub，
锁文件里记的是 commit SHA。

⚠⚠ **改 tag 后光跑 `npm install` 不会重新拉**（2026-10-07 实测：spec 从 `#v0.1.1` 改成
`#v0.1.2`，`npm install` 回一句 `up to date`，装着的还是**旧 commit** —— npm 只更新了锁文件里
那条 spec，没重解析已固定的 SHA）。要真升级，得**显式再装一次**：

```bash
cd web && npm install -D --no-audit --no-fund "git+https://github.com/asunajs/lite.git#v0.1.2"
npm ls @asunajs/lite        # 判据：这里显示的 commit 必须是新 tag 指的那个
```

（`npm ci` 是按锁文件重装，同样不会自己发现新 tag ⇒ 也要先这样升级一次、把新 SHA 落进锁文件。）
判据别只看版本号：`package.json` 的 `version` 与 tag 不一定同步，**认 commit** ✓。

⚠ 写**完整 URL**，不要用 `github:asunajs/lite` 简写：npm 会把简写解析成 `git+ssh://`，
而公开仓的 SSH 同样要密钥 ⇒ CI 与没配 SSH 的人装不上 ✗（2026-10-07 实测）。
⚠ 另一条实测（免得被锁文件吓到）：npm 会把 `git+https://` 在**锁文件的 `resolved` 字段里**
规范化成 `git+ssh://git@github.com/...` —— 但**取包用的是 package.json 里那条 spec**。
判据：冷 cache + `GIT_ALLOW_PROTOCOL=https`（禁掉 ssh 传输）下 `npm ci` **照样成功** ✓。

接线三处，缺一不可：

| 文件 | 内容 | 为什么 |
|---|---|---|
| `web/vite.config.ts` | `import lite from '@asunajs/lite/vite'` + `lite({ runtime: '@asunajs/lite' })` | 插件把 TSX 折成模板 + 逐槽 effect；`runtime` 决定产物里 `import … from '…'` 写什么 |
| `web/tsconfig.json` | `"types": ["vite/client", "@asunajs/lite/jsx"]` | 全局 `JSX` 命名空间声明来自这个入口；少了它每个标签都报 `TS7026` |
| `web/src/**` | `import { ref } from '@asunajs/lite'` | 业务源码写**包名**（26 个文件） |

⚠ **没有 `resolve.alias`，也没有 `paths` 映射了**（2026-10-07 删的）：以前框架在仓里、
业务源码写裸名 `lite`，才需要那两条把裸名指到 `web/lite/src/index.ts`。现在它是一份**正常
依赖**，走常规解析 —— 留着 alias 反而让"它到底是依赖还是仓里的源码"变得含糊 ✗。

⚠ 运行时交的是 **TS 源码**（框架包只发 `src/` + `dist/`，`dist/` 里只有两个 Node 侧入口
`vite` / `compiler` 的 JS）。这不是将就：`src/dev.ts` 靠 `import.meta.env.DEV` 在**我们的**
构建里被静态替换（生产折掉诊断代码、开发保留），预编译一遍那句话就变成字面量 `false` ✗。
实测 dev（依赖预打包）与 build（Rolldown）两条路都对 ✓。

## 门禁怎么分

| 门 | 在哪跑 | 跑什么 |
|---|---|---|
| 框架自己的门 | **框架仓**：`npm run gates` 在 CI 里跑；`test:demo` / `test:bench` 要浏览器，**故意不进 CI**，由改框架的人本地跑（`LITE_CHROME_NO_SANDBOX=1`） | 编译器负例（13 条形态断言）、真 DOM 验收（62 条）、编译产物行为 + 耗时护栏、运行时体积 |
| 页面级验收 | **本仓** `scripts/gates-web.mjs`（两档，见 `docs/conventions.md` §9.2） | typecheck、build、交互验收（三个认证态）、大体量（分页 + 窗口化）、配置中心搜索框、窄屏 320/375、泄漏量测 |

⚠ 框架那三条**不再**出现在本仓门禁里 —— 包不发夹具（`demo/` / `bench/` / `regress/` 不在
`files` 里），本仓拿不到它们。这是有意的分工：框架"自己对不对"由框架仓回答，
"这套页面在这个应用上对不对"只有本仓能回答 ✓。

本仓那份 `web/scripts/chrome-flags.mjs` 是**页面级验收**用的副本，环境变量叫
`MCLOUD_CHROME_NO_SANDBOX`；框架仓那份叫 `LITE_CHROME_NO_SANDBOX`（两仓各一份，有意为之：
它们是两套不同的门）。

## 要改框架本身怎么办

**别改 `web/node_modules/@asunajs/lite/`** ✗ —— 那是安装产物，`npm ci` 一来就没了。

```bash
# 1) 在框架仓改（本机检出：../lite）
cd ../lite && npm ci && npm run gates && LITE_CHROME_NO_SANDBOX=1 npm run test:demo
# 2) 提交、推送、打新 tag（例：v0.1.3）
git commit -am "…" && git push && git tag v0.1.3 && git push origin v0.1.3
# 3) 回本仓升级：⚠ 只改 package.json + npm install 是**不生效**的（见上）
cd ../mcloud/web && npm install -D --no-audit --no-fund "git+https://github.com/asunajs/lite.git#v0.1.3"
npm ls @asunajs/lite        # 确认 commit 换了
cd .. && MCLOUD_CHROME_NO_SANDBOX=1 node scripts/gates-web.mjs --push
```

⚠ `dist/`（`vite.js` / `compiler.js` / `*.d.ts`）在框架仓里是**入库**的，改完源码忘了
`npm run build` 会被框架仓 CI 的「dist 与源码同步」一步拦下 ✓ —— 本仓因此不需要为它操心。
