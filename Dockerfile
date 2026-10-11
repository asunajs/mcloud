# syntax=docker/dockerfile:1.7
#
# ══════════════════════════════════════════════════════════════════════════
# mcloud —— Linux 容器镜像
# ══════════════════════════════════════════════════════════════════════════
#
# 用法见 `docs/docker.md`（那一份是给人看的）；这里只写**为什么这么写**。
#
# ── 两个架构（arm / amd）────────────────────────────────────────────────
#
# 支持 **`linux/amd64` 与 `linux/arm64`**（arm64 = aarch64；**不含** 32 位 armv7 ——
# `crates/mcloud-upstream/Cargo.toml` 只给 `x86_64/aarch64-unknown-linux-musl` 开了
# vendored OpenSSL，armv7 要另加一条并重验一遍 ✗）。
#
# 这个 Dockerfile **不含任何架构判断**：在哪个架构的机器上编，就出哪个架构的镜像 ——
# 因为 alpine 的本机三元组就是那台的 musl 三元组，而两条 musl 目标都已开 vendored ✓。
# ⇒ **不要**在 x86 上 `--platform linux/arm64`（那会走 QEMU 模拟，本镜像的
# `lto="fat"` + vendored OpenSSL 在模拟下慢到不可用 ✗）；要在 arm 上编就在 arm 机器上编。
# CI（`.github/workflows/docker.yml`）就是这么做的：**每个架构各一台原生 runner**，
# 各编一份、各自冒烟，最后合并成一个多架构清单列表 ✓。
#
# 三条硬约束（都是本仓既有的形状，镜像必须跟着走，不能另搞一套）：
#
# 1. **前端产物必须在 `cargo build` 之前就位**。`crates/mcloud-server/src/static_files.rs`
#    用 `#[derive(RustEmbed)]` 在**编译期**把 `web/dist` 读进二进制，而 `cargo` 不跟踪
#    `web/dist` 的变化（`build.rs` 里那句 `rerun-if-changed` 只管本机增量）。顺序反了会
#    内嵌旧产物 —— 表现是页面白屏 / chunk 404，而 `cargo build` 照样打 "Finished"。
#    ⇒ 所以这里**分两个阶段**：先 `npm run build` 出 `dist`，再 `cargo build`。
#
# 2. **一个二进制两种用法**（2026-10-03 起 CLI 与服务合并）：不带参数 = 起服务、
#    带子命令 = CLI ⇒ 镜像的 ENTRYPOINT 就是它，不带参数 ⇒ 默认行为就是起服务 ✓。
#
# 3. **TLS 走 `native-tls`**（兼容性优先：rustls 不实现 CBC 套件，连不上只给老套件的主机，
#    见根 `Cargo.toml` 里 reqwest 那一段）。Linux 上 `native-tls` = **OpenSSL**，于是：
#
#    * `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` 两条在
#      `crates/mcloud-upstream/Cargo.toml` 里已经开了 openssl 的 **`vendored`** ✓
#      ⇒ 本镜像**在 alpine 上编**（alpine 的本机三元组就是 musl）正好命中那两条配置，
#      OpenSSL 从源码编进来、**静态链**进二进制。
#      好处：运行时**不需要** libssl、也不依赖 glibc（拷进任何 Linux 都能跑的那份形态 ✓，
#      与 Release 里"拿不准就下静态那份"同一条路 ✓）。
#      代价：构建多花几分钟（要 `perl` + C 工具链编一遍 OpenSSL），见下面 `apk add` 那条。
#
#    * ⚠ 别把 `build-base perl` 删掉当优化：缺了它 `openssl-src` 编不出来，报的是
#      `openssl-sys` 的 "Could not find directory of OpenSSL installation"（看着像"系统没装
#      libssl"，其实是**我们主动要源码编**、而编它的工具不在 ✗）。
#
# ⚠ 需要 **BuildKit**（下面用了 `RUN --mount=type=cache`）：Docker 23+ 默认开启 ✓；
#    更老的引擎要 `DOCKER_BUILDKIT=1 docker build …`。
#
# ⚠ 构建要**联网**（拉基础镜像 + crates.io + npm registry + 那个 git 依赖
#    `@asunajs/lite`）。离线环境请走 `docs/docker.md` §9「不构建，直接用 Release 二进制」。

