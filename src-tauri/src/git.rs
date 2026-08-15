use crate::models::{
    HealthSummary, LineMode, NodeDefinition, NodeStatus, RouteEntry, RouteScope, Settings,
    TEST_REPOSITORY,
};
use chrono::Utc;
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use url::Url;
use wait_timeout::ChildExt;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(18);
const SLOW_THRESHOLD_MS: u64 = 2_500;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    let mut command = Command::new(git_locator());
    command.arg("git");
    hide_console(&mut command);
    command.output().ok().and_then(|output| {
        output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
    })
}

#[cfg(target_os = "windows")]
fn git_locator() -> &'static str {
    "where.exe"
}

#[cfg(not(target_os = "windows"))]
fn git_locator() -> &'static str {
    "which"
}

#[cfg(target_os = "windows")]
fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_console(_command: &mut Command) {}

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
            node.enabled
                && health.in_auto_pool
                && matches!(health.status, NodeStatus::Available | NodeStatus::Slow)
        })
        .collect();
    candidates.sort_by_key(|(_, health)| health_score(health));
    candidates.first().map(|(node, _)| node)
}

pub fn health_score(health: &HealthSummary) -> u64 {
    let failure_penalty = u64::from(health.consecutive_failures) * 100_000;
    let success_rate_penalty = if health.attempt_count == 0 {
        100_000
    } else {
        100_000 - (u64::from(health.success_count) * 100_000 / u64::from(health.attempt_count))
    };
    failure_penalty + success_rate_penalty + health.median_latency_ms.unwrap_or(u64::MAX / 4)
}

