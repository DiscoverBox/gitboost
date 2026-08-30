use crate::core::AppCore;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{Emitter, Manager, Runtime};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

pub const MCP_ENDPOINT: &str = "http://127.0.0.1:38476/mcp";
const MCP_BIND_ADDRESS: &str = "127.0.0.1:38476";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AddRepositoryRequest {
    /// Non-empty public GitHub repository in owner/repository or HTTPS URL form.
    repository_url: String,
}

#[derive(Clone)]
struct GitBoostMcp<R: Runtime> {
    app: tauri::AppHandle<R>,
    server_cancellation: CancellationToken,
}

#[tool_router]
impl<R: Runtime> GitBoostMcp<R> {
    fn new(app: tauri::AppHandle<R>, server_cancellation: CancellationToken) -> Self {
        Self {
            app,
            server_cancellation,
        }
    }

    #[tool(
        name = "add_accelerated_repository",
        description = "Validate that a non-empty public GitHub repository can be read anonymously through a healthy GitBoost mirror, then add it to the acceleration allowlist. The repository must have at least one commit. Never use this for private repositories."
    )]
    async fn add_accelerated_repository(
        &self,
        Parameters(request): Parameters<AddRepositoryRequest>,
        request_cancellation: CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        let app = self.app.clone();
        let repository_url = request.repository_url.clone();
        let validation = tauri::async_runtime::spawn_blocking(move || {
            app.state::<AppCore>().validate_route(&repository_url)
        })
        .await;
        let repository = match validation {
            Ok(Ok(repository)) => repository,
            Ok(Err(error)) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(error)]));
            }
            Err(error) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "仓库验证任务异常结束：{error}"
                ))]));
            }
        };
        if self.server_cancellation.is_cancelled() || request_cancellation.is_cancelled() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "MCP 调用已取消，仓库未加入加速清单",
            )]));
        }

        let app = self.app.clone();
        let repository_to_add = repository.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            app.state::<AppCore>().add_route(&repository_to_add)
        })
        .await;
        match result {
            Ok(Ok(snapshot)) => {
                let _ = self.app.emit("snapshot-updated", &snapshot);
                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "已加入 GitBoost 加速清单：{repository}"
                ))]))
            }
            Ok(Err(error)) => Ok(CallToolResult::error(vec![ContentBlock::text(error)])),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "仓库添加任务异常结束：{error}"
            ))])),
        }
    }
}

#[tool_handler]
impl<R: Runtime> ServerHandler for GitBoostMcp<R> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions("仅将确认公开的 GitHub HTTPS 仓库加入 GitBoost 加速清单。")
    }
}

struct ActiveServer {
    id: u64,
    cancellation: CancellationToken,
    stopped: oneshot::Receiver<()>,
}

#[derive(Default)]
pub struct McpServer {
    active: Mutex<Option<ActiveServer>>,
    next_id: AtomicU64,
}

