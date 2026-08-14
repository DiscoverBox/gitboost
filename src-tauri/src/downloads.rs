use crate::{
    http,
    models::{DownloadTarget, NodeDefinition},
};
use std::{process::Command, time::Duration};
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
    let suffix = original_url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| "下载地址不是 GitHub HTTPS 地址".to_string())?;
    Ok(DownloadTarget {
        accelerated_url: format!("{}{suffix}", node.rewrite_base),
        original_url,
        file_name,
        node_id: node.id.clone(),
        node_name: node.name.clone(),
    })
}

pub fn probe_and_open(target: DownloadTarget) -> Result<DownloadTarget, String> {
    probe(&target)?;
    let mut command = browser_command(&target.accelerated_url);
    hide_console(&mut command);
    let status = command
        .status()
        .map_err(|error| format!("无法调用默认浏览器：{error}"))?;
    if !status.success() {
        return Err("默认浏览器未能打开下载地址".into());
    }
    Ok(target)
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

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

fn normalize_github_url(input: &str) -> Result<(String, String), String> {
    let trimmed = input.trim();
    if trimmed.len() > 4_096 {
        return Err("下载地址过长".into());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "请输入完整的 GitHub 地址")?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err("仅支持 https://github.com/ 下的地址".into());
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
    .map_err(|error| format!("无法通过 {} 获取此文件：{error}", target.node_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_github_urls() {
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
}
