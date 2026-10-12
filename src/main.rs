//! ntfy-windows: a Windows notification daemon for ntfy.sh.
//!
//! Usage:
//!   ntfy-windows                     Start the GUI (daemon runs in-process)
//!   ntfy-windows --no-gui            Run headless (for auto-start / services)
//!   ntfy-windows --config PATH       Use an explicit configuration file
//!   ntfy-windows --install-autostart Register auto-start and exit
//!   ntfy-windows --uninstall-autostart Remove auto-start and exit

// Hide the console window for the release GUI build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod config;
mod cpu_debug;
mod daemon;
mod emoji;
mod gui;
mod i18n;
mod logging;
mod models;
mod ntfy_client;
mod state;
mod tray;
mod windows_notifier;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::config::Config;
use crate::daemon::Daemon;
use crate::state::{Command, SharedState};
use crate::windows_notifier::WindowsNotifier;

#[derive(Debug, Default)]
struct CliOptions {
    no_gui: bool,
    minimized: bool,
    cpu_debug: bool,
    config: Option<PathBuf>,
    install_autostart: bool,
    uninstall_autostart: bool,
    help: bool,
    version: bool,
}

impl CliOptions {
    fn parse(args: &[String]) -> Self {
        let mut options = CliOptions::default();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--no-gui" | "--headless" | "--daemon" => options.no_gui = true,
                "--minimized" | "--start-minimized" => options.minimized = true,
                "--cpu-debug" | "--debug-cpu" => options.cpu_debug = true,
                "--install-autostart" => options.install_autostart = true,
                "--uninstall-autostart" => options.uninstall_autostart = true,
                "--config" => {
                    i += 1;
                    if let Some(path) = args.get(i) {
                        options.config = Some(PathBuf::from(path));
                    }
                }
                "-h" | "--help" => options.help = true,
                "-V" | "--version" => options.version = true,
                _ => {}
            }
            i += 1;
        }
        options
    }
}

/// Result of the single instance check.
enum InstanceLock {
    /// We own the lock (or could not create one for a non-conflict reason).
    Acquired(Option<std::net::TcpListener>),
    /// Another instance already holds the lock.
    AlreadyRunning,
}

