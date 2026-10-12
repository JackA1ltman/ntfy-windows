//! Configuration model plus load/save helpers.
//!
//! The configuration lives in `config.json`, next to the executable, so the
//! program can be used as a portable application. If that directory is not
//! writable (e.g. installed under `Program Files`), saving transparently falls
//! back to `%APPDATA%\ntfy-windows\config.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Root configuration, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub topics: Vec<String>,
    pub autostart: bool,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub connection: ConnectionConfig,
    pub notification: NotificationConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            topics: Vec::new(),
            autostart: false,
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
            connection: ConnectionConfig::default(),
            notification: NotificationConfig::default(),
            ui: UiConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub base_url: String,
    pub insecure_tls: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            base_url: "https://ntfy.sh".to_string(),
            insecure_tls: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    None,
    Basic,
    Token,
}

impl Default for AuthMethod {
    fn default() -> Self {
        AuthMethod::None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub method: AuthMethod,
    pub username: String,
    pub password: String,
    pub token: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            method: AuthMethod::None,
            username: String::new(),
            password: String::new(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub min_priority: i32,
    pub sound: bool,
    pub show_attachments: bool,
    pub download_images: bool,
    pub max_image_mb: u64,
    pub app_name: String,
    pub app_id: String,
    pub app_icon: String,
    pub ignore_topics: String,
    /// Save received attachment images to the images folder.
    pub save_images: bool,
    /// Directory for saved images. Empty = default (`%LOCALAPPDATA%\ntfy-windows\images`).
    pub image_dir: String,
    /// Save received non-image attachments to the files folder.
    pub save_files: bool,
    /// Directory for saved files. Empty = default (`%LOCALAPPDATA%\ntfy-windows\files`).
    pub files_dir: String,
    /// Maximum size of a saved file attachment, in MB.
    pub max_file_mb: u64,
    /// Notifications with a priority >= this value stay on screen until the
    /// user handles them (`scenario="reminder"`/`"alarm"`). `0` disables it.
    pub sticky_min_priority: i32,
    /// Scenario used for sticky notifications: `"reminder"` or `"alarm"`.
    pub sticky_scenario: String,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_priority: 1,
            sound: true,
            show_attachments: true,
            download_images: true,
            max_image_mb: 10,
            app_name: "ntfy-windows".to_string(),
            app_id: "com.ntfy.windows".to_string(),
            app_icon: String::new(),
            ignore_topics: String::new(),
            save_images: true,
            image_dir: String::new(),
            save_files: true,
            files_dir: String::new(),
            max_file_mb: 20,
            sticky_min_priority: 5,
            sticky_scenario: "reminder".to_string(),
        }
    }
}

impl NotificationConfig {
    /// Split the comma separated `ignore_topics` field.
    pub fn ignored(&self) -> Vec<String> {
        self.ignore_topics
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// Directory where received images are archived.
    pub fn image_dir_path(&self) -> PathBuf {
        let custom = self.image_dir.trim();
        if custom.is_empty() {
            images_dir()
        } else {
            PathBuf::from(custom)
        }
    }

    /// Directory where received (non-image) files are archived.
    pub fn files_dir_path(&self) -> PathBuf {
        let custom = self.files_dir.trim();
        if custom.is_empty() {
            files_dir()
        } else {
            PathBuf::from(custom)
        }
    }
}

/// Reconnection / retry policy for the ntfy stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    /// Wire transport: `auto`, `json` or `sse`.
    /// `auto` tries the JSON stream and falls back to SSE if the JSON stream is
    /// closed immediately (common behind Cloudflare / buffering proxies).
    pub transport: Transport,
    /// Delay before the first retry, in seconds.
    pub retry_initial_secs: u64,
    /// Upper bound of the exponential backoff, in seconds.
    pub retry_max_secs: u64,
    /// Maximum consecutive failed attempts before giving up.
    /// `0` means retry forever.
    pub retry_max_attempts: u32,
    /// Interval between HTTP polls when `transport = "poll"`, in seconds.
    pub poll_interval_secs: u64,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            transport: Transport::Auto,
            retry_initial_secs: 2,
            retry_max_secs: 120,
            retry_max_attempts: 0,
            poll_interval_secs: 15,
        }
    }
}

/// How notification messages are received from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Try JSON stream, then SSE, then WebSocket, then polling.
    Auto,
    /// `/json` newline delimited JSON stream.
    Json,
    /// `/sse` Server-Sent Events stream (more proxy friendly).
    Sse,
    /// `/ws` WebSocket stream.
    Ws,
    /// Repeated `/json?poll=1&since=...` requests (works without streaming).
    Poll,
}

