//! egui based control panel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::{AuthMethod, Config, Transport};
use crate::state::{Command, ConnectionStatus, SharedState};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Dashboard,
    Topics,
    Settings,
    History,
    Logs,
    About,
}

/// Identifiers of the tray menu entries we react to.
#[derive(Clone)]
struct TrayMenuIds {
    show: tray_icon::menu::MenuId,
    reconnect: tray_icon::menu::MenuId,
    test: tray_icon::menu::MenuId,
    quit: tray_icon::menu::MenuId,
}

pub struct GuiApp {
    state: SharedState,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    ctx: egui::Context,
    draft: Config,
    /// Font path currently applied to `ctx`. Used to avoid re-reading the font
    /// file and rebuilding the font atlas when nothing changed.
    applied_font_path: String,
    tab: Tab,
    new_topic: String,
    history_search: String,
    history_topic: Option<String>,
    toast: Option<(String, Instant)>,
    tray: Option<tray_icon::TrayIcon>,
    tray_failed: bool,
    force_hide: Arc<AtomicBool>,
    focused: Arc<AtomicBool>,
    hwnd: isize,
    hide_pending: bool,
    quit: bool,
}

/// Entry point used by `main.rs`.
pub fn run(
    state: SharedState,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    _config: Config,
    start_minimized: bool,
) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title("ntfy-windows")
            .with_visible(!start_minimized),
        ..Default::default()
    };
    tracing::info!(
        "{}",
        crate::i18n::tr_args(
            "图形界面启动 (start_minimized={minimized})",
            &[("minimized", if start_minimized { "true" } else { "false" })]
        )
    );
    eframe::run_native(
        "ntfy-windows",
        options,
        Box::new(move |cc| Box::new(GuiApp::new(cc, state, cmd_tx, start_minimized))),
    )
    .map_err(|e| anyhow::anyhow!("运行图形界面失败: {e}"))?;
    Ok(())
}

