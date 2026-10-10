//! Windows toast notification backend.
//!
//! The notification is rendered through the WinRT toast APIs. Because WinRT
//! requires COM to be initialized and `ToastNotification` objects are not
//! guaranteed to be agile, all WinRT work happens on a dedicated thread that is
//! initialized as an MTA. The async part of the application only builds an XML
//! payload and pushes it into a channel.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

/// A button shown on the toast.
#[derive(Debug, Clone, Default)]
pub struct ToastAction {
    pub label: String,
    pub url: String,
}

/// Everything required to render one toast notification.
#[derive(Debug, Clone, Default)]
pub struct ToastContent {
    pub title: Option<String>,
    pub body: String,
    pub priority: i32,
    pub silent: bool,
    pub long_duration: bool,
    pub app_logo: Option<PathBuf>,
    pub hero_image: Option<PathBuf>,
    pub inline_images: Vec<PathBuf>,
    pub click_url: Option<String>,
    pub actions: Vec<ToastAction>,
}

/// Handle used to submit notifications. Cheap to clone and `Send + Sync`.
pub struct WindowsNotifier {
    tx: mpsc::UnboundedSender<String>,
}

impl WindowsNotifier {
    pub fn new(app_id: &str) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let app_id = app_id.to_string();
        std::thread::Builder::new()
            .name("ntfy-toast".to_string())
            .spawn(move || {
                #[cfg(windows)]
                {
                    // SAFETY: we are on a dedicated thread; CoInitializeEx is safe
                    // to call here and the result is intentionally ignored.
                    unsafe {
                        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                }
                while let Some(xml) = rx.blocking_recv() {
                    if let Err(e) = show_toast(&app_id, &xml) {
                        let err = format!("{e:#}");
                        tracing::warn!(
                            "{}",
                            crate::i18n::tr_args(
                                "显示 Windows 通知失败: {err}",
                                &[("err", err.as_str())]
                            )
                        );
                    }
                }
            })
            .expect("无法启动通知线程");
        Self { tx }
    }

    /// Build the toast XML and enqueue it for the notification thread.
    pub fn show(&self, content: ToastContent) -> Result<()> {
        let xml = build_xml(&content);
        self.tx
            .send(xml)
            .map_err(|e| anyhow::anyhow!("通知线程不可用: {e}"))?;
        Ok(())
    }
}

/// Register the AppUserModelID and set the process AUMID so that toasts are
/// attributed to this application even though it is not an MSIX package.
#[cfg(windows)]
pub fn prepare(app_id: &str, app_name: &str, icon: Option<&str>) -> Result<()> {
    set_process_aumid(app_id)?;
    register_aumid(app_id, app_name, icon)?;
    Ok(())
}

#[cfg(not(windows))]
pub fn prepare(_app_id: &str, _app_name: &str, _icon: Option<&str>) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn set_process_aumid(app_id: &str) -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id = HSTRING::from(app_id);
    // SAFETY: `id` outlives the call and contains a valid wide string.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn register_aumid(app_id: &str, app_name: &str, icon: Option<&str>) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = format!("Software\\Classes\\AppUserModelId\\{app_id}");
    let (key, _) = hkcu.create_subkey(&path)?;
    key.set_value("DisplayName", &app_name)?;
    if let Some(icon) = icon {
        if !icon.is_empty() {
            key.set_value("IconUri", &icon)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn show_toast(app_id: &str, xml: &str) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?;
    notifier.Show(&toast)?;
    Ok(())
}

#[cfg(not(windows))]
fn show_toast(_app_id: &str, _xml: &str) -> Result<()> {
    anyhow::bail!("Windows 通知仅可在 Windows 平台显示")
}

// ---------------------------------------------------------------------------
// XML generation
// ---------------------------------------------------------------------------

fn build_xml(c: &ToastContent) -> String {
    let mut out = String::with_capacity(768);
    let click = c.click_url.as_deref().unwrap_or("");
    let has_launch = !click.is_empty();

    out.push_str("<toast");
    if c.long_duration {
        out.push_str(" duration=\"long\"");
    }
    if has_launch {
        out.push_str(" activationType=\"protocol\" launch=\"");
        out.push_str(&escape_attr(click));
        out.push('"');
    }
    out.push('>');

    // Visual
    out.push_str("<visual><binding template=\"ToastGeneric\">");
    if let Some(title) = &c.title {
        if !title.is_empty() {
            out.push_str("<text>");
            out.push_str(&escape_xml(title));
            out.push_str("</text>");
        }
    }
    out.push_str("<text>");
    out.push_str(&escape_xml(&c.body));
    out.push_str("</text>");
    if let Some(logo) = &c.app_logo {
        out.push_str("<image placement=\"appLogoOverride\" hint-crop=\"circle\" src=\"");
        out.push_str(&escape_attr(&file_uri(logo)));
        out.push_str("\"/>");
    }
    if let Some(hero) = &c.hero_image {
        out.push_str("<image placement=\"hero\" src=\"");
        out.push_str(&escape_attr(&file_uri(hero)));
        out.push_str("\"/>");
    }
    for img in &c.inline_images {
        out.push_str("<image src=\"");
        out.push_str(&escape_attr(&file_uri(img)));
        out.push_str("\"/>");
    }
    out.push_str("</binding></visual>");

    // Audio
    if c.silent {
        out.push_str("<audio silent=\"true\"/>");
    } else {
        let src = match c.priority {
            5 => "ms-winsoundevent:Notification.Looping.Alarm",
            4 => "ms-winsoundevent:Notification.IM",
            _ => "ms-winsoundevent:Notification.Default",
        };
        out.push_str("<audio src=\"");
        out.push_str(src);
        out.push_str("\"/>");
    }

    // Actions (at most 4 plus a dismiss button; Windows allows 5 total)
    let mut actions = String::new();
    for action in c.actions.iter().take(4) {
        if action.url.is_empty() {
            continue;
        }
        actions.push_str("<action content=\"");
        actions.push_str(&escape_attr(&action.label));
        actions.push_str("\" activationType=\"protocol\" arguments=\"");
        actions.push_str(&escape_attr(&action.url));
        actions.push_str("\"/>");
    }
    if !actions.is_empty() {
        actions.push_str(
            "<action content=\"忽略\" activationType=\"system\" arguments=\"dismiss\"/>",
        );
        out.push_str("<actions>");
        out.push_str(&actions);
        out.push_str("</actions>");
    }

    out.push_str("</toast>");
    out
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("&#10;"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Convert a local path into a `file:///` URI usable by the toast renderer.
fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.replace(' ', "%20");
    if s.starts_with("//") {
        format!("file:{s}")
    } else {
        format!("file:///{s}")
    }
}
