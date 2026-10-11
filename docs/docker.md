# Docker（Linux）

面向**想用容器跑 mcloud** 的人。想直接拿二进制跑看 [`docs/usage.md`](usage.md)；
Dockerfile 里那些"为什么这么写"的注释在 [`Dockerfile`](../Dockerfile) 本身。

> ⚠⚠ **先读这一条**：这份 Docker 支持是 2026-10-10 加的，而**开发机上没有 Docker**
> ⇒ 镜像**没有被本机跑起来过**。哪些验过、哪些没验、怎么自己验，都写在 §13。
> 别把"文档写得细"读成"跑过了"。

---

## 1. 三条命令

```bash
git clone https://github.com/asunajs/mcloud.git && cd mcloud
docker compose up -d --build      # 首次：构建镜像（十几分钟）+ 起服务
open http://127.0.0.1:3000        # 首次进「创建管理员」向导
```

之后常用的是这三条：

```bash
docker compose logs -f mcloud     # 看日志（= docker logs -f mcloud）
docker compose restart mcloud     # 重启（改了环境变量、或改了口令/用户之后）
docker compose down               # 停（数据在命名卷里，不会丢）
```

不想用 compose 的话，等价的一条：

```bash
docker build -t mcloud:local .
docker run -d --name mcloud --restart unless-stopped \
  -p 127.0.0.1:3000:3000 -v mcloud-data:/data mcloud:local
```

**不想自己编**（CI 已经出了多架构镜像，`docker pull` 会按你的机器自动挑 amd64 / arm64）：

```bash
docker pull ghcr.io/asunajs/mcloud:latest
docker run -d --name mcloud --restart unless-stopped \
  -p 127.0.0.1:3000:3000 -v mcloud-data:/data ghcr.io/asunajs/mcloud:latest
```

（标签规则、私有包的登录、arm64 那份怎么来的，见 §10。）

---

## 2. 镜像里是什么

**两个架构**：`linux/amd64` 与 `linux/arm64`（arm64 = aarch64）。
⚠ **不含 32 位 armv7** —— `crates/mcloud-upstream/Cargo.toml` 只给 `x86_64` / `aarch64`
两条 musl 目标开了 vendored OpenSSL，armv7 要另加一条并重验一遍。
CI 出的那份是**多架构清单列表**（一个标签含两个架构），`docker pull` 时按你的机器自动挑 ✓；
自己 `docker build` 时**在哪个架构的机器上编就出哪个架构**（原生，不走模拟 —— 见 §8）。

| 内容 | 说明 |
|---|---|
| 一个**静态**二进制 `mcloud` | musl 静态链（含 vendored OpenSSL）⇒ 运行时不需要 libssl、也不依赖 glibc |
| `ca-certificates` | 出网验 TLS 证书链要它（⚠ OpenSSL 是静态链进来的，但**信任库在系统里**） |
| `tzdata` | 只影响日志日期与时间戳（`data/server-logs/<日期>.jsonl`）；**定时/编排的时区不靠它** |
| 一个非 root 用户 `mcloud` | **固定 uid 1000** ⇒ 绑定挂载的宿主机目录只要 `chown 1000:1000` 就对齐 |

基础镜像是 `alpine`（运行时）—— **不是**为了"alpine 更小"这个笼统理由，而是因为
**在 alpine 上编出来的就是 musl 静态**，与 Release 里"拿不准就下静态那份"是同一条路：
产物不依赖运行时的 libc / libssl。代价是构建多花几分钟（`openssl-src` 要从源码编一遍）。

> 体积：**≈ 20 MB**（alpine ≈ 8 MB + 静态二进制 ≈ 10 MB + 证书/时区数据 ≈ 4 MB）。
> ⚠ 这个数是**估**的，没实测（本机没有 Docker ⇒ 见 §13）。参考读数：Release 里那份
> 静态 musl 二进制是 **10 MB**（`docs/task-log.md` 第 130 行）。
> 想要更小见 §8 末尾的 UPX 那一段（Release 流水线对 Linux 产物就是压的）。

