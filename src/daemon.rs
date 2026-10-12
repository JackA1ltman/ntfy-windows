//! Background daemon: subscription lifecycle, reconnection and forwarding of
//! ntfy messages to Windows toast notifications.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::{Config, Transport};
use crate::models::NtfyMessage;
use crate::ntfy_client::NtfyClient;
use crate::state::{Command, ConnectionStatus, HistoryEntry, SharedState};
use crate::windows_notifier::{ToastAction, ToastContent, WindowsNotifier};

pub struct Daemon {
    pub state: SharedState,
    pub notifier: Arc<WindowsNotifier>,
    pub cmd_rx: mpsc::UnboundedReceiver<Command>,
    pub cancel: CancellationToken,
    pub http: reqwest::Client,
}

impl Daemon {
    pub async fn run(mut self) {
        let mut config = Arc::new(self.state.lock().config.clone());

        'outer: loop {
            let cancel = self.cancel.child_token();

            let client = match NtfyClient::new(&config) {
                Ok(client) => client,
                Err(e) => {
                    let err = format!("{e:#}");
                    error!(
                        "{}",
                        crate::i18n::tr_args("创建 ntfy 客户端失败: {err}", &[("err", err.as_str())])
                    );
                    let err = format!("{e:#}");
                    self.state.set_status(
                        ConnectionStatus::Error,
                        crate::i18n::tr_args("客户端错误: {err}", &[("err", err.as_str())]),
                    );
                    self.state.set_error(format!("{e:#}"));
                    tokio::select! {
                        _ = self.cancel.cancelled() => break 'outer,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue 'outer,
                    }
                }
            };

            let state = self.state.clone();
            let notifier = self.notifier.clone();
            let http = self.http.clone();
            let topics = config.topics.clone();
            let cfg = config.clone();
            let stream_cancel = cancel.clone();

            let task = tokio::spawn(async move {
                stream_loop(state, notifier, client, cfg, topics, stream_cancel, http).await;
            });

            loop {
                tokio::select! {
                    _ = self.cancel.cancelled() => {
                        info!("{}", crate::i18n::tr("守护进程退出"));
                        cancel.cancel();
                        task.abort();
                        let _ = task.await;
                        break 'outer;
                    }
                    maybe = self.cmd_rx.recv() => {
                        match maybe {
                            Some(Command::Reconnect) => {
                                info!("{}", crate::i18n::tr("收到手动重连请求"));
                                cancel.cancel();
                                task.abort();
                                let _ = task.await;
                                continue 'outer;
                            }
                            Some(Command::UpdateConfig(new_config)) => {
                                info!("{}", crate::i18n::tr("配置已更新，正在重新连接"));
                                config = Arc::from(new_config);
                                self.state.lock().config = (*config).clone();
                                cancel.cancel();
                                task.abort();
                                let _ = task.await;
                                continue 'outer;
                            }
                            Some(Command::TestNotification) => {
                                let message = test_message(&config);
                                let state = self.state.clone();
                                let notifier = self.notifier.clone();
                                let http = self.http.clone();
                                let cfg = config.clone();
                                tokio::spawn(async move {
                                    handle_message(state, notifier, &cfg, &http, message).await;
                                });
                            }
                            Some(Command::Shutdown) | None => {
                                info!("{}", crate::i18n::tr("收到退出指令"));
                                cancel.cancel();
                                task.abort();
                                let _ = task.await;
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Maintain a single stream with exponential backoff and automatic reconnection.
///
/// The backoff is only reset once the stream actually delivered data (a message
/// or a keepalive). If a connection is accepted and then closed without any
/// payload (for example by a proxy or a mis-configured server), the failure
/// counter grows and the delay increases until `retry_max_attempts` is reached.
async fn stream_loop(
    state: SharedState,
    notifier: Arc<WindowsNotifier>,
    client: NtfyClient,
    config: Arc<Config>,
    topics: Vec<String>,
    cancel: CancellationToken,
    http: reqwest::Client,
) {
    let initial = Duration::from_secs(config.connection.retry_initial_secs.max(1));
    let max = Duration::from_secs(
        config
            .connection
            .retry_max_secs
            .max(config.connection.retry_initial_secs.max(1)),
    );
    let max_attempts = config.connection.retry_max_attempts;
    let mut backoff = initial;
    let mut consecutive: u32 = 0;

    // `auto` tries JSON stream, then SSE, then WebSocket, then polling.
    let mut transport = match config.connection.transport {
        Transport::Json => Transport::Json,
        Transport::Sse => Transport::Sse,
        Transport::Ws => Transport::Ws,
        Transport::Poll => Transport::Poll,
        Transport::Auto => Transport::Json,
    };
    let mut tried_sse = matches!(
        config.connection.transport,
        Transport::Sse | Transport::Ws | Transport::Poll
    );
    let mut tried_ws = matches!(config.connection.transport, Transport::Ws | Transport::Poll);
    let mut tried_poll = matches!(config.connection.transport, Transport::Poll);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        if transport == Transport::Poll {
            poll_loop(
                state.clone(),
                notifier.clone(),
                &client,
                config.clone(),
                topics.clone(),
                cancel.clone(),
                http.clone(),
            )
            .await;
            return;
        }

        if topics.is_empty() {
            state.set_status(
                ConnectionStatus::Disconnected,
                crate::i18n::tr("尚未配置订阅主题"),
            );
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(3600)) => continue,
            }
        }

        state.set_status(
            ConnectionStatus::Connecting,
            crate::i18n::tr("正在连接 ntfy..."),
        );

        let received = Arc::new(AtomicBool::new(false));
        let received_handler = received.clone();
        let open_state = state.clone();

        let msg_state = state.clone();
        let msg_notifier = notifier.clone();
        let msg_config = config.clone();
        let msg_http = http.clone();

        let on_open = move || {
            open_state.set_status(
                ConnectionStatus::Connected,
                crate::i18n::tr("已连接，等待通知"),
            );
        };
        let handler = move |message: NtfyMessage| {
            // `open` is only the handshake; require real traffic (a message or a
            // keepalive) before treating the connection as healthy.
            if message.event != "open" {
                received_handler.store(true, Ordering::SeqCst);
            }
            let state = msg_state.clone();
            let notifier = msg_notifier.clone();
            let config = msg_config.clone();
            let http = msg_http.clone();
            async move {
                handle_message(state, notifier, &config, &http, message).await;
            }
        };

        let result = if transport == Transport::Ws {
            client.run_ws(&topics, &cancel, on_open, handler).await
        } else {
            client
                .run_stream(&topics, transport, &cancel, on_open, handler)
                .await
        };

        if cancel.is_cancelled() {
            break;
        }

        let had_data = received.load(Ordering::SeqCst);
        if had_data {
            consecutive = 0;
            state.record_success();
        } else {
            consecutive = consecutive.saturating_add(1);
        }

        match &result {
            Ok(()) => {
                if had_data {
                    warn!("{}", crate::i18n::tr("与 ntfy 的连接已关闭，将自动重连"));
                } else {
                    let n = consecutive.to_string();
                    warn!(
                        "{}",
                        crate::i18n::tr_args(
                            "与 ntfy 的连接被立即关闭（连续失败 {n} 次）",
                            &[("n", n.as_str())]
                        )
                    );
                    let n = consecutive.to_string();
                    state.record_failure(crate::i18n::tr_args(
                        "连接被立即关闭（连续失败 {n} 次）",
                        &[("n", n.as_str())],
                    ));
                }
            }
            Err(e) => {
                let n = consecutive.to_string();
                let err = format!("{e:#}");
                error!(
                    "{}",
                    crate::i18n::tr_args(
                        "订阅出错（连续失败 {n} 次）: {err}",
                        &[("n", n.as_str()), ("err", err.as_str())]
                    )
                );
                state.record_failure(format!("{e:#}"));
            }
        }

        state.inc_reconnect();

        if !had_data && config.connection.transport == Transport::Auto {
            if !tried_sse {
                tried_sse = true;
                transport = Transport::Sse;
                info!("{}", crate::i18n::tr("JSON 流被立即关闭，改用 SSE 端点重试"));
            } else if !tried_ws {
                tried_ws = true;
                transport = Transport::Ws;
                info!("{}", crate::i18n::tr("SSE 也被关闭，改用 WebSocket 重试"));
            } else if !tried_poll {
                tried_poll = true;
                transport = Transport::Poll;
                info!("{}", crate::i18n::tr("WebSocket 也被关闭，改用轮询模式"));
            }
        }

        if max_attempts > 0 && consecutive >= max_attempts {
            let n = consecutive.to_string();
            let msg = crate::i18n::tr_args(
                "已连续失败 {n} 次，停止自动重试；请检查网络或配置后手动重连",
                &[("n", n.as_str())],
            );
            warn!("{msg}");
            state.set_status(ConnectionStatus::Error, msg);
            return;
        }

        let wait = if had_data { initial } else { backoff };
        if had_data {
            backoff = initial;
        } else {
            backoff = (backoff * 2).min(max);
        }
        let n = wait.as_secs().to_string();
        let c = consecutive.to_string();
        state.set_status(
            ConnectionStatus::Disconnected,
            crate::i18n::tr_args("{n} 秒后重连（连续失败 {c} 次）", &[("n", n.as_str()), ("c", c.as_str())]),
        );
        let n = wait.as_secs().to_string();
        debug!(
            "{}",
            crate::i18n::tr_args("{n} 秒后重新连接", &[("n", n.as_str())])
        );

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Poll the server periodically. Used when streaming is not available.
async fn poll_loop(
    state: SharedState,
    notifier: Arc<WindowsNotifier>,
    client: &NtfyClient,
    config: Arc<Config>,
    topics: Vec<String>,
    cancel: CancellationToken,
    http: reqwest::Client,
) {
    let interval = Duration::from_secs(config.connection.poll_interval_secs.max(1));
    let mut since = chrono::Utc::now().timestamp();

    loop {
        if cancel.is_cancelled() {
            break;
        }
        if topics.is_empty() {
            state.set_status(
                ConnectionStatus::Disconnected,
                crate::i18n::tr("尚未配置订阅主题"),
            );
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(3600)) => continue,
            }
        }

        let interval_secs = interval.as_secs().to_string();
        state.set_status(
            ConnectionStatus::Connecting,
            crate::i18n::tr_args(
                "正在轮询 ntfy（每 {n} 秒）...",
                &[("n", interval_secs.as_str())],
            ),
        );

        let st = state.clone();
        let nt = notifier.clone();
        let cfg = config.clone();
        let dl = http.clone();
        let result = client
            .poll(&topics, since, &cancel, move |message: NtfyMessage| {
                let st = st.clone();
                let nt = nt.clone();
                let cfg = cfg.clone();
                let dl = dl.clone();
                async move {
                    handle_message(st, nt, &cfg, &dl, message).await;
                }
            })
            .await;

        match result {
            Ok(newest) => {
                if newest > since {
                    since = newest;
                }
                state.record_success();
                state.set_status(
                    ConnectionStatus::Connected,
                    crate::i18n::tr_args("轮询中（每 {n} 秒）", &[("n", interval_secs.as_str())]),
                );
            }
            Err(e) => {
                let err = format!("{e:#}");
                error!(
                    "{}",
                    crate::i18n::tr_args("轮询出错: {err}", &[("err", err.as_str())])
                );
                state.record_failure(format!("{e:#}"));
                let err = e.to_string();
                state.set_status(
                    ConnectionStatus::Error,
                    crate::i18n::tr_args("轮询出错: {err}", &[("err", err.as_str())]),
                );
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

/// Filter, persist and forward a single message.
async fn handle_message(
    state: SharedState,
    notifier: Arc<WindowsNotifier>,
    config: &Config,
    http: &reqwest::Client,
    message: NtfyMessage,
) {
    if !message.is_message() || message.topic.is_empty() {
        return;
    }
    if config.notification.ignored().iter().any(|t| t == &message.topic) {
        debug!(
            "{}",
            crate::i18n::tr_args("忽略主题 {topic} 的消息", &[("topic", message.topic.as_str())])
        );
        return;
    }

    let n = &config.notification;

    info!(
        topic = %message.topic,
        id = %message.id,
        priority = message.priority(),
        "{}",
        crate::i18n::tr("收到 ntfy 通知")
    );

    let attachment = message.attachment.as_ref();
    let url = attachment
        .and_then(|a| a.url.as_deref())
        .filter(|u| !u.is_empty());
    let is_image = attachment.map(|a| a.is_image()).unwrap_or(false);

    let mut saved_image: Option<PathBuf> = None;
    let mut saved_file: Option<PathBuf> = None;

    if let Some(url) = url {
        if is_image {
            // Images are normalised and written straight into the images folder
            // (used as the toast hero image and/or kept as an archive).
            if n.save_images || (n.enabled && n.show_attachments) {
                let stem = attachment_stem(&message);
                saved_image =
                    fetch_image(http, &n.image_dir_path(), url, n.max_image_mb, &stem).await;
            }
        } else if n.save_files {
            // Non-image attachments are stored verbatim in the files folder.
            let name = attachment_file_name(&message);
            saved_file = fetch_file(http, &n.files_dir_path(), url, n.max_file_mb, &name).await;
        }
    }

    state.add_history(HistoryEntry::from_message(
        &message,
        saved_image.as_deref(),
        saved_file.as_deref(),
    ));

    if !n.enabled {
        return;
    }
    if message.priority() < n.min_priority {
        return;
    }

    let content = build_toast(config, http, &message, saved_image).await;
    match notifier.show(content) {
        Ok(()) => state.inc_shown(),
        Err(e) => {
            let err = format!("{e:#}");
            warn!(
                "{}",
                crate::i18n::tr_args("发送 Windows 通知失败: {err}", &[("err", err.as_str())])
            );
            state.set_error(format!("toast: {e:#}"));
        }
    }
}

/// Turn a message into toast content. The hero image is passed in already
/// downloaded (see `handle_message`); the icon is downloaded here.
async fn build_toast(
    config: &Config,
    http: &reqwest::Client,
    message: &NtfyMessage,
    hero_image: Option<PathBuf>,
) -> ToastContent {
    let priority = message.priority();
    let n = &config.notification;

    let silent = !n.sound || priority <= 1;
    // Sticky scenarios keep the toast on screen until the user handles it.
    // `alarm` additionally loops the sound, so fall back to `reminder` when
    // notifications are silent.
    let scenario = if n.sticky_min_priority >= 1 && priority >= n.sticky_min_priority {
        let requested = n.sticky_scenario.trim().to_ascii_lowercase();
        if requested == "alarm" && !silent {
            Some("alarm".to_string())
        } else {
            Some("reminder".to_string())
        }
    } else {
        None
    };

    let mut content = ToastContent {
        priority,
        silent,
        long_duration: priority >= 4,
        scenario,
        ..Default::default()
    };

    let emoji = crate::emoji::for_tags(&message.tags);
    let title = message.display_title();
    content.title = Some(if emoji.is_empty() {
        title
    } else {
        format!("{emoji} {title}")
    });

    let mut body = message.text();

    if let Some(attachment) = &message.attachment {
        let mime = format!("{:?}", attachment.mime);
        debug!(
            "{}",
            crate::i18n::tr_args(
                "通知附件: name={name} type={mime} is_image={image}",
                &[
                    ("name", attachment.name.as_str()),
                    ("mime", mime.as_str()),
                    ("image", if attachment.is_image() { "true" } else { "false" }),
                ]
            )
        );
        if attachment.is_image() && n.show_attachments {
            content.hero_image = hero_image;
        }
        if content.hero_image.is_none() && !attachment.name.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&format!("📎 {}", attachment.name));
        }
    }

    if !message.tags.is_empty() {
        let tags = message
            .tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&tags);
    }
    content.body = body;

    if n.download_images {
        if let Some(icon) = message.icon.as_deref() {
            if !icon.is_empty() {
                let icon_dir = n.image_dir_path().join("icons");
                if let Some(path) = fetch_icon(http, &icon_dir, icon, n.max_image_mb).await {
                    content.app_logo = Some(path);
                }
            }
        }
    }
    if content.app_logo.is_none() && !n.app_icon.is_empty() {
        let path = PathBuf::from(&n.app_icon);
        if path.exists() {
            content.app_logo = Some(path);
        }
    }

    if let Some(click) = message.click.as_deref() {
        if !click.is_empty() {
            content.click_url = Some(click.to_string());
        }
    }

    for action in &message.actions {
        // Windows has no equivalent for Android broadcast intents, so only
        // `view` actions (open a URL) are forwarded.
        if action.action == "view" {
            if let Some(url) = action.url.as_deref() {
                if !url.is_empty() {
                    content.actions.push(ToastAction {
                        label: if action.label.is_empty() {
                            crate::i18n::tr("打开")
                        } else {
                            action.label.clone()
                        },
                        url: url.to_string(),
                    });
                }
            }
        }
    }

    content
}

/// Download an image, normalise it and write it into `dir` using `name_stem`
/// as the file name (the extension is derived from the encoded format).
/// Returns `None` on any failure so that a missing preview never blocks the
/// notification itself.
///
/// The image is decoded and re-encoded (PNG for transparent images, JPEG
/// otherwise) and downscaled so that Windows Toast always accepts it,
/// regardless of the source format or size.
async fn fetch_image(
    http: &reqwest::Client,
    dir: &Path,
    url: &str,
    max_mb: u64,
    name_stem: &str,
) -> Option<PathBuf> {
    let response = match http.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            let err = e.to_string();
            warn!(
                "{}",
                crate::i18n::tr_args("下载图片失败 {url}: {err}", &[("url", url), ("err", err.as_str())])
            );
            return None;
        }
    };
    if !response.status().is_success() {
        let status = response.status().to_string();
        warn!(
            "{}",
            crate::i18n::tr_args(
                "下载图片失败 {url}: HTTP {status}",
                &[("url", url), ("status", status.as_str())]
            )
        );
        return None;
    }

    let limit = max_mb.max(1) * 1024 * 1024;
    if let Some(len) = response.content_length() {
        if len > limit {
            let len = len.to_string();
            let limit = limit.to_string();
            warn!(
                "{}",
                crate::i18n::tr_args(
                    "图片超过大小限制（{len} > {limit} 字节），跳过: {url}",
                    &[("len", len.as_str()), ("limit", limit.as_str()), ("url", url)]
                )
            );
            return None;
        }
    }

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            let err = e.to_string();
            warn!(
                "{}",
                crate::i18n::tr_args("读取图片数据失败 {url}: {err}", &[("url", url), ("err", err.as_str())])
            );
            return None;
        }
    };
    if bytes.len() as u64 > limit {
        warn!(
            "{}",
            crate::i18n::tr_args("图片超过大小限制，跳过: {url}", &[("url", url)])
        );
        return None;
    }

    // Decode and normalise to PNG.
    let decoded = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(e) => {
            let err = e.to_string();
            warn!(
                "{}",
                crate::i18n::tr_args("无法解析图片 {url}: {err}", &[("url", url), ("err", err.as_str())])
            );
            return None;
        }
    };
    const MAX_DIM: u32 = 512;
    let decoded = if decoded.width() > MAX_DIM || decoded.height() > MAX_DIM {
        decoded.resize(MAX_DIM, MAX_DIM, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };

    // Keep transparency as PNG, use JPEG for photos to stay under the toast size limit.
    let has_alpha = decoded.color().has_alpha();
    let mut out: Vec<u8> = Vec::new();
    let ext = if has_alpha {
        match decoded.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png) {
            Ok(()) => "png",
            Err(e) => {
                let err = e.to_string();
                warn!(
                    "{}",
                    crate::i18n::tr_args("编码 PNG 失败 {url}: {err}", &[("url", url), ("err", err.as_str())])
                );
                return None;
            }
        }
    } else {
        match decoded
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
        {
            Ok(()) => "jpg",
            Err(e) => {
                let err = e.to_string();
                warn!(
                    "{}",
                    crate::i18n::tr_args("编码 JPEG 失败 {url}: {err}", &[("url", url), ("err", err.as_str())])
                );
                return None;
            }
        }
    };

    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        let err = e.to_string();
        let dir_str = dir.display().to_string();
        warn!(
            "{}",
            crate::i18n::tr_args(
                "创建图片保存目录失败 {path}: {err}",
                &[("path", dir_str.as_str()), ("err", err.as_str())]
            )
        );
        return None;
    }
    let path = unique_path(dir, &format!("{name_stem}.{ext}"));
    if let Err(e) = tokio::fs::write(&path, &out).await {
        let err = e.to_string();
        let path_str = path.display().to_string();
        warn!(
            "{}",
            crate::i18n::tr_args(
                "保存通知图片失败 {path}: {err}",
                &[("path", path_str.as_str()), ("err", err.as_str())]
            )
        );
        return None;
    }

    let bytes = out.len().to_string();
    let path_string = path.display().to_string();
    info!(
        "{}",
        crate::i18n::tr_args(
            "已保存通知图片（{bytes} 字节）: {path}",
            &[("bytes", bytes.as_str()), ("path", path_string.as_str())]
        )
    );
    Some(path)
}

