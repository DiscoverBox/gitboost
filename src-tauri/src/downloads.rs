use crate::{
    http,
    models::{DownloadTarget, NodeDefinition},
};
use std::{process::Command, sync::mpsc, thread, time::Duration};
use url::Url;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const PROBE_MAX_BYTES: usize = 65_536;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_console(_command: &mut Command) {}

pub fn prepare_target(original_url: &str, node: &NodeDefinition) -> Result<DownloadTarget, String> {
    let (original_url, file_name) = normalize_github_url(original_url)?;
    let accelerated_url = if let Some(suffix) = original_url.strip_prefix("https://github.com/") {
        format!("{}{suffix}", node.rewrite_base)
    } else if original_url.starts_with("https://raw.githubusercontent.com/") {
        let proxy_base = node
            .rewrite_base
            .strip_suffix("https://github.com/")
            .ok_or_else(|| "下载线路格式无效".to_string())?;
        format!("{proxy_base}{original_url}")
    } else {
        return Err("下载地址不是受支持的 GitHub HTTPS 地址".into());
    };
    Ok(DownloadTarget {
        accelerated_url,
        original_url,
        file_name,
        node_id: node.id.clone(),
        node_name: node.name.clone(),
    })
}

#[derive(Debug)]
pub struct DownloadProbeError {
    pub node_name: String,
    pub detail: String,
}

#[derive(Debug)]
pub struct DownloadProbeFailure {
    pub error: DownloadProbeError,
    pub attempted_node_ids: Vec<String>,
}

pub fn probe_target(target: DownloadTarget) -> Result<DownloadTarget, DownloadProbeError> {
    probe(&target).map_err(|detail| DownloadProbeError {
        node_name: target.node_name.clone(),
        detail,
    })?;
    Ok(target)
}

pub fn probe_first_available(
    targets: Vec<DownloadTarget>,
) -> Result<(DownloadTarget, Vec<String>), DownloadProbeFailure> {
    probe_first_available_with(targets, probe_target)
}

fn probe_first_available_with(
    mut targets: Vec<DownloadTarget>,
    probe: fn(DownloadTarget) -> Result<DownloadTarget, DownloadProbeError>,
) -> Result<(DownloadTarget, Vec<String>), DownloadProbeFailure> {
    let first = targets.remove(0);
    let mut attempted_node_ids = vec![first.node_id.clone()];
    let first_error = match probe(first) {
        Ok(target) => return Ok((target, attempted_node_ids)),
        Err(error) => error,
    };
    if targets.is_empty() {
        return Err(DownloadProbeFailure {
            error: first_error,
            attempted_node_ids,
        });
    }

    let last_index = targets.len() - 1;
    let target_count = targets.len();
    attempted_node_ids.extend(targets.iter().map(|target| target.node_id.clone()));
    let (sender, receiver) = mpsc::channel();
    for (index, target) in targets.into_iter().enumerate() {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send((index, probe(target)));
        });
    }
    drop(sender);

    let mut last_error = None;
    for _ in 0..target_count {
        let Ok((index, result)) = receiver.recv() else {
            break;
        };
        match result {
            Ok(target) => return Ok((target, attempted_node_ids)),
            Err(error) if index == last_index => last_error = Some(error),
            Err(_) => {}
        }
    }
    Err(DownloadProbeFailure {
        error: last_error.unwrap_or(first_error),
        attempted_node_ids,
    })
}

pub fn open_target(target: DownloadTarget) -> Result<DownloadTarget, String> {
    open_in_browser(&target.accelerated_url)?;
    Ok(target)
}