impl GuiApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        state: SharedState,
        cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
        start_minimized: bool,
    ) -> Self {
        let mut draft = state.lock().config.clone();
        // Keep the UI in sync with the actual auto-start registry value.
        draft.autostart = crate::autostart::is_autostart_enabled();
        setup_fonts(&cc.egui_ctx, &draft.ui.font_path);
        apply_theme(&cc.egui_ctx, &draft.ui.theme);
        let applied_font_path = draft.ui.font_path.clone();
        // egui by default requests a repaint whenever any widget rect differs
        // from the previous frame. For a mostly-static UI that can turn into a
        // permanent repaint loop (continuous rendering, and a ControlFlow::Poll
        // spin when the window is occluded). We repaint on demand instead.
        cc.egui_ctx
            .options_mut(|o| o.repaint_on_widget_change = false);

        let hwnd = window_handle_of(cc);
        let (tray, tray_ids, tray_failed) = match build_tray() {
            Ok((tray, ids)) => (Some(tray), Some(ids), false),
            Err(e) => {
                let err = format!("{e:#}");
                tracing::warn!(
                    "{}",
                    crate::i18n::tr_args("创建系统托盘失败: {err}", &[("err", err.as_str())])
                );
                (None, None, true)
            }
        };
        // Only keep the window hidden if we actually have a tray to restore it.
        let hide = start_minimized && !tray_failed;
        let force_hide = Arc::new(AtomicBool::new(hide));
        let focused = Arc::new(AtomicBool::new(false));
        // Repaint on demand, but only while the window is visible AND focused.
        // Requesting a repaint for a hidden or occluded window leaves a repaint
        // entry that can never be consumed, which makes eframe spin in Poll.
        {
            let ctx = cc.egui_ctx.clone();
            let force = force_hide.clone();
            let foc = focused.clone();
            state.set_waker(move || {
                if !force.load(Ordering::SeqCst) && foc.load(Ordering::SeqCst) {
                    crate::cpu_debug::note_wake();
                    ctx.request_repaint();
                }
            });
        }
        if hide {
            hide_window_native(hwnd);
        }
        spawn_hide_watchdog(hwnd, force_hide.clone());
        if let Some(ids) = tray_ids {
            tracing::info!("{}", crate::i18n::tr("系统托盘已创建"));
            install_tray_handlers(
                cc.egui_ctx.clone(),
                hwnd,
                cmd_tx.clone(),
                state.clone(),
                ids,
                force_hide.clone(),
            );
        }

        Self {
            state,
            cmd_tx,
            ctx: cc.egui_ctx.clone(),
            draft,
            applied_font_path,
            tab: Tab::Dashboard,
            new_topic: String::new(),
            history_search: String::new(),
            history_topic: None,
            toast: None,
            tray,
            tray_failed,
            force_hide,
            focused,
            hwnd,
            hide_pending: false,
            quit: false,
        }
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
        self.ctx.request_repaint();
    }

    fn save(&mut self) {
        if let Err(e) = self.draft.save() {
            let err = format!("{e:#}");
            let message = crate::i18n::tr_args("保存配置失败: {err}", &[("err", err.as_str())]);
            self.toast(message);
            return;
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Err(e) = crate::autostart::set_autostart(self.draft.autostart, &exe, true) {
                let err = format!("{e:#}");
                let message = crate::i18n::tr_args("设置开机自启失败: {err}", &[("err", err.as_str())]);
                self.toast(message);
            }
        }
        apply_theme(&self.ctx, &self.draft.ui.theme);
        // Only reload fonts (which reads the file again and rebuilds the glyph
        // atlas) when the configured font actually changed.
        if self.draft.ui.font_path != self.applied_font_path {
            setup_fonts(&self.ctx, &self.draft.ui.font_path);
            self.applied_font_path = self.draft.ui.font_path.clone();
        }
        crate::i18n::set_language(&self.draft.ui.language);
        crate::config::ensure_data_dirs(&self.draft);
        self.state.lock().config = self.draft.clone();
        let _ = self
            .cmd_tx
            .send(Command::UpdateConfig(Box::new(self.draft.clone())));
        let message = crate::i18n::tr("配置已保存并应用");
        self.toast(message);
    }

    /// Create `path` (if needed) and open it in the file manager.
    fn open_dir(&mut self, path: std::path::PathBuf) {
        if let Err(e) = std::fs::create_dir_all(&path) {
            let err = e.to_string();
            let p = path.display().to_string();
            let message = crate::i18n::tr_args(
                "创建目录失败 {path}: {err}",
                &[("path", p.as_str()), ("err", err.as_str())],
            );
            self.toast(message);
            return;
        }
        if let Err(e) = open::that(&path) {
            let err = e.to_string();
            let p = path.display().to_string();
            let message = crate::i18n::tr_args(
                "打开目录失败 {path}: {err}",
                &[("path", p.as_str()), ("err", err.as_str())],
            );
            self.toast(message);
        }
    }

    /// Export the notification history to a file chosen by the user.
    fn export_history(&mut self) {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let Some(path) = rfd::FileDialog::new()
            .set_title(crate::i18n::tr("导出历史"))
            .set_file_name(format!("history-{stamp}.json"))
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        let entries: Vec<_> = self.state.lock().history.iter().cloned().collect();
        let text = match serde_json::to_string_pretty(&entries) {
            Ok(text) => text,
            Err(e) => {
                let err = e.to_string();
                let message =
                    crate::i18n::tr_args("导出历史失败: {err}", &[("err", err.as_str())]);
                self.toast(message);
                return;
            }
        };
        match std::fs::write(&path, text) {
            Ok(()) => {
                let path_str = path.display().to_string();
                let message =
                    crate::i18n::tr_args("已导出到 {path}", &[("path", path_str.as_str())]);
                self.toast(message);
            }
            Err(e) => {
                let err = e.to_string();
                let message =
                    crate::i18n::tr_args("导出历史失败: {err}", &[("err", err.as_str())]);
                self.toast(message);
            }
        }
    }

    /// Export the in-memory logs to a file chosen by the user.
    fn export_logs(&mut self) {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let Some(path) = rfd::FileDialog::new()
            .set_title(crate::i18n::tr("导出日志"))
            .set_file_name(format!("logs-{stamp}.log"))
            .add_filter(crate::i18n::tr("日志"), &["log", "txt"])
            .save_file()
        else {
            return;
        };
        let logs = self
            .state
            .logs()
            .iter()
            .map(|l| {
                format!(
                    "{} [{}] {}: {}",
                    l.time.format("%Y-%m-%d %H:%M:%S%.3f"),
                    l.level,
                    l.target,
                    l.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        match std::fs::write(&path, logs) {
            Ok(()) => {
                let path_str = path.display().to_string();
                let message =
                    crate::i18n::tr_args("已导出到 {path}", &[("path", path_str.as_str())]);
                self.toast(message);
            }
            Err(e) => {
                let err = e.to_string();
                let message =
                    crate::i18n::tr_args("导出日志失败: {err}", &[("err", err.as_str())]);
                self.toast(message);
            }
        }
    }

    fn render_toast(&mut self, ctx: &egui::Context) {
        let expired = match &self.toast {
            Some((_, at)) => at.elapsed() >= Duration::from_secs(4),
            None => true,
        };
        if expired {
            self.toast = None;
            return;
        }
        if let Some((message, _)) = &self.toast {
            let message = message.clone();
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(&ctx.style()).show(ui, |ui| {
                        ui.label(message);
                    });
                });
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::cpu_debug::note_frame();
        if crate::cpu_debug::enabled() {
            let causes: Vec<String> = ctx
                .repaint_causes()
                .iter()
                .map(|cause| format!("{cause:?}"))
                .collect();
            crate::cpu_debug::note_causes(&causes);
            let events: Vec<String> = ctx.input(|i| {
                i.events
                    .iter()
                    .map(|event| {
                        let text = format!("{event:?}");
                        text.split(['(', '{', ' '])
                            .next()
                            .unwrap_or("?")
                            .to_string()
                    })
                    .collect()
            });
            crate::cpu_debug::note_events(&events);
        }
        self.focused
            .store(ctx.input(|i| i.focused), Ordering::SeqCst);
        // Hide one frame after a close request so the repaint that scheduled
        // this frame is consumed while the window is still visible.
        if self.hide_pending {
            self.hide_pending = false;
            hide_window_native(self.hwnd);
        } else if self.force_hide.load(Ordering::SeqCst) && is_window_visible(self.hwnd) {
            hide_window_native(self.hwnd);
        }
        self.handle_close(ctx);
        // Event-driven only: repaint on state changes (waker) and user input.
        if self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
        self.top_panel(ctx);
        self.nav_panel(ctx);
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Dashboard => self.dashboard(ui),
            Tab::Topics => self.topics(ui),
            Tab::Settings => self.settings(ui),
            Tab::History => self.history(ui),
            Tab::Logs => self.logs(ui),
            Tab::About => self.about(ui),
        });
        self.render_toast(ctx);
    }
}

