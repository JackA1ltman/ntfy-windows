//! Lightweight process memory diagnostics, enabled with `--mem-debug`.
//!
//! Every interval it logs the process working set / commit size plus the size
//! of the in-memory history and log buffers and the egui font atlas. That is
//! enough to tell apart "egui / the font atlas is holding memory" from
//! "the log or history buffer grew".

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::state::SharedState;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FONT_ATLAS_W: AtomicUsize = AtomicUsize::new(0);
static FONT_ATLAS_H: AtomicUsize = AtomicUsize::new(0);
static GALLEYS: AtomicUsize = AtomicUsize::new(0);

/// Whether memory diagnostics are enabled.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record the current egui font atlas size and galley cache size.
pub fn note_fonts(width: usize, height: usize, galleys: usize) {
    FONT_ATLAS_W.store(width, Ordering::Relaxed);
    FONT_ATLAS_H.store(height, Ordering::Relaxed);
    GALLEYS.store(galleys, Ordering::Relaxed);
}

/// Start the memory monitor thread.
pub fn start(state: SharedState, interval_secs: u64) {
    ENABLED.store(true, Ordering::Relaxed);
    let interval = Duration::from_secs(interval_secs.max(1));
    if let Err(e) = std::thread::Builder::new()
        .name("mem-debug".to_string())
        .spawn(move || loop {
            std::thread::sleep(interval);
            log_once(&state);
        })
    {
        let err = e.to_string();
        tracing::warn!(
            "{}",
            crate::i18n::tr_args("启动内存监控线程失败: {err}", &[("err", err.as_str())])
        );
    }
}

fn log_once(state: &SharedState) {
    let (working_set, peak, commit) = sample();
    let history = state.lock().history.len();
    let (log_count, log_bytes) = state.log_stats();

    let ws = mb(working_set);
    let peak = mb(peak);
    let commit = mb(commit);
    let history = history.to_string();
    let log_count = log_count.to_string();
    let log_bytes = log_bytes.to_string();

    let atlas_w = FONT_ATLAS_W.load(Ordering::Relaxed);
    let atlas_h = FONT_ATLAS_H.load(Ordering::Relaxed);
    // `epaint::FontImage` stores one `f32` of coverage per pixel.
    let atlas_bytes = (atlas_w * atlas_h * std::mem::size_of::<f32>()) as u64;
    let atlas_size = format!("{atlas_w}x{atlas_h}");
    let atlas_mb = mb(atlas_bytes);
    let galleys = GALLEYS.load(Ordering::Relaxed).to_string();

    tracing::info!(
        "{}",
        crate::i18n::tr_args(
            "内存监控: 工作集 {ws} MB（峰值 {peak} MB），提交 {commit} MB，历史 {history} 条，日志 {log_count} 条/{log_bytes} 字节，字体图集 {atlas_size}（约 {atlas_mb} MB），galley {galleys} 个",
            &[
                ("ws", ws.as_str()),
                ("peak", peak.as_str()),
                ("commit", commit.as_str()),
                ("history", history.as_str()),
                ("log_count", log_count.as_str()),
                ("log_bytes", log_bytes.as_str()),
                ("atlas_size", atlas_size.as_str()),
                ("atlas_mb", atlas_mb.as_str()),
                ("galleys", galleys.as_str()),
            ]
        )
    );
}

/// `(working_set, peak_working_set, commit)` in bytes.
#[cfg(windows)]
fn sample() -> (u64, u64, u64) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe {
        // The result is intentionally ignored: if the call fails the counters
        // stay zeroed, which shows up as `0.0 MB` in the log.
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size);
    }
    (
        counters.WorkingSetSize as u64,
        counters.PeakWorkingSetSize as u64,
        counters.PagefileUsage as u64,
    )
}

#[cfg(not(windows))]
fn sample() -> (u64, u64, u64) {
    (0, 0, 0)
}

fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}