/// Use a loopback TCP port as a cross-platform single-instance lock.
///
/// The listener is kept alive for the whole process lifetime and released by
/// the OS automatically when the process exits (so stale locks are impossible).
fn single_instance() -> InstanceLock {
    match std::net::TcpListener::bind(("127.0.0.1", 49723)) {
        Ok(listener) => InstanceLock::Acquired(Some(listener)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => InstanceLock::AlreadyRunning,
        Err(_) => InstanceLock::Acquired(None),
    }
}

fn print_help() {    println!(
        "ntfy-windows {version} - 将 ntfy.sh 通知转发为 Windows 系统通知\n\
         \n\
         用法: ntfy-windows [选项]\n\
         \n\
         选项:\n\
           --no-gui                无界面后台运行（带系统托盘，无 GUI 窗口）\n\
           --minimized             启动图形界面但隐藏到系统托盘\n\
           --cpu-debug             定期输出 CPU 占用诊断日志\n\
           --config <路径>         使用指定的配置文件\n\
           --install-autostart     写入开机自启并退出\n\
           --uninstall-autostart   取消开机自启并退出\n\
           -h, --help              显示帮助\n\
           -V, --version           显示版本\n",
        version = env!("CARGO_PKG_VERSION")
    );
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let options = CliOptions::parse(&args);

    if options.help {
        print_help();
        return Ok(());
    }
    if options.version {
        println!("ntfy-windows {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config = match &options.config {
        Some(path) => Config::load_from(path),
        None => Config::load(),
    }
    .unwrap_or_else(|e| {
        eprintln!("{}", crate::i18n::tr_args("加载配置失败，改用默认配置: {err}", &[("err", &format!("{e:#}"))]));
        Config::default()
    });

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ntfy-windows.exe"));

    if options.install_autostart {
        autostart::set_autostart(true, &exe, true)?;
        println!("{}", crate::i18n::tr_args("已设置开机自启（后台托盘，无界面）: {path}", &[("path", &exe.display().to_string())]));
        return Ok(());
    }
    if options.uninstall_autostart {
        autostart::set_autostart(false, &exe, true)?;
        println!("{}", crate::i18n::tr("已取消开机自启"));
        return Ok(());
    }

    let state = SharedState::new(config.clone());
    let _log_guard = logging::init(state.clone(), &config.logging.level);
    i18n::set_language(&config.ui.language);
    config::ensure_data_dirs(&config);

    // Enforce a single running instance.
    let _instance_guard = match single_instance() {
        InstanceLock::Acquired(guard) => guard,
        InstanceLock::AlreadyRunning => {
            error!(
                "{}",
                crate::i18n::tr("检测到另一个 ntfy-windows 实例正在运行，本次启动已取消")
            );
            return Ok(());
        }
    };

    info!(
        "{}",
        crate::i18n::tr_args(
            "ntfy-windows v{version} 启动",
            &[("version", env!("CARGO_PKG_VERSION"))]
        )
    );

    if options.cpu_debug {
        cpu_debug::start(10);
        info!("{}", crate::i18n::tr("已启用 CPU 监控（每 10 秒输出一次）"));
    }

    // Register the AppUserModelID so that toasts are attributed correctly.
    #[cfg(windows)]
    {
        let icon = if config.notification.app_icon.is_empty() {
            None
        } else {
            Some(config.notification.app_icon.as_str())
        };
        if let Err(e) = windows_notifier::prepare(
            &config.notification.app_id,
            &config.notification.app_name,
            icon,
        ) {
            let err = format!("{e:#}");
            error!(
                "{}",
                crate::i18n::tr_args("初始化 Windows 通知失败: {err}", &[("err", err.as_str())])
            );
        }
    }

    let notifier = Arc::new(WindowsNotifier::new(&config.notification.app_id));

    // The daemon is I/O bound (one long-lived subscription stream plus the
    // occasional attachment download), so a small fixed worker pool is enough.
    // The default is one worker per CPU core, each with a 2 MiB stack, which is
    // wasteful on high-core-count machines.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .thread_name("ntfy-rt")
        .thread_stack_size(1024 * 1024)
        .enable_all()
        .build()
        .context("创建 tokio 运行时失败")?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
    let cancel = CancellationToken::new();

    let http = reqwest::Client::builder()
        .user_agent(format!("ntfy-windows/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let daemon = Daemon {
        state: state.clone(),
        notifier: notifier.clone(),
        cmd_rx,
        cancel: cancel.clone(),
        http,
    };
    runtime.spawn(daemon.run());

    // Periodically persist history so a crash does not lose everything.
    {
        let state = state.clone();
        let cancel = cancel.clone();
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                if cancel.is_cancelled() {
                    break;
                }
                if state.take_dirty() {
                    state.save_history();
                }
            }
        });
    }

    state.load_history();

    if options.no_gui {
        info!("{}", crate::i18n::tr("以无界面模式运行"));
        #[cfg(windows)]
        {
            if let Err(e) = tray::run_headless(cmd_tx.clone(), state.clone()) {
                let err = format!("{e:#}");
                error!(
                    "{}",
                    crate::i18n::tr_args(
                        "托盘模式启动失败，回退到无界面运行（Ctrl-C 退出）: {err}",
                        &[("err", err.as_str())]
                    )
                );
                runtime.block_on(async {
                    let _ = tokio::signal::ctrl_c().await;
                });
            }
        }
        #[cfg(not(windows))]
        {
            info!("{}", crate::i18n::tr("按 Ctrl-C 退出"));
            runtime.block_on(async {
                let _ = tokio::signal::ctrl_c().await;
            });
        }
        let _ = cmd_tx.send(Command::Shutdown);
        cancel.cancel();
        std::thread::sleep(Duration::from_millis(300));
        state.save_history();
        runtime.shutdown_timeout(Duration::from_secs(3));
    } else {
        info!("{}", crate::i18n::tr("启动图形界面"));
        if let Err(e) = gui::run(state.clone(), cmd_tx.clone(), config.clone(), options.minimized) {
            let err = format!("{e:#}");
            error!(
                "{}",
                crate::i18n::tr_args("图形界面退出: {err}", &[("err", err.as_str())])
            );
        }
        let _ = cmd_tx.send(Command::Shutdown);
        cancel.cancel();
        std::thread::sleep(Duration::from_millis(300));
        state.save_history();
        runtime.shutdown_timeout(Duration::from_secs(3));
    }

    info!("{}", crate::i18n::tr("已退出"));
    Ok(())
}
