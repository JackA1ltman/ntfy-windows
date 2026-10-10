# 以任务计划程序注册 ntfy-windows 为后台任务（可选，比 Run 注册表更稳健）
# 需要以管理员身份运行 PowerShell。
#
# 用法:
#   powershell -ExecutionPolicy Bypass -File packaging\register-task.ps1 -ExePath "C:\path\to\ntfy-windows.exe"
#   powershell -ExecutionPolicy Bypass -File packaging\register-task.ps1 -ExePath "C:\path\to\ntfy-windows.exe" -Unregister

param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    [string]$TaskName = "ntfy-windows",

    [switch]$Unregister
)

$ErrorActionPreference = "Stop"

if ($Unregister) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "已移除计划任务: $TaskName"
    exit 0
}

if (-not (Test-Path $ExePath)) {
    throw "找不到可执行文件: $ExePath"
}

$action = New-ScheduledTaskAction -Execute $ExePath -Argument "--no-gui"

# 用户登录时启动
$trigger = New-ScheduledTaskTrigger -AtLogOn

$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -MultipleInstances IgnoreNew

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -Description "将 ntfy.sh 通知转发为 Windows 系统通知" `
    -Force | Out-Null

Write-Host "已注册计划任务: $TaskName -> $ExePath --no-gui"
