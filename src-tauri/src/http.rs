use reqwest::{
    blocking::{Client, Response},
    header::RANGE,
    redirect::Policy,
    Url,
};
use std::{io::Read, time::Duration};

const MAX_REDIRECTS: usize = 5;

fn redirect_error(url: &Url, previous: usize) -> Option<&'static str> {
    if url.scheme() != "https" {
        Some("只允许 HTTPS 重定向")
    } else if previous > MAX_REDIRECTS {
        Some("HTTPS 重定向次数过多")
    } else {
        None
    }
}

fn client(connect_timeout: Duration, timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .redirect(Policy::custom(|attempt| {
            match redirect_error(attempt.url(), attempt.previous().len()) {
                Some(error) => attempt.error(error),
                None => attempt.follow(),
            }
        }))
        .build()
        .map_err(|error| format!("无法初始化 HTTPS 客户端：{error}"))
}

fn validate_https(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|_| "不是有效的 HTTPS 地址".to_string())?;
    if parsed.scheme() != "https" {
        return Err("仅允许 HTTPS 地址".into());
    }
    Ok(())
}

fn successful(response: Response) -> Result<Response, String> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(format!("HTTP {}", status.as_u16()))
    }
}

fn read_limited(
    reader: impl Read,
    content_length: Option<u64>,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if content_length.is_some_and(|length| length > max_bytes as u64) {
        return Err(format!("响应超过 {max_bytes} 字节"));
    }
    let mut bytes = Vec::new();
    reader
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 HTTPS 响应：{error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("响应超过 {max_bytes} 字节"));
    }
    Ok(bytes)
}

pub fn fetch_limited(
    url: &str,
    connect_timeout: Duration,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    validate_https(url)?;
    let response = successful(
        client(connect_timeout, timeout)?
            .get(url)
            .send()
            .map_err(|error| format!("HTTPS 请求失败：{error}"))?,
    )?;
    let content_length = response.content_length();
    read_limited(response, content_length, max_bytes)
}

pub fn probe_range(
    url: &str,
    connect_timeout: Duration,
    timeout: Duration,
    max_bytes: usize,
) -> Result<(), String> {
    validate_https(url)?;
    let mut response = client(connect_timeout, timeout)?
        .get(url)
        .header(RANGE, "bytes=0-0")
        .send()
        .map_err(|error| format!("HTTPS 探测失败：{error}"))?;
    if !matches!(response.status().as_u16(), 200 | 206) {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let mut sampled = Vec::new();
    response
        .by_ref()
        .take(max_bytes as u64)
        .read_to_end(&mut sampled)
        .map_err(|error| format!("无法读取 HTTPS 探测响应：{error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_urls_before_requesting() {
        assert_eq!(
            fetch_limited(
                "http://example.com/catalog.json",
                Duration::from_secs(1),
                Duration::from_secs(1),
                16,
            )
            .unwrap_err(),
            "仅允许 HTTPS 地址"
        );
        assert_eq!(
            probe_range(
                "file:///tmp/archive.zip",
                Duration::from_secs(1),
                Duration::from_secs(1),
                16,
            )
            .unwrap_err(),
            "仅允许 HTTPS 地址"
        );
    }

    #[test]
    fn limits_responses_with_and_without_content_length() {
        assert_eq!(read_limited(&b"small"[..], Some(5), 5).unwrap(), b"small");
        assert_eq!(
            read_limited(&b"large"[..], Some(5), 4).unwrap_err(),
            "响应超过 4 字节"
        );
        assert_eq!(
            read_limited(&b"large"[..], None, 4).unwrap_err(),
            "响应超过 4 字节"
        );
    }

    #[test]
    fn allows_only_bounded_https_redirects() {
        let https = Url::parse("https://cdn.example/catalog.json").unwrap();
        let http = Url::parse("http://cdn.example/catalog.json").unwrap();
        assert_eq!(redirect_error(&https, 0), None);
        assert_eq!(redirect_error(&http, 0), Some("只允许 HTTPS 重定向"));
        assert_eq!(
            redirect_error(&https, MAX_REDIRECTS + 1),
            Some("HTTPS 重定向次数过多")
        );
    }
}
