//! Minimal i18n with independent language files.
//!
//! The **source strings (Chinese) are used as keys**, so `zh-CN` works even
//! with no file at all: `tr()` falls back to the key itself. Other languages
//! are simple TOML files mapping the source string to the translation, e.g.
//!
//! ```toml
//! "设置" = "Settings"
//! "已导出到 {path}" = "Exported to {path}"
//! ```
//!
//! Built-in languages (`lang/en-US.toml`, `lang/zh-CN.toml`) are embedded in
//! the binary. A file under `<exe dir>/lang/<code>.toml` overrides/extends it,
//! so new languages can be added **without recompiling**.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

const EN_US: &str = include_str!("../lang/en-US.toml");
const ZH_CN: &str = include_str!("../lang/zh-CN.toml");

fn current() -> &'static RwLock<HashMap<String, String>> {
    static CURRENT: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    CURRENT.get_or_init(|| RwLock::new(HashMap::new()))
}

fn parse(text: &str) -> HashMap<String, String> {
    match toml::from_str::<HashMap<String, String>>(text) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!("语言文件解析失败（将回退到中文/英文）: {e}");
            HashMap::new()
        }
    }
}

/// Resolve `"system"` (or empty) to a concrete language code.
pub fn resolve(language: &str) -> String {
    let language = language.trim();
    if !language.is_empty() && !language.eq_ignore_ascii_case("system") {
        return language.to_string();
    }
    let locale = sys_locale::get_locale().unwrap_or_default().to_ascii_lowercase();
    if locale.starts_with("zh") {
        "zh-CN".to_string()
    } else {
        "en-US".to_string()
    }
}

/// Load the embedded + external language files for `language`.
pub fn set_language(language: &str) {
    let resolved = resolve(language);
    let mut map = HashMap::new();
    match resolved.as_str() {
        "zh-CN" | "zh" => map.extend(parse(ZH_CN)),
        _ => map.extend(parse(EN_US)),
    }

    // Optional external override: <exe dir>/lang/<code>.toml
    let path = crate::config::exe_dir()
        .join("lang")
        .join(format!("{resolved}.toml"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        map.extend(parse(&text));
    }

    *current().write().unwrap_or_else(|e| e.into_inner()) = map;
}

/// Translate a source string.
///
/// The Chinese source string is the fallback, so `zh-CN` works without a file
/// and any key missing from another language still shows something reasonable.
pub fn tr(key: &str) -> String {
    if let Ok(map) = current().read() {
        if let Some(value) = map.get(key) {
            return value.clone();
        }
    }
    key.to_string()
}

/// Translate with `{name}` placeholders replaced by `args`.
pub fn tr_args(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = tr(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}
