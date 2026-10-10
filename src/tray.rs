//! System tray: shared icon generator + a headless tray-only mode.
//!
//! The headless mode runs the daemon with a tray icon but **no eframe/egui
//! window**, so there is no GUI rendering at all (CPU stays near zero).

use crate::state::{Command, SharedState};

/// Generate the 32x32 tray icon (a green notification dot).
pub fn make_icon() -> anyhow::Result<tray_icon::Icon> {
    const S: u32 = 32;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    let center = (S as f32 - 1.0) / 2.0;
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= 14.5 {
                let idx = ((y * S + x) * 4) as usize;
                let ring = (9.5..11.5).contains(&dist);
                let (r, g, b) = if ring {
                    (0xff, 0xff, 0xff)
                } else {
                    (0x2e, 0xc4, 0x6b)
                };
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 0xff;
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, S, S).map_err(|e| anyhow::anyhow!("生成托盘图标失败: {e}"))
}

/// Run the daemon headless with a system tray icon and a win32 message loop.
///
/// This blocks until the user chooses "退出" (which exits the process).
pub fn run_headless(
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    state: SharedState,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};

        let reconnect = MenuItem::new(crate::i18n::tr("重新连接"), true, None);
        let test = MenuItem::new(crate::i18n::tr("测试通知"), true, None);
        let open_config = MenuItem::new(crate::i18n::tr("打开配置文件"), true, None);
        let open_data = MenuItem::new(crate::i18n::tr("打开数据目录"), true, None);
        let quit = MenuItem::new(crate::i18n::tr("退出"), true, None);
        let separator = PredefinedMenuItem::separator();

        let menu = Menu::new();
        let items: [&dyn tray_icon::menu::IsMenuItem; 6] = [
            &reconnect,
            &test,
            &open_config,
            &open_data,
            &separator,
            &quit,
        ];
        for item in items {
            menu.append(item)
                .map_err(|e| anyhow::anyhow!("菜单项添加失败: {e}"))?;
        }

        let reconnect_id = reconnect.id().clone();
        let test_id = test.id().clone();
        let config_id = open_config.id().clone();
        let data_id = open_data.id().clone();
        let quit_id = quit.id().clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let id = format!("{:?}", event.id);
            tracing::info!(
                "{}",
                crate::i18n::tr_args("托盘菜单: {id}", &[("id", id.as_str())])
            );
            if event.id == reconnect_id {
                let _ = cmd_tx.send(Command::Reconnect);
            } else if event.id == test_id {
                let _ = cmd_tx.send(Command::TestNotification);
            } else if event.id == config_id {
                let _ = open::that(crate::config::config_path());
            } else if event.id == data_id {
                let _ = open::that(crate::config::localappdata_dir());
            } else if event.id == quit_id {
                state.save_history();
                std::process::exit(0);
            }
        }));

        let _tray = tray_icon::TrayIconBuilder::new()
            .with_tooltip("ntfy-windows")
            .with_icon(make_icon()?)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| anyhow::anyhow!("创建托盘图标失败: {e}"))?;

        tracing::info!(
            "{}",
            crate::i18n::tr("已进入托盘模式（无界面），右键托盘图标操作")
        );
        run_message_loop();
    }

    #[cfg(not(windows))]
    {
        let _ = (cmd_tx, state);
        anyhow::bail!("托盘模式仅支持 Windows");
    }

    Ok(())
}

#[cfg(windows)]
fn run_message_loop() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
