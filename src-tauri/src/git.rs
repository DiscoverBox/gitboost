use crate::models::{
    HealthSummary, LineMode, NodeDefinition, NodeStatus, RouteEntry, RouteScope, Settings,
    TEST_REPOSITORY,
};
use chrono::Utc;
use std::{
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use url::Url;
use wait_timeout::ChildExt;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(18);
const SLOW_THRESHOLD_MS: u64 = 2_500;

pub fn git_version() -> Option<String> {
    run_git(["--version"], None, Duration::from_secs(5))
        .ok()
        .and_then(|output| {
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
}

pub fn git_path() -> Option<String> {
    Command::new("which")
        .arg("git")
        .output()
        .ok()
        .and_then(|output| {
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
}

pub fn test_node(node: &NodeDefinition, previous: &HealthSummary) -> HealthSummary {
    let mut next = previous.clone();
    for _ in 0..2 {
        let started = Instant::now();
        let config = format!("url.{}.insteadOf=https://github.com/", node.rewrite_base);
        let result = run_git(
            ["-c", config.as_str(), "ls-remote", TEST_REPOSITORY, "HEAD"],
            None,
            COMMAND_TIMEOUT,
        );
        let elapsed = started.elapsed().as_millis() as u64;
        next.attempt_count = next.attempt_count.saturating_add(1);
        match result {
            Ok(output) if output.status.success() && valid_ls_remote(&output.stdout) => {
                next.success_count = next.success_count.saturating_add(1);
                next.consecutive_failures = 0;
                next.failure_reason = None;
                next.recent_latencies_ms.push(elapsed);
                if next.recent_latencies_ms.len() > 7 {
                    next.recent_latencies_ms.remove(0);
                }
            }
            Ok(output) => {
                next.consecutive_failures = next.consecutive_failures.saturating_add(1);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                next.failure_reason = Some(classify_failure(&format!("{stderr}\n{stdout}")));
            }
            Err(error) => {
                next.consecutive_failures = next.consecutive_failures.saturating_add(1);
                next.failure_reason = Some(error);
            }
        }
    }
    next.checked_at = Some(Utc::now());
    next.median_latency_ms = median(&next.recent_latencies_ms);
    next.status = if next.consecutive_failures >= 2 {
        if next
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Smart HTTP") || reason.contains("网页内容"))
        {
            NodeStatus::Incompatible
        } else {
            NodeStatus::Unavailable
        }
    } else if next
        .median_latency_ms
        .is_some_and(|latency| latency > SLOW_THRESHOLD_MS)
    {
        NodeStatus::Slow
    } else if next.recent_latencies_ms.is_empty() {
        NodeStatus::Unavailable
    } else {
        NodeStatus::Available
    };
    next
}

fn valid_ls_remote(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next().unwrap_or_default();
        let reference = parts.next().unwrap_or_default();
        hash.len() >= 40
            && hash.chars().all(|character| character.is_ascii_hexdigit())
            && reference == "HEAD"
    })
}

fn classify_failure(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let summary = if lower.contains("timed out") || lower.contains("timeout") {
        "连接超时"
    } else if lower.contains("could not resolve host")
        || lower.contains("name or service not known")
    {
        "DNS 解析失败"
    } else if lower.contains("ssl") || lower.contains("tls") || lower.contains("certificate") {
        "TLS 连接失败"
    } else if lower.contains("authentication") || lower.contains("terminal prompts disabled") {
        "节点要求认证，不符合公开 Smart HTTP"
    } else if lower.contains("redirect") {
        "重定向异常"
    } else if lower.contains("html") || lower.contains("content-type") {
        "返回网页内容，不支持 Git Smart HTTP"
    } else if lower.contains("repository not found") || lower.contains("not found") {
        "测试仓库无法访问，节点可能不兼容 Smart HTTP"
    } else if lower.contains("failed to connect") || lower.contains("connection refused") {
        "无法连接节点"
    } else {
        "Git Smart HTTP 检测失败"
    };
    let detail = sanitize_line(
        raw.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default(),
    );
    if detail.is_empty() {
        summary.into()
    } else {
        format!("{summary}：{detail}")
    }
}

fn median(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2
    } else {
        sorted[middle]
    })
}

pub fn choose_node<'a>(nodes: &'a [(NodeDefinition, HealthSummary)]) -> Option<&'a NodeDefinition> {
    let mut candidates: Vec<_> = nodes
        .iter()
        .filter(|(node, health)| {
            node.enabled && matches!(health.status, NodeStatus::Available | NodeStatus::Slow)
        })
        .collect();
    candidates.sort_by_key(|(_, health)| {
        let failure_penalty = u64::from(health.consecutive_failures) * 100_000;
        let success_rate_penalty = if health.attempt_count == 0 {
            100_000
        } else {
            100_000 - (u64::from(health.success_count) * 100_000 / u64::from(health.attempt_count))
        };
        failure_penalty + success_rate_penalty + health.median_latency_ms.unwrap_or(u64::MAX / 4)
    });
    candidates.first().map(|(node, _)| node)
}