impl GuiApp {
    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("📨 ntfy-windows");
                ui.separator();
                let (status, detail) = {
                    let s = self.state.lock();
                    (s.status, s.status_detail.clone())
                };
                ui.colored_label(
                    status_color(status),
                    format!("● {}", crate::i18n::tr(status.label())),
                );
                ui.label(egui::RichText::new(detail).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("⏻ {}", crate::i18n::tr("退出")))
                        .clicked()
                    {
                        self.request_quit(ctx);
                    }
                    if ui
                        .button(format!("💾 {}", crate::i18n::tr("保存并应用")))
                        .clicked()
                    {
                        self.save();
                    }
                    if ui
                        .button(format!("🔔 {}", crate::i18n::tr("测试通知")))
                        .clicked()
                    {
                        let _ = self.cmd_tx.send(Command::TestNotification);
                    }
                    if ui
                        .button(format!("🔄 {}", crate::i18n::tr("重连")))
                        .clicked()
                    {
                        let _ = self.cmd_tx.send(Command::Reconnect);
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn nav_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(170.0)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                let items = [
                    (Tab::Dashboard, "🏠", "状态面板"),
                    (Tab::Topics, "📡", "订阅主题"),
                    (Tab::Settings, "⚙", "设置"),
                    (Tab::History, "🕘", "通知历史"),
                    (Tab::Logs, "📄", "运行日志"),
                    (Tab::About, "ℹ", "关于"),
                ];
                for (tab, icon, key) in items {
                    let label = format!("{icon}  {}", crate::i18n::tr(key));
                    if ui.selectable_label(self.tab == tab, label).clicked() {
                        self.tab = tab;
                    }
                    ui.add_space(2.0);
                }
            });
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let (status, detail, since, stats, topic_count, last_error, hist_len) = {
            let s = self.state.lock();
            (
                s.status,
                s.status_detail.clone(),
                s.connected_since,
                s.stats.clone(),
                s.config.topics.len(),
                s.last_error.clone(),
                s.history.len(),
            )
        };

        ui.heading(crate::i18n::tr("状态面板"));
        ui.separator();
        egui::Grid::new("stats_grid")
            .num_columns(2)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                ui.label(crate::i18n::tr("连接状态"));
                ui.colored_label(status_color(status), crate::i18n::tr(status.label()));
                ui.end_row();
                ui.label(crate::i18n::tr("详细信息"));
                ui.label(detail.as_str());
                ui.end_row();
                ui.label(crate::i18n::tr("订阅主题数"));
                ui.label(topic_count.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("已接收通知"));
                ui.label(stats.received.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("已弹出系统通知"));
                ui.label(stats.shown.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("重连次数"));
                ui.label(stats.reconnects.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("错误次数"));
                ui.label(stats.errors.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("连续失败"));
                ui.label(stats.consecutive_failures.to_string());
                ui.end_row();
                ui.label(crate::i18n::tr("累计失败"));
                ui.label(stats.failures.to_string());
                ui.end_row();
                if let Some(since) = since {
                    ui.label(crate::i18n::tr("连接起始时间"));
                    ui.label(since.format("%Y-%m-%d %H:%M:%S").to_string());
                    ui.end_row();
                }
                ui.label(crate::i18n::tr("历史记录数"));
                ui.label(hist_len.to_string());
                ui.end_row();
            });

        if let Some(err) = last_error {
            ui.add_space(8.0);
            ui.colored_label(
                egui::Color32::from_rgb(0xe0, 0x4f, 0x4f),
                crate::i18n::tr_args("最近错误: {msg}", &[("msg", err.as_str())]),
            );
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .button(format!("🔄 {}", crate::i18n::tr("立即重连")))
                .clicked()
            {
                let _ = self.cmd_tx.send(Command::Reconnect);
            }
            if ui
                .button(format!("✖ {}", crate::i18n::tr("清空历史")))
                .clicked()
            {
                self.state.clear_history();
                let message = crate::i18n::tr("历史已清空");
                self.toast(message);
            }
            if ui
                .button(format!("📤 {}", crate::i18n::tr("导出历史")))
                .clicked()
            {
                self.export_history();
            }
            if ui
                .button(format!("📂 {}", crate::i18n::tr("打开数据目录")))
                .clicked()
            {
                self.open_dir(crate::config::localappdata_dir());
            }
            if ui
                .button(format!("🖼 {}", crate::i18n::tr("打开图片目录")))
                .clicked()
            {
                self.open_dir(self.draft.notification.image_dir_path());
            }
            if ui
                .button(format!("📎 {}", crate::i18n::tr("打开文件目录")))
                .clicked()
            {
                self.open_dir(self.draft.notification.files_dir_path());
            }
        });

        ui.add_space(12.0);
        ui.heading(crate::i18n::tr("当前订阅"));
        let topics = self.state.lock().config.topics.clone();
        if topics.is_empty() {
            ui.weak(crate::i18n::tr("尚未订阅任何主题，请到“订阅主题”页面添加。"));
        } else {
            for topic in topics {
                ui.label(format!("• {topic}"));
            }
        }
    }

    fn topics(&mut self, ui: &mut egui::Ui) {
        ui.heading(crate::i18n::tr("订阅主题"));
        ui.label(crate::i18n::tr("可同时订阅多个主题；它们会复用同一个长连接。"));
        ui.separator();

        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("新主题:"));
            let response = ui.text_edit_singleline(&mut self.new_topic);
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(crate::i18n::tr("添加")).clicked() || enter {
                let topic = self.new_topic.trim().to_string();
                if !topic.is_empty() && !self.draft.topics.contains(&topic) {
                    self.draft.topics.push(topic);
                    self.new_topic.clear();
                    let message = crate::i18n::tr("已添加主题，点击“保存并应用”生效");
                    self.toast(message);
                }
            }
        });

        ui.add_space(8.0);
        let mut remove: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, topic) in self.draft.topics.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label("📡");
                        ui.label(topic.as_str());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(format!("❌ {}", crate::i18n::tr("移除")))
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                }
                if self.draft.topics.is_empty() {
                    ui.weak(crate::i18n::tr("还没有主题。"));
                }
            });
        if let Some(index) = remove {
            self.draft.topics.remove(index);
            let message = crate::i18n::tr("已移除主题，点击“保存并应用”生效");
            self.toast(message);
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading(crate::i18n::tr("设置"));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.collapsing(crate::i18n::tr("服务器"), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("服务器地址:"));
                        ui.text_edit_singleline(&mut self.draft.server.base_url);
                    });
                    ui.weak(crate::i18n::tr("例如 https://ntfy.sh 或自建服务器地址"));
                    ui.checkbox(
                        &mut self.draft.server.insecure_tls,
                        crate::i18n::tr("跳过 TLS 证书校验（不安全）"),
                    );
                    ui.separator();
                    egui::ComboBox::from_label(crate::i18n::tr("传输方式"))
                        .selected_text(crate::i18n::tr(transport_label(
                            self.draft.connection.transport,
                        )))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.draft.connection.transport,
                                Transport::Auto,
                                crate::i18n::tr("自动（JSON → SSE → WS → 轮询）"),
                            );
                            ui.selectable_value(
                                &mut self.draft.connection.transport,
                                Transport::Json,
                                crate::i18n::tr("JSON 流"),
                            );
                            ui.selectable_value(
                                &mut self.draft.connection.transport,
                                Transport::Sse,
                                "SSE",
                            );
                            ui.selectable_value(
                                &mut self.draft.connection.transport,
                                Transport::Ws,
                                "WebSocket",
                            );
                            ui.selectable_value(
                                &mut self.draft.connection.transport,
                                Transport::Poll,
                                crate::i18n::tr("轮询（poll）"),
                            );
                        });
                    ui.add(
                        egui::Slider::new(&mut self.draft.connection.poll_interval_secs, 1..=300)
                            .text(crate::i18n::tr("轮询间隔（秒）")),
                    );
                    ui.label(crate::i18n::tr("重连策略"));
                    ui.add(
                        egui::Slider::new(&mut self.draft.connection.retry_initial_secs, 1..=60)
                            .text(crate::i18n::tr("初始重试间隔（秒）")),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.draft.connection.retry_max_secs, 1..=600)
                            .text(crate::i18n::tr("最大重试间隔（秒）")),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.draft.connection.retry_max_attempts, 0..=100)
                            .text(crate::i18n::tr("最大重试次数（0 = 无限）")),
                    );
                });

                ui.collapsing(crate::i18n::tr("认证"), |ui| {
                    egui::ComboBox::from_label(crate::i18n::tr("认证方式"))
                        .selected_text(crate::i18n::tr(auth_label(self.draft.auth.method)))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.draft.auth.method,
                                AuthMethod::None,
                                crate::i18n::tr("无"),
                            );
                            ui.selectable_value(
                                &mut self.draft.auth.method,
                                AuthMethod::Basic,
                                crate::i18n::tr("账户密码 (Basic)"),
                            );
                            ui.selectable_value(
                                &mut self.draft.auth.method,
                                AuthMethod::Token,
                                crate::i18n::tr("令牌 (Bearer)"),
                            );
                        });
                    match self.draft.auth.method {
                        AuthMethod::Basic => {
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::tr("用户名:"));
                                ui.text_edit_singleline(&mut self.draft.auth.username);
                            });
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::tr("密码:"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.draft.auth.password)
                                        .password(true),
                                );
                            });
                        }
                        AuthMethod::Token => {
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::tr("令牌:"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.draft.auth.token)
                                        .password(true)
                                        .desired_width(320.0),
                                );
                            });
                        }
                        AuthMethod::None => {
                            ui.weak(crate::i18n::tr("未启用认证，仅适用于公开主题。"));
                        }
                    }
                });

                ui.collapsing(crate::i18n::tr("通知"), |ui| {
                    ui.checkbox(
                        &mut self.draft.notification.enabled,
                        crate::i18n::tr("启用 Windows 通知"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.draft.notification.min_priority, 1..=5)
                            .text(crate::i18n::tr("最低显示优先级")),
                    );
                    ui.checkbox(
                        &mut self.draft.notification.sound,
                        crate::i18n::tr("播放提示音"),
                    );
                    ui.checkbox(
                        &mut self.draft.notification.show_attachments,
                        crate::i18n::tr("显示附件图片"),
                    );
                    ui.checkbox(
                        &mut self.draft.notification.download_images,
                        crate::i18n::tr("下载并显示图标"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.draft.notification.max_image_mb, 1..=50)
                            .text(crate::i18n::tr("最大图片大小 (MB)")),
                    );
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("应用名称:"));
                        ui.text_edit_singleline(&mut self.draft.notification.app_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("应用 ID:"));
                        ui.text_edit_singleline(&mut self.draft.notification.app_id);
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("应用图标:"));
                        ui.text_edit_singleline(&mut self.draft.notification.app_icon);
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("忽略主题:"));
                        ui.text_edit_singleline(&mut self.draft.notification.ignore_topics);
                    });
                    ui.weak(crate::i18n::tr("忽略主题使用英文逗号分隔。"));
                    ui.checkbox(
                        &mut self.draft.notification.save_images,
                        crate::i18n::tr("保存收到的图片到本地目录"),
                    );
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("图片保存目录:"));
                        ui.text_edit_singleline(&mut self.draft.notification.image_dir);
                    });
                    ui.weak(crate::i18n::tr(
                        "留空则使用默认目录 %LOCALAPPDATA%\\ntfy-windows\\images。文件名格式：时间_主题_原始名。",
                    ));
                    ui.checkbox(
                        &mut self.draft.notification.save_files,
                        crate::i18n::tr("保存收到的其他文件到本地目录"),
                    );
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("文件保存目录:"));
                        ui.text_edit_singleline(&mut self.draft.notification.files_dir);
                    });
                    ui.weak(crate::i18n::tr(
                        "留空则使用默认目录 %LOCALAPPDATA%\\ntfy-windows\\files。",
                    ));
                    ui.add(
                        egui::Slider::new(&mut self.draft.notification.max_file_mb, 1..=200)
                            .text(crate::i18n::tr("最大文件大小 (MB)")),
                    );
                });

                ui.collapsing(crate::i18n::tr("界面与日志"), |ui| {
                    egui::ComboBox::from_label(crate::i18n::tr("界面语言"))
                        .selected_text(language_label(&self.draft.ui.language))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.draft.ui.language,
                                "system".to_string(),
                                crate::i18n::tr("跟随系统"),
                            );
                            ui.selectable_value(
                                &mut self.draft.ui.language,
                                "zh-CN".to_string(),
                                "简体中文",
                            );
                            ui.selectable_value(
                                &mut self.draft.ui.language,
                                "en-US".to_string(),
                                "English",
                            );
                        });
                    egui::ComboBox::from_label(crate::i18n::tr("界面主题"))
                        .selected_text(theme_label(&self.draft.ui.theme))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.draft.ui.theme,
                                "dark".to_string(),
                                crate::i18n::tr("深色"),
                            );
                            ui.selectable_value(
                                &mut self.draft.ui.theme,
                                "light".to_string(),
                                crate::i18n::tr("浅色"),
                            );
                        });
                    ui.add(
                        egui::Slider::new(&mut self.draft.ui.history_limit, 50..=10000)
                            .text(crate::i18n::tr("历史条数上限")),
                    );
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("日志级别:"));
                        ui.text_edit_singleline(&mut self.draft.logging.level);
                    });
                    ui.weak(crate::i18n::tr("日志级别: trace / debug / info / warn / error"));
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("中文字体:"));
                        ui.text_edit_singleline(&mut self.draft.ui.font_path);
                    });
                    ui.weak(crate::i18n::tr(
                        "留空则自动检测系统字体（微软雅黑 / 黑体等）；也可填入 ttf/ttc 绝对路径。",
                    ));
                    ui.checkbox(
                        &mut self.draft.ui.close_to_tray,
                        crate::i18n::tr("关闭窗口时最小化到系统托盘"),
                    );
                });

                ui.collapsing(crate::i18n::tr("开机自启"), |ui| {
                    ui.checkbox(
                        &mut self.draft.autostart,
                        crate::i18n::tr("开机自动启动（后台无界面运行）"),
                    );
                    ui.weak(crate::i18n::tr(
                        "勾选后点击右上角“保存并应用”写入注册表 HKCU\\...\\Run。",
                    ));
                });

                ui.add_space(8.0);
            });
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        ui.heading(crate::i18n::tr("通知历史"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("搜索:"));
            ui.text_edit_singleline(&mut self.history_search);
            if ui
                .button(format!("✖ {}", crate::i18n::tr("清空")))
                .clicked()
            {
                self.state.clear_history();
                let message = crate::i18n::tr("历史已清空");
                self.toast(message);
            }
            if ui
                .button(format!("📤 {}", crate::i18n::tr("导出")))
                .clicked()
            {
                self.export_history();
            }
        });

        let topics = {
            let s = self.state.lock();
            let mut topics: Vec<String> = s.history.iter().map(|h| h.topic.clone()).collect();
            topics.sort();
            topics.dedup();
            topics
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::tr("主题:"));
            if ui
                .selectable_label(self.history_topic.is_none(), crate::i18n::tr("全部"))
                .clicked()
            {
                self.history_topic = None;
            }
            for topic in &topics {
                let selected = self.history_topic.as_deref() == Some(topic.as_str());
                if ui.selectable_label(selected, topic.as_str()).clicked() {
                    self.history_topic = Some(topic.clone());
                }
            }
        });
        ui.separator();

        let query = self.history_search.to_lowercase();
        let topic_filter = self.history_topic.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let s = self.state.lock();
                let mut shown = 0usize;
                for entry in s.history.iter() {
                    if let Some(filter) = &topic_filter {
                        if &entry.topic != filter {
                            continue;
                        }
                    }
                    if !query.is_empty() {
                        let haystack =
                            format!("{} {} {}", entry.title, entry.message, entry.topic)
                                .to_lowercase();
                        if !haystack.contains(&query) {
                            continue;
                        }
                    }
                    shown += 1;
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                priority_color(entry.priority),
                                format!("P{}", entry.priority),
                            );
                            ui.label(egui::RichText::new(entry.topic.as_str()).strong());
                            ui.weak(entry.received_at.format("%Y-%m-%d %H:%M:%S").to_string());
                        });
                        if !entry.title.is_empty() {
                            ui.label(egui::RichText::new(entry.title.as_str()).strong());
                        }
                        if !entry.message.is_empty() {
                            ui.label(entry.message.as_str());
                        }
                        if let Some(path) = &entry.image_path {
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button(format!("🖼 {}", crate::i18n::tr("打开图片")))
                                    .clicked()
                                {
                                    let _ = open::that(path);
                                }
                                if ui
                                    .small_button(format!("📂 {}", crate::i18n::tr("打开所在文件夹")))
                                    .clicked()
                                {
                                    if let Some(parent) = std::path::Path::new(path).parent() {
                                        let _ = open::that(parent);
                                    }
                                }
                            });
                        }
                        if let Some(path) = &entry.file_path {
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button(format!("📎 {}", crate::i18n::tr("打开文件")))
                                    .clicked()
                                {
                                    let _ = open::that(path);
                                }
                                if ui
                                    .small_button(format!("📂 {}", crate::i18n::tr("打开所在文件夹")))
                                    .clicked()
                                {
                                    if let Some(parent) = std::path::Path::new(path).parent() {
                                        let _ = open::that(parent);
                                    }
                                }
                            });
                        }
                        if !entry.tags.is_empty() {
                            ui.weak(
                                entry
                                    .tags
                                    .iter()
                                    .map(|t| format!("#{t}"))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            );
                        }
                    });
                    ui.add_space(4.0);
                }
                if shown == 0 {
                    ui.weak(crate::i18n::tr("没有匹配的通知记录。"));
                }
            });
    }

    fn logs(&mut self, ui: &mut egui::Ui) {
        ui.heading(crate::i18n::tr("运行日志"));
        ui.horizontal(|ui| {
            if ui
                .button(format!("✖ {}", crate::i18n::tr("清空显示")))
                .clicked()
            {
                self.state.clear_logs();
            }
            if ui
                .button(format!("📤 {}", crate::i18n::tr("导出日志")))
                .clicked()
            {
                self.export_logs();
            }
            if ui
                .button(format!("📂 {}", crate::i18n::tr("打开日志目录")))
                .clicked()
            {
                self.open_dir(crate::config::log_dir());
            }
        });
        let log_dir = crate::config::log_dir().display().to_string();
        ui.weak(crate::i18n::tr_args("日志目录: {path}", &[("path", log_dir.as_str())]));
        ui.separator();

        let logs = self.state.logs();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in logs.iter().rev() {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(entry.time.format("%H:%M:%S").to_string());
                        ui.colored_label(level_color(entry.level), format!("{:<5}", entry.level));
                        ui.label(entry.message.as_str());
                    });
                }
                if logs.is_empty() {
                    ui.weak(crate::i18n::tr("暂无日志。"));
                }
            });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        ui.heading(crate::i18n::tr("关于 ntfy-windows"));
        ui.separator();
        let config_file = crate::config::config_path().display().to_string();
        let program_dir = crate::config::exe_dir().display().to_string();
        let data_dir = crate::config::localappdata_dir().display().to_string();
        let log_dir = crate::config::log_dir().display().to_string();
        ui.label(crate::i18n::tr_args(
            "版本: {version}",
            &[("version", env!("CARGO_PKG_VERSION"))],
        ));
        ui.label(crate::i18n::tr_args(
            "配置文件: {path}",
            &[("path", config_file.as_str())],
        ));
        ui.label(crate::i18n::tr_args(
            "程序目录: {path}",
            &[("path", program_dir.as_str())],
        ));
        ui.label(crate::i18n::tr_args(
            "数据目录: {path}",
            &[("path", data_dir.as_str())],
        ));
        ui.label(crate::i18n::tr_args(
            "日志目录: {path}",
            &[("path", log_dir.as_str())],
        ));
        ui.add_space(8.0);
        ui.hyperlink_to(
            crate::i18n::tr("项目 GitHub"),
            "https://github.com/JackA1ltman/ntfy-windows",
        );
        ui.hyperlink_to(crate::i18n::tr("ntfy 官方文档"), "https://docs.ntfy.sh/");
        ui.hyperlink_to(
            crate::i18n::tr("ntfy GitHub"),
            "https://github.com/binwiederhier/ntfy",
        );
        ui.add_space(8.0);
        ui.weak(crate::i18n::tr(
            "使用 Rust + tokio + reqwest + egui + Windows Toast API 构建。",
        ));
    }

    /// Quit for real. Sends a close request but also guarantees the process
    /// terminates even if the GUI event loop does not exit on its own.
    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.quit {
            return;
        }
        self.quit = true;
        self.state.save_history();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            std::process::exit(0);
        });
    }

    /// Intercept a window close and hide to tray instead of exiting.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if self.quit {
            return;
        }
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.tray.is_some() && !self.tray_failed && self.draft.ui.close_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.force_hide.store(true, Ordering::SeqCst);
            self.hide_pending = true;
            // Ensure a next frame runs so the deferred hide is executed.
            ctx.request_repaint();
        }
    }
}

