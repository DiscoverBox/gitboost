use crate::core::{AppCore, FailoverOutcome};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};
use tauri::{AppHandle, Emitter, Manager};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug)]
pub struct CompletedTrace {
    pub occurred_at: DateTime<Utc>,
    pub command: String,
    pub original_url: Option<String>,
    pub effective_url: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

#[derive(Debug, Deserialize)]
struct TraceEnvelope {
    event: String,
    sid: String,
    time: Option<DateTime<Utc>>,
    argv: Option<Vec<String>>,
    name: Option<String>,
    child_class: Option<String>,
    code: Option<i32>,
    t_abs: Option<f64>,
}

#[derive(Debug)]
struct PendingTrace {
    occurred_at: DateTime<Utc>,
    command: String,
    original_url: Option<String>,
    effective_url: Option<String>,
}

#[derive(Default)]
struct TraceAssembler {
    pending: HashMap<String, PendingTrace>,
}

impl TraceAssembler {
    fn ingest(&mut self, bytes: &[u8]) -> Option<CompletedTrace> {
        let event: TraceEnvelope = serde_json::from_slice(bytes).ok()?;
        let root = event.sid.split('/').next()?.to_string();
        let is_root = root == event.sid;
        match event.event.as_str() {
            "start" if is_root => {
                let argv = event.argv.unwrap_or_default();
                let original_url = argv.iter().find(|arg| looks_like_https(arg)).cloned();
                self.pending.insert(
                    root,
                    PendingTrace {
                        occurred_at: event.time.unwrap_or_else(Utc::now),
                        command: command_from_argv(&argv),
                        original_url,
                        effective_url: None,
                    },
                );
            }
            "cmd_name" if is_root => {
                if let (Some(pending), Some(name)) = (self.pending.get_mut(&root), event.name) {
                    pending.command = name;
                }
            }
            "child_start" if event.child_class.as_deref() == Some("remote-https") => {
                if let (Some(pending), Some(argv)) = (self.pending.get_mut(&root), event.argv) {
                    pending.effective_url =
                        argv.into_iter().rev().find(|arg| looks_like_https(arg));
                }
            }
            "exit" if is_root => {
                let pending = self.pending.remove(&root)?;
                let effective_url = pending.effective_url?;
                return Some(CompletedTrace {
                    occurred_at: pending.occurred_at,
                    command: pending.command,
                    original_url: pending.original_url,
                    effective_url,
                    exit_code: event.code.unwrap_or(-1),
                    duration_ms: (event.t_abs.unwrap_or_default() * 1000.0).round() as u64,
                });
            }
            _ => {}
        }
        None
    }
}

fn looks_like_https(value: &str) -> bool {
    value.starts_with("https://")
}

fn command_from_argv(argv: &[String]) -> String {
    argv.iter()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| "git".into())
}

fn bind_listener(path: &Path) -> std::io::Result<Socket> {
    let listener = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    listener.bind(&SockAddr::unix(path)?)?;
    listener.listen(128)?;
    Ok(listener)
}

pub fn start_listener(app: AppHandle) {
    std::thread::spawn(move || {
        let socket_path = app.state::<AppCore>().trace_socket_path();
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }
        let listener = match bind_listener(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                app.state::<AppCore>()
                    .usage_listener_failed(&format!("Trace2 socket bind failed: {error}"));
                return;
            }
        };
        #[cfg(unix)]
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));
        app.state::<AppCore>().set_usage_listening(true);
        let mut assembler = TraceAssembler::default();
        loop {
            let (stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => {
                    app.state::<AppCore>()
                        .usage_listener_failed(&format!("Trace2 socket accept failed: {error}"));
                    break;
                }
            };
            for line in BufReader::new(stream).split(b'\n') {
                match line {
                    Ok(line) if !line.is_empty() => {
                        if let Some(completed) = assembler.ingest(&line) {
                            match app.state::<AppCore>().observe_git_operation(completed) {
                                Ok(Some(node_id)) => {
                                    start_failed_node_recheck(app.clone(), node_id)
                                }
                                Ok(None) => {}
                                Err(error) => app.state::<AppCore>().usage_connection_failed(
                                    &format!("Git operation observation failed: {error}"),
                                ),
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = app.state::<AppCore>().usage_connection_failed(&format!(
                            "Trace2 socket read failed: {error}"
                        ));
                    }
                }
            }
        }
        app.state::<AppCore>().set_usage_listening(false);
        let _ = fs::remove_file(&socket_path);
    });
}

