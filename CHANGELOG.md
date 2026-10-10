# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/YOUR_USER/ntfy-windows/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/YOUR_USER/ntfy-windows/releases/tag/v0.1.0
