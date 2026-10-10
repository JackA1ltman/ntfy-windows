//! Auto-start management through the per-user Run registry key.
//!
//! Using `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` avoids requiring
//! administrator rights. For a machine-wide service the user can instead
//! register the executable with the Task Scheduler (see README).

use std::path::Path;

use anyhow::Result;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ntfy-windows";

/// Enable or disable auto-start. When `headless` is true the GUI is skipped.
#[cfg(windows)]
pub fn set_autostart(enabled: bool, exe: &Path, headless: bool) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(RUN_KEY)?;
    if enabled {
        let args = if headless { " --no-gui" } else { " --minimized" };
        let command = format!("\"{}\"{}", exe.display(), args);
        key.set_value(VALUE_NAME, &command)?;
        tracing::info!(
            "{}",
            crate::i18n::tr_args("已写入开机自启注册表项: {command}", &[("command", command.as_str())])
        );
    } else {
        match key.delete_value(VALUE_NAME) {
            Ok(()) => tracing::info!("{}", crate::i18n::tr("已移除开机自启注册表项")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn is_autostart_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.open_subkey(RUN_KEY) {
        Ok(key) => key.get_value::<String, _>(VALUE_NAME).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_enabled: bool, _exe: &Path, _headless: bool) -> Result<()> {
    anyhow::bail!("开机自启仅支持 Windows")
}

#[cfg(not(windows))]
pub fn is_autostart_enabled() -> bool {
    false
}