镜像里**没有**：glibc、libssl、`web/dist` 目录（前端产物在编译期嵌进二进制了）、
任何源码或 `.git`（⇒ 见 §12「关于卡片提交信息为空」那条）。

---

## 3. 环境变量

镜像里已经写好的默认值（`docker run` 时就是这些）：

| 变量 | 镜像默认 | 说明 |
|---|---|---|
| `MCLOUD_ADDR` | `0.0.0.0:3000` | ⚠ 容器里**必须**是 `0.0.0.0`，绑 `127.0.0.1` 的话端口映射打不进来 |
| `MCLOUD_DATA_DIR` | `/data` | 数据目录（= 那个卷） |
| `TZ` | `Asia/Shanghai` | 日志日期/时间戳用；`-e TZ=UTC` 可覆盖 |
| `MCLOUD_COOKIE_SECURE` | 关 | 走 HTTPS 时打开（见 §6） |
| `MCLOUD_ALLOW_LOCAL_PUSH` | 关 | ⚠ 危险开关：放行"推送地址指向本机/内网"（盲 SSRF 那道闸，见 `docs/usage.md` §2） |
| `RUST_LOG` | 服务 `info,mcloud=debug` | 排查上游时用 `info,mcloud=trace`（正文已脱敏） |

⚠ 是 `MCLOUD_DATA_DIR`，**不是** `MCLOUD_DATA`；`PORT` 会被忽略。
三个变量的完整口径见 [`docs/usage.md`](usage.md) §2（`MCLOUD_ALLOW_LOCAL_PUSH` 那一节
解释了为什么它默认关着，以及为什么它是**服务级**而不是页面里能勾的）。

---

## 4. 数据、备份、升级

**全部**持久化数据都在 `MCLOUD_DATA_DIR`（= `/data`）下：账号、Cookie、会话、定时、编排、
历史、运行日志、服务端日志、口令池。**换一个卷 = 换一套全新数据**。

```bash
# 备份（卷名是 compose 的 <项目名>_<卷名>；本项目 name: mcloud）
docker run --rm -v mcloud_mcloud-data:/data -v "$PWD":/backup alpine \
  tar -czf /backup/mcloud-data-$(date +%F).tar.gz -C /data .

# 恢复：先停服务，再把内容解回去
docker compose down
docker run --rm -v mcloud_mcloud-data:/data -v "$PWD":/backup alpine \
  sh -c 'rm -rf /data/* && tar -xzf /backup/mcloud-data-<日期>.tar.gz -C /data'
docker compose up -d
```

⚠ 备份里是**凭证**（Cookie / 口令哈希 / 会话）⇒ 按密钥对待（别扔公共网盘）。
仓库本身也把 `data/` 挡在 `.gitignore` 与 `.dockerignore` 之外（同一条口径）。

**升级**：

```bash
git pull && docker compose up -d --build     # 重建镜像；数据卷不动
```

**绑定挂载（`./data:/data`）** —— 想把数据放在仓库目录旁边时：

```bash
mkdir -p ./data && sudo chown -R 1000:1000 ./data      # ⚠ 漏了这步会 Permission denied
# compose 里改成： - ./data:/data
```

**⚠ 别把同一个卷挂给两个容器**：存储层是 JSON 文件 + 进程内缓存（`JsonStore` 启动时全量读进
内存），**没有跨进程锁** ⇒ 两个实例写同一份文件会互相覆盖。

---

## 5. 第一次用

1. 浏览器打开 <http://127.0.0.1:3000> ⇒ **「创建管理员」向导**（设用户名 + 口令，≥ 8 位）；
2. 「账号」页登录云盘账号（扫码 / 短信 / App 短信 / 账号密码 / 设备号轮换）；
3. 「任务」页跑任务，「定时」页排班。

这三步与二进制部署**完全一样**（同一份后端、同一套页面）。

命令行（CLI 与服务是**同一个二进制**）：

```bash
docker exec -it mcloud mcloud tasks              # 列出任务
docker exec -it mcloud mcloud run sign <账号-id>  # 跑一次
# 不想 exec 也行（--rm：用完就删）
docker run --rm -v mcloud_mcloud-data:/data mcloud:local tasks
```