impl McpServer {
    pub async fn start<R: Runtime>(&self, app: tauri::AppHandle<R>) -> Result<(), String> {
        let active = self.active.lock().await;
        if active.is_some() {
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind(MCP_BIND_ADDRESS)
            .await
            .map_err(|error| format!("无法监听 {MCP_ENDPOINT}：{error}"))?;
        self.start_with_listener(app, listener, active).await
    }

    async fn start_with_listener<R: Runtime>(
        &self,
        app: tauri::AppHandle<R>,
        listener: tokio::net::TcpListener,
        mut active: tokio::sync::MutexGuard<'_, Option<ActiveServer>>,
    ) -> Result<(), String> {
        let cancellation = CancellationToken::new();
        let handler_cancellation = cancellation.child_token();
        let service_cancellation = cancellation.child_token();
        let (stopped_sender, stopped) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let service_app = app.clone();
        let service = StreamableHttpService::new(
            move || {
                Ok(GitBoostMcp::new(
                    service_app.clone(),
                    handler_cancellation.clone(),
                ))
            },
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default()
                .with_allowed_origins(["http://127.0.0.1:38476", "http://localhost:38476"])
                .with_cancellation_token(service_cancellation),
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        *active = Some(ActiveServer {
            id,
            cancellation: cancellation.clone(),
            stopped,
        });
        drop(active);

        tauri::async_runtime::spawn(async move {
            let result = tokio::select! {
                result = axum::serve(listener, router) => result,
                _ = cancellation.cancelled_owned() => Ok(()),
            };
            let server = app.state::<McpServer>();
            if server.clear_if_active(id).await {
                let _ = app.state::<AppCore>().set_mcp_enabled(false);
                if let Ok(snapshot) = app.state::<AppCore>().snapshot() {
                    let _ = app.emit("snapshot-updated", snapshot);
                }
                if let Err(error) = result {
                    let _ = app.emit("operation-error", format!("MCP 服务异常停止：{error}"));
                }
            }
            let _ = stopped_sender.send(());
        });
        Ok(())
    }

    pub async fn stop(&self) {
        let active = self.active.lock().await.take();
        if let Some(active) = active {
            active.cancellation.cancel();
            let _ = active.stopped.await;
        }
    }

    async fn clear_if_active(&self, id: u64) -> bool {
        let mut active = self.active.lock().await;
        if active.as_ref().is_some_and(|server| server.id == id) {
            active.take();
            true
        } else {
            false
        }
    }
}

pub fn start_enabled_server(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = app.state::<McpServer>().start(app.clone()).await {
            let _ = app.state::<AppCore>().set_mcp_enabled(false);
            if let Ok(snapshot) = app.state::<AppCore>().snapshot() {
                let _ = app.emit("snapshot-updated", snapshot);
            }
            let _ = app.emit("operation-error", format!("MCP 服务启动失败：{error}"));
        }
    });
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use rmcp::{
        model::{CallToolRequest, CallToolRequestParams, ClientInfo, ClientRequest},
        service::PeerRequestOptions,
        transport::{
            streamable_http_client::StreamableHttpClientTransportConfig,
            StreamableHttpClientTransport,
        },
        ServiceExt,
    };
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    fn rerun_with_isolated_git(test_name: &str, marker: &str) -> bool {
        if std::env::var_os(marker).is_some() {
            return false;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(marker, "1")
            .env("RUST_TEST_THREADS", "1")
            .status()
            .unwrap();
        assert!(status.success(), "isolated Git test failed: {test_name}");
        true
    }

    async fn start_test_server(core: AppCore) -> (tauri::App<tauri::test::MockRuntime>, String) {
        let app = tauri::test::mock_app();
        app.manage(core);
        app.manage(McpServer::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = app.state::<McpServer>();
        let active = server.active.lock().await;
        server
            .start_with_listener(app.handle().clone(), listener, active)
            .await
            .unwrap();
        (app, format!("http://{address}/mcp"))
    }

    async fn connect(
        endpoint: String,
    ) -> rmcp::service::RunningService<rmcp::RoleClient, ClientInfo> {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint),
        );
        ClientInfo::default().serve(transport).await.unwrap()
    }

    #[test]
    fn streamable_http_lists_and_calls_the_real_tool_without_saving_on_failure() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let core = AppCore::new(directory.path().to_path_buf()).unwrap();
            let (app, endpoint) = start_test_server(core).await;
            let client = connect(endpoint).await;

            let tools = client.list_tools(None).await.unwrap();
            assert_eq!(tools.tools.len(), 1);
            assert_eq!(tools.tools[0].name.as_ref(), "add_accelerated_repository");

            let arguments = serde_json::from_value(serde_json::json!({
                "repositoryUrl": "openai/codex"
            }))
            .unwrap();
            let result = client
                .call_tool(
                    CallToolRequestParams::new("add_accelerated_repository")
                        .with_arguments(arguments),
                )
                .await
                .unwrap();

            assert_eq!(result.is_error, Some(true));
            assert!(app.state::<AppCore>().snapshot().unwrap().routes.is_empty());