pub fn open_in_browser(url: &str) -> Result<(), String> {
    let mut command = browser_command(url);
    hide_console(&mut command);
    let status = command
        .status()
        .map_err(|error| format!("无法调用默认浏览器：{error}"))?;
    if !status.success() {
        return Err("默认浏览器未能打开地址".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(target_os = "windows")]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("rundll32.exe");
    command.args(["url.dll,FileProtocolHandler", url]);
    command
}

fn normalize_github_url(input: &str) -> Result<(String, String), String> {
    let trimmed = input.trim();
    if trimmed.len() > 4_096 {
        return Err("下载地址过长".into());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "请输入完整的 GitHub 地址")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("github.com" | "raw.githubusercontent.com")
        )
    {
        return Err(
            "仅支持 https://github.com/ 或 https://raw.githubusercontent.com/ 下的地址".into(),
        );
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err("GitHub 下载地址不能指定自定义端口".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("下载地址不能包含用户名、密码或 Token".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("下载地址不能包含查询参数或片段".into());
    }
    let file_name = url
        .path_segments()
        .and_then(|parts| parts.filter(|part| !part.is_empty()).next_back())
        .unwrap_or("github.com")
        .to_owned();
    if url.port() == Some(443) {
        url.set_port(None).map_err(|_| "无法规范化下载地址")?;
    }
    Ok((url.to_string(), file_name))
}

fn probe(target: &DownloadTarget) -> Result<(), String> {
    http::probe_range(
        &target.accelerated_url,
        Duration::from_secs(4),
        Duration::from_secs(8),
        PROBE_MAX_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn failed_target(id: &str) -> DownloadTarget {
        DownloadTarget {
            original_url: "https://github.com/owner/repository/file.zip".into(),
            accelerated_url: format!("https://{id}.example/file.zip"),
            file_name: "file.zip".into(),
            node_id: id.into(),
            node_name: id.into(),
        }
    }

    fn delayed_failure(target: DownloadTarget) -> Result<DownloadTarget, DownloadProbeError> {
        if target.node_id != "first" {
            thread::sleep(Duration::from_millis(350));
        }
        Err(DownloadProbeError {
            node_name: target.node_name,
            detail: "HTTP 500".into(),
        })
    }

    fn first_success(target: DownloadTarget) -> Result<DownloadTarget, DownloadProbeError> {
        Ok(target)
    }

    fn fallback_success(target: DownloadTarget) -> Result<DownloadTarget, DownloadProbeError> {
        if target.node_id == "second" {
            Ok(target)
        } else {
            Err(DownloadProbeError {
                node_name: target.node_name,
                detail: "HTTP 500".into(),
            })
        }
    }

    #[test]
    fn first_success_only_marks_the_first_target_as_attempted() {
        let targets = ["first", "second", "third", "fourth"]
            .map(failed_target)
            .into_iter()
            .collect();

        let (winner, attempted_node_ids) =
            probe_first_available_with(targets, first_success).unwrap();

        assert_eq!(winner.node_id, "first");
        assert_eq!(attempted_node_ids, vec!["first"]);
    }

    #[test]
    fn fallback_probe_marks_every_started_target_as_attempted() {
        let targets = ["first", "second", "third", "fourth"]
            .map(failed_target)
            .into_iter()
            .collect();

        let (winner, attempted_node_ids) =
            probe_first_available_with(targets, fallback_success).unwrap();

        assert_eq!(winner.node_id, "second");
        assert_eq!(
            attempted_node_ids,
            vec!["first", "second", "third", "fourth"]
        );
    }

    #[test]
    fn probes_three_fallback_targets_concurrently() {
        let targets = ["first", "second", "third", "fourth"]
            .map(failed_target)
            .into_iter()
            .collect();
        let started = Instant::now();

        let failure = probe_first_available_with(targets, delayed_failure).unwrap_err();

        assert_eq!(failure.error.node_name, "fourth");
        assert_eq!(
            failure.attempted_node_ids,
            vec!["first", "second", "third", "fourth"]
        );
        assert!(started.elapsed() < Duration::from_millis(800));
    }

    #[test]
    fn accepts_supported_github_urls() {
        let (url, name) = normalize_github_url(
            "https://github.com/ollama/ollama/releases/download/v0.32.5/OllamaSetup.exe",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://github.com/ollama/ollama/releases/download/v0.32.5/OllamaSetup.exe"
        );
        assert_eq!(name, "OllamaSetup.exe");
        assert!(normalize_github_url(
            "https://github.com/owner/repo/releases/latest/download/tool.zip"
        )
        .is_ok());
        assert!(normalize_github_url(
            "https://github.com/DiscoverBox/gitboost/archive/refs/heads/main.zip"
        )
        .is_ok());
        assert!(normalize_github_url("https://github.com/a/b/releases/tag/v1").is_ok());
        let (url, name) = normalize_github_url(
            "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh"
        );
        assert_eq!(name, "install.sh");
    }

    #[test]
    fn rejects_untrusted_or_sensitive_urls() {
        assert!(
            normalize_github_url("https://example.com/a/b/releases/download/v1/a.zip").is_err()
        );
        assert!(normalize_github_url(
            "https://github.com/a/b/releases/download/v1/a.zip?token=secret"
        )
        .is_err());
        assert!(
            normalize_github_url("https://token@github.com/a/b/releases/download/v1/a.zip")
                .is_err()
        );
        assert!(normalize_github_url(
            "https://raw.githubusercontent.com.example/owner/repo/main/file.txt"
        )
        .is_err());
        assert!(normalize_github_url(
            "https://raw.githubusercontent.com/owner/repo/main/file.txt?token=secret"
        )
        .is_err());
    }

    #[test]
    fn builds_node_download_url() {
        let target = prepare_target(
            "https://github.com/ollama/ollama/releases/download/v1/tool.zip",
            &NodeDefinition::fastgit(),
        )
        .unwrap();
        assert_eq!(
            target.accelerated_url,
            "https://fastgit.cc/https://github.com/ollama/ollama/releases/download/v1/tool.zip"
        );
        assert_eq!(target.node_id, crate::models::FASTGIT_REWRITE_BASE);
        assert_eq!(target.node_name, "FastGit");
    }

    #[test]
    fn builds_node_download_url_for_raw_github_content() {
        let target = prepare_target(
            "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
            &NodeDefinition::fastgit(),
        )
        .unwrap();
        assert_eq!(
            target.accelerated_url,
            "https://fastgit.cc/https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh"
        );
        assert_eq!(
            target.original_url,
            "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh"
        );
        assert_eq!(target.file_name, "install.sh");
    }

    #[test]
    #[ignore = "requires live GitHub proxy access"]
    fn live_raw_github_content_uses_the_production_probe() {
        let target = prepare_target(
            "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
            &NodeDefinition::fastgit(),
        )
        .unwrap();
        let probed = probe_target(target.clone()).unwrap();
        assert_eq!(probed.accelerated_url, target.accelerated_url);
    }
}
