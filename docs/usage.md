# 拿到二进制怎么用

面向**下载了 Release 压缩包**的人。整包里**只有一个可执行文件**，没有别的依赖：

| 文件 | 作用 |
|---|---|
| `mcloud` | 服务本体 + CLI：WebUI、定时/编排调度，以及命令行。**发布包里就它一个** |

> **一个二进制两种用法**（2026-10-03 起 `mcloud-server` 与 `mcloud-cli` 合并）：
> **不带参数 = 起服务、带子命令 = CLI**，见 §4。

产物按平台命名：unix 是 `mcloud-v<版本>-<target>.tar.gz`，Windows 是 `...zip`
（**不再有 `.tar.xz`** —— 一个平台只出一种包）。
`<版本>` 取自 `Cargo.toml` 的 `[workspace.package] version`（发 tag 时以 tag 名去掉 `v` 为准），
**别背数**，现算：

```bash
sed -n '/\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1
```

---

## 1. 解压与校验

```bash
# Linux / macOS：unix 一律 tar.gz
tar -xzf mcloud-v<版本>-x86_64-unknown-linux-gnu.tar.gz
cd mcloud-v<版本>-x86_64-unknown-linux-gnu

# 校验（包内附 .sha256）
shasum -a 256 -c ../mcloud-v<版本>-x86_64-unknown-linux-gnu.sha256
```

> ⚠ **x86_64 Linux 有两份，别拿错**（拿错的症状见下一节）：
> 拿不准就下 **`x86_64-unknown-linux-musl`** 那份 —— 它静态链了 glibc 与 OpenSSL，
> 任何 Linux x86_64 都能跑 ✓。