# 三处版本都提上来，改的时候只动这里。
# ⚠ `RUST_VERSION` 与 `.github/workflows/{ci,release}.yml` 钉的 **1.91.1** 保持一致：
#    版本漂了会出现"本地绿、CI 红"（跨 rustc 版本冒新 lint，`-D warnings` 下直接失败）。
# ⚠ `ALPINE_VERSION` 只管**运行时**那一层。静态 musl 二进制不吃运行时的 libc
#    ⇒ 它不必与 `rust:` 基础镜像内部的 alpine 版本一致 ✓（两处独立升级互不影响）。
ARG NODE_VERSION=22
ARG RUST_VERSION=1.91.1
ARG ALPINE_VERSION=3.22

# ── 阶段 1/3：前端产物（web/dist）────────────────────────────────────────
#
# 为什么是 `node:22-bookworm-slim`（glibc）而不是 alpine：`npm ci` 会装
# `@typescript/native-preview`（tsgo）与 `oxc-parser` 这类**原生**包，
# glibc 那套是它们的主场；换 alpine 就多一份"原生包有没有 musl 构建"的风险，
# 而这一层**不进最终镜像**（dist 拷走就完了）⇒ 体积上没有任何收益，只有风险 ✗。
FROM node:${NODE_VERSION}-bookworm-slim AS web

WORKDIR /src/web

# `git` 是**必须**的：`web/package.json` 里 `@asunajs/lite` 是 **git 依赖**
# （`git+https://github.com/asunajs/lite.git#v0.5.0`）⇒ `npm ci` 要调 git 去 clone。
# ⚠ 它**不是** `github:` 简写（那个会被解析成 `git+ssh://` ⇒ 公开仓也要密钥 ⇒ 装不上）；
# 这里用的是完整 https URL + 锁文件里的 commit SHA ⇒ 公开仓匿名可拉 ✓。
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# 先只拷清单再 `npm ci`：源码改动不会让这一层失效（依赖没变就命中缓存 ✓）。
# ⚠ `npm ci` 要 `package-lock.json`（已入库 ✓），且**必须**用它 —— `npm install`
# 会改写锁文件 ⇒ 每次构建的依赖树都可能不同 ✗。
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

# 源码（`.dockerignore` 里排掉了 `web/node_modules` 与 `web/dist`）
COPY web/ ./

# `npm run build` = `vite build && node scripts/gzip-sidecars.mjs`（顺带产出 `.gz` 边车，
# 服务端优先发边车、省掉每请求现压，见 static_files.rs 顶部那段实测）。
# ⚠ 这里**不**跑 `npm run typecheck`：类型检查是**门禁**（CI / 本地 `gates-web.mjs`）的事，
#    放进镜像构建只会让每次构建多花一份时间，且它失败时 CI 早就已经红了。
RUN npm run build

# ── 阶段 2/3：release 二进制（静态 musl）──────────────────────────────────
#
# 为什么在 alpine 上编：**它的本机目标就是 musl** ⇒ 不用 `--target`、不用 `cross`、
# 也不用 `Cross.toml`。而那两条 musl 目标的 `openssl = { features = ["vendored"] }`
# 配置（`crates/mcloud-upstream/Cargo.toml`）会自动生效 ⇒ 产物静态链 OpenSSL ✓。
# 与 release.yml 里 `x86_64-unknown-linux-musl` 那份是**同一个形态**（多平台同理：
# `aarch64-unknown-linux-musl` 也已经开了 vendored ⇒ 在 arm64 机器上直接编就行 ✓）。
FROM rust:${RUST_VERSION}-alpine AS build

# `build-base` = gcc/make/musl-dev（链接与编 C 都要）；`perl` = OpenSSL 的 Configure 要它。
# 两者都是 **vendored OpenSSL** 的构建依赖，不是运行时依赖（运行时装不下任何 libssl ✓）。
RUN apk add --no-cache build-base perl

WORKDIR /src

# 清单先来（同上：依赖层可复用）
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
# `test/` 一起带上是**故意**的：`crates/mcloud-crypto/src/tn.rs` 的黄金向量用例
# 用 `include_str!("../../../test/golden/crypto-vectors.json")` —— 它虽然在
# `#[cfg(test)]` 里、`cargo build` 不编它，但少这一份会让"想在构建阶段顺手跑测试"直接失败 ✗，
# 而带上它只占几百 KB ✓。
COPY test/ ./test/

# ⚠⚠ 这一行是**顺序**的全部意义所在：`rust-embed` 编译期要读 `web/dist`
#（`#[derive(RustEmbed)] folder = "../../web/dist"`，相对 `crates/mcloud-server/src/`）
# ⇒ 必须在 `cargo build` 之前把阶段 1 的产物放到位。
COPY --from=web /src/web/dist ./web/dist