fn start_failed_node_recheck(app: AppHandle, node_id: String) {
    tauri::async_runtime::spawn_blocking(move || {
        let result = app.state::<AppCore>().recheck_failed_node(&node_id);
        match result {
            Ok(FailoverOutcome::Skipped) => {}
            Ok(outcome) => {
                if let Ok(snapshot) = app.state::<AppCore>().snapshot() {
                    crate::refresh_tray(&app, &snapshot);
                    let _ = app.emit("snapshot-updated", snapshot);
                }
                match outcome {
                    FailoverOutcome::Switched { from, to } => {
                        let _ = app.emit(
                            "operation-error",
                            format!(
                                "线路 {from} 检测失败，已切换到 {to}。请重新执行刚才的 Git 命令。"
                            ),
                        );
                    }
                    FailoverOutcome::FellBackToDirect { from } => {
                        let _ = app.emit(
                            "operation-error",
                            format!("线路 {from} 检测失败，暂无其他可用线路，已恢复 GitHub 直连。请重新执行刚才的 Git 命令。"),
                        );
                    }
                    FailoverOutcome::Unconfirmed => {}
                    FailoverOutcome::Skipped => unreachable!(),
                }
            }
            Err(error) => app
                .state::<AppCore>()
                .usage_connection_failed(&format!("automatic failover recheck failed: {error}")),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TEST_REPOSITORY;

    #[test]
    fn assembles_remote_operation_without_retaining_raw_argv() {
        let mut assembler = TraceAssembler::default();
        assert!(assembler.ingest(br#"{"event":"start","sid":"root","time":"2026-08-12T07:22:37Z","argv":["git","clone","https://github.com/octocat/Hello-World.git"]}"#).is_none());
        assert!(assembler.ingest(br#"{"event":"child_start","sid":"root/child","child_class":"remote-https","argv":["git","remote-https","https://github.com/octocat/Hello-World.git","https://fastgit.cc/https://github.com/octocat/Hello-World.git"]}"#).is_none());
        let event = assembler
            .ingest(br#"{"event":"exit","sid":"root","code":0,"t_abs":1.234}"#)
            .unwrap();
        assert_eq!(event.command, "clone");
        assert_eq!(
            event.effective_url,
            "https://fastgit.cc/https://github.com/octocat/Hello-World.git"
        );
        assert_eq!(event.duration_ms, 1234);
    }

    #[test]
    fn real_git_failure_produces_a_completed_remote_trace() {
        let directory = tempfile::tempdir().unwrap();
        let trace_path = directory.path().join("trace.json");
        let rewrite_base = "https://127.0.0.1:1/https://github.com/";
        let rewrite = format!("url.{rewrite_base}.insteadOf=https://github.com/");
        let output = std::process::Command::new("git")
            .args(["-c", &rewrite, "ls-remote", TEST_REPOSITORY, "HEAD"])
            .env("GIT_TRACE2_EVENT", &trace_path)
            .output()
            .unwrap();
        assert!(!output.status.success());

        let mut assembler = TraceAssembler::default();
        let completed = fs::read(&trace_path)
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| assembler.ingest(line))
            .next()
            .expect("real Git must emit a completed remote trace");

        assert_eq!(completed.command, "ls-remote");
        assert!(completed.effective_url.starts_with(rewrite_base));
        assert_ne!(completed.exit_code, 0);
    }
}
