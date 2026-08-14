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
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, Runtime, State,
};
#[cfg(target_os = "macos")]
use tauri_plugin_autostart::MacosLauncher;

type CommandResult<T> = Result<T, String>;

struct TrayMenu<R: Runtime> {
    status: MenuItem<R>,
    toggle: MenuItem<R>,
    refresh_version: AtomicU64,
}

fn tray_labels(settings: &Settings, nodes: &[NodeEntry]) -> (String, &'static str) {
    let current = if settings.acceleration_enabled {
        settings
            .current_node_id
            .as_ref()
            .and_then(|id| nodes.iter().find(|node| &node.node.id == id))
            .map(|node| node.node.name.as_str())
            .unwrap_or("GitHub 加速")
    } else {
        "GitHub 直连"
    };
    let toggle = if settings.acceleration_enabled {
        "关闭加速"
    } else {
        "开启加速"
    };
    (format!("当前线路：{current}"), toggle)
}

fn refresh_tray<R: Runtime>(app: &tauri::AppHandle<R>, snapshot: &AppSnapshot) {
    let Some(menu) = app.try_state::<TrayMenu<R>>() else {
        return;
    };
    let version = menu.refresh_version.fetch_add(1, Ordering::SeqCst) + 1;
    if menu.refresh_version.load(Ordering::SeqCst) != version {
        return;
    }
    let (status, toggle) = tray_labels(&snapshot.settings, &snapshot.nodes);
    let _ = menu.status.set_text(status);
    if menu.refresh_version.load(Ordering::SeqCst) != version {
        return;
    }
    let _ = menu.toggle.set_text(toggle);
}

fn refresh_tray_from_core<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Ok(snapshot) = app.state::<AppCore>().snapshot() {
        refresh_tray(app, &snapshot);
    }
}

async fn run_node_test<T, F>(operation: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CommandResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| format!("节点检测任务异常结束：{error}"))?
}

fn emit_node_test_progress(app: &tauri::AppHandle, completed: usize, total: usize) {
    let _ = app.emit(
        "node-test-progress",
        NodeTestProgress {
            completed,
            total,
            finished: false,
        },
    );
}

fn emit_node_test_finished(app: &tauri::AppHandle) {
    let _ = app.emit(
        "node-test-progress",
        NodeTestProgress {
            completed: 0,
            total: 0,
            finished: true,
        },
    );
}

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
async fn test_node(app: tauri::AppHandle, node_id: String) -> CommandResult<NodeEntry> {
    let worker_app = app.clone();
    let result = run_node_test(move || worker_app.state::<AppCore>().test_node(&node_id)).await?;
    refresh_tray_from_core(&app);
    Ok(result)
}

#[tauri::command]
async fn test_all_nodes(app: tauri::AppHandle) -> CommandResult<Vec<NodeEntry>> {
    let progress_app = app.clone();
    let worker_app = app.clone();
    let result = run_node_test(move || {
        worker_app
            .state::<AppCore>()
            .test_all_nodes_or_join_with_progress(|completed, total| {
                emit_node_test_progress(&progress_app, completed, total);
            })
    })
    .await;
    emit_node_test_finished(&app);
    let result = result?;
    refresh_tray_from_core(&app);
    Ok(result)
}

#[tauri::command]
async fn refresh_system_nodes(app: tauri::AppHandle) -> CommandResult<bool> {
    let worker_app = app.clone();
    let result =
        run_node_test(move || worker_app.state::<AppCore>().refresh_system_nodes()).await?;
    refresh_tray_from_core(&app);
    Ok(result)
}

