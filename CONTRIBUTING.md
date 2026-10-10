# Contributing

Thanks for your interest in `ntfy-windows`! This project is a Windows-only Rust daemon,
so all development and testing happens on Windows.

> 中文简要说明：这是一个 **Windows 专用** 的 Rust 项目；欢迎提交 PR。开发前请先装好
> MSVC 工具链与 Windows SDK，遵循 `cargo fmt` 风格；新增/修改面向用户的文本请同步更新
> `lang/en-US.toml`。

## Prerequisites

- Windows 10 1809+ / Windows 11
- Rust stable (`rustup`) with the `x86_64-pc-windows-msvc` target
- Visual Studio Build Tools (C++ build tools + Windows SDK)

## Build & run

```powershell
cargo fmt
cargo build
cargo run -- --cpu-debug           # GUI
cargo run -- --no-gui --cpu-debug  # headless tray
```

## Project conventions

- Keep modules focused: `config`, `models`, `ntfy_client`, `daemon`, `windows_notifier`,
  `state`, `logging`, `autostart`, `emoji`, `gui`, `tray`, `cpu_debug`, `i18n`.
- The GUI (`gui.rs`) must stay responsive; never block the UI thread. Background work runs
  on the tokio runtime; the GUI talks to the daemon through `state::Command`.
- Prefer `tracing` macros for diagnostics, not `println!`.
- Avoid unbounded loops / repaint storms. The GUI is event-driven; see `--cpu-debug` to
  verify idle CPU after changes.

## Adding / fixing a translation

1. UI strings **and log messages** use the **Chinese source text as the key**.
2. Add or update the entry in `lang/en-US.toml` (and optionally `lang/zh-CN.toml`).
3. Users can override without rebuilding by dropping `lang/<code>.toml` next to the exe.
4. Keep placeholders (`{path}`, `{n}`, `{err}`, …) unchanged.

## Commit / PR guidelines

- Small, focused commits with a clear message (e.g. `fix(gui): stop idle repaint loop`).
- Run `cargo fmt` and make sure `cargo build --release` succeeds before pushing.
- Describe user-visible behaviour changes in the PR and update `CHANGELOG.md` if relevant.
- Do not commit generated files: `target/`, `dist/`, `config.json`, logs, or binaries
  (see `.gitignore`).

## Reporting issues

Please include:

- Windows version and the app version (`ntfy-windows --version`).
- Relevant log lines (`%LOCALAPPDATA%\ntfy-windows\logs\`), especially with
  `--cpu-debug` when reporting performance problems.
- Whether you use the GUI or `--no-gui`.
