# ntfy-windows

[**English**](README.en.md) | **简体中文**

一个使用 **Rust** 编写的 Windows 通知守护进程：订阅 [ntfy.sh](https://ntfy.sh)（或自建 ntfy 服务器）主题，
并把收到的推送实时转发为 **Windows 原生 Toast 系统通知**。自带轻量 GUI（egui），支持配置、历史与日志查看。

## 界面预览

![ntfy-windows 界面](ui.png)

---

## 目录

- [界面预览](#界面预览)
- [功能特性](#功能特性)
- [项目结构](#项目结构)
- [编译](#编译)
- [运行](#运行)
- [配置说明](#配置说明)
- [界面语言（i18n）](#界面语言i18n)
- [重连与单实例](#重连与单实例)
- [认证与私有主题](#认证与私有主题)
- [开机自启](#开机自启)
- [打包与分发](#打包与分发)
- [优先级与通知映射](#优先级与通知映射)
- [故障排查](#故障排查)

---

## 功能特性

- **长期后台驻守**：核心为 tokio 异步守护进程，支持断线自动重连（指数退避，最长 120 秒）、优雅退出与资源清理。
- **完整 ntfy 协议**：
  - 订阅端点遵循官方 API（**主题在路径中**）：JSON 流 `/{topic}/json`、SSE `/{topic}/sse`、
    WebSocket `/{topic}/ws`、轮询 `/{topic}/json?poll=1`；默认 `auto` 按 JSON → SSE → WS → 轮询自动回退。
  - 支持多主题，逗号分隔：`/{topic1,topic2}/json`。
  - 支持文本、标题、优先级（1~5）、标签 / 表情（tags / emojis）、附件（attachments）、
    点击跳转（click）、动作按钮（actions）、图标（icon）等字段。
- **Windows 原生通知**：基于 WinRT `ToastNotificationManager` 生成 Toast，
  自动注册 `AppUserModelID`，可在通知中心持久展示。
- **优先级映射**：不同 ntfy 优先级对应不同的提示音 / 时长 / 静音策略（见下文）。
- **认证**：支持 Basic Auth（账户密码）与 Bearer Token，用于私有主题。
- **GUI（egui / eframe）**：
  - 配置服务器地址、认证方式、主题列表、通知选项、日志级别。
  - 显示连接状态、统计信息、按主题 / 关键字过滤的通知历史、实时日志。
  - 一键手动重连、清空历史、导出历史 / 日志（弹窗自选保存位置）、测试通知、开机自启开关。
- **系统托盘**：支持两种模式——GUI 模式关闭窗口隐藏到托盘；`--no-gui` 为**纯托盘后台**（无 GUI 窗口，
  CPU 接近 0）。右键托盘可显示窗口 / 重连 / 测试通知 / 打开配置 / 退出。
- **低占用**：GUI 采用事件驱动重绘（空闲时几乎不重绘），并提供 `--cpu-debug` 诊断；
  长期后台建议用 `--no-gui`，完全避开 GUI 渲染开销。
- **可移植配置**：配置保存在**程序所在目录**的 `config.json`（若目录不可写则自动回退到
  `%APPDATA%\ntfy-windows\config.json`），首次启动会自动从旧版 `config.toml` 迁移。
- **中文字体**：自动加载系统中文字体（微软雅黑 / 黑体等），GUI 不再显示方块。
- **日志**：`tracing` + `tracing-subscriber`，按天滚动写入文件，并同步显示在 GUI。
- **图片附件预览**：自动下载图片附件与图标，并作为 Toast 主图 / 圆角图标显示（图标缓存在
  `images\icons` 子目录，按哈希复用）。
- **图片归档**：收到的图片会直接保存到持久目录（默认 `%LOCALAPPDATA%\ntfy-windows\images`，文件名如
  `20261009-215349_laptop_photo.jpg`），历史记录中可一键打开图片或所在文件夹。
- **文件归档**：ntfy 支持发送任意文件；非图片附件会原样保存到 `%LOCALAPPDATA%\ntfy-windows\files`
  （文件名 `时间_主题_原名.ext`），历史记录中可一键打开或定位。

> 提示：ntfy 的**附件功能需要服务器端开启** `attachment-cache-dir`（官方 ntfy.sh 已开启）。
> 私有服务器请参考 ntfy 官方文档配置。

---

## 项目结构

```
src/
├── main.rs               # 入口：CLI 解析、运行时初始化、GUI / 无界面模式切换
├── config.rs             # 配置模型、JSON 读写、路径工具
├── models.rs             # ntfy JSON 数据模型（NtfyMessage / Action / Attachment）
├── ntfy_client.rs        # 异步订阅客户端（/json、/sse、/ws、轮询）
├── daemon.rs             # 守护进程：重连、过滤、转发、图片归档编排
├── windows_notifier.rs   # Windows Toast 后端（WinRT）与 XML 生成
├── state.rs              # 共享状态、命令、历史、日志缓冲
├── logging.rs            # tracing 初始化（文件 + GUI 双写）
├── autostart.rs          # HKCU Run 注册表开机自启
├── emoji.rs              # ntfy 标签 -> Unicode 表情映射
├── gui.rs                # egui 控制面板（GUI 模式）
├── tray.rs               # 托盘图标 + 纯托盘（--no-gui）消息循环
├── cpu_debug.rs          # --cpu-debug 的 CPU / 重绘诊断
├── i18n.rs               # 多语言查找
lang/
├── en-US.toml            # 英文翻译
└── zh-CN.toml            # 中文（源语言）
packaging/
├── ntfy-windows.iss      # Inno Setup 安装脚本
└── register-task.ps1     # 通过任务计划程序注册后台服务（可选，需管理员）
config.example.json       # 示例配置
```

---

## 编译

前置条件：

- [Rust 稳定版工具链](https://rustup.rs/)（MSVC 目标，推荐 `x86_64-pc-windows-msvc`）
- Windows 10 1809+ / Windows 11
- 已安装 **Visual Studio 生成工具**（C++ 生成工具 + Windows SDK）

```powershell
# 调试构建（带控制台窗口）
cargo build

# 发布构建（无控制台窗口，体积更小）
cargo build --release
```

产物位于 `target\release\ntfy-windows.exe`。

> 首次编译需要下载并编译依赖（含 egui/winit），耗时较长属正常现象。

---

## 运行

```powershell
# 启动图形界面（守护进程在同一进程内运行）
.\ntfy-windows.exe

# 后台托盘模式（无 GUI 窗口，CPU 极低；托盘图标可操作）
.\ntfy-windows.exe --no-gui

# 指定配置文件
.\ntfy-windows.exe --config D:\configs\ntfy.json

# 输出 CPU 占用诊断日志（每 10 秒）
.\ntfy-windows.exe --no-gui --cpu-debug

# 安装 / 卸载开机自启（写入 HKCU Run，参数为 --no-gui）
.\ntfy-windows.exe --install-autostart
.\ntfy-windows.exe --uninstall-autostart

# 查看帮助
.\ntfy-windows.exe --help
```

首次启动会在**程序所在目录**生成 `config.json`；若该目录不可写（如安装在 `Program Files`），
则自动回退到 `%APPDATA%\ntfy-windows\config.json`。请在 GUI 的「订阅主题」页面添加主题，或直接编辑配置文件后重启。

> **两种后台方式**：
> - `--no-gui`：**无 GUI 窗口**，只有系统托盘（推荐，CPU 接近 0）。右键托盘：重新连接 / 测试通知 /
>   打开配置文件 / 打开数据目录 / 退出。
> - GUI 模式：关闭窗口会隐藏到托盘；左键托盘显示窗口，右键菜单同上。
>
> 想进一步降低占用就用 `--no-gui`；它不启动 eframe/egui，因此没有 GUI 渲染开销。

---

## 配置说明

默认路径：`<程序目录>\config.json`（也可用 `--config` 指定；以 `.toml` 结尾的旧文件仍可读取）。

```json
{
  "topics": ["mytopic", "alerts"],
  "autostart": false,
  "server": {
    "base_url": "https://ntfy.sh",
    "insecure_tls": false
  },
  "auth": {
    "method": "none",
    "username": "",
    "password": "",
    "token": ""
  },
  "connection": {
    "transport": "auto",
    "retry_initial_secs": 2,
    "retry_max_secs": 120,
    "retry_max_attempts": 0,
    "poll_interval_secs": 15
  },
  "notification": {
    "enabled": true,
    "min_priority": 1,
    "sound": true,
    "show_attachments": true,
    "download_images": true,
    "max_image_mb": 10,
    "app_name": "ntfy-windows",
    "app_id": "com.ntfy.windows",
    "app_icon": "",
    "ignore_topics": "",
    "save_images": true,
    "image_dir": "",
    "save_files": true,
    "files_dir": "",
    "max_file_mb": 20,
    "sticky_min_priority": 5,
    "sticky_scenario": "reminder"
  },
  "ui": {
    "theme": "dark",
    "history_limit": 500,
    "font_path": "",
    "close_to_tray": true,
    "language": "system"
  },
  "logging": {
    "level": "info"
  }
}
```

数据文件位置：

| 文件 | 路径 |
| --- | --- |
| 配置 | `<程序目录>\config.json`（回退：`%APPDATA%\ntfy-windows\config.json`） |
| 历史 | `%APPDATA%\ntfy-windows\history.json` |
| 日志 | `%LOCALAPPDATA%\ntfy-windows\logs\ntfy-windows.YYYY-MM-DD.log` |
| 图片 | `<image_dir>`（默认 `%LOCALAPPDATA%\ntfy-windows\images\`，文件名 `时间_主题_原名.jpg`；图标在 `images\icons\`） |
| 文件 | `<files_dir>`（默认 `%LOCALAPPDATA%\ntfy-windows\files\`，文件名 `时间_主题_原名.ext`） |
| 导出 | 点击「导出」时由系统保存对话框选择保存位置（不再写入固定目录） |

---

## 界面语言（i18n）

- 界面语言在「设置 → 界面与日志 → 界面语言」中切换，配置项为 `ui.language`：
  `system`（跟随系统）/ `zh-CN` / `en-US`。
- 界面文本、托盘菜单、以及**运行日志**都会随语言切换。
- 内置中文与英文；**新增/覆盖语言无需重新编译**：在程序目录下放一个
  `lang/<code>.toml`（例如 `lang/ja-JP.toml`）即可，键为源码中的中文原文，值为译文：
  ```toml
  "设置" = "設定"
  "已导出到 {path}" = "{path} にエクスポートしました"
  ```
  参考仓库中的 [`lang/en-US.toml`](lang/en-US.toml)。

---

## 重连与单实例

- **失败计数与退避**：只有当一次连接真正收到数据（消息或 keepalive）后才重置退避。
  如果连接被接受后立即关闭（例如代理 / 服务器异常），会累加失败次数并按指数退避重试；
  状态面板会显示「连续失败」与「累计失败」。
- **可配置重试**（`connection` 节）：
  - `transport`：`auto`（默认，依次尝试 JSON 流 → SSE → WebSocket → 轮询）/ `json` / `sse` / `ws` / `poll`
  - `retry_initial_secs`：首次重试间隔（秒）
  - `retry_max_secs`：最大重试间隔（秒）
  - `retry_max_attempts`：最大连续失败次数，`0` 表示无限重试；达到上限后停止自动重试，
    可在 GUI 或托盘点「重连」手动恢复。
  - `poll_interval_secs`：`poll` 模式下的轮询间隔（秒）

> 订阅地址遵循 ntfy 官方 API：主题在**路径**中，如 `https://host/{topic}/json`、`/{topic}/sse`、
> `/{topic}/ws`；多主题用逗号：`/{topic1,topic2}/json`。
- **单实例**：程序通过本机回环端口（`127.0.0.1:49723`）保证同一时间只有一个实例运行，
  重复启动会直接退出并在日志中提示。
- **托盘退出**：托盘菜单与窗口右上角的「退出」按钮都会真正结束进程；
  关闭窗口（X）则最小化到托盘（可在设置中关闭该行为）。

---

## 认证与私有主题

在 ntfy 服务器上为账号或主题启用访问控制后，选择对应认证方式：

- **Basic Auth**：`"method": "basic"`，填写 `username` / `password`。
- **Bearer Token**：`"method": "token"`，填写 `token`（形如 `tk_xxxxxxxx`）。

对应 HTTP 头分别为 `Authorization: Basic ...` 与 `Authorization: Bearer tk_...`。
订阅时会自动携带，GUI 中的令牌 / 密码字段以密文显示。

---

## 开机自启

### 方式一：注册表（默认，无需管理员）

```powershell
.\ntfy-windows.exe --install-autostart     # 写入 HKCU\...\Run，参数为 --no-gui
.\ntfy-windows.exe --uninstall-autostart
```

或在 GUI「设置 → 开机自启」勾选后点击「保存并应用」。

对应注册表项：

```
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
    ntfy-windows = "C:\path\to\ntfy-windows.exe" --no-gui
```

> 默认使用 `--no-gui`（无界面、仅托盘、CPU 接近 0）。
> 若希望开机后是“图形界面最小化到托盘”，可将参数改为 `--minimized`。

### 方式二：任务计划程序（可选，更稳健，可随机启动 / 失败重启）

以管理员身份运行：

```powershell
powershell -ExecutionPolicy Bypass -File packaging\register-task.ps1 -ExePath "C:\path\to\ntfy-windows.exe"
```

### 方式三：Windows 服务

本程序本身未实现服务控制接口。如需以服务方式运行，可使用
[NSSM](https://nssm.cc/) 或 [WinSW](https://github.com/winsw/winsw) 包装：

```powershell
nssm install ntfy-windows "C:\path\to\ntfy-windows.exe" --no-gui
nssm start ntfy-windows
```

---

## 打包与分发

### 1. 生成发布版

```powershell
cargo build --release
# 产物: target\release\ntfy-windows.exe
```

### 2. Inno Setup 安装程序

安装 [Inno Setup](https://jrsoftware.org/isinfo.php)，然后：

```powershell
iscc packaging\ntfy-windows.iss
```

会在 `dist\` 生成 `ntfy-windows-setup-x.y.z.exe` 安装包。

### 3. WiX / MSI（可选）

```powershell
cargo install cargo-wix
cargo wix init
cargo wix
```

---

## 优先级与通知映射

| ntfy 优先级 | 含义 | Windows Toast 表现 |
| --- | --- | --- |
| 1 (min) | 最低 | 静音（`<audio silent="true"/>`） |
| 2 (low) | 较低 | 默认提示音 |
| 3 (default) | 默认 | 默认提示音 |
| 4 (high) | 高 | `Notification.IM` 提示音 + `duration="long"` |
| 5 (max) | 紧急 | `Notification.Looping.Alarm` 提示音 + `duration="long"` |

- `notification.sound = false` 时全部静音。
- `notification.min_priority` 可用于只显示较高优先级的消息。
- 优先级 ≥ `notification.sticky_min_priority`（默认 5）的通知会以 `scenario`（`reminder` 或 `alarm`）
  **常驻屏幕**直到用户处理；`alarm` 额外循环提示音。
- `tags` 中已知的表情短代码会转成 emoji 并加在标题前，未知标签以 `#tag` 形式附在正文末尾。
- `click` 作为 Toast 的主体点击行为（`activationType="protocol"`，用默认浏览器打开）。
- `actions` 中类型为 `view` 的按钮会转为 Toast 动作按钮（最多 4 个，另附「忽略」）。
  `broadcast`（Android 专用）与 `http` 动作在 Windows 上会被忽略。

---

## 故障排查

| 现象 | 排查方向 |
| --- | --- |
| 收不到通知 | 检查「状态面板」连接状态与日志；确认主题名正确、认证信息正确；用 GUI「测试通知」验证 Toast 链路。 |
| 日志反复出现「连接被立即关闭」 | 多为反向代理 / Cloudflare 缓冲或丢弃 NDJSON 流。`transport` 设为 `sse` / `ws`（或保持 `auto`）即可；也可在反向代理中关闭响应缓冲。 |
| 日志出现「返回了 HTML 页面」/ `<!doctype html>` | 请求打到了 ntfy 的 Web 前端而非 API。请确认 `base_url` 正确（自建服务器若挂载在子路径，需写成 `https://host/子路径`），且订阅 URL 形如 `/{topic}/json`。 |
| 托盘左键无反应 | 现在左键单击显示窗口、右键打开菜单。若仍无效，把日志级别设为 `debug`，看是否有「托盘图标事件 / 托盘菜单事件」。 |
| 通知不显示图片 | 需服务器开启附件缓存；程序会下载并转码为小尺寸 PNG/JPEG 保存到 `images`。日志会打印「已保存通知图片」或失败原因；图片过大/格式不支持的会被跳过。 |
| 没有弹出 Toast | 检查 Windows「设置 → 系统 → 通知」是否允许；检查 `app_id` 是否正确注册；查看日志中的 toast 错误。 |
| 附件图片不显示 | 确认 ntfy 服务器开启了附件缓存；图片可能超过 Toast 限制（建议 < 200KB）；查看 `max_image_mb`。 |
| TLS 报错 | 自建服务器使用自签名证书时可临时开启 `insecure_tls`（不推荐长期使用）。 |
| 开机自启无效 | 运行 `--install-autostart` 或检查 `HKCU\...\Run`；确认使用的是绝对路径。 |
| GUI 后台 / 隐藏时 CPU 偏高 | GUI 采用事件驱动重绘；若仍有异常占用，用 `--cpu-debug` 输出诊断日志定位，长期后台建议用 `--no-gui`。 |
| 日志在哪 | `%LOCALAPPDATA%\ntfy-windows\logs\`，GUI「运行日志」页也可实时查看。 |

调试时可把日志级别调成 `debug` 或 `trace`，或设置环境变量：

```powershell
$env:RUST_LOG = "ntfy_windows=debug"
.\ntfy-windows.exe --no-gui
```

---

## 许可证

MIT
