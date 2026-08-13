use crate::models::{NodeImportFile, NodeImportItem};
use url::Url;

pub fn normalize_rewrite_base(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("地址为空".into());
    }
    if ["{", "}", "<", ">", "$", "*"]
        .iter()
        .any(|token| trimmed.contains(token))
    {
        return Err("不接受占位符或通配符".into());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "不是有效 URL")?;
    if url.scheme() != "https" {
        return Err("仅接受 HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("地址中不能包含用户名、密码或 Token".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("地址中不能包含查询参数或片段".into());
    }
    if url.host_str().is_none() {
        return Err("地址缺少主机名".into());
    }
    if url.port() == Some(443) {
        url.set_port(None).map_err(|_| "无法规范化端口")?;
    }
    let required_suffix = "/https://github.com/";
    if !url.path().ends_with(required_suffix) {
        let base_path = url.path().trim_end_matches('/');
        let normalized_path = if base_path.ends_with("/https://github.com") {
            format!("{base_path}/")
        } else {
            format!("{base_path}{required_suffix}")
        };
        url.set_path(&normalized_path);
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn default_node_name(base: &str) -> String {
    Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "未命名节点".into())
}

pub fn parse_import_text(text: &str) -> Result<Vec<NodeImportItem>, String> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let file: NodeImportFile =
            serde_json::from_str(trimmed).map_err(|error| format!("JSON 格式错误：{error}"))?;
        if file.schema_version != 1 {
            return Err(format!("不支持 schemaVersion {}", file.schema_version));
        }
        return Ok(file.nodes);
    }
    Ok(trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| NodeImportItem {
            name: None,
            rewrite_base: line.trim().into(),
        })
        .collect())
}

pub fn normalize_repository_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("仓库地址为空".into());
    }
    let expanded = if trimmed.contains("://") {
        trimmed.to_string()
    } else if let Some(path) = trimmed.strip_prefix("github.com/") {
        format!("https://github.com/{path}")
    } else {
        format!("https://github.com/{trimmed}")
    };
    let url =
        Url::parse(&expanded).map_err(|_| "请输入 owner/repository 或完整的 GitHub HTTPS 地址")?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err("仅接受 https://github.com/ 下的仓库".into());
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err("GitHub 仓库地址不能指定自定义端口".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("仓库地址不能包含凭据、查询参数或片段".into());
    }
    let mut segments: Vec<&str> = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() != 2 {
        return Err("仓库地址必须是 github.com/owner/repository".into());
    }
    let repository = segments.pop().unwrap().trim_end_matches(".git");
    let owner = segments.pop().unwrap();
    if owner.is_empty() || repository.is_empty() {
        return Err("仓库 owner 和名称不能为空".into());
    }
    if !owner
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
        || !repository.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err("仓库简写必须是 owner/repository 格式".into());
    }
    Ok(format!("https://github.com/{owner}/{repository}.git"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_fixed_prefix() {
        assert_eq!(
            normalize_rewrite_base(" https://fastgit.cc/https://github.com/ ").unwrap(),
            "https://fastgit.cc/https://github.com/"
        );
    }

    #[test]
    fn expands_proxy_addresses_to_fixed_prefixes() {
        assert_eq!(
            normalize_rewrite_base("https://fastgit.cc").unwrap(),
            "https://fastgit.cc/https://github.com/"
        );
        assert_eq!(
            normalize_rewrite_base("https://proxy.example/service/").unwrap(),
            "https://proxy.example/service/https://github.com/"
        );
        assert_eq!(
            normalize_rewrite_base("https://fastgit.cc/https://github.com").unwrap(),
            "https://fastgit.cc/https://github.com/"
        );
    }

    #[test]
    fn rejects_credentials_queries_and_wrong_shape() {
        assert!(normalize_rewrite_base("https://token@fastgit.cc/https://github.com/").is_err());
        assert!(normalize_rewrite_base("https://fastgit.cc/https://github.com/?token=x").is_err());
        assert!(normalize_rewrite_base("https://fastgit.cc/{url}").is_err());
        assert!(normalize_rewrite_base("http://fastgit.cc").is_err());
    }

    #[test]
    fn normalizes_repository() {
        assert_eq!(
            normalize_repository_url("https://github.com/openai/codex/").unwrap(),
            "https://github.com/openai/codex.git"
        );
        assert_eq!(
            normalize_repository_url("anthropics/skills.git").unwrap(),
            "https://github.com/anthropics/skills.git"
        );
        assert_eq!(
            normalize_repository_url("anthropics/skills").unwrap(),
            "https://github.com/anthropics/skills.git"
        );
        assert_eq!(
            normalize_repository_url("github.com/anthropics/skills").unwrap(),
            "https://github.com/anthropics/skills.git"
        );
        assert!(normalize_repository_url("https://github.com/openai/codex/issues").is_err());
        assert!(normalize_repository_url("git@github.com:anthropics/skills.git").is_err());
        assert!(normalize_repository_url("other.example/anthropics/skills.git").is_err());
    }
}