pub fn build_config(
    settings: &Settings,
    node: Option<&NodeDefinition>,
    routes: &[RouteEntry],
    trace_socket: Option<&Path>,
) -> Result<String, String> {
    let mut output =
        String::from("# Managed by GitBoost. Do not store credentials in this file.\n");
    if settings.usage_logging_enabled {
        if let Some(socket) = trace_socket {
            let target = format!("af_unix:dgram:{}", socket.display());
            output.push_str(&format!(
                "[trace2]\n\teventTarget = \"{}\"\n\n",
                escape_subsection(&target)
            ));
        }
    }
    if !settings.acceleration_enabled || settings.line_mode == LineMode::Direct {
        output.push_str("# Acceleration is disabled; GitHub remains direct.\n");
        return Ok(output);
    }
    let node = node.ok_or_else(|| "没有通过检测的可用节点".to_string())?;
    let base = escape_subsection(&node.rewrite_base);
    match settings.route_scope {
        RouteScope::Global => {
            output.push_str(&format!(
                "[url \"{base}\"]\n\tinsteadOf = https://github.com/\n\n"
            ));
        }
        RouteScope::Allowlist => {
            for route in routes {
                let suffix = route
                    .repository_url
                    .strip_prefix("https://github.com/")
                    .ok_or_else(|| "路由不是 GitHub HTTPS 地址".to_string())?;
                let accelerated = escape_subsection(&format!("{}{suffix}", node.rewrite_base));
                output.push_str(&format!(
                    "[url \"{accelerated}\"]\n\tinsteadOf = {}\n\n",
                    route.repository_url
                ));
            }
        }
    }
    output.push_str("[url \"https://github.com/\"]\n\tpushInsteadOf = https://github.com/\n");
    Ok(output)
}

fn escape_subsection(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn include_registered(config_path: &Path) -> bool {
    run_git(
        ["config", "--global", "--get-all", "include.path"],
        None,
        Duration::from_secs(5),
    )
    .ok()
    .filter(|output| output.status.success())
    .is_some_and(|output| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| Path::new(line.trim()) == config_path)
    })
}

pub fn register_include(config_path: &Path) -> Result<(), String> {
    if include_registered(config_path) {
        return Ok(());
    }
    let path = config_path.to_string_lossy();
    let output = run_git(
        ["config", "--global", "--add", "include.path", path.as_ref()],
        None,
        Duration::from_secs(8),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(&output))
    }
}

pub fn unregister_include(config_path: &Path) -> Result<(), String> {
    if !include_registered(config_path) {
        return Ok(());
    }
    let path = config_path.to_string_lossy();
    let output = run_git(
        [
            "config",
            "--global",
            "--unset-all",
            "include.path",
            path.as_ref(),
        ],
        None,
        Duration::from_secs(8),
    )?;
    if output.status.success() || output.status.code() == Some(5) {
        Ok(())
    } else {
        Err(command_error(&output))
    }
}

pub fn find_conflicts(config_path: &Path) -> Vec<String> {
    let pattern = r"^url\..*\.(insteadOf|pushInsteadOf)$";
    let Ok(output) = run_git(
        [
            "config",
            "--global",
            "--includes",
            "--show-origin",
            "--get-regexp",
            pattern,
        ],
        None,
        Duration::from_secs(8),
    ) else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let own = config_path.to_string_lossy();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.contains(own.as_ref()))
        .map(sanitize_line)
        .collect()
}

pub fn effective_urls(config_path: &Path, original_url: &str) -> Result<(String, String), String> {
    let temp = tempfile::tempdir().map_err(|error| format!("无法创建诊断目录：{error}"))?;
    let directory = temp.path().to_string_lossy();
    let include = format!("include.path={}", config_path.display());
    let init = run_git(
        [
            "-c",
            include.as_str(),
            "-C",
            directory.as_ref(),
            "init",
            "-q",
        ],
        None,
        Duration::from_secs(8),
    )?;
    if !init.status.success() {
        return Err(command_error(&init));
    }
    let add = run_git(
        [
            "-c",
            include.as_str(),
            "-C",
            directory.as_ref(),
            "remote",
            "add",
            "origin",
            original_url,
        ],
        None,
        Duration::from_secs(8),
    )?;
    if !add.status.success() {
        return Err(command_error(&add));
    }
    let fetch = run_git(
        [
            "-c",
            include.as_str(),
            "-C",
            directory.as_ref(),
            "remote",
            "get-url",
            "origin",
        ],
        None,
        Duration::from_secs(8),
    )?;
    let push = run_git(
        [
            "-c",
            include.as_str(),
            "-C",
            directory.as_ref(),
            "remote",
            "get-url",
            "--push",
            "origin",
        ],
        None,
        Duration::from_secs(8),
    )?;
    if !fetch.status.success() || !push.status.success() {
        return Err("无法解析 Git 有效地址".into());
    }
    Ok((
        String::from_utf8_lossy(&fetch.stdout).trim().into(),
        String::from_utf8_lossy(&push.stdout).trim().into(),
    ))
}

