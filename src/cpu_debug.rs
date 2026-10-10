//! Lightweight CPU usage diagnostics, enabled with `--cpu-debug`.
//!
//! Every interval it logs the process CPU usage (100% = one full core), the
//! GUI repaint rate, the rate of daemon state-change wakes, and (if the GUI is
//! repainting) why egui wants to repaint. That is usually enough to tell apart
//! "the GUI is repainting in a loop" from "a background thread is spinning".

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static WAKES: AtomicU64 = AtomicU64::new(0);
static CAUSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether CPU diagnostics are enabled.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record that the GUI produced a frame.
pub fn note_frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// Record that the daemon woke the GUI.
pub fn note_wake() {
    WAKES.fetch_add(1, Ordering::Relaxed);
}

/// Record why egui wants to repaint this frame.
pub fn note_causes(causes: &[String]) {
    if causes.is_empty() {
        return;
    }
    if let Ok(mut buffer) = CAUSES.lock() {
        if buffer.len() < 200_000 {
            buffer.extend(causes.iter().cloned());
        }
    }
}

/// Record the input events egui received this frame.
pub fn note_events(events: &[String]) {
    if events.is_empty() {
        return;
    }
    if let Ok(mut buffer) = EVENTS.lock() {
        if buffer.len() < 200_000 {
            buffer.extend(events.iter().cloned());
        }
    }
}

fn take_counts(buffer: &Mutex<Vec<String>>) -> String {
    let mut items: Vec<String> = {
        let mut guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    if items.is_empty() {
        return String::new();
    }
    items.sort();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for item in items {
        if let Some(last) = counts.last_mut() {
            if last.0 == item {
                last.1 += 1;
                continue;
            }
        }
        counts.push((item, 1));
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts
        .iter()
        .take(6)
        .map(|(item, n)| format!("{item} x{n}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn start(interval_secs: u64) {
    ENABLED.store(true, Ordering::Relaxed);
    let interval = Duration::from_secs(interval_secs.max(1));
    std::thread::Builder::new()
        .name("cpu-debug".to_string())
        .spawn(move || monitor_loop(interval))
        .ok();
}

#[cfg(windows)]
fn process_cpu_time() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn to_u64(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    unsafe {
        let _ = GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        );
    }
    to_u64(kernel) + to_u64(user)
}

#[cfg(not(windows))]
fn process_cpu_time() -> u64 {
    0
}

fn monitor_loop(interval: Duration) {
    let mut prev_proc = process_cpu_time();
    let mut prev_frames = FRAMES.load(Ordering::Relaxed);
    let mut prev_wakes = WAKES.load(Ordering::Relaxed);
    let mut prev = Instant::now();

    loop {
        std::thread::sleep(interval);
        let now = Instant::now();
        let elapsed = now.duration_since(prev).as_secs_f64().max(0.001);

        let proc = process_cpu_time();
        let cpu = (proc.saturating_sub(prev_proc)) as f64 / 10_000_000.0 / elapsed * 100.0;
        let frames = FRAMES.load(Ordering::Relaxed);
        let wakes = WAKES.load(Ordering::Relaxed);
        let fps = (frames.saturating_sub(prev_frames)) as f64 / elapsed;
        let wps = (wakes.saturating_sub(prev_wakes)) as f64 / elapsed;

        let cause_summary = {
            let summary = take_counts(&CAUSES);
            if summary.is_empty() {
                String::new()
            } else {
                format!(
                    "；{}",
                    crate::i18n::tr_args("重绘原因: {list}", &[("list", summary.as_str())])
                )
            }
        };
        let event_summary = {
            let summary = take_counts(&EVENTS);
            if summary.is_empty() {
                String::new()
            } else {
                format!(
                    "；{}",
                    crate::i18n::tr_args("输入事件: {list}", &[("list", summary.as_str())])
                )
            }
        };

        let cpu = format!("{cpu:.1}");
        let fps = format!("{fps:.1}");
        let wps = format!("{wps:.1}");
        let line = crate::i18n::tr_args(
            "CPU 监控: 进程占用 {cpu}%（单核=100%），GUI 重绘 {fps}/s，状态唤醒 {wps}/s",
            &[("cpu", cpu.as_str()), ("fps", fps.as_str()), ("wps", wps.as_str())],
        );
        tracing::info!("{}{}", line, format!("{cause_summary}{event_summary}"));

        prev_proc = proc;
        prev_frames = frames;
        prev_wakes = wakes;
        prev = now;
    }
}