            client.cancel().await.unwrap();
            app.state::<McpServer>().stop().await;
        });
    }

    #[test]
    fn stop_waits_until_the_listener_is_released() {
        tauri::async_runtime::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let core = AppCore::new(directory.path().to_path_buf()).unwrap();
            let app = tauri::test::mock_app();
            app.manage(core);
            app.manage(McpServer::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = app.state::<McpServer>();
            let active = server.active.lock().await;
            server
                .start_with_listener(app.handle().clone(), listener, active)
                .await
                .unwrap();

            server.stop().await;

            let rebound = tokio::net::TcpListener::bind(address).await.unwrap();
            drop(rebound);
        });
    }

    #[test]
    fn cancelled_or_stopped_component_call_does_not_commit_after_validation() {
        const MARKER: &str = "GITBOOST_MCP_CANCELLED_CALL_CHILD";
        if rerun_with_isolated_git(
            "mcp::tests::cancelled_or_stopped_component_call_does_not_commit_after_validation",
            MARKER,
        ) {
            return;
        }

        tauri::async_runtime::block_on(async {
            let sandbox = tempfile::tempdir().unwrap();
            let fake_bin = sandbox.path().join("bin");
            fs::create_dir(&fake_bin).unwrap();
            let started = sandbox.path().join("probe-started");
            let finished = sandbox.path().join("probe-finished");
            let fake_git = fake_bin.join("git");
            fs::write(
                &fake_git,
                r#"#!/bin/sh
case "$*" in
  *openai/codex.git*)
    : > "$GITBOOST_MCP_PROBE_STARTED"
    sleep 2
    : > "$GITBOOST_MCP_PROBE_FINISHED"
    printf '0123456789abcdef0123456789abcdef01234567\tHEAD\n'
    ;;
  *octocat/Hello-World.git*)
    printf '0123456789abcdef0123456789abcdef01234567\tHEAD\n'
    ;;
  *) exec /usr/bin/git "$@" ;;
esac
"#,
            )
            .unwrap();
            let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake_git, permissions).unwrap();
            let original_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![fake_bin];
            paths.extend(std::env::split_paths(&original_path));
            unsafe {
                std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
                std::env::set_var("GITBOOST_MCP_PROBE_STARTED", &started);
                std::env::set_var("GITBOOST_MCP_PROBE_FINISHED", &finished);
            }

            let core = AppCore::new(sandbox.path().join("state")).unwrap();
            core.test_all_nodes_with_progress(|_, _| {}).unwrap();
            let (app, endpoint) = start_test_server(core).await;
            let client = connect(endpoint).await;
            let arguments = serde_json::from_value(serde_json::json!({
                "repositoryUrl": "openai/codex"
            }))
            .unwrap();
            let call = client
                .send_cancellable_request(
                    ClientRequest::CallToolRequest(CallToolRequest::new(
                        CallToolRequestParams::new("add_accelerated_repository")
                            .with_arguments(arguments),
                    )),
                    PeerRequestOptions::no_options(),
                )
                .await
                .unwrap();

            tokio::time::timeout(Duration::from_secs(2), async {
                while !started.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("repository validation did not start");
            call.cancel(Some("test cancellation".into())).await.unwrap();
            tokio::time::timeout(Duration::from_secs(4), async {
                while !finished.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("repository validation did not finish");
            tokio::time::sleep(Duration::from_millis(100)).await;

            assert!(app.state::<AppCore>().snapshot().unwrap().routes.is_empty());

            fs::remove_file(&started).unwrap();
            fs::remove_file(&finished).unwrap();
            let arguments = serde_json::from_value(serde_json::json!({
                "repositoryUrl": "openai/codex"
            }))
            .unwrap();
            let _call = client
                .send_cancellable_request(
                    ClientRequest::CallToolRequest(CallToolRequest::new(
                        CallToolRequestParams::new("add_accelerated_repository")
                            .with_arguments(arguments),
                    )),
                    PeerRequestOptions::no_options(),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !started.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("second repository validation did not start");
            app.state::<McpServer>().stop().await;
            tokio::time::timeout(Duration::from_secs(4), async {
                while !finished.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("second repository validation did not finish");
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(app.state::<AppCore>().snapshot().unwrap().routes.is_empty());
            let _ = client.cancel().await;
        });
    }

    #[test]
    #[ignore = "requires live system-node and GitHub access"]
    fn live_streamable_http_call_adds_a_public_repository() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        core.refresh_system_nodes().unwrap();
        core.test_all_nodes_with_progress(|_, _| {}).unwrap();

        tauri::async_runtime::block_on(async {
            let (app, endpoint) = start_test_server(core).await;
            let client = connect(endpoint).await;
            let arguments = serde_json::from_value(serde_json::json!({
                "repositoryUrl": "octocat/Hello-World"
            }))
            .unwrap();

            let result = client
                .call_tool(
                    CallToolRequestParams::new("add_accelerated_repository")
                        .with_arguments(arguments),
                )
                .await
                .unwrap();

            assert_ne!(result.is_error, Some(true));
            assert_eq!(
                app.state::<AppCore>().snapshot().unwrap().routes[0].repository_url,
                "https://github.com/octocat/Hello-World.git"
            );

            client.cancel().await.unwrap();
            app.state::<McpServer>().stop().await;
        });
    }
}