pub fn explicit_push_url(repository_path: &Path) -> Option<String> {
    if !repository_path.exists() {
        return None;
    }
    let path = repository_path.to_string_lossy();
    run_git(
        [
            "-C",
            path.as_ref(),
            "config",
            "--get",
            "remote.origin.pushurl",
        ],
        None,
        Duration::from_secs(5),
    )
    .ok()
    .and_then(|output| {
        output
            .status
            .success()
            .then(|| sanitize_url(String::from_utf8_lossy(&output.stdout).trim()))
    })
}

pub fn sanitize_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw.trim()) else {
        return raw.trim().to_string();
    };
    if !url.username().is_empty() {
        let _ = url.set_username("[redacted]");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("[redacted]"));
    }
    if url.query().is_some() {
        url.set_query(Some("[redacted]"));
    }
    url.set_fragment(None);
    url.to_string()
}

fn sanitize_line(line: &str) -> String {
    line.split_whitespace()
        .map(|part| {
            if part.contains("://") {
                sanitize_url(part)
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(sanitize_line)
        .unwrap_or_else(|| format!("Git 命令退出：{}", output.status));
    message
}

fn run_git<const N: usize>(
    args: [&str; N],
    current_dir: Option<&Path>,
    timeout: Duration,
) -> Result<Output, String> {
    let mut command = Command::new("git");
    // GitBoost's own probes and configuration checks are operational noise, not user traffic.
    command.env("GIT_TRACE2_EVENT", "0");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动系统 Git：{error}"))?;
    match child
        .wait_timeout(timeout)
        .map_err(|error| format!("等待 Git 命令失败：{error}"))?
    {
        Some(_) => child
            .wait_with_output()
            .map_err(|error| format!("读取 Git 输出失败：{error}")),
        None => {
            let _ = child.kill();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let _ = child.wait();
            Err(format!(
                "Git 命令超时（{} 秒）{}",
                timeout.as_secs(),
                if stderr.is_empty() {
                    "".into()
                } else {
                    format!("：{}", sanitize_line(&stderr))
                }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RouteEntry;
    use pretty_assertions::assert_eq;

    fn node() -> NodeDefinition {
        NodeDefinition::fastgit()
    }

    #[test]
    fn builds_allowlist_without_global_fetch_rule() {
        let mut settings = Settings::default();
        settings.acceleration_enabled = true;
        let routes = vec![RouteEntry {
            id: "1".into(),
            repository_url: "https://github.com/openai/codex.git".into(),
            created_at: Utc::now(),
        }];
        let config = build_config(&settings, Some(&node()), &routes, None).unwrap();
        assert!(config.contains("https://fastgit.cc/https://github.com/openai/codex.git"));
        assert!(!config.contains("[url \"https://fastgit.cc/https://github.com/\"]"));
        assert!(config.contains("pushInsteadOf = https://github.com/"));
    }

    #[test]
    fn direct_config_has_no_rewrite() {
        let mut settings = Settings::default();
        settings.acceleration_enabled = true;
        settings.line_mode = LineMode::Direct;
        settings.usage_logging_enabled = false;
        assert_eq!(build_config(&settings, Some(&node()), &[], None).unwrap(), "# Managed by GitBoost. Do not store credentials in this file.\n# Acceleration is disabled; GitHub remains direct.\n");
    }

    #[test]
    fn global_mode_ignores_repository_routes() {
        let mut settings = Settings::default();
        settings.acceleration_enabled = true;
        settings.route_scope = RouteScope::Global;
        let routes = vec![RouteEntry {
            id: "1".into(),
            repository_url: "https://github.com/openai/codex.git".into(),
            created_at: Utc::now(),
        }];
        let config = build_config(&settings, Some(&node()), &routes, None).unwrap();
        assert!(config.contains("insteadOf = https://github.com/"));
        assert!(!config.contains("openai/codex.git"));
    }

    #[test]
    fn redacts_url_secrets() {
        let safe = sanitize_url("https://user:secret@example.com/a?token=abc#x");
        assert!(!safe.contains("secret"));
        assert!(!safe.contains("abc"));
        assert!(!safe.contains("#x"));
    }

    #[test]
    fn median_is_stable() {
        assert_eq!(median(&[30, 10, 20]), Some(20));
        assert_eq!(median(&[10, 20]), Some(15));
    }
}