/// Download a non-image attachment and store it verbatim into `dir`.
async fn fetch_file(
    http: &reqwest::Client,
    dir: &Path,
    url: &str,
    max_mb: u64,
    file_name: &str,
) -> Option<PathBuf> {
    let bytes = match download_bytes(http, url, max_mb).await {
        Ok(b) => b,
        Err(FetchError::Request(err)) => {
            warn!(
                "{}",
                crate::i18n::tr_args(
                    "下载文件失败 {url}: {err}",
                    &[("url", url), ("err", err.as_str())]
                )
            );
            return None;
        }
        Err(FetchError::Status(status)) => {
            warn!(
                "{}",
                crate::i18n::tr_args(
                    "下载文件失败 {url}: HTTP {status}",
                    &[("url", url), ("status", status.as_str())]
                )
            );
            return None;
        }
        Err(FetchError::TooLarge { len, limit }) => {
            let len = len.to_string();
            let limit = limit.to_string();
            warn!(
                "{}",
                crate::i18n::tr_args(
                    "文件超过大小限制（{len} > {limit} 字节），跳过: {url}",
                    &[("len", len.as_str()), ("limit", limit.as_str()), ("url", url)]
                )
            );
            return None;
        }
        Err(FetchError::Read(err)) => {
            warn!(
                "{}",
                crate::i18n::tr_args(
                    "读取文件数据失败 {url}: {err}",
                    &[("url", url), ("err", err.as_str())]
                )
            );
            return None;
        }
    };

    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        let err = e.to_string();
        let dir_str = dir.display().to_string();
        warn!(
            "{}",
            crate::i18n::tr_args(
                "创建文件保存目录失败 {path}: {err}",
                &[("path", dir_str.as_str()), ("err", err.as_str())]
            )
        );
        return None;
    }
    let path = unique_path(dir, file_name);
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        let err = e.to_string();
        let path_str = path.display().to_string();
        warn!(
            "{}",
            crate::i18n::tr_args(
                "保存附件文件失败 {path}: {err}",
                &[("path", path_str.as_str()), ("err", err.as_str())]
            )
        );
        return None;
    }

    let size = bytes.len().to_string();
    let path_str = path.display().to_string();
    info!(
        "{}",
        crate::i18n::tr_args(
            "已保存附件文件（{bytes} 字节）: {path}",
            &[("bytes", size.as_str()), ("path", path_str.as_str())]
        )
    );
    Some(path)
}

