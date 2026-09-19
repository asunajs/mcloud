# Windows LTSC 环境体检 + 微软商店/应用宝安装建议
# 只读检查,不修改系统。
# 用法(管理员 PowerShell): powershell -ExecutionPolicy Bypass -File .\ltsc-env-check.ps1

$ErrorActionPreference = 'Continue'

function Write-Section($title) {
    Write-Output ""
    Write-Output ("=" * 60)
    Write-Output "  $title"
    Write-Output ("=" * 60)
}

function Write-Item($label, $value, $ok = $null) {
    $prefix = "  "
    if ($null -ne $ok) {
        if ($ok) { $prefix = "  [OK] " } else { $prefix = "  [!!] " }
    }
    Write-Output ("{0}{1,-22}{2}" -f $prefix, $label, $value)
}

function Write-Note($text) {
    Write-Output "       -> $text"
}

$issues = New-Object System.Collections.Generic.List[string]

Write-Section "1. 系统信息"

$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1

Write-Item "系统名称" $os.Caption
Write-Item "版本号" $os.Version
Write-Item "Build" $os.BuildNumber
Write-Item "架构" $os.OSArchitecture

# LTSC 判定
$isLtsc = ($os.Caption -match 'LTSC') -or ($os.Caption -match 'Long-Term')
Write-Item "是否 LTSC" $(if ($isLtsc) { "是" } else { "否(普通版)" }) $true
if ($isLtsc) {
    Write-Output "    -> LTSC 默认不含 Microsoft Store,需按方案手动安装"
}

Write-Section "2. 硬件检查(应用宝要求:Intel 8 代+/同级 AMD,8GB+ 内存)"

Write-Item "CPU" $cpu.Name
$cores = $cpu.NumberOfCores
$threads = $cpu.NumberOfLogicalProcessors
Write-Item "核心/线程" "$cores 核 / $threads 线程"

$ramGB = [math]::Round($cs.TotalPhysicalMemory / 1GB, 1)
$ramOK = $ramGB -ge 8
Write-Item "内存" "$ramGB GB" $ramOK
if (-not $ramOK) {
    $issues.Add("内存不足 8GB(当前 ${ramGB}GB),应用宝可能无法正常运行,建议升至 16GB")
}

# CPU 代次粗判(仅 Intel 命名启发式,AMD 不做严格判定)
$cpuName = $cpu.Name
if ($cpuName -match 'Intel') {
    $gen = 0
    if ($cpuName -match 'i[3579]-(\d{4,5})') {
        $model = $Matches[1]
        if ($model.Length -eq 5) { $gen = [int]$model.Substring(0, 2) }
        elseif ($model.Length -eq 4) { $gen = [int]$model.Substring(0, 1) }
    }
    if ($gen -gt 0) {
        $cpuOK = $gen -ge 8
        Write-Item "CPU 代次(推测)" "第 $gen 代" $cpuOK
        if (-not $cpuOK) {
            $issues.Add("CPU 推测为第 $gen 代,低于应用宝要求的 Intel 8 代,可能不兼容")
        }
    } else {
        Write-Item "CPU 代次(推测)" "无法解析,请人工确认是否 8 代及以上"
    }
} elseif ($cpuName -match 'AMD') {
    Write-Item "CPU 平台" "AMD(需同级 Ryzen 及以上,应用宝要求同级 AMD 芯片)"
}

Write-Section "3. 虚拟化检查(应用宝引擎必需)"

$vtFirmware = $cpu.VirtualizationFirmwareEnabled
Write-Item "固件级虚拟化" $(if ($vtFirmware) { "已开启" } else { "未开启/未知" }) $vtFirmware
if (-not $vtFirmware) {
    $issues.Add("BIOS 中未开启虚拟化,请进入 BIOS 开启 Intel VT-x 或 AMD SVM")
}

# Hyper-V / 虚拟机监控程序状态
$hyperv = (Get-CimInstance Win32_ComputerSystem).HypervisorPresent
Write-Item "Hyper-V 监控程序" $(if ($hyperv) { "存在" } else { "不存在" })
Write-Output "    -> 若应用宝启动异常,可尝试: bcdedit /set hypervisorlaunchtype auto  (需管理员+重启)"

Write-Section "4. 区域设置(应用宝商店专区要求中国区)"

$locale = Get-WinSystemLocale
Write-Item "系统区域" $locale.Name
$regionOK = $locale.Name -eq 'zh-CN'
if (-not $regionOK) {
    $issues.Add("系统区域为 $($locale.Name),应用宝商店专区要求中国区(zh-CN),建议改为中国")
}

Write-Section "5. Microsoft Store 安装状态"

$store = Get-AppxPackage -Name "Microsoft.WindowsStore" -ErrorAction SilentlyContinue
if ($store) {
    Write-Item "Microsoft Store" "已安装 (版本 $($store.Version))" $true
} else {
    Write-Item "Microsoft Store" "未安装(LTSC 默认状态)" $false
    Write-Output "    -> 安装方法见文档方案 B:wsreset -i / Xbox App / LTSC-Add-MicrosoftStore"
}

Write-Section "6. 应用宝安装状态"

$yyb = Get-AppxPackage -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match 'Tencent|yingyongbao|YYB' } |
    Select-Object -First 1

$scanDirs = @()
foreach ($p in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
    if ($p -and (Test-Path -LiteralPath $p)) { $scanDirs += $p }
}
$yybExe = @()
foreach ($d in $scanDirs) {
    $yybExe += Get-ChildItem -LiteralPath $d -ErrorAction SilentlyContinue |
        Where-Object { $_.PSIsContainer -and $_.Name -match '应用宝|yingyongbao|Tencent' }
}

if ($yyb) {
    Write-Item "应用宝(商店版)" "已安装: $($yyb.Name)" $true
} elseif ($yybExe.Count -gt 0) {
    Write-Item "应用宝(独立版)" "已安装: $($yybExe[0].FullName)" $true
} else {
    Write-Item "应用宝" "未检测到" $false
    Write-Output "    -> 推荐直接安装独立版: https://sj.qq.com/download"
}

Write-Section "7. 结论与建议"

if ($issues.Count -eq 0) {
    Write-Output "  环境检查通过,未发现阻塞项。"
} else {
    Write-Output "  发现 $($issues.Count) 项需处理:"
    $i = 1
    foreach ($it in $issues) {
        Write-Output "    [$i] $it"
        $i++
    }
}

Write-Output ""
Write-Output "  推荐动作:"
if ($isLtsc) {
    Write-Output "    * LTSC 无法加入 Insider,商店内'应用宝专区'大概率不可见"
    Write-Output "    * 直接安装应用宝电脑版独立客户端即可运行手机应用:sj.qq.com/download"
    Write-Output "    * 如需微软商店本身,用文档方案 B 方法 2/3 安装"
} else {
    Write-Output "    * 可尝试商店内'腾讯应用宝专区'(需 Dev/Canary 频道 + 中国区)"
    Write-Output "    * 若专区不可见,改用独立客户端:sj.qq.com/download"
}
Write-Output ""
Write-Output "  完成。详细步骤见同目录上级 docs\ltsc-store-yingyongbao-guide.md"
Write-Output ""