/// Register synchronous tray/menu handlers.
///
/// These run on the main thread while the always-running win32 message loop
/// dispatches messages, so they keep working even when the window is hidden
/// and `egui::App::update` is no longer being called.
fn install_tray_handlers(
    ctx: egui::Context,
    hwnd: isize,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    state: SharedState,
    ids: TrayMenuIds,
    force_hide: Arc<AtomicBool>,
) {
    let ctx_menu = ctx.clone();
    let force_menu = force_hide.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(
        move |event: tray_icon::menu::MenuEvent| {
            let id = format!("{:?}", event.id);
            tracing::info!(
                "{}",
                crate::i18n::tr_args("托盘菜单: {id}", &[("id", id.as_str())])
            );
            if event.id == ids.show {
                force_menu.store(false, Ordering::SeqCst);
                show_window_native(hwnd, &ctx_menu);
            } else if event.id == ids.reconnect {
                let _ = cmd_tx.send(Command::Reconnect);
            } else if event.id == ids.test {
                let _ = cmd_tx.send(Command::TestNotification);
            } else if event.id == ids.quit {
                state.save_history();
                std::process::exit(0);
            }
        },
    ));

    let ctx_tray = ctx;
    let force_tray = force_hide;
    tray_icon::TrayIconEvent::set_event_handler(Some(
        move |event: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                force_tray.store(false, Ordering::SeqCst);
                show_window_native(hwnd, &ctx_tray);
            }
        },
    ));
}

