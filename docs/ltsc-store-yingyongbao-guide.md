# Windows 11 LTSC:安装微软商店 + 启用应用宝(运行手机应用)

> 目标:在 Windows 11 LTSC 上恢复 Microsoft Store,并通过应用宝在 PC 上运行安卓手机应用。
> 适用:Windows 11 LTSC 2024(24H2)/ LTSC 2021;Windows 10 LTSC 亦可参考。
>
> ✅ **本文方案已在 Windows 11 IoT 企业版 LTSC (Build 26100) 上实测成功**:
> Microsoft Store v22608.1401.3.0、腾讯应用宝 v5.10.75.11 均已安装完成,
> 引擎服务 `AndrowsSvr` 处于 Running / Automatic。

---

## 0. 先搞清一个关键事实(避免走错路)

**WSA(Windows Subsystem for Android)已被微软终止**,亚马逊商店退场,2025-03-05 起 WSA 上的安卓应用全部停止运行。

**现在的应用宝电脑版不走 WSA**,而是腾讯自研的「移动应用引擎」(代号 **Androws**)。因此:

| 方案 | 是否依赖 WSA | LTSC 是否可行 | 推荐度 |
|---|---|---|---|
| A. 应用宝电脑版**独立客户端** | 否 | ✅ 完全可行(已实测) | ★★★★★ |
| B. 微软商店里的「**腾讯应用宝专区**」 | 否 | ⚠️ 商店可装,但专区分发有区域/版本限制 | ★★★ |

**结论:装商店与跑手机应用是两件解耦的事。** 应用宝独立客户端就能跑手机应用,商店是"顺带装一下更完整"。

---

## 1. 硬件与系统前置检查

应用宝电脑版官方要求(独立版与商店版基本一致):

- **CPU**:Intel 8 代及以上,或同级 AMD(需支持虚拟化 VT-x / AMD-V)
- **内存**:8 GB 及以上(建议 16 GB)
- **系统**:Windows 10 及以上
- **其他**:BIOS 中开启虚拟化;系统区域建议设为「中国」

### 检查命令(管理员 PowerShell 中执行)

```powershell
# 1. 系统版本
winver

# 2. CPU 型号与核心
Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors

# 3. 内存
Get-CimInstance Win32_ComputerSystem | Select-Object @{n='RAM_GB';e={[math]::Round($_.TotalPhysicalMemory/1GB,1)}}

# 4. 虚拟化是否开启(CIM 方式,无需 systeminfo 的性能开销)
(Get-CimInstance Win32_Processor).VirtualizationFirmwareEnabled
# 返回 True 即已在固件层开启;若 False,需进 BIOS 开启 VT-x / SVM

# 5. 系统区域
Get-WinSystemLocale | Select-Object Name, DisplayName
# 建议为 zh-CN
```

---

## 2. 方案 A:安装应用宝电脑版独立客户端(推荐,已实测)

### 步骤

**方式 1:图形界面安装(最简单)**

1. 打开浏览器访问官方下载页:**https://sj.qq.com/download**
2. 点击「**Windows 版**」下载安装包(约 6.3 MB,安装时再释放引擎)
3. 双击安装。安装程序会自动部署腾讯 Androws 移动应用引擎(约 120 秒)
4. 首次启动会做环境自检;若提示虚拟化未开启,回 BIOS 打开 VT-x / AMD-V 后重启

**方式 2:命令行静默安装(实测有效)**

```powershell
# 官方安装包直链(从官网 JS 中解析所得)
$url = 'https://down.pc.yyb.qq.com/channel/55/115/10000/pcyyb__installer.exe'
$exe = "$env:TEMP\pcyyb_installer.exe"
Invoke-WebRequest -Uri $url -OutFile $exe

# NSIS 静默安装(参数必须是大写 /S)
Start-Process -FilePath $exe -ArgumentList '/S' -Wait
```

**安装结果(实测)**:

| 项目 | 值 |
|---|---|
| 版本 | 5.10.75.11 |
| 安装目录 | `C:\Program Files\Tencent\Androws\Application` |
| 启动器 | `AndrowsLauncher.exe` |
| 引擎 | `Application\5.10.7500.6534\AndrowsSvr.exe` |
| 服务 | `AndrowsSvr` — Running / Automatic |
| 协议 | `HKLM\SOFTWARE\Classes\androws` |
| 卸载项 | `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Androws` |

### ⚠️ 必须开启虚拟化平台组件

LTSC 默认**关闭**了虚拟机平台,应用宝的安卓引擎无法运行。实测需开启:

```powershell
Enable-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform -All -NoRestart
Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All -NoRestart
```

> 执行后需**重启**才能生效。开启前这两项为 `Disabled`,开启后为 `Enabled`。

BIOS 中还需确认已开启 **Intel VT-x** 或 **AMD SVM**。

### 常见报错与处理

| 现象 | 原因 | 处理 |
|---|---|---|
| 启动提示"虚拟化未开启" | BIOS 未开 VT-x/SVM | 进 BIOS 开启 Intel VT-x 或 AMD SVM |
| 引擎无法启动 | VirtualMachinePlatform 未启用 | 执行上面的 Enable-WindowsOptionalFeature 并重启 |
| 启动卡在初始化 | Hyper-V/WSL 等虚拟化组件冲突 | `bcdedit /set hypervisorlaunchtype auto` 后重启 |
| 安装包安装失败 | 系统组件缺失(VC++ 运行库) | 安装最新 Visual C++ Redistributable x64 |
| 应用打不开 / 黑屏 | 显卡驱动老旧 | 更新显卡驱动(intel/AMD/NVIDIA 官方) |
| 内存不足 | 8 GB 勉强 | 关闭后台程序;建议升到 16 GB |