⚠⚠ **在容器里用 CLI 改用户/口令之后必须重启容器**：`mcloud user add|passwd` 写的是
`data/users.json`，而 `JsonStore` 是**启动时全量读进内存**的 ⇒ 不重启的话用它登录会 401，
而且服务下一次写这个文件会拿陈旧快照整份覆盖，刚加的人就没了。
（`docker compose restart mcloud` 即可；这条坑与二进制部署同源，见 `AGENTS.md`。）

---

## 6. 端口与访问控制

| 你想要 | 怎么做 | 先想清楚什么 |
|---|---|---|
| **只本机用**（默认） | compose 里就是 `127.0.0.1:3000:3000` | 什么都不用改 ✓ |
| **局域网/公网访问** | 改成 `3000:3000`（或 `-p 3000:3000`） | 登录页会**直接暴露** ⇒ 先按下面那条挂 HTTPS |
| **HTTPS** | 前面挂反代/隧道（§7） | 设 `MCLOUD_COOKIE_SECURE=1`，否则会话 cookie 在明文 HTTP 上也会发 |

两条与容器直接相关的注意：

* **`MCLOUD_ADDR` 必须留在 `0.0.0.0:3000`**。暴露面由 `ports` 那行决定 ——
  「容器里绑什么」与「宿主机上放不放出去」是两件事，别只看其中一个。
* ⚠ **反代后面限速会退化成"所有客户端共用一个桶"**。认证面（`POST /api/session`、
  `POST /api/setup`）的限速按 **peer IP** 分桶，而 `X-Forwarded-For` 本仓**完全没读**
  （读它就等于信任一个可伪造的头）。后果：反代后面所有请求都来自反代那一个 IP ⇒
  一个人拿错口令猛试，会把**管理员**也一起挡在登录页外（429）。
  这是**刻意**的取舍（宁可粗，也不要"拿不到键就不限"），完整口径写在
  `crates/mcloud-server/src/ratelimit.rs` 的文件头。实践上的两条：
  ① 别把登录页摊在不受信任的网络上（本仓是**多用户**自托管，不是公网服务）；
  ② 真要公网，就在反代那层再加一道 IP 白名单 / WAF / 基本认证。

---

## 7. 反向代理示例

**Caddy**（自动申请证书，最省事）：

