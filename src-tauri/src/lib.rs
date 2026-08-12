mod core;
mod downloads;
mod git;
mod importer;
mod models;
mod storage;
mod usage;

use crate::{core::AppCore, models::*};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
use tauri_plugin_autostart::MacosLauncher;

type CommandResult<T> = Result<T, String>;

#[tauri::command]
fn get_snapshot(core: State<'_, AppCore>) -> CommandResult<AppSnapshot> {
    core.snapshot()
}

#[tauri::command]
fn import_nodes(core: State<'_, AppCore>, text: String) -> CommandResult<ImportResult> {
    core.import_nodes(&text)
}

#[tauri::command]
fn import_node_file(core: State<'_, AppCore>, path: String) -> CommandResult<ImportResult> {
    core.import_node_file(Path::new(&path))
}

#[tauri::command]
fn export_nodes(core: State<'_, AppCore>, path: String) -> CommandResult<String> {
    core.export_nodes(Path::new(&path))
}

#[tauri::command]
fn test_node(core: State<'_, AppCore>, node_id: String) -> CommandResult<NodeEntry> {
    core.test_node(&node_id)
}

#[tauri::command]
fn test_all_nodes(core: State<'_, AppCore>) -> CommandResult<Vec<NodeEntry>> {
    core.test_all_nodes()
}

#[tauri::command]
fn rename_node(
    core: State<'_, AppCore>,
    node_id: String,
    name: String,
) -> CommandResult<AppSnapshot> {
    core.rename_node(&node_id, &name)
}

#[tauri::command]
fn set_node_enabled(
    core: State<'_, AppCore>,
    node_id: String,
    enabled: bool,
) -> CommandResult<AppSnapshot> {
    core.set_node_enabled(&node_id, enabled)
}

#[tauri::command]
fn delete_node(core: State<'_, AppCore>, node_id: String) -> CommandResult<AppSnapshot> {
    core.delete_node(&node_id)
}

#[tauri::command]
fn set_acceleration(core: State<'_, AppCore>, enabled: bool) -> CommandResult<AppSnapshot> {
    core.set_acceleration(enabled)
}

#[tauri::command]
fn set_line_mode(
    core: State<'_, AppCore>,
    mode: LineMode,
    node_id: Option<String>,
) -> CommandResult<AppSnapshot> {
    core.set_line_mode(mode, node_id.as_deref())
}

#[tauri::command]
fn set_route_scope(core: State<'_, AppCore>, scope: RouteScope) -> CommandResult<AppSnapshot> {
    core.set_route_scope(scope)
}

#[tauri::command]
fn add_route(core: State<'_, AppCore>, repository_url: String) -> CommandResult<AppSnapshot> {
    core.add_route(&repository_url)
}

#[tauri::command]
fn delete_route(core: State<'_, AppCore>, route_id: String) -> CommandResult<AppSnapshot> {
    core.delete_route(&route_id)
}

#[tauri::command]
fn run_diagnostics(
    core: State<'_, AppCore>,
    repository_path: Option<String>,
) -> CommandResult<DiagnosticReport> {
    core.diagnostics(repository_path.as_deref().map(Path::new))
}

#[tauri::command]
fn update_settings(
    core: State<'_, AppCore>,
    health_check_minutes: u32,
    log_level: String,
) -> CommandResult<AppSnapshot> {
    core.update_settings(health_check_minutes, &log_level)
}

#[tauri::command]
fn update_launch_at_login(core: State<'_, AppCore>, enabled: bool) -> CommandResult<AppSnapshot> {
    core.update_launch_at_login(enabled)
}

#[tauri::command]
fn restore_git_config(core: State<'_, AppCore>) -> CommandResult<AppSnapshot> {
    core.restore_git_config()
}

#[tauri::command]
fn clear_logs(core: State<'_, AppCore>) -> CommandResult<AppSnapshot> {
    core.clear_logs()
}

#[tauri::command]
fn get_usage_log(core: State<'_, AppCore>) -> CommandResult<UsageLogSnapshot> {
    core.usage_log()
}

#[tauri::command]
fn set_usage_logging(core: State<'_, AppCore>, enabled: bool) -> CommandResult<AppSnapshot> {
    core.set_usage_logging(enabled)
}

#[tauri::command]
fn prepare_download(
    core: State<'_, AppCore>,
    original_url: String,
) -> CommandResult<DownloadTarget> {
    core.prepare_download(&original_url)
}

