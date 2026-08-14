use crate::models::UsageEvent;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const USAGE_LOG_RETENTION_DAYS: i64 = 7;

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("无法创建数据目录 {}：{error}", path.display()))
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    if !path.exists() {
        return Ok(T::default());
    }
    let bytes = fs::read(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{} 数据损坏：{error}", path.display()))
}

pub fn load_or_rebuild_json<T>(
    path: &Path,
    backups_dir: &Path,
    backup_label: &str,
) -> Result<(T, bool), String>
where
    T: DeserializeOwned + Default + Serialize,
{
    if !path.exists() {
        let value = T::default();
        atomic_write_json(path, &value)?;
        return Ok((value, false));
    }
    match load_json(path) {
        Ok(value) => Ok((value, false)),
        Err(load_error) => {
            backup_file(path, backups_dir, backup_label).map_err(|backup_error| {
                format!("{load_error}；隔离损坏文件失败：{backup_error}")
            })?;
            let value = T::default();
            atomic_write_json(path, &value)
                .map_err(|write_error| format!("{load_error}；重建默认数据失败：{write_error}"))?;
            Ok((value, true))
        }
    }
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "目标文件没有父目录".to_string())?;
    ensure_dir(parent)?;
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| format!("JSON 序列化失败：{error}"))?;
    atomic_write(path, &bytes)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "目标文件没有父目录".to_string())?;
    ensure_dir(parent)?;
    let mut temp =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时文件：{error}"))?;
    temp.write_all(bytes)
        .map_err(|error| format!("无法写入临时文件：{error}"))?;
    temp.flush()
        .map_err(|error| format!("无法刷新临时文件：{error}"))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("无法同步临时文件：{error}"))?;
    temp.persist(path)
        .map_err(|error| format!("无法原子替换 {}：{}", path.display(), error.error))?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub fn backup_file(
    source: &Path,
    backups_dir: &Path,
    label: &str,
) -> Result<Option<PathBuf>, String> {
    if !source.exists() {
        return Ok(None);
    }
    ensure_dir(backups_dir)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let destination = backups_dir.join(format!("{stamp}-{label}"));
    fs::copy(source, &destination)
        .map_err(|error| format!("无法备份 {}：{error}", source.display()))?;
    Ok(Some(destination))
}

pub fn append_log(logs_dir: &Path, level: &str, event: &str) -> Result<(), String> {
    ensure_dir(logs_dir)?;
    let active = logs_dir.join("gitboost.log");
    if fs::metadata(&active)
        .map(|metadata| metadata.len() > 512 * 1024)
        .unwrap_or(false)
    {
        let oldest = logs_dir.join("gitboost.log.2");
        if oldest.exists() {
            fs::remove_file(&oldest).map_err(|error| format!("无法轮转日志：{error}"))?;
        }
        let previous = logs_dir.join("gitboost.log.1");
        if previous.exists() {
            fs::rename(&previous, &oldest).map_err(|error| format!("无法轮转日志：{error}"))?;
        }
        fs::rename(&active, &previous).map_err(|error| format!("无法轮转日志：{error}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&active)
        .map_err(|error| format!("无法打开日志：{error}"))?;
    writeln!(
        file,
        "{} [{}] {}",
        chrono::Utc::now().to_rfc3339(),
        level,
        event.replace(['\n', '\r'], " ")
    )
    .map_err(|error| format!("无法写入日志：{error}"))
}

pub fn append_usage_event(logs_dir: &Path, event: &UsageEvent) -> Result<(), String> {
    append_usage_event_at(logs_dir, event, Utc::now())
}

pub fn load_usage_events(logs_dir: &Path, limit: usize) -> Result<Vec<UsageEvent>, String> {
    load_usage_events_at(logs_dir, limit, Utc::now())
}

fn append_usage_event_at(
    logs_dir: &Path,
    event: &UsageEvent,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut events = read_usage_events(logs_dir)?;
    retain_recent_usage_events(&mut events, now);
    if event.occurred_at >= usage_log_cutoff(now) {
        events.push(event.clone());
    }
    persist_usage_events(logs_dir, &events)
}

fn load_usage_events_at(
    logs_dir: &Path,
    limit: usize,
    now: DateTime<Utc>,
) -> Result<Vec<UsageEvent>, String> {
    let previous_exists = logs_dir.join("usage.jsonl.1").exists();
    let mut events = read_usage_events(logs_dir)?;
    let original_len = events.len();
    retain_recent_usage_events(&mut events, now);
    if events.len() != original_len || previous_exists {
        persist_usage_events(logs_dir, &events)?;
    }
    let keep_from = events.len().saturating_sub(limit);
    let mut recent = events.split_off(keep_from);
    recent.reverse();
    Ok(recent)
}

fn read_usage_events(logs_dir: &Path) -> Result<Vec<UsageEvent>, String> {
    let mut events = Vec::new();
    for path in [logs_dir.join("usage.jsonl.1"), logs_dir.join("usage.jsonl")] {
        if !path.exists() {
            continue;
        }
        let file = fs::File::open(&path)
            .map_err(|error| format!("无法读取使用日志 {}：{error}", path.display()))?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| format!("无法读取使用日志：{error}"))?;
            if let Ok(event) = serde_json::from_str::<UsageEvent>(&line) {
                events.push(event);
            }
        }
    }
    Ok(events)
}

fn usage_log_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - ChronoDuration::days(USAGE_LOG_RETENTION_DAYS)
}