/// Hide the window while `force_hide` is set (also counters eframe's first-frame
/// `set_visible(true)` at startup). Only acts when the window is visible, so it
/// never spins on an already-hidden window.
fn spawn_hide_watchdog(hwnd: isize, force_hide: Arc<AtomicBool>) {
    if hwnd == 0 {
        return;
    }
    std::thread::Builder::new()
        .name("hide-watchdog".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(300));
            if force_hide.load(Ordering::SeqCst) && is_window_visible(hwnd) {
                hide_window_native(hwnd);
            }
        })
        .ok();
}

/// Whether the given window is currently visible (Win32 `IsWindowVisible`).
fn is_window_visible(hwnd: isize) -> bool {
    #[cfg(windows)]
    if hwnd != 0 {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        return unsafe { IsWindowVisible(HWND(hwnd as *mut core::ffi::c_void)).as_bool() };
    }
    false
}

/// Extract the raw Win32 window handle from the eframe creation context.
fn window_handle_of(cc: &eframe::CreationContext<'_>) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle() {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => 0,
        },
        Err(_) => 0,
    }
}

/// Show and focus the main window directly via Win32 (reliable while hidden).
fn show_window_native(hwnd: isize, ctx: &egui::Context) {
    tracing::info!("{}", crate::i18n::tr("显示主窗口"));
    #[cfg(windows)]
    if hwnd != 0 {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        let handle = HWND(hwnd as *mut core::ffi::c_void);
        unsafe {
            let _ = ShowWindow(handle, SW_RESTORE);
            let _ = ShowWindow(handle, SW_SHOW);
            let _ = SetForegroundWindow(handle);
        }
    }
    ctx.request_repaint();
}