```powershell
# Windows
Expand-Archive mcloud-v<版本>-x86_64-pc-windows-msvc.zip -DestinationPath .
cd .\mcloud-v<版本>-x86_64-pc-windows-msvc
Get-FileHash ..\mcloud-v<版本>-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

⚠ **三个 Linux 产物不是一回事**（选错的那份，症状是
`./mcloud: /lib/x86_64-linux-gnu/libc.so.6: version GLIBC_2.39 not found` ✗）：

* **x86_64**（`x86_64-unknown-linux-gnu`）：动态链接 —— 构建于 Ubuntu 24.04
  （glibc 2.39）+ **系统** OpenSSL ⇒ 要在 **glibc ≥ 2.39** 且装了 `libssl3` 的发行版上跑
  （Ubuntu 24.04+、Debian 13+、Fedora 40+ 一类）。体积最小 ✓。
* **x86_64 静态**（`x86_64-unknown-linux-musl`，2026-10-06 起 ✓）：glibc 与 OpenSSL
  **都静态链进二进制** ⇒ **任何** Linux x86_64 都能跑（CentOS 7、Debian 11 这类老发行版
  也一样 ✓），运行时**什么都不用装** ✓；代价是体积大 2–3 MB ✓。**拿不准就下这份** ✓。
* **arm64**（`aarch64-unknown-linux-gnu`）：走 `cross` 交叉编 —— 镜像基线更老
  （glibc 2.23 那一档），且 OpenSSL 是 `vendored`（从源码编、**静态链**）⇒
  **不用装 `libssl`**，代价是体积大 2–3 MB。

判断手上那份是哪种，一条命令：

```bash
ldd ./mcloud        # 静态那份会说 "not a dynamic executable"（退出码非 0 是正常的 ✓）
```

⚠ 在这条之前，更老的发行版只能"自行从源码构建"（本文件原先就是这么写的 ✗）——
现在下**静态**那份即可 ✓。

## 2. 起服务

```bash
./mcloud                   # Linux / macOS
.\mcloud.exe               # Windows
```

**不带子命令**就是起服务（systemd / 托管那条命令行也是这个形状）。看到这两行就成了：

```
INFO mcloud: mcloud 启动 data_dir="data" tasks=<已注册任务条数>
INFO mcloud: 监听 http://127.0.0.1:3000
```

⚠ `mcloud --help` **不是**帮助：任何不认识的子命令都会打出用法文本然后 `exit 2`。

> `tasks=` 后面是**注册表里任务的条数**，随版本变，别背数 —— 现算：
> `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`。

然后用浏览器打开 **<http://127.0.0.1:3000>**。

* **首次运行**会进「创建管理员」向导（设用户名 + 口令，至少 8 位）；之后是登录闸。
* 数据（账号、Cookie、定时、日志）都在**运行目录下的 `data/`**。
  也就是说：**在哪个目录运行，数据就在哪里** —— 换个目录跑等于换一套全新数据。
* 想让它只监听本机（默认）就不要改 `MCLOUD_ADDR`；要局域网访问才改（见下）。

### 两个环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `MCLOUD_ADDR` | `127.0.0.1:3000` | 监听地址 |
| `MCLOUD_DATA_DIR` | `data`（相对当前目录） | 数据目录 |

```bash
MCLOUD_ADDR=0.0.0.0:3000 MCLOUD_DATA_DIR=/srv/mcloud ./mcloud
```

⚠ 用的是 `MCLOUD_DATA_DIR`，**不是** `MCLOUD_DATA`；`PORT` 环境变量会被忽略。

### 如果你是来改前端的

下载用户看到这里就够了。改前端要注意一处**默认值不一致**：
`web/vite.config.ts` 的 dev 代理写死目标是 **38123**，而服务默认监听 **3000**
⇒ 照着上面启动服务、再 `cd web && npm run dev`，页面能开但所有接口都会连不上（静默）。
两者对齐即可：

```bash
MCLOUD_ADDR=127.0.0.1:38123 ./mcloud   # 让后端去迁就 dev 代理
```

## 3. 平台注意事项（这三条都是踩过的）

**Linux** —— **x86_64** 那份用系统 TLS（`native-tls` → OpenSSL），需要目标机有 `libssl`
（**arm64** 那份是静态链的 vendored OpenSSL，不装也能跑）：

```bash
sudo apt install -y libssl3        # Debian/Ubuntu
sudo dnf install -y openssl-libs   # Fedora/RHEL
```

**macOS** —— 产物**未压缩**（UPX 会破坏 arm64 Mach-O 的代码签名，M 芯片上直接跑不起来），
但二进制没有签名，从浏览器下载会被 Gatekeeper 拦：

```bash
xattr -dr com.apple.quarantine ./mcloud
```

（或右键 → 打开 → 「仍要打开」。终端里 `tar` 解出来的通常不带 quarantine。）

**Windows** —— 产物**用 UPX 压过**（体积约为原来的 1/3），代价是
Defender / SmartScreen **可能误报**：弹窗里走「更多信息 → 仍要运行」，或把文件加白名单。
介意这一点就自行从源码构建（`npm run build` 后 `cargo build --release`）。

## 4. 命令行怎么用（可选）

**CLI 就在发布包那个二进制里**（2026-10-03 起原来两个 bin 合并成 `mcloud`）：
**不带参数 = 起服务、带子命令 = CLI** ✓。CLI 与服务**共用同一个数据目录** —— 想让 CLI
看到服务里配好的账号，要么在同一个目录下运行，要么给它同一个 `MCLOUD_DATA_DIR`。

顶层命令清单以 `crates/mcloud-server/src/cli/run.rs` 的分派为准，**别背数**，现算：

```bash
grep -n '^        Some("' crates/mcloud-server/src/cli/run.rs
```

```bash
mcloud                       # 无参数 = 起 WebUI 服务（默认 127.0.0.1:3000）
mcloud bogus                 # 不认识的子命令 = 打印用法并 exit 2
mcloud tasks                 # 列出可用任务
mcloud run sign <账号id>      # 跑一次任务；要参数的任务用 --body '<JSON>'
mcloud run live-room <账号id> --body '{"codes":"口令甲 口令乙","listenSeconds":45}'
mcloud login qr|sms|app|password|device    # 登录云盘账号
mcloud auth refresh --all    # 刷新凭据
mcloud pending               # 查看本次进程里还没清理的网盘产物
mcloud cron <定时id>          # 给系统 crontab 调用
mcloud pipeline run <编排id>
```

定时与编排的**创建**不在命令行 —— 在 WebUI 里配，或用上面 `cron` / `pipeline run`
让系统计划任务去触发。


## 5. 常驻与自启

**Linux（systemd）**

```ini
# /etc/systemd/system/mcloud.service
[Unit]
Description=mcloud
After=network.target

[Service]
User=mcloud
WorkingDirectory=/srv/mcloud
ExecStart=/srv/mcloud/mcloud
Environment=MCLOUD_ADDR=127.0.0.1:3000
Restart=always

[Install]
WantedBy=multi-user.target
```

**macOS（launchd）**：写一个 `~/Library/LaunchAgents/<label>.plist`，
`ProgramArguments` 指向 `mcloud` 的**绝对路径**，`KeepAlive` 设 `true`，
`WorkingDirectory` 指向放 `data/` 的目录。

⚠ 别把 `WorkingDirectory` 与 `MCLOUD_DATA_DIR` 搞成两处不一致 —— 那会出现
「服务明明在跑，账号却是空的」这类看起来像 bug 的现象。

## 6. 升级与卸载

* **升级**：停掉服务 → 用新包里的二进制覆盖旧的 → 再起。`data/` 不用动（放数据目录里就不会丢）；
* **卸载**：删二进制 + 删 `data/` 目录即可（它不往系统别处写东西）。