#[tauri::command]
fn rename_node(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    node_id: String,
    name: String,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.rename_node(&node_id, &name)?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_node_enabled(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    node_id: String,
    enabled: bool,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.set_node_enabled(&node_id, enabled)?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn delete_node(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    node_id: String,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.delete_node(&node_id)?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_acceleration(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    enabled: bool,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.set_acceleration(enabled)?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_line_mode(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    mode: LineMode,
    node_id: Option<String>,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.set_line_mode(mode, node_id.as_deref())?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
fn set_route_scope(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
    scope: RouteScope,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.set_route_scope(scope)?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
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
fn restore_git_config(
    app: tauri::AppHandle,
    core: State<'_, AppCore>,
) -> CommandResult<AppSnapshot> {
    let snapshot = core.restore_git_config()?;
    refresh_tray(&app, &snapshot);
    Ok(snapshot)
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
    excluded_node_ids: Vec<String>,
) -> CommandResult<DownloadTarget> {
    core.prepare_download_excluding(&original_url, &excluded_node_ids)
}

#[tauri::command]
async fn open_download(
    core: State<'_, AppCore>,
    original_url: String,
    node_id: String,
) -> CommandResult<DownloadTarget> {
    let target = core.prepare_download_with_node(&original_url, &node_id)?;
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
    let (status_text, toggle_text) = snapshot
        .as_ref()
        .map(|state| tray_labels(&state.settings, &state.nodes))
        .unwrap_or_else(|| ("当前线路：GitHub 直连".into(), "开启加速"));
    let status = MenuItem::with_id(app, "status", status_text, false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", toggle_text, true, None::<&str>)?;
    let retest = MenuItem::with_id(app, "retest", "重新测速并切换", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开 GitBoost", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &toggle, &retest, &open, &separator, &quit])?;
    app.manage(TrayMenu {
        status: status.clone(),
        toggle: toggle.clone(),
        refresh_version: AtomicU64::new(0),
    });
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
                        refresh_tray(app, &state);
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
                    let result = handle
                        .state::<AppCore>()
                        .test_all_nodes_or_join_with_progress(|completed, total| {
                            emit_node_test_progress(&handle, completed, total)
                        });
                    emit_node_test_finished(&handle);
                    match result {
                        Ok(_) => {
                            if let Ok(state) = handle.state::<AppCore>().snapshot() {
                                refresh_tray(&handle, &state);
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
        let mut initial_discovery_pending = true;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let core = app.state::<AppCore>();
            let minutes = core
                .snapshot()
                .map(|state| state.settings.health_check_minutes)
                .unwrap_or(0);
            let needs_initial_discovery = initial_discovery_pending
                && core.needs_background_node_discovery().unwrap_or(false);
            if !health_check_due(minutes, last_check.elapsed(), needs_initial_discovery) {
                continue;
            }
            let handle = app.clone();
            let succeeded = tauri::async_runtime::spawn_blocking(move || {
                let started = AtomicBool::new(false);
                let result = handle
                    .state::<AppCore>()
                    .test_background_nodes_with_progress(|completed, total| {
                        started.store(true, Ordering::Relaxed);
                        emit_node_test_progress(&handle, completed, total);
                    });
                if started.load(Ordering::Relaxed) {
                    emit_node_test_finished(&handle);
                }
                if result.is_ok() {
                    if let Ok(state) = handle.state::<AppCore>().snapshot() {
                        refresh_tray(&handle, &state);
                        let _ = handle.emit("snapshot-updated", state);
                    }
                }
                result.is_ok()
            })
            .await
            .unwrap_or(false);
            if succeeded {
                last_check = Instant::now();
                initial_discovery_pending = false;
            }
        }
    });
}

fn start_system_node_monitor(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let handle = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let core = handle.state::<AppCore>();
                if let Err(error) = core.refresh_system_nodes() {
                    core.system_node_refresh_failed(&error);
                }
                if let Ok(state) = core.snapshot() {
                    refresh_tray(&handle, &state);
                    let _ = handle.emit("snapshot-updated", state);
                }
            })
            .await;
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

fn health_check_due(minutes: u32, elapsed: Duration, needs_initial_discovery: bool) -> bool {
    minutes != 0
        && (needs_initial_discovery || elapsed >= Duration::from_secs(u64::from(minutes) * 60))
}

pub fn run() {
    let autostart = tauri_plugin_autostart::Builder::new();
    #[cfg(target_os = "macos")]
    let autostart = autostart.macos_launcher(MacosLauncher::LaunchAgent);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(autostart.build())
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
            start_system_node_monitor(app.handle().clone());
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
            refresh_system_nodes,
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

#[cfg(test)]
mod tests {
    use super::{health_check_due, run_node_test, tray_labels};
    use crate::models::{HealthSummary, NodeDefinition, NodeEntry, Settings};
    use std::time::Duration;

    #[test]
    fn node_tests_run_off_the_calling_thread() {
        let calling_thread = std::thread::current().id();
        let ran_in_background = tauri::async_runtime::block_on(run_node_test(move || {
            Ok(std::thread::current().id() != calling_thread)
        }))
        .unwrap();

        assert!(ran_in_background);
    }

    #[test]
    fn tray_labels_follow_acceleration_and_current_node() {
        let node = NodeEntry {
            node: NodeDefinition::fastgit(),
            health: HealthSummary::default(),
        };
        let enabled = Settings {
            acceleration_enabled: true,
            current_node_id: Some(node.node.id.clone()),
            ..Settings::default()
        };

        assert_eq!(
            tray_labels(&enabled, std::slice::from_ref(&node)),
            ("当前线路：FastGit".into(), "关闭加速")
        );

        let disabled = Settings {
            acceleration_enabled: false,
            current_node_id: Some(node.node.id.clone()),
            ..Settings::default()
        };
        assert_eq!(
            tray_labels(&disabled, &[node]),
            ("当前线路：GitHub 直连".into(), "开启加速")
        );
    }

    #[test]
    fn health_check_schedule_honors_interval_and_disabled_setting() {
        assert!(!health_check_due(0, Duration::from_secs(86_400), true));
        assert!(!health_check_due(
            24 * 60,
            Duration::from_secs(6 * 60 * 60),
            false
        ));
        assert!(health_check_due(
            8 * 60,
            Duration::from_secs(8 * 60 * 60),
            false
        ));
        assert!(health_check_due(24 * 60, Duration::from_secs(60), true));
    }
}