/// Hide the main window directly via Win32.
///
/// We minimize *before* hiding: eframe drops pending repaint entries for
/// minimized windows, which prevents the `ControlFlow::Poll` spin that happens
/// when a hidden/occluded window keeps a repaint request that can never be
/// consumed.
fn hide_window_native(hwnd: isize) {
    #[cfg(windows)]
    if hwnd != 0 {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_MINIMIZE};
        let handle = HWND(hwnd as *mut core::ffi::c_void);
        unsafe {
            let _ = ShowWindow(handle, SW_MINIMIZE);
            let _ = ShowWindow(handle, SW_HIDE);
        }
    }
}

/// Build the tray icon and its context menu.
fn build_tray() -> anyhow::Result<(tray_icon::TrayIcon, TrayMenuIds)> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};

    let show = MenuItem::new(crate::i18n::tr("显示窗口"), true, None);
    let reconnect = MenuItem::new(crate::i18n::tr("重新连接"), true, None);
    let test = MenuItem::new(crate::i18n::tr("测试通知"), true, None);
    let quit = MenuItem::new(crate::i18n::tr("退出"), true, None);
    let separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    let items: [&dyn tray_icon::menu::IsMenuItem; 5] =
        [&show, &reconnect, &test, &separator, &quit];
    for item in items {
        menu.append(item)
            .map_err(|e| anyhow::anyhow!("菜单项添加失败: {e}"))?;
    }

    let ids = TrayMenuIds {
        show: show.id().clone(),
        reconnect: reconnect.id().clone(),
        test: test.id().clone(),
        quit: quit.id().clone(),
    };

    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ntfy-windows")
        .with_icon(crate::tray::make_icon()?)
        // Left click should return to the window; right click opens the menu.
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| anyhow::anyhow!("创建托盘图标失败: {e}"))?;

    Ok((tray, ids))
}

