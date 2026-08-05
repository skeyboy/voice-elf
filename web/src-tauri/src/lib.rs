mod static_server;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(desktop)]
use tauri::{
    WebviewWindow,
    menu::{MenuBuilder, MenuId, MenuItemBuilder},
    tray::TrayIconBuilder,
};

#[cfg(desktop)]
const QUIT_REQUEST_EVENT: &str = "voice-elf:native-quit-requested";

#[derive(Default)]
struct AppLifecycle {
    allow_exit: AtomicBool,
    quit_prompt_visible: AtomicBool,
}

struct LocalServer {
    #[cfg(desktop)]
    origin: url::Url,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(desktop)]
pub(crate) fn show_and_focus(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    show_and_focus(&window)
}

#[cfg(desktop)]
fn dispatch_native_toast(app: &AppHandle, message: &str, kind: &str) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let detail = serde_json::json!({ "message": message, "kind": kind });
    let script = format!(
        "window.dispatchEvent(new CustomEvent('voice-elf:native-toast', {{ detail: {detail} }}));"
    );
    if let Err(error) = window.eval(script) {
        eprintln!("failed to send native toast: {error}");
    }
}

#[cfg(desktop)]
fn request_exit_confirmation(app: &AppHandle) {
    let lifecycle = app.state::<AppLifecycle>();
    if lifecycle.quit_prompt_visible.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = show_main_window(app) {
        lifecycle
            .quit_prompt_visible
            .store(false, Ordering::Release);
        eprintln!("failed to show the main window before quitting: {error}");
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        lifecycle
            .quit_prompt_visible
            .store(false, Ordering::Release);
        return;
    };
    let script = format!(
        "window.dispatchEvent(new CustomEvent({}));",
        serde_json::to_string(QUIT_REQUEST_EVENT).expect("event name is serializable")
    );
    if let Err(error) = window.eval(script) {
        lifecycle
            .quit_prompt_visible
            .store(false, Ordering::Release);
        eprintln!("failed to request quit confirmation: {error}");
    }
}

pub(crate) fn confirm_app_exit(app: &AppHandle) {
    let lifecycle = app.state::<AppLifecycle>();
    lifecycle.allow_exit.store(true, Ordering::Release);
    lifecycle
        .quit_prompt_visible
        .store(false, Ordering::Release);
    app.exit(0);
}

pub(crate) fn cancel_app_exit(app: &AppHandle) {
    app.state::<AppLifecycle>()
        .quit_prompt_visible
        .store(false, Ordering::Release);
}

#[cfg(desktop)]
fn show_subtitle_window(app: &AppHandle) {
    let mut windows = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with("subtitles-"))
        .collect::<Vec<_>>();
    windows.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some((_, window)) = windows.into_iter().next() {
        if let Err(error) = show_and_focus(&window) {
            eprintln!("failed to show subtitle window: {error}");
        }
        return;
    }
    if let Err(error) = show_main_window(app) {
        eprintln!("failed to show the main window: {error}");
        return;
    }
    dispatch_native_toast(app, "请先进入会议房间并打开字幕大屏", "warning");
}

#[cfg(desktop)]
fn show_settings_window(app: &AppHandle) {
    let origin = app.state::<LocalServer>().origin.clone();
    if let Err(error) = static_server::show_settings_window(app, &origin) {
        eprintln!("failed to show settings window: {error}");
        if show_main_window(app).is_ok() {
            dispatch_native_toast(app, "无法打开设置窗口", "error");
        }
    }
}

#[cfg(desktop)]
fn handle_status_menu(app: &AppHandle, item_id: &MenuId) {
    match item_id.as_ref() {
        "status-show-main" => {
            if let Err(error) = show_main_window(app) {
                eprintln!("failed to show the main window: {error}");
            }
        }
        "status-show-subtitles" => show_subtitle_window(app),
        "status-show-settings" => show_settings_window(app),
        "status-quit" => request_exit_confirmation(app),
        _ => {}
    }
}

#[cfg(desktop)]
fn setup_status_bar(app: &tauri::App) -> tauri::Result<()> {
    let show_main = MenuItemBuilder::with_id("status-show-main", "显示 Voice Elf").build(app)?;
    let show_subtitles =
        MenuItemBuilder::with_id("status-show-subtitles", "显示字幕大屏").build(app)?;
    let show_settings =
        MenuItemBuilder::with_id("status-show-settings", "字幕大屏设置…").build(app)?;
    let background = MenuItemBuilder::with_id("status-background", "关闭窗口后继续后台运行")
        .enabled(false)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("status-quit", "退出 Voice Elf…").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_main)
        .item(&show_subtitles)
        .item(&show_settings)
        .separator()
        .item(&background)
        .separator()
        .item(&quit)
        .build()?;
    let mut tray = TrayIconBuilder::with_id("voice-elf-status")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Voice Elf");
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.on_menu_event(|app, event| handle_status_menu(app, event.id()))
        .build(app)?;
    Ok(())
}

#[cfg(desktop)]
fn hides_in_background(label: &str) -> bool {
    label == "main" || label == "subtitle-settings" || label.starts_with("subtitles-")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event
            && hides_in_background(window.label())
        {
            api.prevent_close();
            if window.label().starts_with("subtitles-") && window.is_fullscreen().unwrap_or(false) {
                let _ = window.set_fullscreen(false);
                let _ = window.set_always_on_top(true);
            }
            if let Err(error) = window.hide() {
                eprintln!("failed to hide window {}: {error}", window.label());
            }
        }
    });
    let app = builder
        .setup(|app| {
            app.manage(AppLifecycle::default());
            let server = static_server::start(app.handle().clone(), app.path().app_config_dir()?)?;
            let origin = server.origin.clone();
            let local_origin: url::Url =
                origin.parse().expect("local server origin is a valid URL");
            app.manage(LocalServer {
                #[cfg(desktop)]
                origin: local_origin.clone(),
                task: server.task,
            });

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(local_origin))
                .title("Voice Elf")
                .inner_size(1180.0, 820.0)
                .min_inner_size(360.0, 640.0)
                .resizable(true);
            #[cfg(desktop)]
            let window = window.maximizable(true).minimizable(true).closable(true);
            window.build()?;
            #[cfg(desktop)]
            setup_status_bar(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Voice Elf");

    #[cfg(desktop)]
    app.run(|app, event| match event {
        tauri::RunEvent::ExitRequested { api, .. }
            if !app
                .state::<AppLifecycle>()
                .allow_exit
                .load(Ordering::Acquire) =>
        {
            api.prevent_exit();
            request_exit_confirmation(app);
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } if !has_visible_windows => {
            if let Err(error) = show_main_window(app) {
                eprintln!("failed to reopen the main window: {error}");
            }
        }
        _ => {}
    });
    #[cfg(mobile)]
    app.run(|_, _| {});
}