> ⚠️ 不要安装第三方"应用宝绿色版/破解版",下载一律走官方域名 `sj.qq.com` / `pc.qq.com` / `down.pc.yyb.qq.com`。
> 安装包应验证数字签名:`Get-AuthenticodeSignature xxx.exe`,签名者须为 `Tencent Technology (Shenzhen) Company Limited`。

---

## 3. 方案 B:先装微软商店,再用商店里的「腾讯应用宝专区」

### 3.1 在 LTSC 上安装 Microsoft Store

**方法 1(简单,但 LTSC 上常不生效)**:管理员 PowerShell 执行

```powershell
wsreset -i
```

等待几分钟,商店会自动出现。**实测在 LTSC 2024 上此法无效**,请用方法 2。

**方法 2(推荐,实测有效)**:离线包直接安装

> 实测步骤(Windows 11 IoT 企业版 LTSC Build 26100 通过):

1. **先杀掉占用进程**(否则报 `0x80073D02 资源正在使用中`):

```powershell
foreach($n in 'gamingservices','gamingservicesnet','XboxPcApp','XboxPcAppFT','XboxPcTray'){
  Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force
}
```

2. **获取商店安装包**:访问 https://store.rg-adguard.net/ ,输入
   `Microsoft.WindowsStore_8wekyb3d8bbwe`,Ring 选 **RP**,Channel 选 **Retail**,
   下载最新的 `Microsoft.WindowsStore_*.msixbundle`(约 126 MB)。
   同时下载 3 个 x64 依赖:
   - `Microsoft.NET.Native.Framework.2.2_*_x64__8wekyb3d8bbwe.appx`
   - `Microsoft.NET.Native.Runtime.2.2_*_x64__8wekyb3d8bbwe.appx`
   - `Microsoft.UI.Xaml.2.8_*_x64__8wekyb3d8bbwe.appx`

3. **安装**(依赖通常已在系统中,可跳过):

```powershell
Add-AppxPackage -Path ".\Microsoft.WindowsStore_22608.1401.3.0_neutral_~_8wekyb3d8bbwe.msixbundle"
```

4. 验证:

```powershell
Get-AppxPackage -Name Microsoft.WindowsStore | Select-Object Name, Version
```

**方法 3(图形化备选)**:装 Xbox App 后,依次点「Browse the catalog」→「review now」→「install Microsoft Store」。

**方法 4(封装场景)**:开源项目 LTSC-Add-MicrosoftStore
- https://github.com/nickweiminhan/LTSC-Add-MicrosoftStore-
- 解决了 `wsreset -i` 在封装时"非所有用户安装"导致的报错

### 3.2 在商店里启用「腾讯应用宝专区」

商店装好后:

1. 打开 Microsoft Store → 顶部搜索「**腾讯应用宝**」,或直接访问:
   **https://apps.microsoft.com/tencent?hl=zh-CN&gl=CN**
2. 安装应用宝(商店版会自动部署腾讯移动应用引擎)
3. 之后即可在商店内直接安装安卓 App

### 3.3 重要限制(务必先看)

商店版「腾讯应用宝专区」的完整分发此前有额外门槛:

- 需要**加入 Windows 预览体验计划(Insider)**,且处于 **Dev / Canary 频道**
- 商店版本需 ≥ **22410.1402.100.0**
- 系统**区域必须为中国**

> ❗ **LTSC 无法加入 Insider 预览体验计划**(长期服务版不走该通道)。
> 因此如果你要的是"商店内那个应用宝专区",LTSC 上大概率看不到入口。
> **这种情况下直接走方案 A(独立客户端),功能完全一致。**

---

## 4. 两条路怎么选

```
你的目标:PC 上跑安卓手机应用
        │
        ├── 只要这个 → 方案 A,装独立客户端,收工
        │
        └── 顺便还想要微软商店(装微信/其他商店应用)
                 │
                 ├── LTSC 上能装商店(方法 2/3)
                 │
                 └── 但商店里的"应用宝专区"在 LTSC 上大概率不可见
                          → 安卓应用仍需用方案 A 的独立版
```

**一句话:装商店是为了商店本身;跑手机应用靠应用宝电脑版独立客户端。二者解耦。**

---

## 5. 一键检查脚本

同目录下的 `ltsc-env-check.ps1` 可一键输出环境体检结果与建议安装路径。

用法(管理员 PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -File .\ltsc-env-check.ps1
```

---

## 6. 参考来源

- 应用宝电脑版官方下载:https://sj.qq.com/download
- 应用宝商店页(微软):https://apps.microsoft.com/tencent?hl=zh-CN&gl=CN
- LTSC 安装商店教程:https://windiscover.com/posts/install-microsoft-store-on-windows-11-ltsc.html
- LTSC-Add-MicrosoftStore:https://github.com/nickweiminhan/LTSC-Add-MicrosoftStore-
- WSA 终止公告相关报道(应用宝与 WSA 无关,走腾讯移动引擎)