/// Fetch (and cache) a small icon used as the toast logo. Icons live in `dir`
/// under a stable hash name, so repeated messages reuse the same file.
async fn fetch_icon(
    http: &reqwest::Client,
    dir: &Path,
    url: &str,
    max_mb: u64,
) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let stem = format!("icon-{:016x}", hasher.finish());
    if let Some(existing) = find_by_stem(dir, &stem) {
        return Some(existing);
    }
    fetch_image(http, dir, url, max_mb, &stem).await
}

/// Find an existing file in `dir` whose file stem equals `stem`.
fn find_by_stem(dir: &Path, stem: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_stem().and_then(|s| s.to_str()) == Some(stem) {
            return Some(path);
        }
    }
    None
}

/// Shared downloader used by `fetch_image` / `fetch_file`.
enum FetchError {
    Request(String),
    Status(String),
    TooLarge { len: u64, limit: u64 },
    Read(String),
}

async fn download_bytes(
    http: &reqwest::Client,
    url: &str,
    max_mb: u64,
) -> Result<Vec<u8>, FetchError> {
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|e| FetchError::Request(e.to_string()))?;
    if !response.status().is_success() {
        return Err(FetchError::Status(response.status().to_string()));
    }
    let limit = max_mb.max(1) * 1024 * 1024;
    if let Some(len) = response.content_length() {
        if len > limit {
            return Err(FetchError::TooLarge { len, limit });
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| FetchError::Read(e.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(FetchError::TooLarge {
            len: bytes.len() as u64,
            limit,
        });
    }
    Ok(bytes.to_vec())
}

/// Descriptive file stem for an attachment: `YYYYmmdd-HHMMSS_topic_name`.
fn attachment_stem(message: &NtfyMessage) -> String {
    let timestamp = message.received_at().format("%Y%m%d-%H%M%S");
    let topic = sanitize_component(&message.topic);
    let stem = message
        .attachment
        .as_ref()
        .map(|a| a.name.as_str())
        .map(|name| {
            Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        })
        .map(sanitize_component)
        .unwrap_or_default();
    if stem.is_empty() {
        format!("{timestamp}_{topic}")
    } else {
        format!("{timestamp}_{topic}_{stem}")
    }
}

/// Descriptive file name (with the original extension) for a non-image file.
fn attachment_file_name(message: &NtfyMessage) -> String {
    let stem = attachment_stem(message);
    let ext = message
        .attachment
        .as_ref()
        .map(|a| a.name.as_str())
        .map(|name| {
            Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
        })
        .map(sanitize_component)
        .unwrap_or_default();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

/// Make a string safe to use as a single Windows file name component.
fn sanitize_component(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim().trim_matches('.').to_string();
    if out.chars().count() > 60 {
        out = out.chars().take(60).collect();
    }
    out
}

/// Return a path inside `dir` that does not exist yet.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(file_name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            for i in 1..10_000 {
                let candidate = dir.join(format!("{stem}-{i}.{ext}"));
                if !candidate.exists() {
                    return candidate;
                }
            }
            dir.join(format!(
                "{stem}-{}.{ext}",
                chrono::Utc::now().timestamp_millis()
            ))
        }
        None => {
            for i in 1..10_000 {
                let candidate = dir.join(format!("{stem}-{i}"));
                if !candidate.exists() {
                    return candidate;
                }
            }
            dir.join(format!("{stem}-{}", chrono::Utc::now().timestamp_millis()))
        }
    }
}

fn test_message(config: &Config) -> NtfyMessage {
    NtfyMessage {
        id: format!("test-{}", chrono::Utc::now().timestamp_millis()),
        time: chrono::Utc::now().timestamp(),
        event: "message".to_string(),
        topic: config
            .topics
            .first()
            .cloned()
            .unwrap_or_else(|| "test".to_string()),
        title: Some("ntfy-windows 测试通知".to_string()),
        message: Some("如果你能看到这条 Windows 通知，说明通知转发链路工作正常。".to_string()),
        priority: Some(4),
        tags: vec!["test_tube".to_string(), "bell".to_string()],
        ..Default::default()
    }
}