```caddyfile
mcloud.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

**nginx**：

```nginx
server {
    listen 443 ssl;
    server_name mcloud.example.com;
    ssl_certificate     /etc/letsencrypt/live/mcloud.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/mcloud.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        # SSE（实时日志）要关缓冲，否则事件会被 nginx 攒着一起发
        proxy_buffering off;
        proxy_read_timeout 3600s;
    }
}
```

两种情况都记得：`MCLOUD_COOKIE_SECURE=1`（compose 里那一行是注释掉的，取消注释即可）。

⚠ SSE 那条不是"优化"：WebUI 跑任务时的实时日志走 `GET /api/runs/{id}/events`，
反代默认的响应缓冲会让它看起来"卡住不动"。

---

## 8. 构建镜像

```bash
docker build -t mcloud:local .            # 在**本机架构**上原生编（Docker 23+ 默认就有 BuildKit）
```

要知道的五件事：

1. **要联网**：拉基础镜像 + crates.io + npm registry + 那个 git 依赖
   （`@asunajs/lite`，`web/package.json` 里的 git URL）⇒ 离线环境走 §9。
2. **耗时**：冷启动 ≈ 十几分钟（`npm ci` + 全量 Rust release 编译；`lto = "fat"` +
   `codegen-units = 1` + vendored OpenSSL 都要时间）。第二次构建靠 cache mount 快很多。
   ⚠ 这个数是**估**的（见 §13）。
3. **平台（arm / amd 怎么来）**：这个 Dockerfile **不含任何架构判断** ——
   在哪个架构的机器上编就出哪个架构的镜像（`linux/amd64` 或 `linux/arm64`），
   因为 alpine 的本机三元组就是那台的 musl 三元组，而两条 musl 目标都已开 `vendored` ✓。

   | 你的机器 | 直接 `docker build` 出什么 | 要另一个架构怎么办 |
   |---|---|---|
   | x86_64（多数服务器 / Intel Mac） | `linux/amd64` | arm64 去 arm 机器上编（见下） |
   | arm64（Apple 芯片 / arm 服务器） | `linux/arm64` | amd64 去 x86 机器上编 |

   ⚠ **别在 x86 上用 `--platform linux/arm64`**：那会走 QEMU 模拟，而本镜像的
   `lto="fat"` + vendored OpenSSL 在模拟下慢到不可用 ✗（CI 的做法是**给每个架构各一台
   原生 runner**，见 §10 —— 这条路才是能用的）。
   ⚠ **不含 32 位 armv7**（要的话得先给 `armv7-unknown-linux-musleabihf` 开 vendored 并验一遍）。

   想一次出两个架构（**慢**，只适合放在 arm64 机器上让它编 amd64 反向模拟，或纯粹图省事）：

   ```bash
   docker buildx build --platform linux/amd64,linux/arm64 --push -t <你的仓库>/mcloud:local .
   ```

   验"编出来的到底是哪个架构"（**别靠猜**）：

   ```bash
   docker image inspect --format '{{.Architecture}}' mcloud:local     # 期望 amd64 或 arm64
   docker buildx imagetools inspect <你的仓库>/mcloud:local           # 多架构清单时看这一条
   ```
4. **版本钉在哪**：Dockerfile 顶部的 `NODE_VERSION` / `RUST_VERSION` / `ALPINE_VERSION`
   三个 ARG。`RUST_VERSION` 与 CI 钉的 **1.91.1** 保持一致（漂版本会"本地绿、CI 红"）。
   ⚠ 三个基础镜像的 tag 是**钉死**的（`rust:1.91.1-alpine` 这种形状）。万一某个 tag 在镜像
   仓库里不存在（上游删过旧 tag / 你用的仓库是内网镜像），**不用改文件**，用 build-arg 覆盖即可：

   ```bash
   docker build --build-arg RUST_VERSION=1.91 --build-arg ALPINE_VERSION=3.21 -t mcloud:local .
   ```

   ⚠ 这三个 tag **本机没能核对**（沙箱里连不上镜像仓库）—— 见 §13 那条。
5. **arm64 那份镜像与 Release 里的 arm64 二进制**不是同一条路编的：镜像是 CI 在
   **arm64 runner 上原生编**（alpine + musl），Release 那份走 `cross` 交叉编
   （aarch64-gnu）⇒ 两者都静态链 OpenSSL，但**不是同一个产物**，别互相替代 ✓。

**可选：把二进制 UPX 压一下**（Release 流水线对 Linux 产物就是这么干的，体积 ≈ 1/5）。
在 runtime 阶段 `COPY` 之后、`USER mcloud` 之前插一行：

```dockerfile
RUN apk add --no-cache upx && upx --best --lzma /usr/local/bin/mcloud && upx -t /usr/local/bin/mcloud
```

⚠ 压完**一定要让冒烟跑一遍**（`docker run` + 打 `/api/health`）：UPX 压坏二进制的表现就是
"加载即崩"。Release 流水线里的顺序也是先 `upx -t`、再真起服务冒烟 ✓。

---

## 9. 不构建：拿 Release 里的静态二进制

不想在这台机器上编 Rust（或没有网络编），用**已经编好的静态 musl 二进制**做一个镜像，
三行就够：

```dockerfile
# Dockerfile.from-release
FROM alpine:3.22
RUN apk add --no-cache ca-certificates tzdata \
 && adduser -D -u 1000 -h /app mcloud && mkdir -p /data && chown mcloud:mcloud /data /app
