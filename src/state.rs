//! Shared, thread safe application state used by both the daemon and the GUI.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Local};
use tracing::Level;

use crate::config::Config;
use crate::models::NtfyMessage;

/// Maximum number of log lines kept in memory for the GUI.
const MAX_LOGS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionStatus::Disconnected => "已断开",
            ConnectionStatus::Connecting => "连接中",
            ConnectionStatus::Connected => "已连接",
            ConnectionStatus::Error => "错误",
        }
    }
}

/// Commands sent from the GUI to the daemon.
#[derive(Debug, Clone)]
pub enum Command {
    /// Drop the current stream and reconnect immediately.
    Reconnect,
    /// Apply a new configuration and restart the subscription.
    UpdateConfig(Box<Config>),
    /// Emit a synthetic notification to verify the pipeline.
    TestNotification,
    /// Shut the daemon down.
    Shutdown,
}

/// A notification that has already been received, persisted for the GUI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub topic: String,
    pub title: String,
    pub message: String,
    pub priority: i32,
    pub tags: Vec<String>,
    pub received_at: DateTime<Local>,
    pub raw: NtfyMessage,
    /// Path of the archived image, if the notification had an image attachment.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Path of the archived (non-image) attachment, if any.
    #[serde(default)]
    pub file_path: Option<String>,
}

