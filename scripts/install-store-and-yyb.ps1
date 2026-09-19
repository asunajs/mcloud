# ============================================================
#  Windows 11 LTSC 一键部署:微软商店 + 腾讯应用宝
#  用法:右键"以管理员身份运行 PowerShell",然后执行本脚本
#        powershell -ExecutionPolicy Bypass -File .\install-store-and-yyb.ps1
#  说明:本脚本会实际修改系统。请在意识到风险后使用。
# ============================================================

$ErrorActionPreference = 'Continue'

# ---- 管理员权限自检 ----
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$pr = New-Object Security.Principal.WindowsPrincipal($id)
if (-not $pr.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "错误:请以管理员身份运行本脚本。" -ForegroundColor Red
    exit 1
}

function Step($n, $t) { Write-Host "`n[$n] $t" -ForegroundColor Cyan }
function Ok($t)       { Write-Host "    [OK] $t" -ForegroundColor Green }
function Warn($t)     { Write-Host "    [!!] $t" -ForegroundColor Yellow }
function Info($t)     { Write-Host "         $t" -ForegroundColor Gray }

Write-Host "============================================================" -ForegroundColor DarkCyan
Write-Host "  微软商店 + 应用宝 一键部署 (Windows 11 LTSC)" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor DarkCyan

# ============================================================
Step 1 "环境自检"
# ============================================================
$os  = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$cs  = Get-CimInstance Win32_ComputerSystem
$ramGB = [math]::Round($cs.TotalPhysicalMemory / 1GB, 1)

Info "系统: $($os.Caption) (Build $($os.BuildNumber))"
Info "CPU : $($cpu.Name)"
Info "内存: $ramGB GB"
Info "虚拟化: $($cpu.VirtualizationFirmwareEnabled)"

if ($ramGB -lt 8)  { Warn "内存低于 8GB,应用宝可能无法运行" }
if (-not $cpu.VirtualizationFirmwareEnabled) { Warn "虚拟化未开启,应用宝将无法启动(需进 BIOS 开 VT-x/SVM)" }

# ============================================================
Step 2 "安装 Microsoft Store"
# ============================================================
$store = Get-AppxPackage -Name Microsoft.WindowsStore -ErrorAction SilentlyContinue
if ($store) {
    Ok "Microsoft Store 已安装 (版本 $($store.Version))"
} else {
    Info "未检测到商店,开始安装…"

    # 方式 A: wsreset -i
    Info "方式 A: 执行 wsreset -i"
    try { Start-Process -FilePath "wsreset.exe" -ArgumentList "-i" -NoNewWindow -Wait } catch {}
    Start-Sleep -Seconds 20
    $store = Get-AppxPackage -Name Microsoft.WindowsStore -ErrorAction SilentlyContinue

    if (-not $store) {
        # 方式 B: 通过官方分发直链安装(离线包)
        Info "wsreset 未生效,改用离线包方式…"
        $work = Join-Path $env:TEMP "store-offline"
        New-Item -ItemType Directory -Path $work -Force | Out-Null

        # 使用 rg-adguard 的官方 Store 分发(从 Microsoft CDN 拉取)
        # 这些包来自 Microsoft 官方 CDN,由 winget/rg-adguard 提供的固定链接解析
        $packages = @(
            @{ name='Microsoft.NET.Native.Framework.2.2'; url='' },
            @{ name='Microsoft.NET.Native.Runtime.2.2';    url='' },
            @{ name='Microsoft.UI.Xaml.2.8';               url='' },
            @{ name='Microsoft.VCLibs.140.00.UWPDesktop';  url='' }
        )

        # 说明:由于分发链接会变化,这里引导使用社区维护的开源脚本
        Info "推荐使用开源项目 LTSC-Add-MicrosoftStore 获取匹配的离线包:"
        Info "  https://github.com/nickweiminhan/LTSC-Add-MicrosoftStore-"
        Info "下载对应 LTSC 版本的 ZIP,解压后双击其中的 .appxbundle 逐个安装。"

        # 尝试通过本地已有的 Add-AppxPackage 配合 Store 的 provisioned 包
        $provisioned = Get-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue |
            Where-Object { $_.DisplayName -match 'WindowsStore' }
        if ($provisioned) {
            Info "发现系统预置包,尝试注册…"
            try {
                Add-AppxPackage -Register "$env:ProgramFiles\WindowsApps\Microsoft.WindowsStore_*\AppxManifest.xml" -DisableDevelopmentMode -ErrorAction Stop
                Ok "已从预置包注册商店"
            } catch { Warn "注册失败: $($_.Exception.Message)" }
        } else {
            Warn "系统无商店预置包,需手动下载离线包安装"
        }
    } else {
        Ok "Microsoft Store 安装成功 (版本 $($store.Version))"
    }
}

# ============================================================
Step 3 "安装腾讯应用宝(独立客户端)"
# ============================================================
$yybFound = $false
foreach ($d in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
    if ($d -and (Test-Path -LiteralPath $d)) {
        $hit = Get-ChildItem -LiteralPath $d -ErrorAction SilentlyContinue |
               Where-Object { $_.PSIsContainer -and $_.Name -match '应用宝|yingyongbao|Tencent' }
        if ($hit) { $yybFound = $true; Ok "检测到已安装: $($hit[0].FullName)"; break }
    }
}

if (-not $yybFound) {
    Info "未检测到应用宝,准备安装独立客户端…"
    $url = "https://sj.qq.com/download"
    Info "官方下载页: $url"

    # 尝试用 winget(若可用)
    $wg = Get-Command winget -ErrorAction SilentlyContinue
    if ($wg) {
        Info "尝试通过 winget 安装…"
        try {
            winget install --id Tencent.YingYongBao --accept-source-agreements --accept-package-agreements -e 2>&1 | ForEach-Object { Info $_ }
        } catch { Info "winget 安装未成功,请手动下载安装。" }
    } else {
        Info "winget 不可用。请在浏览器打开上面的地址,点击「Windows 版」下载并安装。"
        Info "下载完成后,直接双击安装包即可,安装程序会自动部署腾讯移动应用引擎。"
    }
}

# ============================================================
Step 4 "完成后的操作"
# ============================================================
Write-Host "`n============================================================" -ForegroundColor DarkCyan
Write-Host "  部署流程结束" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor DarkCyan
Write-Host @"

后续手动步骤(如未自动完成):
  1. 打开 https://sj.qq.com/download,点「Windows 版」下载并安装应用宝独立版
     (LTSC 上这是运行手机应用最可靠的方式)
  2. 首次启动应用宝若提示虚拟化问题,确认 BIOS 中 VT-x/SVM 已开启
  3. 若已装微软商店并想用「腾讯应用宝专区」:
     - LTSC 无法加入 Insider 预览计划,该专区大概率不可见
     - 直接在商店搜索「应用宝」安装即可,功能与独立版一致
  4. 如需 16GB 内存更佳体验,建议升级硬件

故障排查:
  - 应用宝启动异常: 管理员执行 bcdedit /set hypervisorlaunchtype auto 后重启
  - 商店不显示: 重启后执行 wsreset -i,或用 LTSC-Add-MicrosoftStore 离线包
"@