COPY mcloud /usr/local/bin/mcloud     # ← Release 里的 x86_64-unknown-linux-musl 那份
ENV MCLOUD_ADDR=0.0.0.0:3000 MCLOUD_DATA_DIR=/data TZ=Asia/Shanghai
USER mcloud
WORKDIR /app
VOLUME ["/data"]
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/mcloud"]
```

```bash
gh release download v3.0.0-rc.4 --pattern 'mcloud-*-x86_64-unknown-linux-musl.tar.gz'
tar -xzf mcloud-*-x86_64-unknown-linux-musl.tar.gz
docker build -f Dockerfile.from-release -t mcloud:local .
```

⚠ 一定挑 **`-musl`** 那份（静态、不依赖 glibc/libssl）；`-gnu` 那份要求
`glibc ≥ 2.39`，放进 `alpine` 里**跑不起来**（alpine 用的是 musl）。核对方式：

```bash
file ./mcloud-v*/mcloud       # 期望：statically linked
```

---

## 10. CI 出的镜像（ghcr.io，多架构）

`.github/workflows/docker.yml` 在 **`v*` tag** 上构建并推送到 `ghcr.io/asunajs/mcloud`。

**架构怎么来的（这条是关键）**：**每个架构各跑一台原生 runner**，不用 QEMU ——

| 平台 | runner | 为什么 |
|---|---|---|
| `linux/amd64` | `ubuntu-latest` | x86_64，原生编 |
| `linux/arm64` | `ubuntu-24.04-arm` | aarch64，**原生编**（不是模拟） |

两台各自 `--load` 起容器**在真机上冒烟**，并断言 `docker image inspect` 的 `Architecture`
就是该矩阵项期望值（防"以为在编 arm64、其实编了 amd64"那种**看起来全绿**的错）。
tag 上各自按架构名推一份（`<版本>-amd64` / `<版本>-arm64`），最后由 `merge` job 用
`docker buildx imagetools create` 合成**一个清单列表** ⇒ 用户 `docker pull` 时按机器自动挑 ✓。
合成之后还会**从 ghcr 拉回来再冒烟一遍**（清单解析错了只有真拉一次才看得出来）。

标签规则：

| tag | 镜像标签 |
|---|---|
| `v3.0.0` | `3.0.0`、`latest`、`sha-<短提交>`（**清单列表**，含 amd64 + arm64） |
| `v3.0.0-rc.4` | `3.0.0-rc.4`、`sha-<短提交>`（**不打 `latest`** —— RC 不该冒充正式版） |

```bash
docker pull ghcr.io/asunajs/mcloud:latest          # 自动按你的机器挑架构
docker buildx imagetools inspect ghcr.io/asunajs/mcloud:latest   # 看清单里到底有哪几个架构
# 私有仓库的包要带凭据：
echo "$GITHUB_TOKEN" | docker login ghcr.io -u <你的用户名> --password-stdin
```

⚠ 单架构那份也留着（`<版本>-amd64` / `<版本>-arm64`），想钉死架构时可以拉它。

**手动触发**（⚠ 只构建 + 冒烟，**不推送** —— 免得点一下就留一份来源不明的镜像）：

```bash
gh workflow run docker.yml                    # 两个架构都编 + 都冒烟（默认）
gh workflow run docker.yml -f arm64=false     # 只编 amd64（arm runner 可能要计费时用）