impl HistoryEntry {
    pub fn from_message(
        msg: &NtfyMessage,
        image_path: Option<&Path>,
        file_path: Option<&Path>,
    ) -> Self {
        Self {
            id: msg.id.clone(),
            topic: msg.topic.clone(),
            title: msg.display_title(),
            message: msg.text(),
            priority: msg.priority(),
            tags: msg.tags.clone(),
            received_at: msg.received_at(),
            raw: msg.clone(),
            image_path: image_path.map(|p| p.to_string_lossy().into_owned()),
            file_path: file_path.map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: DateTime<Local>,
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// Log ring buffer plus a lazily rebuilt, immutable snapshot for cheap reads.
///
/// `logs()` used to clone the whole buffer on every call (once per frame while
/// the Logs tab is open). The snapshot is now rebuilt only when the buffer
/// actually changes, so readers can cheaply clone an `Arc` instead.
#[derive(Default)]
struct LogBuffer {
    entries: VecDeque<LogEntry>,
    /// Cached snapshot of `entries`, rebuilt only when `dirty` is set.
    snapshot: Option<Arc<Vec<LogEntry>>>,
    dirty: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub received: u64,
    pub shown: u64,
    pub errors: u64,
    pub reconnects: u64,
    /// Total number of failed connection attempts.
    pub failures: u64,
    /// Failed attempts since the last successful data-carrying connection.
    pub consecutive_failures: u32,
}

/// The mutable part of the application state.
pub struct AppState {
    pub config: Config,
    pub status: ConnectionStatus,
    pub status_detail: String,
    pub connected_since: Option<DateTime<Local>>,
    pub history: VecDeque<HistoryEntry>,
    pub stats: Stats,
    pub last_error: Option<String>,
    /// Whether the history has unsaved changes.
    pub dirty: bool,
}

/// Cheap cloneable handle to the shared state.
#[derive(Clone)]
pub struct SharedState {
    inner: Arc<Mutex<AppState>>,
    logs: Arc<Mutex<LogBuffer>>,
    waker: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
}

impl SharedState {
    pub fn new(config: Config) -> Self {
        let state = AppState {
            config,
            status: ConnectionStatus::Disconnected,
            status_detail: "未连接".to_string(),
            connected_since: None,
            history: VecDeque::new(),
            stats: Stats::default(),
            last_error: None,
            dirty: false,
        };
        Self {
            inner: Arc::new(Mutex::new(state)),
            logs: Arc::new(Mutex::new(LogBuffer::default())),
            waker: Arc::new(Mutex::new(None)),
        }
    }

    /// Lock the primary state. Recovers from a poisoned mutex instead of panicking.
    pub fn lock(&self) -> MutexGuard<'_, AppState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a callback invoked whenever the state changes. Used by the GUI
    /// to repaint on demand instead of polling continuously.
    pub fn set_waker<F: Fn() + Send + Sync + 'static>(&self, f: F) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// Wake the registered waker, if any.
    fn wake(&self) {
        let guard = self.waker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_ref() {
            f();
        }
    }

    /// Snapshot of the current log buffer (oldest first).
    ///
    /// The snapshot is cached and only rebuilt when the buffer changed, so
    /// callers can cheaply clone the returned `Arc` on every frame instead of
    /// copying the whole buffer each time.
    pub fn logs(&self) -> Arc<Vec<LogEntry>> {
        let mut guard = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        if guard.dirty || guard.snapshot.is_none() {
            let snapshot: Arc<Vec<LogEntry>> = Arc::new(guard.entries.iter().cloned().collect());
            guard.snapshot = Some(snapshot);
            guard.dirty = false;
        }
        guard.snapshot.clone().unwrap_or_default()
    }

    /// Append a log line. Uses a dedicated mutex to avoid deadlocks with `lock()`.
    pub fn push_log(&self, level: Level, target: &str, message: &str) {
        let mut guard = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        guard.entries.push_back(LogEntry {
            time: Local::now(),
            level,
            target: target.to_string(),
            message: message.to_string(),
        });
        while guard.entries.len() > MAX_LOGS {
            guard.entries.pop_front();
        }
        guard.dirty = true;
    }

    pub fn clear_logs(&self) {
        let mut guard = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        guard.entries.clear();
        guard.snapshot = Some(Arc::new(Vec::new()));
        guard.dirty = false;
    }

    /// Returns `true` (and clears the flag) if the history changed since the
    /// last save. Used to avoid writing the history file when nothing changed.
    pub fn take_dirty(&self) -> bool {
        let mut s = self.lock();
        std::mem::take(&mut s.dirty)
    }

    pub fn set_status(&self, status: ConnectionStatus, detail: impl Into<String>) {
        {
            let mut s = self.lock();
            s.status = status;
            s.status_detail = detail.into();
            if status == ConnectionStatus::Connected {
                s.connected_since = Some(Local::now());
            } else {
                s.connected_since = None;
            }
        }
        self.wake();
    }

    pub fn set_error(&self, message: String) {
        {
            let mut s = self.lock();
            s.last_error = Some(message);
            s.stats.errors += 1;
        }
        self.wake();
    }

    /// Record a failed connection attempt (accepted then closed, or an error).
    pub fn record_failure(&self, message: impl Into<String>) {
        {
            let mut s = self.lock();
            s.stats.failures += 1;
            s.stats.consecutive_failures = s.stats.consecutive_failures.saturating_add(1);
            s.last_error = Some(message.into());
        }
        self.wake();
    }

    /// Reset the consecutive failure counter after a healthy connection.
    pub fn record_success(&self) {
        self.lock().stats.consecutive_failures = 0;
        self.wake();
    }

    pub fn add_history(&self, entry: HistoryEntry) {
        {
            let mut s = self.lock();
            s.stats.received += 1;
            s.history.push_front(entry);
            let limit = s.config.ui.history_limit.max(1);
            while s.history.len() > limit {
                s.history.pop_back();
            }
            s.dirty = true;
        }
        self.wake();
    }

    pub fn clear_history(&self) {
        {
            let mut s = self.lock();
            s.history.clear();
            s.dirty = true;
        }
        self.wake();
    }

    pub fn inc_shown(&self) {
        self.lock().stats.shown += 1;
        self.wake();
    }

    pub fn inc_reconnect(&self) {
        self.lock().stats.reconnects += 1;
        self.wake();
    }

    // -- persistence --------------------------------------------------------

    pub fn save_history(&self) {
        let entries: Vec<HistoryEntry> = {
            let mut s = self.lock();
            s.dirty = false;
            s.history.iter().cloned().collect()
        };
        let path = crate::config::history_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&entries) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn load_history(&self) {
        let path = crate::config::history_path();
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(entries) = serde_json::from_str::<Vec<HistoryEntry>>(&text) else {
            return;
        };
        let mut s = self.lock();
        s.history = VecDeque::from(entries);
        let limit = s.config.ui.history_limit.max(1);
        while s.history.len() > limit {
            s.history.pop_back();
        }
    }
}