pub fn build_config(
    settings: &Settings,
    node: Option<&NodeDefinition>,
    routes: &[RouteEntry],
    trace_socket: Option<&Path>,
) -> Result<String, String> {
    let mut output =
        String::from("# Managed by GitBoost. Do not store credentials in this file.\n");
    let needs_trace = settings.usage_logging_enabled
        || (settings.acceleration_enabled && settings.line_mode == LineMode::Automatic);
    if needs_trace {
        if let Some(socket) = trace_socket {
            let target = format!("af_unix:stream:{}", socket.display());
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
                let repository = route
                    .repository_url
                    .strip_suffix(".git")
                    .ok_or_else(|| "路由不是规范化的 GitHub HTTPS 仓库地址".to_string())?;
                let suffix = repository
                    .strip_prefix("https://github.com/")
                    .ok_or_else(|| "路由不是 GitHub HTTPS 地址".to_string())?;
                let accelerated = escape_subsection(&format!("{}{suffix}", node.rewrite_base));
                output.push_str(&format!(
                    "[url \"{accelerated}\"]\n\tinsteadOf = {repository}\n\n"
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
    matching_include_values(config_path).is_ok_and(|values| !values.is_empty())
}

fn matching_include_values(config_path: &Path) -> Result<Vec<String>, String> {
    let output = run_git(
        ["config", "--global", "--get-all", "include.path"],
        None,
        Duration::from_secs(5),
    )?;
    if output.status.code() == Some(1) {
        return Ok(vec![]);
    }
    if !output.status.success() {
        return Err(command_error(&output));
    }
    let mut values = vec![];
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if same_config_file(Path::new(line), config_path)? {
            values.push(line.to_owned());
        }
    }
    Ok(values)
}

fn same_config_file(left: &Path, right: &Path) -> Result<bool, String> {
    let right = fs::canonicalize(right)
        .map_err(|error| format!("无法解析 GitBoost 配置路径 {}：{error}", right.display()))?;
    Ok(fs::canonicalize(left).is_ok_and(|left| left == right))
}

pub fn register_include(config_path: &Path) -> Result<(), String> {
    if !matching_include_values(config_path)?.is_empty() {
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
    let mut values = matching_include_values(config_path)?;
    values.sort();
    values.dedup();
    for value in values {
        let output = run_git(
            [
                "config",
                "--global",
                "--unset-all",
                "--fixed-value",
                "include.path",
                value.as_str(),
            ],
            None,
            Duration::from_secs(8),
        )?;
        if !output.status.success() && output.status.code() != Some(5) {
            return Err(command_error(&output));
        }
    }
    if matching_include_values(config_path)?.is_empty() {
        Ok(())
    } else {
        Err("无法精确移除 GitBoost 的 include.path".into())
    }
}

pub fn find_conflicts(
    config_path: &Path,
    repository_path: Option<&Path>,
) -> Result<Vec<String>, String> {
    let pattern = r"^url\..*\.(insteadOf|pushInsteadOf)$";
    let temporary = if repository_path.is_none() {
        Some(tempfile::tempdir().map_err(|error| format!("无法创建诊断目录：{error}"))?)
    } else {
        None
    };
    let directory = repository_path
        .or_else(|| temporary.as_ref().map(|value| value.path()))
        .expect("诊断目录始终存在")
        .to_string_lossy();
    let output = run_git(
        [
            "-C",
            directory.as_ref(),
            "config",
            "--includes",
            "--show-origin",
            "--null",
            "--get-regexp",
            pattern,
        ],
        None,
        Duration::from_secs(8),
    )?;
    if output.status.code() == Some(1) {
        return Ok(vec![]);
    }
    if !output.status.success() {
        return Err(command_error(&output));
    }
    parse_conflicts(&output.stdout, config_path)
}

fn parse_conflicts(output: &[u8], config_path: &Path) -> Result<Vec<String>, String> {
    let mut fields = output.split(|byte| *byte == 0);
    let mut conflicts = vec![];
    while let (Some(origin), Some(entry)) = (fields.next(), fields.next()) {
        let origin = String::from_utf8_lossy(origin);
        if let Some(origin_path) = origin.strip_prefix("file:").map(Path::new) {
            if origin_path.is_absolute() && same_config_file(origin_path, config_path)? {
                continue;
            }
        }
        let entry = String::from_utf8_lossy(entry);
        let Some((_, rewrite_prefix)) = entry.split_once('\n') else {
            continue;
        };
        if !affects_github_https(rewrite_prefix.trim()) {
            continue;
        }
        let entry = entry.replace('\n', " ");
        conflicts.push(redact_path(&sanitize_line(&format!("{origin} {entry}"))));
    }
    Ok(conflicts)
}

fn affects_github_https(rewrite_prefix: &str) -> bool {
    const GITHUB_HTTPS: &str = "https://github.com/";
    GITHUB_HTTPS.starts_with(rewrite_prefix) || rewrite_prefix.starts_with(GITHUB_HTTPS)
}

pub fn effective_urls(config_path: &Path, original_url: &str) -> Result<(String, String), String> {
    effective_urls_with_global_config(config_path, original_url, None)
}

pub fn effective_urls_isolated(
    config_path: &Path,
    original_url: &str,
) -> Result<(String, String), String> {
    effective_urls_with_global_config(config_path, original_url, Some(config_path))
}

fn effective_urls_with_global_config(
    config_path: &Path,
    original_url: &str,
    global_config: Option<&Path>,
) -> Result<(String, String), String> {
    let temp = tempfile::tempdir().map_err(|error| format!("无法创建诊断目录：{error}"))?;
    let directory = temp.path().to_string_lossy();
    let include = format!("include.path={}", config_path.display());
    let init = run_git_with_global_config(
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
        global_config,
    )?;
    if !init.status.success() {
        return Err(command_error(&init));
    }
    let add = run_git_with_global_config(
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
        global_config,
    )?;
    if !add.status.success() {
        return Err(command_error(&add));
    }
    let fetch = run_git_with_global_config(
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
        global_config,
    )?;
    let push = run_git_with_global_config(
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
        global_config,
    )?;
    if !fetch.status.success() || !push.status.success() {
        return Err("无法解析 Git 有效地址".into());
    }
    Ok((
        String::from_utf8_lossy(&fetch.stdout).trim().into(),
        String::from_utf8_lossy(&push.stdout).trim().into(),
    ))
}

pub fn explicit_push_url(repository_path: &Path) -> Result<Option<String>, String> {
    if !repository_path.exists() {
        return Err("仓库路径不存在".into());
    }
    let path = repository_path.to_string_lossy();
    let repository = run_git(
        ["-C", path.as_ref(), "rev-parse", "--git-dir"],
        None,
        Duration::from_secs(5),
    )?;
    if !repository.status.success() {
        return Err("指定路径不是 Git 仓库".into());
    }
    let output = run_git(
        [
            "-C",
            path.as_ref(),
            "config",
            "--get",
            "remote.origin.pushurl",
        ],
        None,
        Duration::from_secs(5),
    )?;
    if output.status.success() {
        Ok(Some(sanitize_url(
            String::from_utf8_lossy(&output.stdout).trim(),
        )))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(command_error(&output))
    }
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

pub fn redact_path(raw: &str) -> String {
    let home = std::env::var("USERPROFILE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOME").ok().filter(|value| !value.is_empty()));
    match home {
        Some(home) => redact_path_with_home(raw, &home),
        None => raw.to_owned(),
    }
}

fn redact_path_with_home(raw: &str, home: &str) -> String {
    let windows_home = home.contains('\\') || home.as_bytes().get(1) == Some(&b':');
    let replacement = if windows_home { "%USERPROFILE%" } else { "~" };
    let mut redacted = raw.to_owned();
    let mut variants = vec![home.to_owned()];
    if windows_home {
        variants.push(home.replace('\\', "/"));
        variants.push(home.replace('/', "\\"));
    }
    for variant in variants {
        redacted = redacted.replace(&variant, replacement);
    }
    redacted
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
    run_git_with_global_config(args, current_dir, timeout, None)
}

fn run_git_with_global_config<const N: usize>(
    args: [&str; N],
    current_dir: Option<&Path>,
    timeout: Duration,
    global_config: Option<&Path>,
) -> Result<Output, String> {
    let mut command = Command::new("git");
    // GitBoost's own probes and configuration checks are operational noise, not user traffic.
    command.env("GIT_TRACE2_EVENT", "0");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(config) = global_config {
        command
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }
    hide_console(&mut command);
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
        assert!(config.contains("[url \"https://fastgit.cc/https://github.com/openai/codex\"]"));
        assert!(config.contains("insteadOf = https://github.com/openai/codex\n"));
        assert!(!config.contains("insteadOf = https://github.com/openai/codex.git"));
        assert!(!config.contains("[url \"https://fastgit.cc/https://github.com/\"]"));
        assert!(config.contains("pushInsteadOf = https://github.com/"));
    }

    #[test]
    fn allowlist_rewrite_preserves_git_and_repository_suffixes() {
        let mut settings = Settings::default();
        settings.acceleration_enabled = true;
        let routes = vec![RouteEntry {
            id: "1".into(),
            repository_url: "https://github.com/foru17/neko-master.git".into(),
            created_at: Utc::now(),
        }];
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("gitboost.gitconfig");
        fs::write(
            &config_path,
            build_config(&settings, Some(&node()), &routes, None).unwrap(),
        )
        .unwrap();

        for (original, expected_fetch) in [
            (
                "https://github.com/foru17/neko-master",
                "https://fastgit.cc/https://github.com/foru17/neko-master",
            ),
            (
                "https://github.com/foru17/neko-master.git",
                "https://fastgit.cc/https://github.com/foru17/neko-master.git",
            ),
            (
                "https://github.com/foru17/neko-master-private",
                "https://fastgit.cc/https://github.com/foru17/neko-master-private",
            ),
        ] {
            let (fetch, push) = effective_urls(&config_path, original).unwrap();
            assert_eq!(fetch, expected_fetch);
            assert_eq!(push, original);
        }
    }

    #[test]
    fn isolated_effective_urls_ignore_the_registered_live_config() {
        let directory = tempfile::tempdir().unwrap();
        let live = directory.path().join("live.gitconfig");
        let candidate = directory.path().join("candidate.gitconfig");
        let global = directory.path().join("global.gitconfig");
        fs::write(
            &live,
            "[url \"https://old.example/https://github.com/openai/codex\"]\n\tinsteadOf = https://github.com/openai/codex\n",
        )
        .unwrap();
        fs::write(
            &candidate,
            "[url \"https://new.example/https://github.com/openai/codex\"]\n\tinsteadOf = https://github.com/openai/codex\n",
        )
        .unwrap();
        fs::write(
            &global,
            format!(
                "[include]\n\tpath = \"{}\"\n",
                escape_subsection(&live.to_string_lossy())
            ),
        )
        .unwrap();

        let original = "https://github.com/openai/codex.git";
        let (conflicted, _) =
            effective_urls_with_global_config(&candidate, original, Some(&global)).unwrap();
        assert_eq!(
            conflicted,
            "https://old.example/https://github.com/openai/codex.git"
        );

        let (fetch, _) = effective_urls_isolated(&candidate, original).unwrap();
        assert_eq!(
            fetch,
            "https://new.example/https://github.com/openai/codex.git"
        );
    }

    #[test]
    fn configures_trace2_with_a_cross_platform_stream_socket() {
        let config = build_config(
            &Settings::default(),
            None,
            &[],
            Some(Path::new("/tmp/gitboost-trace.sock")),
        )
        .unwrap();
        assert!(config.contains("eventTarget = \"af_unix:stream:/tmp/gitboost-trace.sock\""));
    }

    #[test]
    fn automatic_acceleration_keeps_trace2_when_usage_logging_is_disabled() {
        let settings = Settings {
            acceleration_enabled: true,
            usage_logging_enabled: false,
            route_scope: RouteScope::Global,
            ..Settings::default()
        };

        let config = build_config(
            &settings,
            Some(&node()),
            &[],
            Some(Path::new("/tmp/gitboost-trace.sock")),
        )
        .unwrap();

        assert!(config.contains("eventTarget = \"af_unix:stream:/tmp/gitboost-trace.sock\""));
    }

    #[test]
    fn fixed_acceleration_without_usage_logging_does_not_configure_trace2() {
        let node_id = node().id;
        let settings = Settings {
            acceleration_enabled: true,
            usage_logging_enabled: false,
            line_mode: LineMode::Fixed,
            fixed_node_id: Some(node_id.clone()),
            current_node_id: Some(node_id),
            route_scope: RouteScope::Global,
            ..Settings::default()
        };

        let config = build_config(
            &settings,
            Some(&node()),
            &[],
            Some(Path::new("/tmp/gitboost-trace.sock")),
        )
        .unwrap();

        assert!(!config.contains("[trace2]"));
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
    fn conflict_parser_ignores_managed_config_and_unrelated_rules() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("gitboost.gitconfig");
        let other = directory.path().join("user.gitconfig");
        fs::write(&managed, "").unwrap();
        fs::write(&other, "").unwrap();
        let output = format!(
            "file:{}\0url.https://fastgit.cc/.insteadof\nhttps://github.com/\0file:{}\0url.https://proxy.example/.insteadof\nhttps://github.com/\0file:{}\0url.https://gitlab-proxy.example/.insteadof\nhttps://gitlab.com/\0",
            managed.display(),
            other.display(),
            other.display()
        );

        let conflicts = parse_conflicts(output.as_bytes(), &managed).unwrap();

        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("url.https://proxy.example/.insteadof"));
        assert!(!conflicts[0].contains("gitlab-proxy"));
    }

    #[test]
    fn conflict_parser_reports_relative_repository_config() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("gitboost.gitconfig");
        fs::write(&managed, "").unwrap();
        let output =
            b"file:.git/config\0url.https://proxy.example/.insteadof\nhttps://github.com/\0";

        let conflicts = parse_conflicts(output, &managed).unwrap();

        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("file:.git/config"));
        assert!(conflicts[0].contains("url.https://proxy.example/.insteadof"));
    }

    #[test]
    fn config_file_identity_ignores_unresolvable_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("gitboost.gitconfig");
        fs::write(&managed, "").unwrap();

        assert!(!same_config_file(&directory.path().join("missing"), &managed).unwrap());
        assert!(!same_config_file(Path::new("~/.gitconfig.local"), &managed).unwrap());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn config_file_identity_accepts_windows_case_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("GitBoost.GitConfig");
        fs::write(&managed, "").unwrap();
        let case_alias = managed.to_string_lossy().to_ascii_uppercase();

        assert!(same_config_file(Path::new(&case_alias), &managed).unwrap());
    }

    #[test]
    fn only_rewrites_that_can_match_github_https_are_conflicts() {
        assert!(affects_github_https("https://github.com/"));
        assert!(affects_github_https("https://github.com/openai/"));
        assert!(affects_github_https("https://"));
        assert!(!affects_github_https("https://gitlab.com/"));
        assert!(!affects_github_https("git@github.com:"));
    }

    #[test]
    fn redacts_windows_and_unix_home_paths() {
        assert_eq!(
            redact_path_with_home(r#"file:C:\Users\zhaoyun\.gitconfig"#, r#"C:\Users\zhaoyun"#,),
            r#"file:%USERPROFILE%\.gitconfig"#
        );
        assert_eq!(
            redact_path_with_home("file:/Users/zhaoyun/.gitconfig", "/Users/zhaoyun",),
            "file:~/.gitconfig"
        );
    }

    #[test]
    fn repository_pushurl_check_distinguishes_invalid_paths() {
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(
            explicit_push_url(plain.path()).unwrap_err(),
            "指定路径不是 Git 仓库"
        );

        let repository = tempfile::tempdir().unwrap();
        let path = repository.path().to_string_lossy();
        let initialized = run_git(
            ["-C", path.as_ref(), "init", "-q"],
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(initialized.status.success());
        assert_eq!(explicit_push_url(repository.path()).unwrap(), None);

        let configured = run_git(
            [
                "-C",
                path.as_ref(),
                "config",
                "remote.origin.pushurl",
                "https://user:secret@github.com/openai/codex.git",
            ],
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(configured.status.success());
        let pushurl = explicit_push_url(repository.path()).unwrap().unwrap();
        assert!(!pushurl.contains("secret"));
    }

    #[test]
    fn median_is_stable() {
        assert_eq!(median(&[30, 10, 20]), Some(20));
        assert_eq!(median(&[10, 20]), Some(15));
    }
}