fn retain_recent_usage_events(events: &mut Vec<UsageEvent>, now: DateTime<Utc>) {
    let cutoff = usage_log_cutoff(now);
    events.retain(|event| event.occurred_at >= cutoff);
}

fn persist_usage_events(logs_dir: &Path, events: &[UsageEvent]) -> Result<(), String> {
    ensure_dir(logs_dir)?;
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, event)
            .map_err(|error| format!("无法写入使用日志：{error}"))?;
        bytes.push(b'\n');
    }
    atomic_write(&logs_dir.join("usage.jsonl"), &bytes)?;
    let previous = logs_dir.join("usage.jsonl.1");
    if previous.exists() {
        fs::remove_file(&previous).map_err(|error| format!("无法清理过期使用日志：{error}"))?;
    }
    Ok(())
}

pub fn clear_logs(logs_dir: &Path) -> Result<(), String> {
    if !logs_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(logs_dir).map_err(|error| format!("无法读取日志目录：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取日志项：{error}"))?
            .path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == "gitboost.log"
                    || name.starts_with("gitboost.log.")
                    || name == "usage.jsonl"
                    || name.starts_with("usage.jsonl.")
            })
        {
            fs::remove_file(&path)
                .map_err(|error| format!("无法删除日志 {}：{error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageRoute;
    use serde::{Deserialize, Serialize};

    #[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
    struct Fixture {
        value: u32,
    }

    #[test]
    fn atomically_round_trips_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        atomic_write_json(&path, &Fixture { value: 42 }).unwrap();
        assert_eq!(load_json::<Fixture>(&path).unwrap(), Fixture { value: 42 });
    }

    fn usage_event(id: &str, occurred_at: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            id: id.into(),
            occurred_at,
            command: "clone".into(),
            repository: "https://github.com/octocat/Hello-World.git".into(),
            route: UsageRoute::Direct,
            node_name: None,
            connection_host: "github.com".into(),
            succeeded: true,
            exit_code: 0,
            duration_ms: 100,
        }
    }

    #[test]
    fn usage_logs_keep_only_the_most_recent_seven_days() {
        let directory = tempfile::tempdir().unwrap();
        let now = DateTime::parse_from_rfc3339("2026-08-12T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        persist_usage_events(
            directory.path(),
            &[
                usage_event(
                    "expired",
                    now - ChronoDuration::days(7) - ChronoDuration::seconds(1),
                ),
                usage_event("boundary", now - ChronoDuration::days(7)),
            ],
        )
        .unwrap();

        append_usage_event_at(directory.path(), &usage_event("recent", now), now).unwrap();

        let events = load_usage_events_at(directory.path(), 200, now).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["recent", "boundary"]
        );
        assert!(!fs::read_to_string(directory.path().join("usage.jsonl"))
            .unwrap()
            .contains("expired"));
    }
}
