# 初始化 git 仓库并创建**第一个 commit**。
#
# 在项目根目录运行：
#   powershell -ExecutionPolicy Bypass -File scripts\init-repo.ps1
#
# 之后由你绑定远程仓库并推送：
#   git remote add origin https://github.com/<user>/<repo>.git
#   git push -u origin main

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if (-not (Test-Path (Join-Path $repoRoot ".git"))) {
    git init -b main
    Write-Host "已执行 git init -b main"
} else {
    Write-Host "已存在 .git，跳过 git init"
}

$message = @"
Initial commit: ntfy-windows

Windows notification daemon that forwards ntfy.sh notifications to native
Windows toast notifications.

- tokio async daemon with reconnect/backoff (JSON / SSE / WebSocket / poll)
- WinRT toast backend (priority, tags, click, actions, attachments)
- egui GUI + system tray; headless tray mode (--no-gui)
- config.json next to the executable, image archiving, history and logs
- i18n with independent language files (Chinese / English); logs included
- CPU/repaint diagnostics (--cpu-debug)
"@

git add -A

# 如果没有配置 git 用户信息，临时提供一个，避免首次提交报错。
$email = (git config user.email) 2>$null
if ([string]::IsNullOrWhiteSpace($email)) {
    git -c user.name="ntfy-windows" -c user.email="ntfy-windows@localhost" commit -m $message
} else {
    git commit -m $message
}

Write-Host ""
git --no-pager log --oneline -1
Write-Host ""
Write-Host "第一个提交已创建。接下来绑定远程仓库并推送："
Write-Host "  git remote add origin https://github.com/<user>/<repo>.git"
Write-Host "  git push -u origin main"
