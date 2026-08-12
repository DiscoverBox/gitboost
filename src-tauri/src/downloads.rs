use crate::models::{DownloadTarget, NodeDefinition};
use std::process::Command;
use url::Url;

const PROBE_TIMEOUT_SECONDS: &str = "8";
const PROBE_MAX_BYTES: &str = "65536";

pub fn prepare_target(original_url: &str, node: &NodeDefinition) -> Result<DownloadTarget, String> {
    let (original_url, file_name) = normalize_release_url(original_url)?;
    let suffix = original_url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| "下载地址不是 GitHub HTTPS 地址".to_string())?;
    Ok(DownloadTarget {
        accelerated_url: format!("{}{suffix}", node.rewrite_base),
        original_url,
        file_name,
        node_name: node.name.clone(),
    })
}

pub fn probe_and_open(target: DownloadTarget) -> Result<DownloadTarget, String> {
    probe(&target)?;
    let status = Command::new("open")
        .arg(&target.accelerated_url)
        .status()
        .map_err(|error| format!("无法调用默认浏览器：{error}"))?;
    if !status.success() {
        return Err("默认浏览器未能打开下载地址".into());
    }
    Ok(target)
}

fn normalize_release_url(input: &str) -> Result<(String, String), String> {
    let trimmed = input.trim();
    if trimmed.len() > 4_096 {
        return Err("下载地址过长".into());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "请输入完整的 GitHub Release 下载地址")?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err("仅支持 https://github.com/ 下的 Release 下载地址".into());
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
    let segments: Vec<String> = url
        .path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let valid_shape = segments.len() == 6
        && valid_owner(&segments[0])
        && valid_repository(&segments[1])
        && segments[2] == "releases"
        && ((segments[3] == "download" && !segments[4].is_empty() && !segments[5].is_empty())
            || (segments[3] == "latest" && segments[4] == "download" && !segments[5].is_empty()));
    if !valid_shape {
        return Err("仅支持 GitHub Release 的 download 或 latest/download 文件地址".into());
    }
    if url.port() == Some(443) {
        url.set_port(None).map_err(|_| "无法规范化下载地址")?;
    }
    let file_name = segments[5].clone();
    Ok((url.to_string(), file_name))
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn valid_repository(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn probe(target: &DownloadTarget) -> Result<(), String> {
    let output = Command::new("curl")
        .args([
            "--location",
            "--silent",
            "--show-error",
            "--fail",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--range",
            "0-0",
            "--max-filesize",
            PROBE_MAX_BYTES,
            "--connect-timeout",
            "4",
            "--max-time",
            PROBE_TIMEOUT_SECONDS,
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}\t%{content_type}",
            &target.accelerated_url,
        ])
        .output()
        .map_err(|error| format!("无法运行下载探测：{error}"))?;
    let summary = String::from_utf8_lossy(&output.stdout);
    let (http_code, content_type) = summary.split_once('\t').unwrap_or(("", ""));
    let http_code = http_code.trim();
    let content_type = content_type.trim().to_ascii_lowercase();
    let (reachable, html_error) = classify_probe(
        http_code,
        &content_type,
        output.status.code(),
        &target.file_name,
    );
    if reachable && !html_error {
        return Ok(());
    }
    if html_error {
        return Err(format!(
            "{} 返回了网页内容，未识别到目标文件",
            target.node_name
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if detail.is_empty() {
        Err(format!("无法通过 {} 获取此文件", target.node_name))
    } else {
        Err(format!(
            "无法通过 {} 获取此文件：{detail}",
            target.node_name
        ))
    }
}

fn classify_probe(
    http_code: &str,
    content_type: &str,
    exit_code: Option<i32>,
    file_name: &str,
) -> (bool, bool) {
    let file_name = file_name.to_ascii_lowercase();
    let html_error = content_type.starts_with("text/html")
        && !file_name.ends_with(".html")
        && !file_name.ends_with(".htm");
    // curl 63 means the declared or received body exceeded our 64 KiB probe cap.
    let reachable = matches!(http_code, "200" | "206")
        && matches!(exit_code, Some(0) | Some(63))
        && !html_error;
    (reachable, html_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_release_asset_urls() {
        let (url, name) = normalize_release_url(
            "https://github.com/ollama/ollama/releases/download/v0.32.5/OllamaSetup.exe",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://github.com/ollama/ollama/releases/download/v0.32.5/OllamaSetup.exe"
        );
        assert_eq!(name, "OllamaSetup.exe");
        assert!(normalize_release_url(
            "https://github.com/owner/repo/releases/latest/download/tool.zip"
        )
        .is_ok());
    }

    #[test]
    fn rejects_untrusted_or_non_asset_urls() {
        assert!(
            normalize_release_url("https://example.com/a/b/releases/download/v1/a.zip").is_err()
        );
        assert!(normalize_release_url("https://github.com/a/b/releases/tag/v1").is_err());
        assert!(normalize_release_url(
            "https://github.com/a/b/releases/download/v1/a.zip?token=secret"
        )
        .is_err());
        assert!(
            normalize_release_url("https://token@github.com/a/b/releases/download/v1/a.zip")
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
        assert_eq!(target.node_name, "FastGit");
    }

    #[test]
    fn classifies_limited_probes_without_accepting_error_pages() {
        assert_eq!(
            classify_probe("206", "application/octet-stream", Some(0), "tool.zip"),
            (true, false)
        );
        assert_eq!(
            classify_probe("200", "application/octet-stream", Some(63), "tool.zip"),
            (true, false)
        );
        assert_eq!(
            classify_probe("200", "text/html", Some(0), "tool.zip"),
            (false, true)
        );
        assert_eq!(
            classify_probe("404", "application/octet-stream", Some(22), "tool.zip"),
            (false, false)
        );
    }
}