impl Default for Transport {
    fn default() -> Self {
        Transport::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// `dark` or `light`.
    pub theme: String,
    pub history_limit: usize,
    /// Optional path to a `.ttf`/`.ttc` font used for CJK glyphs.
    /// Empty means "auto detect a system CJK font".
    pub font_path: String,
    /// Minimize to the system tray instead of exiting when the window closes.
    pub close_to_tray: bool,
    /// UI language: `system`, `zh-CN`, `en-US`, or any `lang/<code>.toml`.
    pub language: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            history_limit: 500,
            font_path: String::new(),
            close_to_tray: true,
            language: "system".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl Config {
    /// Load from the executable directory, falling back to `%APPDATA%`.
    pub fn load() -> Result<Config> {
        let primary = config_path();
        if primary.exists() {
            return Self::load_from(&primary);
        }
        let fallback = appdata_dir().join("config.json");
        if fallback.exists() {
            return Self::load_from(&fallback);
        }
        Self::load_from(&primary)
    }

    pub fn load_from(path: &Path) -> Result<Config> {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("读取配置失败: {}", path.display()))?;
            let is_toml = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("toml"))
                .unwrap_or(false);
            let cfg: Config = if is_toml {
                toml::from_str(&text)
                    .with_context(|| format!("解析 TOML 配置失败: {}", path.display()))?
            } else {
                serde_json::from_str(&text)
                    .with_context(|| format!("解析 JSON 配置失败: {}", path.display()))?
            };
            return Ok(cfg);
        }

        // Migrate an old TOML config if one exists.
        if let Some(cfg) = load_legacy_config() {
            if let Err(e) = cfg.save_to(path) {
                tracing::warn!("迁移旧配置失败: {e:#}");
            }
            return Ok(cfg);
        }

        let cfg = Config::default();
        if let Err(e) = cfg.save_to(path) {
            tracing::warn!("创建默认配置失败: {e:#}");
        }
        Ok(cfg)
    }

    /// Save to the executable directory, falling back to `%APPDATA%` on error.
    pub fn save(&self) -> Result<()> {
        match self.save_to(&config_path()) {
            Ok(()) => Ok(()),
            Err(e) => {
                let fallback = appdata_dir().join("config.json");
                tracing::warn!(
                    "无法写入程序目录配置 ({e:#})，改用 {}",
                    fallback.display()
                );
                self.save_to(&fallback)
            }
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        std::fs::write(path, text)
            .with_context(|| format!("写入配置失败: {}", path.display()))?;
        Ok(())
    }
}

/// Load a legacy `config.toml` (from the exe dir or `%APPDATA%`) if present.
fn load_legacy_config() -> Option<Config> {
    for path in [
        exe_dir().join("config.toml"),
        appdata_dir().join("config.toml"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = toml::from_str::<Config>(&text) {
                return Some(cfg);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Well known paths
// ---------------------------------------------------------------------------

/// Directory containing the running executable.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `config.json` next to the executable.
pub fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

/// `%APPDATA%\ntfy-windows`
pub fn appdata_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ntfy-windows")
}

/// `%LOCALAPPDATA%\ntfy-windows`
pub fn localappdata_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ntfy-windows")
}

pub fn history_path() -> PathBuf {
    appdata_dir().join("history.json")
}

pub fn log_dir() -> PathBuf {
    localappdata_dir().join("logs")
}

/// Default directory for archived notification images.
pub fn images_dir() -> PathBuf {
    localappdata_dir().join("images")
}

/// Default directory for archived (non-image) attachment files.
pub fn files_dir() -> PathBuf {
    localappdata_dir().join("files")
}

/// Ensure the data directories (logs, images, files) exist.
///
/// Called at startup so that the "open folder" actions always have a target
/// (opening a non-existent folder otherwise silently does nothing).
pub fn ensure_data_dirs(config: &Config) {
    let dirs = [
        log_dir(),
        config.notification.image_dir_path(),
        config.notification.files_dir_path(),
    ];
    for dir in dirs {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            let err = e.to_string();
            let path = dir.display().to_string();
            tracing::warn!(
                "{}",
                crate::i18n::tr_args(
                    "创建目录失败 {path}: {err}",
                    &[("path", path.as_str()), ("err", err.as_str())]
                )
            );
        }
    }
}