# `--locked`：Cargo.lock 入库、锁死依赖版本（与 CI 同口径；它没跟上的话这里就会红）。
# `--bin mcloud`：只编这一个（工作区里也就这一个 bin）。**不要** `--workspace`。
#
# 两个 cache mount（要 BuildKit）：
#   * cargo registry —— 下载的 crate 压缩包，跨构建复用；
#   * target/ —— 编译产物。⚠ `target/` 被挂成 cache mount 后**不是镜像层**，
#     所以**必须在同一条 RUN 里**把二进制拷出去（分两条 RUN 的话第二个 RUN 看不见它 ✗）。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin mcloud \
 && cp target/release/mcloud /usr/local/bin/mcloud

# ── 阶段 3/3：运行时 ─────────────────────────────────────────────────────
#
# 里面只有一个**静态**二进制 ⇒ 运行时不装 libssl、不装 glibc 兼容层。
# 这两样是给"外设"用的，不是给二进制用的：
#   * `ca-certificates` —— 出网要验 TLS 证书链。⚠ OpenSSL 是静态链进来的，但**信任库在系统里**
#     ⇒ 不装它，所有 https 上游请求都会以"证书验证失败"告终（症状极具迷惑性：
#     二进制明明"静态到底"、却连不上任何 https）✗；
#   * `tzdata` —— 只影响**日志**：`data/server-logs/<日期>.jsonl` 的日期与日志时间戳走
#     `chrono::Local`（见 `logbuf.rs`）。⚠ 定时/编排的时区**不靠它**：`parse_timezone`
#     只认固定偏移（UTC / Asia/Shanghai / Asia/Tokyo / +HH:MM，见 scheduler.rs）⇒
#     就算不装 tzdata，定时也照旧准 ✓。
FROM alpine:${ALPINE_VERSION} AS runtime

RUN apk add --no-cache ca-certificates tzdata

# 容器里必须绑 0.0.0.0：绑 127.0.0.1 的话 `-p 3000:3000` 从宿主机打不进来
# （容器自己的 loopback 与宿主机不是同一个）⇒ 端口映射看起来"没生效" ✗。
# ⚠ 因此**暴露面由 compose / `-p` 决定**：默认只发到宿主机的 127.0.0.1（见 docker-compose.yml），
#   要局域网访问得自己改，并且先想清楚鉴权与 HTTPS（见 docs/docker.md §6）。
ENV MCLOUD_ADDR=0.0.0.0:3000 \
    MCLOUD_DATA_DIR=/data \
    TZ=Asia/Shanghai

# 非 root 跑（uid 1000 固定 ⇒ 绑定挂载的宿主机目录只要 `chown 1000:1000` 就对齐，
# 不必去猜镜像里分配了哪个 uid）。
# ⚠ `/app` 是 cwd：`MCLOUD_DATA_DIR` 被显式设成 `/data`，所以正常路径下不往 cwd 写东西；
#    给它 mcloud 的属主是为了"万一有人把 MCLOUD_DATA_DIR 去掉"时不至于一启动就 Permission denied ✗。
RUN adduser -D -u 1000 -h /app mcloud \
 && mkdir -p /data \
 && chown mcloud:mcloud /data /app

COPY --from=build /usr/local/bin/mcloud /usr/local/bin/mcloud

USER mcloud
WORKDIR /app

# 数据卷：账号 / Cookie / 定时 / 编排 / 日志 / 口令池全在 `MCLOUD_DATA_DIR` 下
# ⇒ 这一个卷就是**全部**需要备份的东西（见 docs/docker.md §5）。
VOLUME ["/data"]

EXPOSE 3000

# 健康检查打 `/api/health`（**不需要登录**的那个口，release.yml 的冒烟用的也是它）。
# ⚠ 端口从 `MCLOUD_ADDR` 里现取（`##*:` 剥掉 `ip:`）而不是写死 3000：
#   有人把 `MCLOUD_ADDR` 改成 `0.0.0.0:8080` 时，写死的话健康检查会**永远红**
#   （而它红了只表现为 "unhealthy"，很容易被当成"服务坏了" ✗）。
# ⚠ 用 busybox 自带的 `wget`：不额外装 curl（alpine 里没有 curl）。
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD wget -q -O /dev/null "http://127.0.0.1:${MCLOUD_ADDR##*:}/api/health" || exit 1

# 不带参数 = 起服务（带子命令 = CLI，两者同一个二进制）⇒ 想跑 CLI 就
# `docker run --rm -v mcloud-data:/data mcloud:local tasks`（见 docs/docker.md §11）。
ENTRYPOINT ["/usr/local/bin/mcloud"]
