//! Logging setup.
//!
//! Logs are written to a daily rolling file under `%LOCALAPPDATA%\ntfy-windows\
//! logs` and mirrored into the shared state so the GUI can display them.

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

use crate::state::SharedState;

struct GuiLayer {
    state: SharedState,
}

impl GuiLayer {
    fn new(state: SharedState) -> Self {
        Self { state }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

impl<S> Layer<S> for GuiLayer
where
    S: Subscriber + 'static,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let text = match (visitor.message.is_empty(), visitor.fields.is_empty()) {
            (true, _) => visitor.fields,
            (false, true) => visitor.message,
            (false, false) => format!("{} {}", visitor.message, visitor.fields),
        };
        self.state.push_log(*metadata.level(), metadata.target(), &text);
    }
}

/// Initialise the global tracing subscriber.
///
/// The returned guard must be kept alive for the duration of the program so
/// that buffered log lines are flushed.
pub fn init(state: SharedState, level: &str) -> Option<WorkerGuard> {
    let dir = crate::config::log_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("创建日志目录失败: {e}");
        return None;
    }

    let file_appender = tracing_appender::rolling::daily(&dir, "ntfy-windows.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("ntfy_windows={level},warn")));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_level(true);

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(GuiLayer::new(state));

    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("初始化日志失败: {e}");
    }

    Some(guard)
}
