# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--mem-debug`: log process working set / commit size and the in-memory history / log
  buffer sizes every 10 s (useful to tell GUI memory apart from buffer growth).
- Sticky notifications: events at or above `notification.sticky_min_priority`
  (default `5`, `0` = off) now use a toast `scenario` so they stay on screen until handled —
  `"reminder"` (stays) or `"alarm"` (stays + loops the alarm sound), selectable via
  `notification.sticky_scenario`. Both options are exposed in the GUI.

## [0.1.1] - 2026-10-11

### Added

- Non-image attachments (ntfy can send arbitrary files) are now stored verbatim in
  `%LOCALAPPDATA%\ntfy-windows\files` (`time_topic_name.ext`) and can be opened from the history.
- Data folders (logs / images / files) are created automatically at startup and whenever the
  configuration is saved; the GUI also creates them right before opening, so the "open folder"
  buttons always work.
- "Open files folder" button on the dashboard, plus file / folder buttons on history entries.
- New `notification` options: `save_files`, `files_dir` and `max_file_mb`.
- About page: link to the project repository.

### Changed

- Images are now written **directly** into the images folder; the separate `cache` folder was
  removed. Toast icons are cached under `images\icons\` and reused by URL hash.
- "Export history" / "Export logs" now open a **native save dialog** so you choose the destination;
  the fixed `exports` folder was removed. The "open log folder" button is kept.
- Log files now use a `.log` suffix: `ntfy-windows.YYYY-MM-DD.log`
  (previously `ntfy-windows.log.YYYY-MM-DD`).
- GUI log view **and** the log file now follow the selected UI language.

### Fixed

- Clicking "Open files folder" (or any data folder) did nothing when the folder did not exist yet.

## [0.1.0] - 2026-10-10

### Added

- ntfy subscription daemon (tokio) with automatic reconnect and exponential backoff.
- Transport fallback: JSON stream (`/{topic}/json`) → SSE (`/{topic}/sse`) →
  WebSocket (`/{topic}/ws`) → polling (`/{topic}/json?poll=1`).
- Multiple topics per subscription (`/{topic1,topic2}/json`).
- Basic Auth and Bearer token authentication.
- Native Windows toast notifications (WinRT) with a registered `AppUserModelID`.
- Priority mapping (silent / default / IM / looping alarm, long duration).
- Tags/emoji, click URL and `view` action buttons forwarded to the toast.
- Attachment image preview: downloaded, re-encoded (PNG/JPEG) and shown in the toast.
- Persistent image archive with `time_topic_name` file names.
- egui GUI: status, stats, topics, settings, filterable history, live logs, export.
- System tray: GUI close-to-tray, and a headless tray mode (`--no-gui`).
- Portable `config.json` next to the executable, with legacy `config.toml` migration.
- Automatic CJK font detection, plus Segoe UI Symbol fallback.
- Daily rolling file logging mirrored into the GUI.
- Auto-start via the `HKCU\...\Run` registry key.
- Single-instance guard (loopback port).
- CPU / repaint diagnostics (`--cpu-debug`).
- i18n with independent language files; Chinese and English built in.

[Unreleased]: https://github.com/JackA1ltman/ntfy-windows/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/JackA1ltman/ntfy-windows/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/JackA1ltman/ntfy-windows/releases/tag/v0.1.0