gh run list --workflow=docker.yml -L1         # 拿 run id
gh run watch <run-id>                         # 跟到结束（构建 + 冒烟）
```

⚠ 计费：`release.yml` 里有一条 2026-10-02 的注释写着「私有仓库的 arm runner 要额外计费」
（口径以 GitHub 当时的价格为准）。所以 arm64 那条留了开关（`-f arm64=false`）；
tag 上默认两个架构都出，要关就删 `prepare` 里 arm 那条矩阵项。

⚠ `workflow_dispatch` 有个 GitHub 的规矩：**这条 workflow 必须已经存在于默认分支上**
才能手动触发 ⇒ 本地改完得先把它 push 上去（在别的分支上点不动）。

**这就是这份 Docker 支持的验证入口** —— 它真构建、真起容器、真打 `/api/health`、
**真断言架构**，比"看起来没问题"强得多（§13）。

---

## 11. 常见故障

| 症状 | 原因 / 怎么修 |
|---|---|
| 端口映射"没生效"，宿主机连不上 | 容器里绑成了 `127.0.0.1` ⇒ `MCLOUD_ADDR` 必须是 `0.0.0.0:3000` |
| 启动就 `Permission denied`（写在 `/data` 上） | 绑定挂载的宿主机目录属主不是 **uid 1000** ⇒ `sudo chown -R 1000:1000 ./data` |
| `exec format error` / 拉到的镜像跑不了 | **架构不对**（清单里没有你的架构，或你拉的是 `-amd64` 那份）⇒ `docker buildx imagetools inspect <镜像>` 看清单；`docker image inspect --format '{{.Architecture}}' <镜像>` 看本地这份。⚠ x86 上要跑 arm64 镜像得先装 `binfmt_misc`（`docker run --platform linux/arm64` 会走 QEMU，慢） |
| 所有 https 上游请求都失败 / 证书验证失败 | 运行时缺信任库 ⇒ 别删掉 `apk add ca-certificates`（它不在二进制里） |
| 日志文件日期差一天 / 时间戳不是本地时间 | `TZ` 没设或时区名写错 ⇒ 装的是 `tzdata`，`-e TZ=Asia/Shanghai` |
| 反代后面登录频繁 429 | 见 §6 那条"限速退化成单桶"（`X-Forwarded-For` 本仓不读） |
| SSE 实时日志"卡住不动" | 反代缓冲 ⇒ `proxy_buffering off`（§7） |
| `docker compose ps` 显示 `unhealthy` | 健康检查打的是 `/api/health`，端口从 `MCLOUD_ADDR` 现取 ⇒ 改了 `MCLOUD_ADDR` 之后要一起看；排查用 `docker logs mcloud` |
| 「关于」卡片里提交信息为空 | **预期**：`.dockerignore` 排掉了 `.git`，`build.rs` 拿不到就写空串（见 §12） |
| 定时/编排的时区不对 | 与 `TZ` **无关**：时区在数据里（`parse_timezone` 只认固定偏移 UTC / Asia/Shanghai / Asia/Tokyo / `+HH:MM`）⇒ 去页面里改 |

看日志：

```bash
docker logs -f mcloud                      # 标准输出（stdout）
docker exec mcloud ls /data/server-logs    # 服务端日志落盘（一天一个 .jsonl）
docker exec mcloud ls /data/logs           # 运行日志（一次运行一个文件）
```

---

## 12. 与二进制部署的已知差异

| 事 | 差异 | 为什么 |
|---|---|---|
| 「关于」卡片 | **提交号 / 改动清单为空**（版本号正常） | `.dockerignore` 排掉 `.git`（50 MB）；`build.rs` 拿不到 git 就给空串。想要那一格有内容就注掉 `.dockerignore` 里那行 |
| 二进制是否 UPX 压过 | 默认**不压** | 想压见 §8 末尾（Release 流水线是压的） |
| `MCLOUD_ADDR` 默认值 | `0.0.0.0:3000`（二进制是 `127.0.0.1:3000`） | 容器里绑 loopback 的话端口映射打不进来 |
| `TZ` | 默认 `Asia/Shanghai` | 只影响日志日期；二进制部署下跟宿主机走 |
| arm64 那份**怎么编出来的** | 镜像：CI 在 **arm64 runner 上原生编**（alpine + musl）；Release 二进制：`cross` **交叉编**（aarch64-gnu） | 两者都静态链 OpenSSL，但**不是同一个产物**，别互相替代 ✓ |
| 架构面 | 镜像有 **arm64 + amd64 两份**（多架构清单）；Release 只有 arm64 与 x86_64（**没有 32 位 arm**） | 见 §2 那条 |

---

## 13. ⚠ 验过没有 / 怎么自己验

**开发机上没有 `docker` 命令**（`which docker` 为空）⇒ 镜像**没有被构建过、也没有被跑过**。
下面这张表把"真做过的检查"与"没验的"分开写，别混着读：

| 项 | 状态 |
|---|---|
| Dockerfile 的阶段依赖 / COPY 顺序 / 工具链 | ✓ 静态核对（`rust-embed` 要 `web/dist` 在 `cargo build` **之前** ✓；`target/` 是 cache mount ⇒ 二进制必须在**同一条 `RUN`** 里拷出去 ✓；两条 musl 目标已开 `vendored` ⇒ alpine 上能静态链 OpenSSL ✓） |
| 健康检查里的端口推导 `${MCLOUD_ADDR##*:}` | ✓ **真跑过**：`MCLOUD_ADDR=0.0.0.0:8080 sh -c 'echo ${MCLOUD_ADDR##*:}'` ⇒ `8080` ✓（POSIX 参数展开，busybox ash 一样） |
| `.github/workflows/docker.yml` 的 YAML | ✓ `ruby -ryaml -e 'YAML.load_file(".github/workflows/docker.yml")'` 通过 ✓（含 `fromJSON` 的动态矩阵：`prepare` 吐的 JSON 也单独解析过 ✓） |
| 该 workflow 的每段 `run:` shell | ✓ 抽出来（`${{ … }}` 换占位符）逐段 `bash -n` 通过 ✓ |
| `docker-compose.yml` 的 YAML | ✓ 同上用 ruby 解析通过 ✓（⚠ `docker compose config` **没跑过** —— 本机没有 docker） |
| 文档里的相对链接 | ✓ 0 断链（README / docs/docker.md / docs/usage.md / AGENTS.md / conventions.md） |
| **基础镜像 tag 是否存在** | ✗ **没核对**（沙箱里连不上镜像仓库）。`node:22-bookworm-slim` / `rust:1.91.1-alpine` / `alpine:3.22` 这三个是**按官方镜像的命名规律**写的（⚠ 它们都得**有 arm64 变体**才编得出 arm64 那份 —— 官方这三个都是多架构的，但同样没核对）；不存在时用 §8 那条 build-arg 覆盖 |
| **镜像能构建出来** | ✗ **没验**（本机没有 Docker） |
| **镜像能起、`/api/health` 通** | ✗ **没验**（同上；`docker.yml` 的冒烟就是为它写的） |
| **arm64 镜像能编出来 / 跑起来** | ✗ **没验**（同上）。链路里的已知事实：`aarch64-unknown-linux-musl` 那条**已开 `vendored`** ✓；`ubuntu-24.04-arm` 这个 runner 标签**没在真 Actions 上跑过**（`release.yml` 从没用过它 ⇒ 名字对不上就会是"workflow 起不来"，一眼能看出 ✓） |
| **多架构清单合并**（`imagetools create`） | ✗ **没验**（要有 tag 才跑得到）；⚠ 这一步的失败模式是"用户拉到 amd64 却以为有 arm64" ⇒ 所以 `merge` 后面接了一条**拉回来冒烟** ✓ |
| 静态二进制不依赖 glibc / libssl | ✓ 同形态的产物在 Release 流水线里验过（`ldd` 必须是 `not a dynamic executable`，且二进制里 `GLIBC_` 命中 0 次） |
| 构建耗时 / 镜像体积 | ✗ **估**的（§8 / §2） |

自己验（**推荐顺序**）：

```bash
# ① 本地有 Docker：一条命令真跑起来
docker build -t mcloud:local . && docker run --rm -p 127.0.0.1:3000:3000 mcloud:local
curl -s http://127.0.0.1:3000/api/health        # 期望 {"status":"ok"}

# ② 本地没有 Docker：走 CI（构建 + 起容器冒烟，不推送任何东西）
gh workflow run docker.yml && gh run list --workflow=docker.yml -L1
# ⚠ 这条 workflow 要先 push 到默认分支，`workflow_dispatch` 才点得动（GitHub 的规矩）

# ③ 不需要 Docker 也能做的静态核对
grep -n "COPY --from=web" Dockerfile             # 必须在 cargo build 之前
ruby -ryaml -e 'YAML.load_file("docker-compose.yml")' && echo "compose YAML ok"
grep -nE '^(data|target|\.git|web/dist)/' .dockerignore   # 这四样绝不能进构建上下文
```

验完把结论写回 `docs/task-log.md`（这个仓库的规矩：**没验过就写没验过**，
"看起来没问题"不算证据）。