fn apply_theme(ctx: &egui::Context, theme: &str) {
    if theme.eq_ignore_ascii_case("light") {
        ctx.set_visuals(egui::Visuals::light());
    } else {
        ctx.set_visuals(egui::Visuals::dark());
    }
}

/// Install CJK + symbol fallback fonts so Chinese text and UI icons render
/// instead of tofu boxes.
///
/// egui ships only Latin + emoji fonts. We append a system CJK font (for
/// Chinese) and Segoe UI Symbol (for symbols such as the power icon) to both
/// the proportional and monospace families.
fn setup_fonts(ctx: &egui::Context, custom_path: &str) {
    let mut fonts = egui::FontDefinitions::default();
    let mut added: Vec<String> = Vec::new();

    let custom_path = custom_path.trim();
    if !custom_path.is_empty() {
        match std::fs::read(custom_path) {
            Ok(data) if ab_glyph::FontRef::try_from_slice(&data).is_ok() => {
                fonts
                    .font_data
                    .insert("custom".to_owned(), egui::FontData::from_owned(data));
                added.push("custom".to_owned());
            }
            Ok(_) => tracing::warn!(
                "{}",
                crate::i18n::tr_args("自定义字体无法解析: {path}", &[("path", custom_path)])
            ),
            Err(e) => {
                let err = e.to_string();
                tracing::warn!(
                    "{}",
                    crate::i18n::tr_args(
                        "读取自定义字体失败 {path}: {err}",
                        &[("path", custom_path), ("err", err.as_str())]
                    )
                );
            }
        }
    }

    for (name, candidates) in [
        ("cjk", &CJK_FONTS[..]),
        ("symbol", &SYMBOL_FONTS[..]),
    ] {
        if let Some((path, data)) = load_first_font(candidates) {
            tracing::info!(
                "{}",
                crate::i18n::tr_args("已加载字体: {path}", &[("path", path.as_str())])
            );
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(data));
            added.push(name.to_owned());
        }
    }

    if added.is_empty() {
        tracing::warn!(
            "{}",
            crate::i18n::tr("未找到可用的中文字体/符号字体，界面中部分字符可能显示为方块")
        );
        return;
    }

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        for name in &added {
            list.push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

/// Windows CJK fonts, in order of preference.
const CJK_FONTS: &[&str] = &[
    "msyh.ttc",    // 微软雅黑
    "msyh.ttf",
    "msyhbd.ttc",
    "simhei.ttf",  // 黑体
    "Deng.ttf",    // 等线
    "simsun.ttc",  // 宋体
    "msjh.ttc",    // 微软正黑
    "YuGothM.ttc", // 游ゴシック
    "NotoSansCJK-Regular.ttc",
    "SourceHanSansSC-Regular.otf",
];

/// Windows symbol fonts, in order of preference.
const SYMBOL_FONTS: &[&str] = &[
    "seguisym.ttf", // Segoe UI Symbol
];

/// Return the first candidate font under `%WINDIR%\Fonts` that exists and can
/// be parsed (so a broken font never panics the app).
fn load_first_font(candidates: &[&str]) -> Option<(String, Vec<u8>)> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    let fonts_dir = std::path::PathBuf::from(windir).join("Fonts");
    for name in candidates {
        let path = fonts_dir.join(name);
        if let Ok(data) = std::fs::read(&path) {
            if ab_glyph::FontRef::try_from_slice(&data).is_ok() {
                return Some((path.display().to_string(), data));
            }
            let path_str = path.display().to_string();
            tracing::warn!(
                "{}",
                crate::i18n::tr_args("字体解析失败，跳过: {path}", &[("path", path_str.as_str())])
            );
        }
    }
    None
}