#[tauri::command]
async fn open_download(
    core: State<'_, AppCore>,
    original_url: String,
) -> CommandResult<DownloadTarget> {
    let target = core.prepare_download(&original_url)?;
    tauri::async_runtime::spawn_blocking(move || downloads::probe_and_open(target))
        .await
        .map_err(|error| format!("下载探测任务异常结束：{error}"))?
}

fn reveal_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let snapshot = app.state::<AppCore>().snapshot().ok();
    let enabled = snapshot
        .as_ref()
        .is_some_and(|state| state.settings.acceleration_enabled);
    let current = snapshot
        .as_ref()
        .and_then(|state| {
            state
                .settings
                .current_node_id
                .as_ref()
                .and_then(|id| state.nodes.iter().find(|node| &node.node.id == id))
        })
        .map(|node| node.node.name.as_str())
        .unwrap_or("GitHub 直连");
    let status = MenuItem::with_id(
        app,
        "status",
        format!("当前线路：{current}"),
        false,
        None::<&str>,
    )?;
    let toggle = MenuItem::with_id(
        app,
        "toggle",
        if enabled {
            "关闭加速"
        } else {
            "开启加速"
        },
        true,
        None::<&str>,
    )?;
    let retest = MenuItem::with_id(app, "retest", "重新测速并切换", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开 GitBoost", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &toggle, &retest, &open, &separator, &quit])?;
    let toggle_item = toggle.clone();
    let status_item = status.clone();
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => reveal_main(app),
            "quit" => app.exit(0),
            "toggle" => {
                let core = app.state::<AppCore>();
                let current_enabled = core
                    .snapshot()
                    .map(|state| state.settings.acceleration_enabled)
                    .unwrap_or(false);
                match core.set_acceleration(!current_enabled) {
                    Ok(state) => {
                        let _ = toggle_item.set_text(if state.settings.acceleration_enabled {
                            "关闭加速"
                        } else {
                            "开启加速"
                        });
                        let node = state
                            .settings
                            .current_node_id
                            .as_ref()
                            .and_then(|id| state.nodes.iter().find(|node| &node.node.id == id))
                            .map(|node| node.node.name.as_str())
                            .unwrap_or("GitHub 直连");
                        let _ = status_item.set_text(format!("当前线路：{node}"));
                        let _ = app.emit("snapshot-updated", state);
                    }
                    Err(error) => {
                        reveal_main(app);
                        let _ = app.emit("operation-error", error);
                    }
                }
            }
            "retest" => {
                let handle = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let result = handle.state::<AppCore>().test_all_nodes();
                    match result {
                        Ok(_) => {
                            if let Ok(state) = handle.state::<AppCore>().snapshot() {
                                let _ = handle.emit("snapshot-updated", state);
                            }
                        }
                        Err(error) => {
                            let _ = handle.emit("operation-error", error);
                        }
                    }
                });
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn start_health_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_check = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let minutes = app
                .state::<AppCore>()
                .snapshot()
                .map(|state| state.settings.health_check_minutes)
                .unwrap_or(0);
            if minutes == 0 || last_check.elapsed() < Duration::from_secs(u64::from(minutes) * 60) {
                continue;
            }
            last_check = Instant::now();
            let handle = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                if handle.state::<AppCore>().test_all_nodes().is_ok() {
                    if let Ok(state) = handle.state::<AppCore>().snapshot() {
                        let _ = handle.emit("snapshot-updated", state);
                    }
                }
            })
            .await;
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .macos_launcher(MacosLauncher::LaunchAgent)
                .build(),
        )
        .setup(|app| {
            let data_dir: PathBuf = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法定位应用数据目录：{error}"))?;
            let core =
                AppCore::new(data_dir).map_err(|error| format!("GitBoost 初始化失败：{error}"))?;
            app.manage(core);
            usage::start_listener(app.handle().clone());
            app.state::<AppCore>()
                .refresh_registered_configuration()
                .map_err(|error| format!("无法升级 GitBoost 配置：{error}"))?;
            install_tray(app)?;
            start_health_monitor(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            import_nodes,
            import_node_file,
            export_nodes,
            test_node,
            test_all_nodes,
            rename_node,
            set_node_enabled,
            delete_node,
            set_acceleration,
            set_line_mode,
            set_route_scope,
            add_route,
            delete_route,
            run_diagnostics,
            update_settings,
            update_launch_at_login,
            restore_git_config,
            clear_logs,
            get_usage_log,
            set_usage_logging,
            prepare_download,
            open_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GitBoost");
}