fn status_color(status: ConnectionStatus) -> egui::Color32 {
    match status {
        ConnectionStatus::Connected => egui::Color32::from_rgb(0x2e, 0xc4, 0x6b),
        ConnectionStatus::Connecting => egui::Color32::from_rgb(0xe6, 0xb4, 0x00),
        ConnectionStatus::Disconnected => egui::Color32::GRAY,
        ConnectionStatus::Error => egui::Color32::from_rgb(0xe0, 0x4f, 0x4f),
    }
}

fn priority_color(priority: i32) -> egui::Color32 {
    match priority {
        5 => egui::Color32::from_rgb(0xe0, 0x4f, 0x4f),
        4 => egui::Color32::from_rgb(0xe6, 0x8a, 0x00),
        3 => egui::Color32::from_rgb(0x4d, 0x9d, 0xe0),
        2 => egui::Color32::from_rgb(0x7a, 0x7a, 0x7a),
        _ => egui::Color32::from_rgb(0x55, 0x55, 0x55),
    }
}

fn level_color(level: tracing::Level) -> egui::Color32 {
    match level {
        tracing::Level::ERROR => egui::Color32::from_rgb(0xe0, 0x4f, 0x4f),
        tracing::Level::WARN => egui::Color32::from_rgb(0xe6, 0xb4, 0x00),
        tracing::Level::INFO => egui::Color32::from_rgb(0x4d, 0x9d, 0xe0),
        tracing::Level::DEBUG => egui::Color32::from_rgb(0x9a, 0x9a, 0x9a),
        tracing::Level::TRACE => egui::Color32::from_rgb(0x70, 0x70, 0x70),
    }
}

fn auth_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::None => "无",
        AuthMethod::Basic => "账户密码 (Basic)",
        AuthMethod::Token => "令牌 (Bearer)",
    }
}

fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::Auto => "自动（JSON → SSE → WS → 轮询）",
        Transport::Json => "JSON 流",
        Transport::Sse => "SSE",
        Transport::Ws => "WebSocket",
        Transport::Poll => "轮询（poll）",
    }
}

/// Human readable label for the configured UI language.
fn language_label(language: &str) -> String {
    match language {
        "zh-CN" | "zh" => "简体中文".to_string(),
        "en-US" | "en" => "English".to_string(),
        _ => crate::i18n::tr("跟随系统"),
    }
}

/// Human readable label for the UI theme.
fn theme_label(theme: &str) -> String {
    if theme.eq_ignore_ascii_case("light") {
        crate::i18n::tr("浅色")
    } else {
        crate::i18n::tr("深色")
    }
}
