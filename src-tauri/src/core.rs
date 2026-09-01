use crate::{
    downloads, git, http,
    importer::{
        default_node_name, normalize_repository_url, normalize_rewrite_base, parse_import_text,
    },
    models::*,
    storage::{
        append_log, append_usage_event, atomic_write, atomic_write_json, backup_file, clear_logs,
        ensure_dir, load_json, load_or_rebuild_json, load_usage_events,
    },
    usage::CompletedTrace,
};
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tempfile::NamedTempFile;
use uuid::Uuid;

const SYSTEM_NODE_CATALOG_URLS: [&str; 3] = [
    "https://cdn.jsdelivr.net/gh/DiscoverBox/gitboost@main/nodes.enc.json",
    "https://cdn.bili33.top/gh/DiscoverBox/gitboost@main/nodes.enc.json",
    "https://cdn.jsdmirror.com/gh/DiscoverBox/gitboost@main/nodes.enc.json",
];
const SYSTEM_NODE_CATALOG_KEY: [u8; 32] = [
    0x2d, 0xbf, 0x43, 0xf2, 0x77, 0xa1, 0x09, 0x79, 0xcb, 0x9e, 0x06, 0xe7, 0x2b, 0x0d, 0xdb, 0x71,
    0x0c, 0xb6, 0x0d, 0x23, 0xb1, 0xb7, 0xc4, 0x65, 0x4a, 0xa1, 0xf6, 0x73, 0x46, 0x7b, 0xd9, 0x69,
];
const MAX_SYSTEM_NODES: usize = 100;
const CATALOG_MAX_BYTES: usize = 262_144;
const NODE_TEST_CONCURRENCY: usize = 4;
const AUTO_POOL_NODE_TARGET: usize = 10;
const DOWNLOAD_BATCH_SIZE: usize = 4;
const HEALTH_CHECK_INTERVALS: [u32; 6] = [
    0,
    60,
    8 * 60,
    DEFAULT_HEALTH_CHECK_MINUTES,
    7 * 24 * 60,
    30 * 24 * 60,
];

#[derive(Debug)]
pub struct AppCore {
    paths: AppPaths,
    lock: Mutex<()>,
    full_node_test_lock: Mutex<()>,
    full_node_test_result: Mutex<Option<Result<Vec<NodeEntry>, String>>>,
    usage_listening: AtomicBool,
}

struct CatalogUpdate {
    changed: bool,
    recovery_applied_at: Option<chrono::DateTime<Utc>>,
}

struct NodePoolTestResult {
    tested: Vec<(String, HealthSummary)>,
    pool_ids: HashSet<String>,
}

enum AutomaticPoolUpdate {
    Rebuild,
    Include(String),
    Replace(HashSet<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverOutcome {
    Skipped,
    Unconfirmed,
    Switched { from: String, to: String },
    FellBackToDirect { from: String },
}

#[derive(Deserialize)]
struct EncryptedSystemCatalog {
    version: u8,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug)]
struct AppPaths {
    root: PathBuf,
    settings: PathBuf,
    nodes: PathBuf,
    system_nodes: PathBuf,
    health: PathBuf,
    routes: PathBuf,
    gitconfig: PathBuf,
    backups: PathBuf,
    logs: PathBuf,
    trace_socket: PathBuf,
}

impl AppCore {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        let paths = AppPaths {
            settings: root.join("settings.json"),
            nodes: root.join("nodes.json"),
            system_nodes: root.join("system-nodes.json"),
            health: root.join("health.json"),
            routes: root.join("routes.json"),
            gitconfig: root.join("gitboost.gitconfig"),
            backups: root.join("backups"),
            logs: root.join("logs"),
            trace_socket: root.join("trace2.sock"),
            root,
        };
        ensure_dir(&paths.root)?;
        ensure_dir(&paths.backups)?;
        ensure_dir(&paths.logs)?;
        let core = Self {
            paths,
            lock: Mutex::new(()),
            full_node_test_lock: Mutex::new(()),
            full_node_test_result: Mutex::new(None),
            usage_listening: AtomicBool::new(false),
        };
        core.initialize()?;
        Ok(core)
    }

    fn initialize(&self) -> Result<(), String> {
        let (mut settings, settings_rebuilt) = load_or_rebuild_json::<Settings>(
            &self.paths.settings,
            &self.paths.backups,
            "corrupt-settings.json",
        )?;
        if settings_rebuilt {
            let _ = append_log(
                &self.paths.logs,
                "ERROR",
                "corrupt settings.json quarantined and rebuilt with safe defaults",
            );
        }
        if !HEALTH_CHECK_INTERVALS.contains(&settings.health_check_minutes) {
            settings.health_check_minutes = DEFAULT_HEALTH_CHECK_MINUTES;
            atomic_write_json(&self.paths.settings, &settings)?;
        }
        self.initialize_node_files()?;
        let (_, health_rebuilt) = load_or_rebuild_json::<HashMap<String, HealthSummary>>(
            &self.paths.health,
            &self.paths.backups,
            "corrupt-health.json",
        )?;
        if health_rebuilt {
            let _ = append_log(
                &self.paths.logs,
                "ERROR",
                "corrupt or legacy health.json quarantined and rebuilt",
            );
        }
        let (_, routes_rebuilt) = load_or_rebuild_json::<Vec<RouteEntry>>(
            &self.paths.routes,
            &self.paths.backups,
            "corrupt-routes.json",
        )?;
        if routes_rebuilt {
            let _ = append_log(
                &self.paths.logs,
                "ERROR",
                "corrupt routes.json quarantined and rebuilt with an empty allowlist",
            );
        }
        if !self.paths.gitconfig.exists() {
            atomic_write(
                &self.paths.gitconfig,
                b"# Managed by GitBoost. Acceleration is disabled.\n",
            )?;
        }
        Ok(())
    }

    fn initialize_node_files(&self) -> Result<(), String> {
        if !self.paths.nodes.exists() {
            atomic_write_json(&self.paths.nodes, &Vec::<NodeDefinition>::new())?;
        }
        if !self.paths.system_nodes.exists() {
            atomic_write_json(
                &self.paths.system_nodes,
                &vec![FASTGIT_REWRITE_BASE.to_string()],
            )?;
        }
        Ok(())
    }

    fn settings(&self) -> Result<Settings, String> {
        load_json(&self.paths.settings)
    }
    fn custom_nodes(&self) -> Result<Vec<NodeDefinition>, String> {
        load_json(&self.paths.nodes)
    }
    fn nodes(&self) -> Result<Vec<NodeDefinition>, String> {
        let mut nodes = self.custom_nodes()?;
        let mut known: HashSet<String> =
            nodes.iter().map(|node| node.rewrite_base.clone()).collect();
        let system_urls: Vec<String> = load_json(&self.paths.system_nodes)?;
        for rewrite_base in normalize_system_urls(system_urls)? {
            if known.insert(rewrite_base.clone()) {
                nodes.push(NodeDefinition {
                    id: rewrite_base.clone(),
                    name: default_node_name(&rewrite_base),
                    rewrite_base,
                    enabled: true,
                    built_in: true,
                });
            }
        }
        Ok(nodes)
    }
    fn health(&self) -> Result<HashMap<String, HealthSummary>, String> {
        load_json(&self.paths.health)
    }
    fn routes(&self) -> Result<Vec<RouteEntry>, String> {
        load_json(&self.paths.routes)
    }

    pub fn trace_socket_path(&self) -> PathBuf {
        self.paths.trace_socket.clone()
    }

    pub fn refresh_system_nodes(&self) -> Result<bool, String> {
        let output = fetch_system_catalog()?;
        self.apply_system_catalog_update(&output, git::test_node)
    }

    fn apply_system_catalog_update<F>(&self, output: &[u8], test: F) -> Result<bool, String>
    where
        F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary + Sync,
    {
        let update = self.apply_system_catalog(output)?;
        if let Some(applied_at) = update.recovery_applied_at.as_ref() {
            let _run = self.full_node_test_lock.lock();
            *self.full_node_test_result.lock() = None;
            let nodes = self.nodes()?;
            let old_health = self.health()?;
            let result = test_node_pool(&nodes, &old_health, test, |_, _| {})?;
            self.persist_test_results(
                result.tested,
                AutomaticPoolUpdate::Replace(result.pool_ids),
            )?;
            self.resume_acceleration_after_catalog_refresh(applied_at)?;
        }
        Ok(update.changed)
    }

    fn apply_system_catalog(&self, output: &[u8]) -> Result<CatalogUpdate, String> {
        let urls = parse_system_catalog(output)?;
        let _guard = self.lock.lock();
        let current: Vec<String> = load_json(&self.paths.system_nodes)?;
        if current == urls {
            return Ok(CatalogUpdate {
                changed: false,
                recovery_applied_at: None,
            });
        }
        let mut available_ids: HashSet<String> = self
            .custom_nodes()?
            .into_iter()
            .map(|node| node.id)
            .collect();
        available_ids.extend(urls.iter().cloned());
        let mut settings = self.settings()?;
        let original_settings = settings.clone();
        let original_health = self.health()?;
        let mut health = original_health.clone();
        health.retain(|node_id, _| available_ids.contains(node_id));
        let mut settings_changed = false;
        let current_removed = settings
            .current_node_id
            .as_ref()
            .is_some_and(|id| !available_ids.contains(id));
        if current_removed {
            settings.current_node_id = None;
            settings_changed = true;
        }
        let should_resume = current_removed && settings.acceleration_enabled;
        let previous_gitconfig = should_resume
            .then(|| fs::read(&self.paths.gitconfig).ok())
            .flatten();
        let registered_before = should_resume && git::include_registered(&self.paths.gitconfig);
        if should_resume {
            settings.acceleration_enabled = false;
            self.write_configuration(&mut settings, &[], &[])?;
        }
        let persist_result = (|| {
            atomic_write_json(&self.paths.system_nodes, &urls)?;
            if settings_changed && !should_resume {
                atomic_write_json(&self.paths.settings, &settings)?;
            }
            atomic_write_json(&self.paths.health, &health)
        })();
        if let Err(error) = persist_result {
            let _ = atomic_write_json(&self.paths.health, &original_health);
            let _ = atomic_write_json(&self.paths.settings, &original_settings);
            let _ = atomic_write_json(&self.paths.system_nodes, &current);
            if should_resume {
                self.restore_git_state(previous_gitconfig.as_deref(), registered_before);
            }
            return Err(error);
        }
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!("system node catalog refreshed: nodes={}", urls.len()),
        );
        Ok(CatalogUpdate {
            changed: true,
            recovery_applied_at: if should_resume {
                settings.last_applied_at
            } else {
                None
            },
        })
    }

    pub fn system_node_refresh_failed(&self, message: &str) {
        let _ = append_log(&self.paths.logs, "ERROR", message);
    }

    pub fn configuration_refresh_failed(&self, message: &str) {
        let _ = append_log(
            &self.paths.logs,
            "ERROR",
            &format!("startup configuration refresh failed: {message}"),
        );
    }

    pub fn set_usage_listening(&self, listening: bool) {
        self.usage_listening.store(listening, Ordering::Relaxed);
    }

    pub fn usage_listener_failed(&self, message: &str) {
        self.set_usage_listening(false);
        let _ = append_log(&self.paths.logs, "ERROR", message);
    }

    pub fn usage_connection_failed(&self, message: &str) {
        let _ = append_log(&self.paths.logs, "ERROR", message);
    }

    pub fn snapshot(&self) -> Result<AppSnapshot, String> {
        let settings = self.settings()?;
        let nodes = self.node_entries()?;
        let routes = self.routes()?;
        let (conflicts, conflict_scan_error) =
            match git::find_conflicts(&self.paths.gitconfig, None) {
                Ok(conflicts) => (conflicts.len(), None),
                Err(error) => (0, Some(error)),
            };
        let expected_trace2_target =
            format!("af_unix:stream:{}", self.paths.trace_socket.display());
        let trace2_target_overridden = settings.acceleration_enabled
            && git::effective_trace2_target().ok().flatten().as_deref()
                != Some(expected_trace2_target.as_str());
        Ok(AppSnapshot {
            environment: EnvironmentSummary {
                git_available: git::git_version().is_some(),
                git_path: git::git_path(),
                git_version: git::git_version(),
                include_registered: git::include_registered(&self.paths.gitconfig),
                config_path: self.paths.gitconfig.display().to_string(),
                conflicts,
                conflict_scan_error,
                trace2_target_overridden,
            },
            settings,
            nodes,
            routes,
        })
    }

    pub fn prepare_download_batch(
        &self,
        original_url: &str,
        excluded_node_ids: &[String],
    ) -> Result<(Vec<DownloadTarget>, bool), String> {
        let settings = self.settings()?;
        let mut pairs: Vec<_> = self
            .node_pairs()?
            .into_iter()
            .filter(|(node, health)| {
                !excluded_node_ids.contains(&node.id)
                    && node.enabled
                    && health.in_auto_pool
                    && is_usable_health(health)
            })
            .collect();
        let mut targets = Vec::new();
        while targets.len() < DOWNLOAD_BATCH_SIZE {
            let selected = if targets.is_empty() {
                settings
                    .current_node_id
                    .as_deref()
                    .and_then(|id| pairs.iter().find(|(node, _)| node.id == id))
                    .map(|(node, _)| node)
                    .or_else(|| git::choose_node(&pairs))
            } else {
                git::choose_node(&pairs)
            };
            let Some(node) = selected else {
                break;
            };
            targets.push(downloads::prepare_target(original_url, node)?);
            let selected_id = node.id.clone();
            pairs.retain(|(node, _)| node.id != selected_id);
        }
        if targets.is_empty() {
            return Err(if excluded_node_ids.is_empty() {
                "自动线路池中没有可用的下载节点".to_string()
            } else {
                "自动线路池中没有其他可用的下载线路".to_string()
            });
        }
        Ok((targets, git::choose_node(&pairs).is_some()))
    }

    fn node_entries(&self) -> Result<Vec<NodeEntry>, String> {
        let mut nodes = self.nodes()?;
        let health = self.health()?;
        nodes.sort_by_key(|node| !node.built_in);
        Ok(nodes
            .into_iter()
            .map(|node| NodeEntry {
                health: health.get(&node.id).cloned().unwrap_or_default(),
                node,
            })
            .collect())
    }

    pub fn import_nodes(&self, text: &str) -> Result<ImportResult, String> {
        let _guard = self.lock.lock();
        let items = parse_import_text(text)?;
        if items.len() > 1_000 {
            return Err("单次最多导入 1000 个节点".into());
        }
        let mut nodes = self.custom_nodes()?;
        let mut known: HashSet<String> = self
            .nodes()?
            .iter()
            .map(|node| node.rewrite_base.clone())
            .collect();
        let mut imported = 0;
        let mut duplicates = 0;
        let mut rejected = vec![];
        for item in items {
            match normalize_rewrite_base(&item.rewrite_base) {
                Ok(base) if known.contains(&base) => duplicates += 1,
                Ok(base) => {
                    let name = item
                        .name
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| default_node_name(&base));
                    nodes.push(NodeDefinition {
                        id: Uuid::new_v4().to_string(),
                        name: name.trim().chars().take(80).collect(),
                        rewrite_base: base.clone(),
                        enabled: true,
                        built_in: false,
                    });
                    known.insert(base);
                    imported += 1;
                }
                Err(reason) => rejected.push(RejectedImport {
                    input: redact_import(&item.rewrite_base),
                    reason,
                }),
            }
        }
        atomic_write_json(&self.paths.nodes, &nodes)?;
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!(
                "node import completed: imported={imported}, duplicates={duplicates}, rejected={}",
                rejected.len()
            ),
        );
        Ok(ImportResult {
            imported,
            duplicates,
            rejected,
            nodes: self.node_entries()?,
        })
    }

    pub fn import_node_file(&self, path: &Path) -> Result<ImportResult, String> {
        let metadata = fs::metadata(path).map_err(|error| format!("无法读取导入文件：{error}"))?;
        if metadata.len() > 2 * 1024 * 1024 {
            return Err("节点文件不能超过 2 MB".into());
        }
        let text =
            fs::read_to_string(path).map_err(|error| format!("节点文件不是有效 UTF-8：{error}"))?;
        self.import_nodes(&text)
    }

    pub fn export_nodes(&self, path: &Path) -> Result<String, String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Export<'a> {
            schema_version: u32,
            nodes: Vec<ExportNode<'a>>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ExportNode<'a> {
            name: &'a str,
            rewrite_base: &'a str,
        }
        let nodes = self.custom_nodes()?;
        let value = Export {
            schema_version: SCHEMA_VERSION,
            nodes: nodes
                .iter()
                .map(|node| ExportNode {
                    name: &node.name,
                    rewrite_base: &node.rewrite_base,
                })
                .collect(),
        };
        atomic_write_json(path, &value)?;
        Ok(path.display().to_string())
    }

    pub fn test_node(&self, node_id: &str) -> Result<NodeEntry, String> {
        let _run = self
            .full_node_test_lock
            .try_lock()
            .ok_or_else(|| "线路检测正在进行，请稍后再试".to_string())?;
        *self.full_node_test_result.lock() = None;
        self.test_node_locked(node_id, git::test_node)
    }

    fn test_node_locked<F>(&self, node_id: &str, test: F) -> Result<NodeEntry, String>
    where
        F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary,
    {
        let (node, tested) = self.test_node_health(node_id, test)?;
        self.persist_test_results(
            vec![(node.id.clone(), tested)],
            AutomaticPoolUpdate::Include(node.id.clone()),
        )?
        .into_iter()
        .find(|entry| entry.node.id == node.id)
        .ok_or_else(|| "节点在检测期间已被删除".to_string())
    }

    fn test_node_health<F>(
        &self,
        node_id: &str,
        test: F,
    ) -> Result<(NodeDefinition, HealthSummary), String>
    where
        F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary,
    {
        let node = self
            .nodes()?
            .into_iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| "节点不存在".to_string())?;
        let previous = self.health()?.remove(node_id).unwrap_or_default();
        let tested = test(&node, &previous);
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!(
                "node test completed: id={}, status={:?}",
                node.id, tested.status
            ),
        );
        Ok((node, tested))
    }

    pub fn recheck_failed_node(&self, node_id: &str) -> Result<FailoverOutcome, String> {
        self.recheck_failed_node_with(node_id, git::test_node)
    }

    fn recheck_failed_node_with<F>(&self, node_id: &str, test: F) -> Result<FailoverOutcome, String>
    where
        F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary,
    {
        let Some(_run) = self.full_node_test_lock.try_lock() else {
            return Ok(FailoverOutcome::Skipped);
        };

        let before = self.settings()?;
        if !before.acceleration_enabled || before.current_node_id.as_deref() != Some(node_id) {
            return Ok(FailoverOutcome::Skipped);
        }
        *self.full_node_test_result.lock() = None;
        let from = self
            .nodes()?
            .into_iter()
            .find(|node| node.id == node_id)
            .map(|node| node.name)
            .ok_or_else(|| "失败线路已不存在".to_string())?;

        let (node, tested) = self.test_node_health(node_id, test)?;
        let after = {
            let _guard = self.lock.lock();
            let current = self.settings()?;
            if !current.acceleration_enabled || current.current_node_id.as_deref() != Some(node_id)
            {
                return Ok(FailoverOutcome::Skipped);
            }
            if is_usable_health(&tested) {
                None
            } else {
                self.persist_test_results_locked(
                    vec![(node.id.clone(), tested)],
                    AutomaticPoolUpdate::Include(node.id),
                )?;
                Some(self.settings()?)
            }
        };
        let outcome = match after
            .as_ref()
            .and_then(|settings| settings.current_node_id.as_deref())
        {
            Some(current) if current != node_id => {
                let to = self
                    .nodes()?
                    .into_iter()
                    .find(|node| node.id == current)
                    .map(|node| node.name)
                    .unwrap_or_else(|| "其他可用线路".into());
                FailoverOutcome::Switched { from, to }
            }
            None if after
                .as_ref()
                .is_some_and(|settings| !settings.acceleration_enabled) =>
            {
                FailoverOutcome::FellBackToDirect { from }
            }
            _ => FailoverOutcome::Unconfirmed,
        };
        let message = match &outcome {
            FailoverOutcome::Skipped => "automatic failover skipped".to_string(),
            FailoverOutcome::Unconfirmed => {
                format!("Git operation failed but node recheck passed: node={node_id}")
            }
            FailoverOutcome::Switched { .. } => {
                format!("automatic failover selected another node: previous={node_id}")
            }
            FailoverOutcome::FellBackToDirect { .. } => {
                format!("automatic failover restored direct routing: previous={node_id}")
            }
        };
        let _ = append_log(&self.paths.logs, "INFO", &message);
        Ok(outcome)
    }

    #[cfg(test)]
    pub fn test_all_nodes_with_progress<F>(&self, on_progress: F) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        let _run = self
            .full_node_test_lock
            .try_lock()
            .ok_or_else(|| "线路检测正在进行，请稍后再试".to_string())?;
        self.execute_full_node_test_locked(&on_progress)
    }

    pub fn test_all_nodes_or_join_with_progress<F>(
        &self,
        on_progress: F,
    ) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        let Some(_run) = self.full_node_test_lock.try_lock() else {
            let _run = self.full_node_test_lock.lock();
            if let Some(result) = self.full_node_test_result.lock().clone() {
                return result;
            }
            return self.execute_full_node_test_locked(&on_progress);
        };
        self.execute_full_node_test_locked(&on_progress)
    }

    pub fn test_background_nodes_with_progress<F>(
        &self,
        on_progress: F,
    ) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        let _run = self
            .full_node_test_lock
            .try_lock()
            .ok_or_else(|| "线路检测正在进行，请稍后再试".to_string())?;
        *self.full_node_test_result.lock() = None;
        self.test_background_nodes_locked(&on_progress)
    }

    fn execute_full_node_test_locked<F>(&self, on_progress: &F) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        *self.full_node_test_result.lock() = None;
        let result = self.test_all_nodes_locked(on_progress);
        *self.full_node_test_result.lock() = Some(result.clone());
        result
    }

    fn test_all_nodes_locked<F>(&self, on_progress: &F) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        let nodes = self.nodes()?;
        let old_health = self.health()?;
        let result = test_node_pool(&nodes, &old_health, git::test_node, on_progress)?;
        self.persist_test_results(result.tested, AutomaticPoolUpdate::Rebuild)
    }

    fn test_background_nodes_locked<F>(&self, on_progress: &F) -> Result<Vec<NodeEntry>, String>
    where
        F: Fn(usize, usize) + Sync,
    {
        let nodes = self.nodes()?;
        let old_health = self.health()?;
        let result = test_node_pool(&nodes, &old_health, git::test_node, on_progress)?;
        self.persist_test_results(result.tested, AutomaticPoolUpdate::Replace(result.pool_ids))
    }

    fn persist_test_results(
        &self,
        tested: Vec<(String, HealthSummary)>,
        pool_update: AutomaticPoolUpdate,
    ) -> Result<Vec<NodeEntry>, String> {
        let _guard = self.lock.lock();
        self.persist_test_results_locked(tested, pool_update)
    }

    fn persist_test_results_locked(
        &self,
        tested: Vec<(String, HealthSummary)>,
        pool_update: AutomaticPoolUpdate,
    ) -> Result<Vec<NodeEntry>, String> {
        let nodes = self.nodes()?;
        let live_ids: HashSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
        let tested_ids: HashSet<String> =
            tested.iter().map(|(node_id, _)| node_id.clone()).collect();
        let mut next_health = self.health()?;
        for (node_id, summary) in tested {
            let is_newer = next_health
                .get(&node_id)
                .is_none_or(|existing| existing.checked_at <= summary.checked_at);
            if is_newer {
                next_health.insert(node_id, summary);
            }
        }
        next_health.retain(|node_id, _| live_ids.contains(node_id.as_str()));
        let (candidate_ids, required_id): (HashSet<String>, Option<String>) = match pool_update {
            AutomaticPoolUpdate::Rebuild => (tested_ids, None),
            AutomaticPoolUpdate::Include(node_id) => (
                next_health
                    .iter()
                    .filter(|(_, summary)| summary.in_auto_pool)
                    .map(|(node_id, _)| node_id.clone())
                    .chain(std::iter::once(node_id.clone()))
                    .collect(),
                Some(node_id),
            ),
            AutomaticPoolUpdate::Replace(pool_ids) => (pool_ids, None),
        };
        let pool_ids =
            select_auto_pool(&nodes, &next_health, &candidate_ids, required_id.as_deref());
        for (node_id, summary) in &mut next_health {
            summary.in_auto_pool = pool_ids.contains(node_id);
        }
        atomic_write_json(&self.paths.health, &next_health)?;
        self.reselect_after_health_change()?;
        self.node_entries()
    }

    pub fn needs_background_node_discovery(&self) -> Result<bool, String> {
        let health = self.health()?;
        Ok(self
            .nodes()?
            .into_iter()
            .filter(|node| {
                node.enabled
                    && health
                        .get(&node.id)
                        .is_some_and(|summary| summary.in_auto_pool && is_usable_health(summary))
            })
            .count()
            < AUTO_POOL_NODE_TARGET)
    }

    fn resume_acceleration_after_catalog_refresh(
        &self,
        expected_applied_at: &chrono::DateTime<Utc>,
    ) -> Result<(), String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        if settings.acceleration_enabled
            || settings.last_applied_at.as_ref() != Some(expected_applied_at)
        {
            return Ok(());
        }
        let pairs = self.node_pairs()?;
        let Some(node) = git::choose_node(&pairs) else {
            return Ok(());
        };
        settings.current_node_id = Some(node.id.clone());
        settings.acceleration_enabled = true;
        self.write_configuration(&mut settings, &pairs, &self.routes()?)
    }

    fn reselect_after_health_change(&self) -> Result<(), String> {
        let mut settings = self.settings()?;
        if settings.acceleration_enabled {
            let pairs = self.node_pairs()?;
            settings.current_node_id = git::choose_node(&pairs).map(|node| node.id.clone());
            if settings.current_node_id.is_none() {
                settings.acceleration_enabled = false;
            }
            self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
        }
        Ok(())
    }

    fn node_pairs(&self) -> Result<Vec<(NodeDefinition, HealthSummary)>, String> {
        let health = self.health()?;
        Ok(self
            .nodes()?
            .into_iter()
            .map(|node| {
                let summary = health.get(&node.id).cloned().unwrap_or_default();
                (node, summary)
            })
            .collect())
    }

    pub fn rename_node(&self, node_id: &str, name: &str) -> Result<AppSnapshot, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 80 {
            return Err("节点名称需为 1–80 个字符".into());
        }
        let _guard = self.lock.lock();
        let mut nodes = self.custom_nodes()?;
        let node = nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .ok_or_else(|| "节点不存在".to_string())?;
        node.name = trimmed.into();
        atomic_write_json(&self.paths.nodes, &nodes)?;
        self.snapshot()
    }

    pub fn set_node_enabled(&self, node_id: &str, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut nodes = self.custom_nodes()?;
        let node = nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .ok_or_else(|| "节点不存在".to_string())?;
        node.enabled = enabled;
        let mut health = self.health()?;
        if !enabled {
            if let Some(summary) = health.get_mut(node_id) {
                summary.in_auto_pool = false;
            }
        }
        let mut settings = self.settings()?;
        if !enabled && settings.current_node_id.as_deref() == Some(node_id) {
            settings.current_node_id = None;
        }
        atomic_write_json(&self.paths.nodes, &nodes)?;
        atomic_write_json(&self.paths.health, &health)?;
        atomic_write_json(&self.paths.settings, &settings)?;
        drop(_guard);
        self.reselect_after_health_change()?;
        self.snapshot()
    }

    pub fn delete_node(&self, node_id: &str) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut nodes = self.custom_nodes()?;
        if !nodes.iter().any(|node| node.id == node_id) {
            return Err("节点不存在".into());
        }
        nodes.retain(|node| node.id != node_id);
        let mut health = self.health()?;
        health.remove(node_id);
        let mut settings = self.settings()?;
        if settings.current_node_id.as_deref() == Some(node_id) {
            settings.current_node_id = None;
        }
        atomic_write_json(&self.paths.nodes, &nodes)?;
        atomic_write_json(&self.paths.health, &health)?;
        atomic_write_json(&self.paths.settings, &settings)?;
        drop(_guard);
        self.reselect_after_health_change()?;
        self.snapshot()
    }

    pub fn trace2_target_conflict(&self) -> Result<Option<String>, String> {
        git::external_trace2_target(&self.paths.gitconfig)
    }

    pub fn set_acceleration(
        &self,
        enabled: bool,
        replace_trace2_target: bool,
    ) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        if enabled && git::git_version().is_none() {
            return Err("未检测到系统 Git".into());
        }
        if enabled && settings.consent_acknowledged_at.is_none() {
            return Err("首次开启加速前，请在 GitBoost 窗口中确认隐私提示".into());
        }
        if enabled && settings.route_scope == RouteScope::Allowlist && self.routes()?.is_empty() {
            return Err("仅加速清单为空，请先加入至少一个公开仓库".into());
        }
        if enabled
            && !replace_trace2_target
            && git::external_trace2_target(&self.paths.gitconfig)?.is_some()
        {
            return Err("检测到其他工具正在使用 Git Trace2，请在 GitBoost 窗口中选择保留现有配置或切换到 GitBoost".into());
        }
        if enabled {
            settings.acceleration_enabled = true;
        } else {
            settings.current_node_id = None;
            settings.acceleration_enabled = false;
        }
        let pairs = self.node_pairs()?;
        if enabled {
            self.select_current(&mut settings, &pairs)?;
        }
        self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
        self.snapshot()
    }

    pub fn acknowledge_consent(&self) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.consent_acknowledged_at = Some(Utc::now());
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    fn select_current(
        &self,
        settings: &mut Settings,
        pairs: &[(NodeDefinition, HealthSummary)],
    ) -> Result<(), String> {
        settings.current_node_id = git::choose_node(pairs).map(|node| node.id.clone());
        if settings.current_node_id.is_none() {
            return Err("没有通过真实 Git 检测的可用节点".into());
        }
        Ok(())
    }

    pub fn set_route_scope(&self, scope: RouteScope) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        let routes = self.routes()?;
        settings.route_scope = scope;
        if scope == RouteScope::Allowlist && settings.acceleration_enabled && routes.is_empty() {
            settings.acceleration_enabled = false;
            settings.current_node_id = None;
        }
        let pairs = self.node_pairs()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        self.snapshot()
    }

    pub fn add_route(&self, repository_url: &str) -> Result<AppSnapshot, String> {
        let normalized = normalize_repository_url(repository_url)?;
        let _guard = self.lock.lock();
        let mut routes = self.routes()?;
        let settings = self.settings()?;
        if settings.route_scope == RouteScope::Global {
            return Err("全局加速无需配置项目清单".into());
        }
        if routes
            .iter()
            .any(|route| route.repository_url == normalized)
        {
            return Err("该仓库已在清单中".into());
        }
        let previous_routes = routes.clone();
        routes.push(RouteEntry {
            id: Uuid::new_v4().to_string(),
            repository_url: normalized,
            created_at: Utc::now(),
        });
        let mut settings = settings;
        let pairs = self.node_pairs()?;
        atomic_write_json(&self.paths.routes, &routes)?;
        if let Err(error) = self.write_configuration(&mut settings, &pairs, &routes) {
            let _ = atomic_write_json(&self.paths.routes, &previous_routes);
            return Err(error);
        }
        self.snapshot()
    }

    pub fn validate_route(&self, repository_url: &str) -> Result<String, String> {
        self.validate_route_with(repository_url, git::probe_repository)
    }

    fn validate_route_with<F>(&self, repository_url: &str, mut probe: F) -> Result<String, String>
    where
        F: FnMut(&NodeDefinition, &str) -> Result<git::RepositoryProbeStatus, String>,
    {
        let normalized = normalize_repository_url(repository_url)?;
        {
            let _guard = self.lock.lock();
            if self.settings()?.route_scope == RouteScope::Global {
                return Err("全局加速无需配置项目清单".into());
            }
            if self
                .routes()?
                .iter()
                .any(|route| route.repository_url == normalized)
            {
                return Err("该仓库已在清单中".into());
            }
        }

        let mut candidates = self
            .node_pairs()?
            .into_iter()
            .filter(|(node, health)| {
                node.enabled && health.in_auto_pool && is_usable_health(health)
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, health)| git::health_score(health));
        if candidates.is_empty() {
            return Err("没有通过真实 Git 检测的可用节点，请先在线路检测中完成测速".into());
        }

        for (node, _) in &candidates {
            match probe(node, &normalized) {
                Ok(git::RepositoryProbeStatus::Readable) => return Ok(normalized),
                Ok(git::RepositoryProbeStatus::Empty) => {
                    return Err("仓库尚无提交，请完成首次提交后再加入加速清单".into());
                }
                Err(_) => {}
            }
        }
        Err("当前可用线路无法匿名读取该仓库，请确认它是公开仓库、检查地址或稍后重试".into())
    }

    pub fn delete_route(&self, route_id: &str) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut routes = self.routes()?;
        if !routes.iter().any(|route| route.id == route_id) {
            return Err("路由不存在".into());
        }
        let previous_routes = routes.clone();
        routes.retain(|route| route.id != route_id);
        let mut settings = self.settings()?;
        if settings.route_scope == RouteScope::Allowlist && routes.is_empty() {
            settings.acceleration_enabled = false;
            settings.current_node_id = None;
        }
        let pairs = self.node_pairs()?;
        atomic_write_json(&self.paths.routes, &routes)?;
        if let Err(error) = self.write_configuration(&mut settings, &pairs, &routes) {
            let _ = atomic_write_json(&self.paths.routes, &previous_routes);
            return Err(error);
        }
        self.snapshot()
    }

    fn write_configuration(
        &self,
        settings: &mut Settings,
        pairs: &[(NodeDefinition, HealthSummary)],
        routes: &[RouteEntry],
    ) -> Result<(), String> {
        let selected = settings.current_node_id.as_ref().and_then(|id| {
            pairs
                .iter()
                .find(|(node, health)| {
                    if &node.id != id || !node.enabled || !is_usable_health(health) {
                        return false;
                    }
                    health.in_auto_pool
                })
                .map(|(node, _)| node)
        });
        let content =
            git::build_config(settings, selected, routes, Some(&self.paths.trace_socket))?;
        let previous = fs::read(&self.paths.gitconfig).ok();
        let registered_before = git::include_registered(&self.paths.gitconfig);
        let preserve_external_trace = !settings.acceleration_enabled
            && git::external_trace2_target(&self.paths.gitconfig)?.is_some();
        if settings.acceleration_enabled {
            let mut candidate = NamedTempFile::new_in(&self.paths.root)
                .map_err(|error| format!("无法创建配置候选文件：{error}"))?;
            std::io::Write::write_all(&mut candidate, content.as_bytes())
                .map_err(|error| format!("无法写入配置候选：{error}"))?;
            candidate
                .as_file()
                .sync_all()
                .map_err(|error| format!("无法同步配置候选：{error}"))?;
            validate_configuration(
                candidate.path(),
                settings,
                selected,
                routes,
                ValidationEnvironment::ManagedConfigOnly,
            )?;
        }
        let _ = backup_file(
            &self.paths.gitconfig,
            &self.paths.backups,
            "gitboost.gitconfig",
        );
        atomic_write(&self.paths.gitconfig, content.as_bytes())?;
        if preserve_external_trace {
            if let Err(error) = git::unregister_include(&self.paths.gitconfig) {
                self.restore_git_state(previous.as_deref(), registered_before);
                return Err(error);
            }
        } else if settings.acceleration_enabled {
            if let Err(error) = git::register_include_last(&self.paths.gitconfig) {
                self.restore_git_state(previous.as_deref(), registered_before);
                return Err(error);
            }
        } else if registered_before {
            if let Err(error) = git::register_include(&self.paths.gitconfig) {
                self.restore_git_state(previous.as_deref(), registered_before);
                return Err(error);
            }
        }
        if settings.acceleration_enabled {
            if let Err(error) = validate_configuration(
                &self.paths.gitconfig,
                settings,
                selected,
                routes,
                ValidationEnvironment::EffectiveGit,
            ) {
                self.restore_git_state(previous.as_deref(), registered_before);
                return Err(format!("写入后的 Git 配置验证失败：{error}"));
            }
        }
        settings.last_applied_at = Some(Utc::now());
        if let Err(error) = atomic_write_json(&self.paths.settings, settings) {
            self.restore_git_state(previous.as_deref(), registered_before);
            return Err(format!("保存设置失败，Git 配置已回滚：{error}"));
        }
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!(
                "git configuration applied: enabled={}, scope={:?}",
                settings.acceleration_enabled, settings.route_scope
            ),
        );
        Ok(())
    }

    fn restore_git_state(&self, previous: Option<&[u8]>, registered_before: bool) {
        if let Some(bytes) = previous {
            let _ = atomic_write(&self.paths.gitconfig, bytes);
        }
        if registered_before {
            let _ = git::register_include(&self.paths.gitconfig);
        } else {
            let _ = git::unregister_include(&self.paths.gitconfig);
        }
    }

    pub fn update_settings(&self, minutes: u32, log_level: &str) -> Result<AppSnapshot, String> {
        if !HEALTH_CHECK_INTERVALS.contains(&minutes) {
            return Err("不支持的检测周期".into());
        }
        if !["error", "info", "debug"].contains(&log_level) {
            return Err("不支持的日志级别".into());
        }
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.health_check_minutes = minutes;
        settings.log_level = log_level.into();
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    pub fn update_launch_at_login(&self, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.launch_at_login = enabled;
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    pub fn set_mcp_enabled(&self, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.mcp_enabled = enabled;
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    pub fn set_usage_logging(&self, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.usage_logging_enabled = enabled;
        let pairs = self.node_pairs()?;
        let routes = self.routes()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            if enabled {
                "sanitized usage audit enabled"
            } else {
                "sanitized usage audit disabled"
            },
        );
        self.snapshot()
    }

    pub fn refresh_registered_configuration(&self) -> Result<(), String> {
        let _guard = self.lock.lock();
        let registered = git::include_registered(&self.paths.gitconfig);
        let mut settings = self.settings()?;
        let routes = self.routes()?;
        let repaired_missing_consent =
            settings.acceleration_enabled && settings.consent_acknowledged_at.is_none();
        if repaired_missing_consent {
            settings.acceleration_enabled = false;
            settings.current_node_id = None;
        }
        let repaired_empty_allowlist = settings.acceleration_enabled
            && settings.route_scope == RouteScope::Allowlist
            && routes.is_empty();
        if repaired_empty_allowlist {
            settings.acceleration_enabled = false;
            settings.current_node_id = None;
        }
        let repaired_disabled_state =
            !settings.acceleration_enabled && settings.current_node_id.is_some();
        if repaired_disabled_state {
            settings.current_node_id = None;
        }
        if !registered && !settings.acceleration_enabled {
            if repaired_missing_consent || repaired_empty_allowlist || repaired_disabled_state {
                atomic_write_json(&self.paths.settings, &settings)?;
            }
            return Ok(());
        }
        let pairs = self.node_pairs()?;
        if settings.acceleration_enabled && self.select_current(&mut settings, &pairs).is_err() {
            settings.acceleration_enabled = false;
            settings.current_node_id = None;
        }
        self.write_configuration(&mut settings, &pairs, &routes)?;
        Ok(())
    }

    pub fn restore_git_config(&self) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.acceleration_enabled = false;
        settings.current_node_id = None;
        let content = git::build_config(&settings, None, &[], Some(&self.paths.trace_socket))?;
        let previous = fs::read(&self.paths.gitconfig).ok();
        let registered_before = git::include_registered(&self.paths.gitconfig);
        let _ = backup_file(
            &self.paths.gitconfig,
            &self.paths.backups,
            "before-restore.gitconfig",
        );
        atomic_write(&self.paths.gitconfig, content.as_bytes())?;
        if let Err(error) = git::unregister_include(&self.paths.gitconfig) {
            self.restore_git_state(previous.as_deref(), registered_before);
            return Err(error);
        }
        settings.last_applied_at = Some(Utc::now());
        if let Err(error) = atomic_write_json(&self.paths.settings, &settings) {
            self.restore_git_state(previous.as_deref(), registered_before);
            return Err(format!("保存设置失败，Git 配置已回滚：{error}"));
        }
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            "GitBoost include removed and managed rules restored to direct",
        );
        self.snapshot()
    }

    pub fn clear_logs(&self) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        clear_logs(&self.paths.logs)?;
        self.snapshot()
    }

    pub fn usage_log(&self) -> Result<UsageLogSnapshot, String> {
        let settings = self.settings()?;
        let events = {
            let _guard = self.lock.lock();
            load_usage_events(&self.paths.logs, 200)?
        };
        Ok(UsageLogSnapshot {
            enabled: settings.usage_logging_enabled,
            listening: self.usage_listening.load(Ordering::Relaxed),
            configured: settings.usage_logging_enabled
                && git::include_registered(&self.paths.gitconfig),
            events,
            storage_path: self.paths.logs.join("usage.jsonl").display().to_string(),
        })
    }

    pub fn observe_git_operation(&self, trace: CompletedTrace) -> Result<Option<String>, String> {
        let _guard = self.lock.lock();
        let settings = self.settings()?;
        let nodes = self.nodes()?;
        let matched_node = match_rewrite_node(&trace.effective_url, &nodes);
        let failed_node_id = matched_node
            .filter(|node| {
                trace.exit_code != 0
                    && supports_automatic_failover(&trace.command)
                    && settings.acceleration_enabled
                    && settings.current_node_id.as_deref() == Some(node.id.as_str())
            })
            .map(|node| node.id.clone());
        let (route, node_name) = if let Some(node) = matched_node {
            (UsageRoute::Accelerated, Some(node.name.clone()))
        } else if url::Url::parse(&trace.effective_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
        {
            (UsageRoute::Direct, None)
        } else {
            (UsageRoute::Other, None)
        };
        let repository = trace
            .original_url
            .as_deref()
            .and_then(|url| sanitize_repository(url, &nodes))
            .or_else(|| sanitize_repository(&trace.effective_url, &nodes))
            .unwrap_or_else(|| "当前仓库".into());
        let connection_host = url::Url::parse(&trace.effective_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| "未知主机".into());
        if settings.usage_logging_enabled {
            if let Err(error) = append_usage_event(
                &self.paths.logs,
                &UsageEvent {
                    id: Uuid::new_v4().to_string(),
                    occurred_at: trace.occurred_at,
                    command: trace.command,
                    repository,
                    route,
                    node_name,
                    connection_host,
                    succeeded: trace.exit_code == 0,
                    exit_code: trace.exit_code,
                    duration_ms: trace.duration_ms,
                },
            ) {
                let _ = append_log(
                    &self.paths.logs,
                    "ERROR",
                    &format!("usage audit write failed: {error}"),
                );
            }
        }
        Ok(failed_node_id)
    }

    pub fn diagnostics(&self, repository_path: Option<&Path>) -> Result<DiagnosticReport, String> {
        let settings = self.settings()?;
        let routes = self.routes()?;
        let original_url = routes
            .first()
            .map(|route| route.repository_url.clone())
            .unwrap_or_else(|| TEST_REPOSITORY.into());
        let (effective, effective_error) =
            match git::effective_urls(&self.paths.gitconfig, &original_url) {
                Ok(urls) => (Some(urls), None),
                Err(error) => (None, Some(error)),
            };
        let (explicit, repository_error) = match repository_path {
            Some(path) => match git::explicit_push_url(path) {
                Ok(value) => (value, None),
                Err(error) => (None, Some(error)),
            },
            None => (None, None),
        };
        let conflict_repository = repository_path.filter(|_| repository_error.is_none());
        let (conflicts, conflict_scan_error) =
            match git::find_conflicts(&self.paths.gitconfig, conflict_repository) {
                Ok(conflicts) => (conflicts, None),
                Err(error) => (vec![], Some(git::redact_path(&error))),
            };
        let repository_error = repository_error.map(|error| git::redact_path(&error));
        let mut warnings = vec![];
        if explicit.is_some() {
            warnings.push(
                "该仓库设置了显式 pushurl，Git 可能忽略 pushInsteadOf；不要将其视为安全直连。"
                    .into(),
            );
        }
        if let Some(error) = repository_error.as_deref() {
            warnings.push(format!("仓库检查失败：{}。无法确认显式 pushurl。", error));
        }
        if let Some(error) = conflict_scan_error.as_deref() {
            warnings.push(format!(
                "URL 重写冲突检查失败：{}。当前不能确认是否存在冲突。",
                error
            ));
        }
        if let Some(error) = effective_error.as_deref() {
            warnings.push(format!(
                "有效 fetch/push 地址解析失败：{}。",
                git::redact_path(error)
            ));
        }
        if !conflicts.is_empty() {
            warnings
                .push("发现其他 URL 重写规则。GitBoost 没有覆盖它们，请按来源逐项检查。".into());
        }
        if settings.acceleration_enabled && settings.route_scope == RouteScope::Global {
            warnings.push(
                "全局加速无法自动识别私有仓库；所有 GitHub HTTPS 读取都会经过当前节点。".into(),
            );
        }
        let generated = Utc::now();
        let fetch = effective
            .as_ref()
            .map(|(fetch, _)| git::sanitize_url(fetch));
        let push = effective.as_ref().map(|(_, push)| git::sanitize_url(push));
        let git_path = git::git_path().map(|path| git::redact_path(&path));
        let config_path = git::redact_path(&self.paths.gitconfig.display().to_string());
        let include_registered = git::include_registered(&self.paths.gitconfig);
        let conflict_text = if let Some(error) = conflict_scan_error.as_deref() {
            format!("- 检查失败：{error}")
        } else if conflicts.is_empty() {
            "- 无".into()
        } else {
            conflicts
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let explicit_text = if let Some(error) = repository_error.as_deref() {
            format!("检查失败：{error}")
        } else {
            explicit.as_deref().unwrap_or("未检测到").into()
        };
        let report_text = format!(
            "GitBoost 诊断报告\n生成时间: {generated}\nGit: {}\nGit 路径: {}\n配置: {}\ninclude: {}\n测试 URL: {}\nfetch: {}\npush: {}\n显式 pushurl: {}\n冲突:\n{}\n警告:\n{}",
            git::git_version().unwrap_or_else(|| "未找到".into()), git_path.as_deref().unwrap_or("未找到"), config_path, include_registered, original_url,
            fetch.as_deref().unwrap_or("无法解析"), push.as_deref().unwrap_or("无法解析"), explicit_text,
            conflict_text,
            if warnings.is_empty() { "- 无".into() } else { warnings.iter().map(|line| format!("- {line}")).collect::<Vec<_>>().join("\n") },
        );
        Ok(DiagnosticReport {
            generated_at: generated,
            git_path,
            git_version: git::git_version(),
            config_path,
            include_registered,
            conflicts,
            conflict_scan_error,
            original_url,
            fetch_url: fetch,
            push_url: push,
            explicit_push_url: explicit,
            repository_error,
            warnings,
            report_text,
        })
    }
}

#[derive(Clone, Copy)]
enum ValidationEnvironment {
    ManagedConfigOnly,
    EffectiveGit,
}

fn validate_configuration(
    config_path: &Path,
    settings: &Settings,
    selected: Option<&NodeDefinition>,
    routes: &[RouteEntry],
    environment: ValidationEnvironment,
) -> Result<(), String> {
    let node = selected.ok_or_else(|| "没有通过检测的可用节点".to_string())?;
    let mut checks = Vec::new();
    match settings.route_scope {
        RouteScope::Global => {
            let suffix = TEST_REPOSITORY
                .strip_prefix("https://github.com/")
                .expect("内置测试地址始终是 GitHub HTTPS 地址");
            checks.push((
                TEST_REPOSITORY.to_string(),
                format!("{}{suffix}", node.rewrite_base),
            ));
        }
        RouteScope::Allowlist => {
            if routes.is_empty() {
                return Err("仅加速清单为空".into());
            }
            for route in routes {
                let suffix = route
                    .repository_url
                    .strip_prefix("https://github.com/")
                    .ok_or_else(|| "清单路由不是 GitHub HTTPS 地址".to_string())?;
                checks.push((
                    route.repository_url.clone(),
                    format!("{}{suffix}", node.rewrite_base),
                ));
            }
            let mut unlisted =
                "https://github.com/gitboost-validation/not-in-allowlist.git".to_string();
            while routes.iter().any(|route| {
                route
                    .repository_url
                    .strip_suffix(".git")
                    .is_some_and(|prefix| unlisted.starts_with(prefix))
            }) {
                unlisted.insert_str(unlisted.len() - 4, "-check");
            }
            checks.push((unlisted.clone(), unlisted));
        }
    }

    for (original, expected_fetch) in checks {
        let (fetch, push) = match environment {
            ValidationEnvironment::ManagedConfigOnly => {
                git::effective_urls_isolated(config_path, &original)
            }
            ValidationEnvironment::EffectiveGit => git::effective_urls(config_path, &original),
        }?;
        if fetch != expected_fetch {
            return Err(format!("配置未正确处理 fetch：{original}"));
        }
        if push != original {
            return Err(format!("配置未保持 push 直连：{original}"));
        }
    }
    Ok(())
}

fn sanitize_repository(raw: &str, nodes: &[NodeDefinition]) -> Option<String> {
    let logical = match_rewrite_node(raw, nodes)
        .and_then(|node| raw.strip_prefix(&node.rewrite_base))
        .map(|suffix| format!("https://github.com/{suffix}"))
        .unwrap_or_else(|| raw.to_owned());
    let mut url = url::Url::parse(&logical).ok()?;
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

fn match_rewrite_node<'a>(raw: &str, nodes: &'a [NodeDefinition]) -> Option<&'a NodeDefinition> {
    nodes
        .iter()
        .filter(|node| raw.starts_with(&node.rewrite_base))
        .max_by_key(|node| node.rewrite_base.len())
}

fn redact_import(input: &str) -> String {
    if input.contains("://") {
        git::sanitize_url(input)
    } else {
        "[无法解析的地址]".into()
    }
}

fn is_usable_health(health: &HealthSummary) -> bool {
    matches!(health.status, NodeStatus::Available | NodeStatus::Slow)
}

fn supports_automatic_failover(command: &str) -> bool {
    matches!(command, "clone" | "fetch" | "pull" | "ls-remote")
}

fn select_auto_pool(
    nodes: &[NodeDefinition],
    health: &HashMap<String, HealthSummary>,
    candidate_ids: &HashSet<String>,
    required_id: Option<&str>,
) -> HashSet<String> {
    let mut candidates: Vec<_> = nodes
        .iter()
        .filter_map(|node| {
            let summary = health.get(&node.id)?;
            (node.enabled && candidate_ids.contains(&node.id) && is_usable_health(summary))
                .then_some((node, summary))
        })
        .collect();
    candidates.sort_by_key(|(_, summary)| git::health_score(summary));
    let mut selected = HashSet::new();
    if let Some(required_id) = required_id {
        if candidates.iter().any(|(node, _)| node.id == required_id) {
            selected.insert(required_id.to_string());
        }
    }
    for (node, _) in candidates {
        if selected.len() == AUTO_POOL_NODE_TARGET {
            break;
        }
        selected.insert(node.id.clone());
    }
    selected
}

fn test_node_pool<F>(
    nodes: &[NodeDefinition],
    old_health: &HashMap<String, HealthSummary>,
    test: F,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<NodePoolTestResult, String>
where
    F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary + Sync,
{
    let usable_count = Mutex::new(0usize);
    on_progress(0, AUTO_POOL_NODE_TARGET);
    let test_and_report = |node: &NodeDefinition, previous: &HealthSummary| {
        let summary = test(node, previous);
        if is_usable_health(&summary) {
            let mut usable = usable_count.lock();
            *usable += 1;
            on_progress(*usable, AUTO_POOL_NODE_TARGET);
        }
        summary
    };
    let maintained: Vec<NodeDefinition> = nodes
        .iter()
        .filter(|node| {
            node.enabled
                && old_health
                    .get(&node.id)
                    .is_some_and(|summary| summary.in_auto_pool)
        })
        .take(AUTO_POOL_NODE_TARGET)
        .cloned()
        .collect();
    let selected: HashSet<String> = maintained.iter().map(|node| node.id.clone()).collect();
    let mut tested = test_nodes_bounded(&maintained, old_health, &test_and_report, |_, _| {})?;

    let mut usable = *usable_count.lock();
    if usable >= AUTO_POOL_NODE_TARGET {
        let pool_ids = tested
            .iter()
            .filter(|(_, summary)| is_usable_health(summary))
            .map(|(node_id, _)| node_id.clone())
            .collect();
        return Ok(NodePoolTestResult { tested, pool_ids });
    }

    let discovery: Vec<NodeDefinition> = nodes
        .iter()
        .filter(|node| node.enabled && !selected.contains(&node.id))
        .cloned()
        .collect();
    let mut cursor = 0;
    while usable < AUTO_POOL_NODE_TARGET && cursor < discovery.len() {
        let remaining = AUTO_POOL_NODE_TARGET - usable;
        let batch_len = NODE_TEST_CONCURRENCY
            .min(remaining)
            .min(discovery.len() - cursor);
        let batch = &discovery[cursor..cursor + batch_len];
        let batch_results = test_nodes_bounded(batch, old_health, &test_and_report, |_, _| {})?;
        usable = *usable_count.lock();
        tested.extend(batch_results);
        cursor += batch_len;
    }
    let pool_ids = tested
        .iter()
        .filter(|(_, summary)| is_usable_health(summary))
        .map(|(node_id, _)| node_id.clone())
        .collect();
    Ok(NodePoolTestResult { tested, pool_ids })
}

fn test_nodes_bounded<F>(
    nodes: &[NodeDefinition],
    old_health: &HashMap<String, HealthSummary>,
    test: F,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<Vec<(String, HealthSummary)>, String>
where
    F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary + Sync,
{
    let next = AtomicUsize::new(0);
    let completed = Mutex::new(0usize);
    let tested = Mutex::new(Vec::with_capacity(nodes.len()));
    on_progress(0, nodes.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..NODE_TEST_CONCURRENCY.min(nodes.len()))
            .map(|_| {
                let next = &next;
                let completed = &completed;
                let tested = &tested;
                let test = &test;
                let on_progress = &on_progress;
                scope.spawn(move || loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(node) = nodes.get(index) else {
                        break;
                    };
                    let previous = old_health.get(&node.id).cloned().unwrap_or_default();
                    let summary = test(node, &previous);
                    tested.lock().push((index, node.id.clone(), summary));
                    let mut completed = completed.lock();
                    *completed += 1;
                    on_progress(*completed, nodes.len());
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .map_err(|_| "节点检测线程异常结束".to_string())?;
        }
        Ok::<(), String>(())
    })?;
    let mut tested = tested.into_inner();
    tested.sort_by_key(|(index, _, _)| *index);
    let tested = tested
        .into_iter()
        .map(|(_, node_id, summary)| (node_id, summary))
        .collect();
    Ok(tested)
}

fn normalize_system_urls(urls: Vec<String>) -> Result<Vec<String>, String> {
    if urls.is_empty() {
        return Err("系统节点目录不能为空".into());
    }
    if urls.len() > MAX_SYSTEM_NODES {
        return Err(format!("系统节点目录最多包含 {MAX_SYSTEM_NODES} 个地址"));
    }
    let mut normalized = Vec::with_capacity(urls.len());
    let mut known = HashSet::new();
    for input in urls {
        let url = normalize_rewrite_base(&input)
            .map_err(|reason| format!("系统节点地址无效：{reason}"))?;
        if known.insert(url.clone()) {
            normalized.push(url);
        }
    }
    if normalized.is_empty() {
        return Err("系统节点目录没有有效地址".into());
    }
    Ok(normalized)
}

fn parse_system_catalog(bytes: &[u8]) -> Result<Vec<String>, String> {
    let catalog: EncryptedSystemCatalog =
        serde_json::from_slice(bytes).map_err(|error| format!("系统节点目录格式错误：{error}"))?;
    if catalog.version != 1 {
        return Err(format!("不支持的系统节点目录版本：{}", catalog.version));
    }
    let nonce = BASE64
        .decode(catalog.nonce)
        .map_err(|_| "系统节点目录 nonce 无效".to_string())?;
    if nonce.len() != 12 {
        return Err("系统节点目录 nonce 长度无效".into());
    }
    let ciphertext = BASE64
        .decode(catalog.ciphertext)
        .map_err(|_| "系统节点目录密文无效".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(&SYSTEM_NODE_CATALOG_KEY)
        .map_err(|_| "系统节点目录密钥无效".to_string())?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| "系统节点目录解密失败".to_string())?;
    let urls: Vec<String> = serde_json::from_slice(&plaintext)
        .map_err(|error| format!("系统节点目录内容无效：{error}"))?;
    normalize_system_urls(urls)
}

fn fetch_system_catalog() -> Result<Vec<u8>, String> {
    fetch_system_catalog_from(&SYSTEM_NODE_CATALOG_URLS, fetch_system_catalog_url)
}

fn fetch_system_catalog_from<F>(urls: &[&str], mut fetch: F) -> Result<Vec<u8>, String>
where
    F: FnMut(&str) -> Result<Vec<u8>, String>,
{
    let mut errors = Vec::with_capacity(urls.len());
    for url in urls {
        match fetch(url) {
            Ok(output) => return Ok(output),
            Err(error) => errors.push(error),
        }
    }
    Err(format!("无法更新系统节点：{}", errors.join("；")))
}

fn fetch_system_catalog_url(url: &str) -> Result<Vec<u8>, String> {
    http::fetch_limited(
        url,
        Duration::from_secs(4),
        Duration::from_secs(10),
        CATALOG_MAX_BYTES,
    )
    .map_err(|error| format!("{url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rerun_with_isolated_git(test_name: &str, marker: &str) -> bool {
        if std::env::var_os(marker).is_some() {
            return false;
        }
        let sandbox = tempfile::tempdir().unwrap();
        let home = sandbox.path().join("home");
        fs::create_dir(&home).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(marker, "1")
            .env("GIT_CONFIG_GLOBAL", sandbox.path().join("global.gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home)
            .status()
            .unwrap();
        assert!(status.success(), "isolated Git test failed: {test_name}");
        true
    }

    fn encrypted_catalog(urls: &[&str]) -> Vec<u8> {
        let nonce = [0u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&SYSTEM_NODE_CATALOG_KEY).unwrap();
        let plaintext = serde_json::to_vec(urls).unwrap();
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .unwrap();
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "nonce": BASE64.encode(nonce),
            "ciphertext": BASE64.encode(ciphertext),
        }))
        .unwrap()
    }

    fn configure_automatic_nodes(core: &AppCore, nodes: &[NodeDefinition], current_node_id: &str) {
        atomic_write_json(&core.paths.nodes, &nodes).unwrap();
        let health: HashMap<String, HealthSummary> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                (
                    node.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        success_count: 2,
                        attempt_count: 2,
                        median_latency_ms: Some(20 + index as u64 * 10),
                        checked_at: Some(Utc::now()),
                        ..HealthSummary::default()
                    },
                )
            })
            .collect();
        atomic_write_json(&core.paths.health, &health).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            current_node_id: Some(current_node_id.into()),
            usage_logging_enabled: false,
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();
    }

    fn custom_node(id: &str, name: &str, rewrite_base: &str) -> NodeDefinition {
        NodeDefinition {
            id: id.into(),
            name: name.into(),
            rewrite_base: rewrite_base.into(),
            enabled: true,
            built_in: false,
        }
    }

    #[test]
    fn seeds_fastgit_and_safe_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let snapshot = core.snapshot().unwrap();
        assert_eq!(
            snapshot.nodes[0].node.rewrite_base,
            "https://fastgit.cc/https://github.com/"
        );
        assert_eq!(snapshot.settings.route_scope, RouteScope::Allowlist);
        assert_eq!(
            snapshot.settings.health_check_minutes,
            DEFAULT_HEALTH_CHECK_MINUTES
        );
        assert!(!snapshot.settings.acceleration_enabled);
        assert!(snapshot.settings.current_node_id.is_none());
        assert!(snapshot.nodes[0].node.built_in);
        assert_eq!(snapshot.nodes[0].node.id, FASTGIT_REWRITE_BASE);
        let cached: Vec<String> = load_json(&directory.path().join("system-nodes.json")).unwrap();
        assert_eq!(cached, vec![FASTGIT_REWRITE_BASE]);
    }

    #[test]
    fn deletes_legacy_health_data_during_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        atomic_write_json(
            &core.paths.health,
            &serde_json::json!({
                FASTGIT_REWRITE_BASE: {
                    "status": "available",
                    "successCount": 2,
                    "attemptCount": 2,
                    "medianLatencyMs": 100,
                    "consecutiveFailures": 0,
                    "checkedAt": null,
                    "failureReason": null
                }
            }),
        )
        .unwrap();
        drop(core);

        let restarted = AppCore::new(directory.path().to_path_buf()).unwrap();

        assert!(restarted.health().unwrap().is_empty());
    }

    #[test]
    fn resets_legacy_health_check_interval_during_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let mut settings = core.settings().unwrap();
        settings.health_check_minutes = 30;
        settings.log_level = "debug".into();
        atomic_write_json(&core.paths.settings, &settings).unwrap();
        drop(core);

        let restarted = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = restarted.settings().unwrap();

        assert_eq!(settings.health_check_minutes, DEFAULT_HEALTH_CHECK_MINUTES);
        assert_eq!(settings.log_level, "debug");
    }

    #[test]
    fn accepts_supported_health_check_intervals() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();

        for minutes in [60, 8 * 60, 24 * 60, 7 * 24 * 60, 30 * 24 * 60] {
            let snapshot = core.update_settings(minutes, "info").unwrap();
            assert_eq!(snapshot.settings.health_check_minutes, minutes);
        }
        assert!(core.update_settings(2 * 60, "info").is_err());
    }

    #[test]
    fn quarantines_and_rebuilds_corrupt_state_files() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        atomic_write(&core.paths.settings, b"{").unwrap();
        atomic_write(&core.paths.health, b"[]").unwrap();
        atomic_write(&core.paths.routes, b"{}").unwrap();
        drop(core);

        let restarted = AppCore::new(directory.path().to_path_buf()).unwrap();

        assert!(!restarted.settings().unwrap().acceleration_enabled);
        assert!(restarted.health().unwrap().is_empty());
        assert!(restarted.routes().unwrap().is_empty());
        let backup_names = fs::read_dir(&restarted.paths.backups)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(backup_names
            .iter()
            .any(|name| name.ends_with("corrupt-settings.json")));
        assert!(backup_names
            .iter()
            .any(|name| name.ends_with("corrupt-health.json")));
        assert!(backup_names
            .iter()
            .any(|name| name.ends_with("corrupt-routes.json")));
    }

    #[test]
    fn validates_every_allowlist_route_and_an_unlisted_repository() {
        const MARKER: &str = "GITBOOST_VALIDATE_ALL_ROUTES_CHILD";
        if rerun_with_isolated_git(
            "core::tests::validates_every_allowlist_route_and_an_unlisted_repository",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = NodeDefinition::fastgit();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Allowlist,
            current_node_id: Some(node.id.clone()),
            ..Settings::default()
        };
        let routes = ["openai/codex", "octocat/Hello-World"]
            .into_iter()
            .map(|repository| RouteEntry {
                id: repository.into(),
                repository_url: format!("https://github.com/{repository}.git"),
                created_at: Utc::now(),
            })
            .collect::<Vec<_>>();
        let complete = git::build_config(
            &settings,
            Some(&node),
            &routes,
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, complete.as_bytes()).unwrap();
        validate_configuration(
            &core.paths.gitconfig,
            &settings,
            Some(&node),
            &routes,
            ValidationEnvironment::ManagedConfigOnly,
        )
        .unwrap();

        let incomplete = git::build_config(
            &settings,
            Some(&node),
            &routes[..1],
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, incomplete.as_bytes()).unwrap();
        let error = validate_configuration(
            &core.paths.gitconfig,
            &settings,
            Some(&node),
            &routes,
            ValidationEnvironment::ManagedConfigOnly,
        )
        .unwrap_err();
        assert!(error.contains("octocat/Hello-World"));
    }

    #[test]
    fn configuration_update_isolates_the_candidate_but_validates_effective_git_rules() {
        const MARKER: &str = "GITBOOST_VALIDATION_ENVIRONMENT_CHILD";
        if rerun_with_isolated_git(
            "core::tests::configuration_update_isolates_the_candidate_but_validates_effective_git_rules",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let route = RouteEntry {
            id: "route".into(),
            repository_url: "https://github.com/openai/codex.git".into(),
            created_at: Utc::now(),
        };
        let replacement = NodeDefinition {
            id: "replacement".into(),
            name: "Replacement".into(),
            rewrite_base: "https://new.example/https://github.com/".into(),
            enabled: true,
            built_in: false,
        };
        let settings = Settings {
            acceleration_enabled: true,
            current_node_id: Some(replacement.id.clone()),
            consent_acknowledged_at: Some(Utc::now()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.routes, &vec![route.clone()]).unwrap();
        atomic_write_json(&core.paths.nodes, &vec![replacement.clone()]).unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([(
                replacement.id.clone(),
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 1,
                    attempt_count: 1,
                    ..HealthSummary::default()
                },
            )]),
        )
        .unwrap();
        atomic_write_json(&core.paths.settings, &settings).unwrap();

        let old_config = git::build_config(
            &Settings {
                acceleration_enabled: true,
                current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
                ..Settings::default()
            },
            Some(&NodeDefinition::fastgit()),
            std::slice::from_ref(&route),
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, old_config.as_bytes()).unwrap();
        git::register_include(&core.paths.gitconfig).unwrap();

        core.refresh_registered_configuration().unwrap();
        let (fetch, push) =
            git::effective_urls(&core.paths.gitconfig, &route.repository_url).unwrap();
        assert_eq!(
            fetch,
            "https://new.example/https://github.com/openai/codex.git"
        );
        assert_eq!(push, route.repository_url);

        let previous_config = fs::read(&core.paths.gitconfig).unwrap();
        let conflict = std::process::Command::new("git")
            .args([
                "config",
                "--global",
                "url.https://push-proxy.example/https://github.com/openai/codex.pushInsteadOf",
                "https://github.com/openai/codex",
            ])
            .output()
            .unwrap();
        assert!(conflict.status.success());
        let error = core.refresh_registered_configuration().unwrap_err();
        assert!(error.contains("配置未保持 push 直连"));
        assert_eq!(fs::read(&core.paths.gitconfig).unwrap(), previous_config);

        git::unregister_include(&core.paths.gitconfig).unwrap();
    }

    #[test]
    fn startup_reconciles_missing_include_and_rolls_back_on_state_write_failure() {
        const MARKER: &str = "GITBOOST_STARTUP_RECONCILE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::startup_reconciles_missing_include_and_rolls_back_on_state_write_failure",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let route = RouteEntry {
            id: "route".into(),
            repository_url: "https://github.com/openai/codex.git".into(),
            created_at: Utc::now(),
        };
        atomic_write_json(&core.paths.routes, &vec![route]).unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([(
                FASTGIT_REWRITE_BASE.to_string(),
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 1,
                    attempt_count: 1,
                    ..HealthSummary::default()
                },
            )]),
        )
        .unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
            consent_acknowledged_at: Some(Utc::now()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();

        assert!(!git::include_registered(&core.paths.gitconfig));
        core.refresh_registered_configuration().unwrap();
        assert!(git::include_registered(&core.paths.gitconfig));
        let (fetch, push) =
            git::effective_urls(&core.paths.gitconfig, "https://github.com/openai/codex.git")
                .unwrap();
        assert_eq!(fetch, format!("{FASTGIT_REWRITE_BASE}openai/codex.git"));
        assert_eq!(push, "https://github.com/openai/codex.git");

        atomic_write(&core.paths.gitconfig, b"# externally changed\n").unwrap();
        core.refresh_registered_configuration().unwrap();
        assert!(fs::read_to_string(&core.paths.gitconfig)
            .unwrap()
            .contains(FASTGIT_REWRITE_BASE));

        let previous_config = fs::read(&core.paths.gitconfig).unwrap();
        let previous_settings = fs::read(&core.paths.settings).unwrap();
        let mut next_settings = core.settings().unwrap();
        let pairs = core.node_pairs().unwrap();
        let routes = core.routes().unwrap();
        fs::remove_file(&core.paths.settings).unwrap();
        fs::create_dir(&core.paths.settings).unwrap();
        let error = core
            .write_configuration(&mut next_settings, &pairs, &routes)
            .unwrap_err();
        assert!(error.contains("Git 配置已回滚"));
        assert_eq!(fs::read(&core.paths.gitconfig).unwrap(), previous_config);
        assert!(git::include_registered(&core.paths.gitconfig));
        fs::remove_dir(&core.paths.settings).unwrap();
        atomic_write(&core.paths.settings, &previous_settings).unwrap();
        git::unregister_include(&core.paths.gitconfig).unwrap();
    }

    #[test]
    fn system_catalog_is_encrypted_and_authenticated() {
        let bytes = include_bytes!("../../nodes.enc.json");
        let urls = parse_system_catalog(bytes).unwrap();
        assert!(!urls.is_empty());
        assert_eq!(urls[0], FASTGIT_REWRITE_BASE);
        assert!(!String::from_utf8_lossy(bytes).contains("fastgit.cc"));

        let duplicate = encrypted_catalog(&[
            "https://fastgit.cc",
            "https://fastgit.cc/https://github.com/",
        ]);
        assert_eq!(
            parse_system_catalog(&duplicate).unwrap(),
            vec![FASTGIT_REWRITE_BASE]
        );
        assert!(parse_system_catalog(&encrypted_catalog(&[])).is_err());
        assert!(parse_system_catalog(&encrypted_catalog(&["http://proxy.example"])).is_err());

        let mut catalog: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        catalog["ciphertext"] = serde_json::Value::String("AAAA".into());
        assert!(parse_system_catalog(&serde_json::to_vec(&catalog).unwrap()).is_err());
        assert!(parse_system_catalog(br#"["https://fastgit.cc"]"#).is_err());
    }

    #[test]
    fn system_catalog_tries_each_fallback_in_order() {
        let mut requested = Vec::new();
        let output = fetch_system_catalog_from(&SYSTEM_NODE_CATALOG_URLS, |url| {
            requested.push(url.to_string());
            if url.contains("cdn.jsdmirror.com") {
                Ok(include_bytes!("../../nodes.enc.json").to_vec())
            } else {
                Err("catalog unavailable".into())
            }
        })
        .unwrap();

        assert_eq!(output, include_bytes!("../../nodes.enc.json"));
        assert_eq!(requested, SYSTEM_NODE_CATALOG_URLS);
    }

    #[test]
    fn removing_current_system_node_immediately_writes_direct_git_config() {
        const MARKER: &str = "GITBOOST_REMOVE_CURRENT_SYSTEM_NODE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::removing_current_system_node_immediately_writes_direct_git_config",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();
        let accelerated = git::build_config(
            &settings,
            Some(&NodeDefinition::fastgit()),
            &[],
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, accelerated.as_bytes()).unwrap();
        assert!(accelerated.contains(FASTGIT_REWRITE_BASE));

        let catalog = encrypted_catalog(&["https://proxy.example"]);
        let update = core.apply_system_catalog(&catalog).unwrap();
        assert!(update.changed);
        assert!(update.recovery_applied_at.is_some());

        let gitconfig = fs::read_to_string(&core.paths.gitconfig).unwrap();
        assert!(!gitconfig.contains(FASTGIT_REWRITE_BASE));
        assert!(gitconfig.contains("Acceleration is disabled; GitHub remains direct."));
        let (fetch, push) = git::effective_urls(&core.paths.gitconfig, TEST_REPOSITORY).unwrap();
        assert_eq!(fetch, TEST_REPOSITORY);
        assert_eq!(push, TEST_REPOSITORY);
        let persisted = core.settings().unwrap();
        assert!(!persisted.acceleration_enabled);
        assert!(persisted.current_node_id.is_none());

        drop(core);
        let restarted = AppCore::new(directory.path().to_path_buf()).unwrap();
        restarted.refresh_registered_configuration().unwrap();
        assert!(!restarted.settings().unwrap().acceleration_enabled);
    }

    #[test]
    fn catalog_refresh_resumes_acceleration_after_testing_replacement_nodes() {
        const MARKER: &str = "GITBOOST_CATALOG_REFRESH_RESUME_CHILD";
        if rerun_with_isolated_git(
            "core::tests::catalog_refresh_resumes_acceleration_after_testing_replacement_nodes",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();
        let accelerated = git::build_config(
            &settings,
            Some(&NodeDefinition::fastgit()),
            &[],
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, accelerated.as_bytes()).unwrap();

        let changed = core
            .apply_system_catalog_update(&encrypted_catalog(&["https://proxy.example"]), |_, _| {
                HealthSummary {
                    status: NodeStatus::Available,
                    success_count: 1,
                    attempt_count: 1,
                    checked_at: Some(Utc::now()),
                    ..HealthSummary::default()
                }
            })
            .unwrap();

        assert!(changed);
        let persisted = core.settings().unwrap();
        assert!(persisted.acceleration_enabled);
        assert_eq!(
            persisted.current_node_id.as_deref(),
            Some("https://proxy.example/https://github.com/")
        );
        let gitconfig = fs::read_to_string(&core.paths.gitconfig).unwrap();
        assert!(gitconfig.contains("https://proxy.example/https://github.com/"));
    }

    #[test]
    fn full_node_tests_allow_only_one_active_run() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();

        let active = core.full_node_test_lock.lock();
        assert_eq!(
            core.test_all_nodes_with_progress(|_, _| {})
                .err()
                .as_deref(),
            Some("线路检测正在进行，请稍后再试")
        );
        drop(active);
        assert!(core.full_node_test_lock.try_lock().is_some());
    }

    #[test]
    fn blocked_background_test_does_not_report_progress() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let active = core.full_node_test_lock.lock();
        let progress_reported = AtomicBool::new(false);

        let result = core.test_background_nodes_with_progress(|_, _| {
            progress_reported.store(true, Ordering::Relaxed);
        });

        assert!(result.is_err());
        assert!(!progress_reported.load(Ordering::Relaxed));
        drop(active);
    }

    #[test]
    fn user_full_node_test_propagates_the_active_run_failure() {
        use std::{sync::mpsc, time::Duration};

        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let active = core.full_node_test_lock.lock();
        *core.full_node_test_result.lock() = Some(Err("检测结果写入失败".into()));
        let (joined, join_finished) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                joined
                    .send(core.test_all_nodes_or_join_with_progress(|_, _| {}))
                    .unwrap();
            });
            assert!(join_finished
                .recv_timeout(Duration::from_millis(30))
                .is_err());
            drop(active);
            assert_eq!(
                join_finished
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
                    .unwrap_err(),
                "检测结果写入失败"
            );
        });
    }

    #[test]
    fn catalog_refresh_prunes_removed_health_without_changing_node_test_state() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let mut health: HashMap<String, HealthSummary> = HashMap::new();
        health.insert(
            FASTGIT_REWRITE_BASE.into(),
            HealthSummary {
                status: NodeStatus::Available,
                ..HealthSummary::default()
            },
        );
        atomic_write_json(&core.paths.health, &health).unwrap();
        *core.full_node_test_result.lock() = Some(Err("existing result".into()));

        let changed = core
            .apply_system_catalog_update(
                &encrypted_catalog(&["https://proxy.example"]),
                |_, previous| previous.clone(),
            )
            .unwrap();

        assert!(changed);
        assert!(!core.health().unwrap().contains_key(FASTGIT_REWRITE_BASE));
        core.apply_system_catalog_update(
            &encrypted_catalog(&["https://fastgit.cc"]),
            |_, previous| previous.clone(),
        )
        .unwrap();
        assert!(!core.health().unwrap().contains_key(FASTGIT_REWRITE_BASE));
        assert!(core
            .full_node_test_result
            .lock()
            .as_ref()
            .is_some_and(|result| result.as_ref().unwrap_err() == "existing result"));
    }

    #[test]
    fn catalog_recovery_does_not_override_a_newer_user_choice() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();
        let accelerated = git::build_config(
            &settings,
            Some(&NodeDefinition::fastgit()),
            &[],
            Some(&core.paths.trace_socket),
        )
        .unwrap();
        atomic_write(&core.paths.gitconfig, accelerated.as_bytes()).unwrap();
        let catalog = encrypted_catalog(&["https://proxy.example"]);
        let update = core.apply_system_catalog(&catalog).unwrap();
        let recovery_applied_at = update.recovery_applied_at.unwrap();

        let mut user_settings = core.settings().unwrap();
        user_settings.last_applied_at = Some(recovery_applied_at + chrono::Duration::seconds(1));
        atomic_write_json(&core.paths.settings, &user_settings).unwrap();
        let mut health = core.health().unwrap();
        health.insert(
            "https://proxy.example/https://github.com/".into(),
            HealthSummary {
                status: NodeStatus::Available,
                success_count: 1,
                attempt_count: 1,
                ..HealthSummary::default()
            },
        );
        atomic_write_json(&core.paths.health, &health).unwrap();

        core.resume_acceleration_after_catalog_refresh(&recovery_applied_at)
            .unwrap();

        let persisted = core.settings().unwrap();
        assert!(!persisted.acceleration_enabled);
        assert!(persisted.current_node_id.is_none());
    }

    #[test]
    fn node_tests_use_four_workers_at_most() {
        use std::{
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };

        let nodes: Vec<NodeDefinition> = (0..8)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: true,
            })
            .collect();
        let active = AtomicUsize::new(0);
        let maximum = AtomicUsize::new(0);

        let progress = Mutex::new(Vec::new());
        let tested = test_nodes_bounded(
            &nodes,
            &HashMap::new(),
            |_, _| {
                let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(running, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                active.fetch_sub(1, Ordering::SeqCst);
                HealthSummary::default()
            },
            |completed, total| {
                if completed == 1 {
                    std::thread::sleep(Duration::from_millis(30));
                }
                progress.lock().push((completed, total));
            },
        )
        .unwrap();

        assert_eq!(tested.len(), 8);
        assert_eq!(maximum.load(Ordering::SeqCst), NODE_TEST_CONCURRENCY);
        assert_eq!(
            progress.into_inner(),
            (0..=8).map(|completed| (completed, 8)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn background_health_check_retests_at_most_ten_usable_nodes() {
        let nodes: Vec<NodeDefinition> = (0..20)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: true,
            })
            .collect();
        let health = nodes
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        ..HealthSummary::default()
                    },
                )
            })
            .collect();

        let result =
            test_node_pool(&nodes, &health, |_, previous| previous.clone(), |_, _| {}).unwrap();

        assert_eq!(result.tested.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(result.pool_ids.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(
            result
                .tested
                .iter()
                .map(|(node_id, _)| node_id.as_str())
                .collect::<Vec<_>>(),
            (0..10)
                .map(|index| format!("node-{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn node_pool_detection_discovers_until_ten_from_a_partial_pool() {
        let nodes: Vec<NodeDefinition> = (0..30)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: true,
            })
            .collect();
        let health = nodes
            .iter()
            .take(4)
            .map(|node| {
                (
                    node.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        ..HealthSummary::default()
                    },
                )
            })
            .collect();

        let result = test_node_pool(
            &nodes,
            &health,
            |_, _| HealthSummary {
                status: NodeStatus::Available,
                ..HealthSummary::default()
            },
            |_, _| {},
        )
        .unwrap();

        assert_eq!(result.tested.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(result.pool_ids.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(
            result
                .tested
                .iter()
                .filter(|(_, summary)| is_usable_health(summary))
                .count(),
            AUTO_POOL_NODE_TARGET
        );
    }

    #[test]
    fn node_pool_detection_fills_to_ten_with_custom_nodes() {
        let nodes: Vec<NodeDefinition> = (0..20)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: false,
            })
            .collect();
        let health = nodes
            .iter()
            .take(5)
            .map(|node| {
                (
                    node.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        ..HealthSummary::default()
                    },
                )
            })
            .collect();

        let result = test_node_pool(
            &nodes,
            &health,
            |_, _| HealthSummary {
                status: NodeStatus::Available,
                ..HealthSummary::default()
            },
            |_, _| {},
        )
        .unwrap();

        assert_eq!(result.tested.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(result.pool_ids.len(), AUTO_POOL_NODE_TARGET);
    }

    #[test]
    fn node_pool_detection_continues_past_failures_and_stops_at_ten_usable_nodes() {
        let nodes: Vec<NodeDefinition> = (0..30)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: true,
            })
            .collect();

        let progress = Mutex::new(Vec::new());
        let result = test_node_pool(
            &nodes,
            &HashMap::new(),
            |node, _| HealthSummary {
                status: if node
                    .id
                    .strip_prefix("node-")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
                    % 2
                    == 0
                {
                    NodeStatus::Available
                } else {
                    NodeStatus::Unavailable
                },
                ..HealthSummary::default()
            },
            |completed, total| progress.lock().push((completed, total)),
        )
        .unwrap();

        assert_eq!(result.tested.len(), 19);
        assert_eq!(result.pool_ids.len(), AUTO_POOL_NODE_TARGET);
        assert_eq!(result.tested.last().unwrap().0, "node-18");
        assert_eq!(
            progress.into_inner(),
            (0..=AUTO_POOL_NODE_TARGET)
                .map(|completed| (completed, AUTO_POOL_NODE_TARGET))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn full_test_rebuilds_a_ten_node_automatic_pool() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let nodes: Vec<NodeDefinition> = (0..11)
            .map(|index| NodeDefinition {
                id: format!("node-{index}"),
                name: format!("Node {index}"),
                rewrite_base: format!("https://proxy-{index}.example/https://github.com/"),
                enabled: true,
                built_in: false,
            })
            .collect();
        atomic_write_json(&core.paths.nodes, &nodes).unwrap();
        let tested = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                (
                    node.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        success_count: 1,
                        attempt_count: 1,
                        median_latency_ms: Some(index as u64 + 1),
                        checked_at: Some(Utc::now()),
                        ..HealthSummary::default()
                    },
                )
            })
            .collect();

        core.persist_test_results(tested, AutomaticPoolUpdate::Rebuild)
            .unwrap();
        let health = core.health().unwrap();
        let pool_ids: HashSet<_> = health
            .iter()
            .filter(|(_, summary)| summary.in_auto_pool)
            .map(|(node_id, _)| node_id.as_str())
            .collect();

        assert_eq!(pool_ids.len(), AUTO_POOL_NODE_TARGET);
        assert!(!pool_ids.contains("node-10"));

        core.persist_test_results(
            vec![("node-10".into(), health["node-10"].clone())],
            AutomaticPoolUpdate::Include("node-10".into()),
        )
        .unwrap();
        let health = core.health().unwrap();
        assert!(health["node-10"].in_auto_pool);
        assert_eq!(
            health
                .values()
                .filter(|summary| summary.in_auto_pool)
                .count(),
            AUTO_POOL_NODE_TARGET
        );
    }

    #[test]
    fn automatic_selection_ignores_available_nodes_outside_the_pool() {
        let pooled = NodeDefinition {
            id: "pooled".into(),
            name: "Pooled".into(),
            rewrite_base: "https://pooled.example/https://github.com/".into(),
            enabled: true,
            built_in: true,
        };
        let outside = NodeDefinition {
            id: "outside".into(),
            name: "Outside".into(),
            rewrite_base: "https://outside.example/https://github.com/".into(),
            enabled: true,
            built_in: true,
        };
        let pairs = vec![
            (
                outside,
                HealthSummary {
                    status: NodeStatus::Available,
                    median_latency_ms: Some(1),
                    ..HealthSummary::default()
                },
            ),
            (
                pooled,
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    median_latency_ms: Some(100),
                    ..HealthSummary::default()
                },
            ),
        ];

        assert_eq!(git::choose_node(&pairs).unwrap().id, "pooled");
    }

    #[test]
    fn imports_and_deduplicates_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let result = core
            .import_nodes(
                "https://fastgit.cc\nhttps://proxy.example\nhttps://proxy.example/https://github.com/",
            )
            .unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.duplicates, 2);
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(
            result.nodes[1].node.rewrite_base,
            "https://proxy.example/https://github.com/"
        );
    }

    #[test]
    fn automatic_selection_requires_verified_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let pairs = vec![(NodeDefinition::fastgit(), HealthSummary::default())];
        let mut settings = Settings::default();
        assert!(core.select_current(&mut settings, &pairs).is_err());
        assert!(settings.current_node_id.is_none());
    }

    #[test]
    fn validated_route_tries_healthy_nodes_in_score_order_before_saving() {
        const MARKER: &str = "GITBOOST_VALIDATED_ROUTE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::validated_route_tries_healthy_nodes_in_score_order_before_saving",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let faster = custom_node(
            "faster",
            "Faster",
            "https://faster.example/https://github.com/",
        );
        let slower = custom_node(
            "slower",
            "Slower",
            "https://slower.example/https://github.com/",
        );
        atomic_write_json(&core.paths.nodes, &vec![slower.clone(), faster.clone()]).unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([
                (
                    faster.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        success_count: 2,
                        attempt_count: 2,
                        median_latency_ms: Some(10),
                        ..HealthSummary::default()
                    },
                ),
                (
                    slower.id.clone(),
                    HealthSummary {
                        status: NodeStatus::Slow,
                        in_auto_pool: true,
                        success_count: 2,
                        attempt_count: 2,
                        median_latency_ms: Some(100),
                        ..HealthSummary::default()
                    },
                ),
            ]),
        )
        .unwrap();
        let mut attempted = vec![];

        let normalized = core
            .validate_route_with("openai/codex", |node, repository_url| {
                attempted.push((node.id.clone(), repository_url.to_owned()));
                if node.id == faster.id {
                    Err("first node failed".into())
                } else {
                    Ok(git::RepositoryProbeStatus::Readable)
                }
            })
            .unwrap();

        assert_eq!(
            attempted,
            [
                (
                    "faster".into(),
                    "https://github.com/openai/codex.git".into()
                ),
                (
                    "slower".into(),
                    "https://github.com/openai/codex.git".into()
                )
            ]
        );
        assert!(core.routes().unwrap().is_empty());
        let snapshot = core.add_route(&normalized).unwrap();
        assert_eq!(
            snapshot.routes[0].repository_url,
            "https://github.com/openai/codex.git"
        );
    }

    #[test]
    fn validated_route_does_not_save_when_all_mirrors_fail() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "available",
            "Available",
            "https://available.example/https://github.com/",
        );
        atomic_write_json(&core.paths.nodes, &vec![node.clone()]).unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([(
                node.id,
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 2,
                    attempt_count: 2,
                    median_latency_ms: Some(10),
                    ..HealthSummary::default()
                },
            )]),
        )
        .unwrap();

        let error = core
            .validate_route_with("openai/codex", |_, _| Err("unreachable".into()))
            .unwrap_err();

        assert!(error.contains("无法匿名读取"));
        assert!(core.routes().unwrap().is_empty());
    }

    #[test]
    fn validated_route_rejects_an_empty_repository_without_trying_more_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let first = custom_node(
            "first",
            "First",
            "https://first.example/https://github.com/",
        );
        let second = custom_node(
            "second",
            "Second",
            "https://second.example/https://github.com/",
        );
        atomic_write_json(&core.paths.nodes, &vec![first.clone(), second.clone()]).unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([
                (
                    first.id,
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        success_count: 2,
                        attempt_count: 2,
                        median_latency_ms: Some(10),
                        ..HealthSummary::default()
                    },
                ),
                (
                    second.id,
                    HealthSummary {
                        status: NodeStatus::Available,
                        in_auto_pool: true,
                        success_count: 2,
                        attempt_count: 2,
                        median_latency_ms: Some(20),
                        ..HealthSummary::default()
                    },
                ),
            ]),
        )
        .unwrap();
        let mut attempts = 0;

        let error = core
            .validate_route_with("owner/empty", |_, _| {
                attempts += 1;
                Ok(git::RepositoryProbeStatus::Empty)
            })
            .unwrap_err();

        assert_eq!(error, "仓库尚无提交，请完成首次提交后再加入加速清单");
        assert_eq!(attempts, 1);
        assert!(core.routes().unwrap().is_empty());
    }

    #[test]
    fn mcp_setting_persists_and_missing_values_default_to_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let core = AppCore::new(root.clone()).unwrap();

        assert!(core.set_mcp_enabled(true).unwrap().settings.mcp_enabled);
        drop(core);
        let core = AppCore::new(root.clone()).unwrap();
        assert!(core.settings().unwrap().mcp_enabled);

        let mut stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&core.paths.settings).unwrap()).unwrap();
        stored.as_object_mut().unwrap().remove("mcpEnabled");
        atomic_write_json(&core.paths.settings, &stored).unwrap();
        assert!(!core.settings().unwrap().mcp_enabled);
    }

    #[test]
    fn acceleration_control_keeps_selection_in_sync() {
        const MARKER: &str = "GITBOOST_LINE_CONTROL_STATE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::acceleration_control_keeps_selection_in_sync",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        core.add_route("openai/codex").unwrap();
        core.acknowledge_consent().unwrap();

        assert_eq!(
            core.set_acceleration(true, false).unwrap_err(),
            "没有通过真实 Git 检测的可用节点"
        );
        let unchanged = core.settings().unwrap();
        assert!(!unchanged.acceleration_enabled);
        assert!(unchanged.current_node_id.is_none());

        atomic_write_json(
            &core.paths.health,
            &HashMap::from([(
                FASTGIT_REWRITE_BASE.to_string(),
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 1,
                    attempt_count: 1,
                    ..HealthSummary::default()
                },
            )]),
        )
        .unwrap();

        let automatic = core.set_acceleration(true, false).unwrap();
        assert!(automatic.settings.acceleration_enabled);
        assert_eq!(
            automatic.settings.current_node_id.as_deref(),
            Some(FASTGIT_REWRITE_BASE)
        );

        let direct = core.set_acceleration(false, false).unwrap();
        assert!(!direct.settings.acceleration_enabled);
        assert!(direct.settings.current_node_id.is_none());

        core.set_acceleration(true, false).unwrap();
        let route_id = core.routes().unwrap()[0].id.clone();
        let empty_allowlist = core.delete_route(&route_id).unwrap();
        assert!(!empty_allowlist.settings.acceleration_enabled);
        assert!(empty_allowlist.settings.current_node_id.is_none());

        core.set_route_scope(RouteScope::Global).unwrap();
        core.set_acceleration(true, false).unwrap();
        let restored_allowlist = core.set_route_scope(RouteScope::Allowlist).unwrap();
        assert!(!restored_allowlist.settings.acceleration_enabled);
        assert!(restored_allowlist.settings.current_node_id.is_none());
    }

    #[test]
    fn trace2_conflict_requires_a_choice_and_restores_the_external_target() {
        const MARKER: &str = "GITBOOST_TRACE2_CONFLICT_CHOICE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::trace2_conflict_requires_a_choice_and_restores_the_external_target",
            MARKER,
        ) {
            return;
        }
        let external_target = "af_unix:stream:/tmp/external-trace2.sock";
        let configured = std::process::Command::new("git")
            .args(["config", "--global", "trace2.eventTarget", external_target])
            .status()
            .unwrap();
        assert!(configured.success());

        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        core.add_route("openai/codex").unwrap();
        core.acknowledge_consent().unwrap();
        atomic_write_json(
            &core.paths.health,
            &HashMap::from([(
                FASTGIT_REWRITE_BASE.to_string(),
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 1,
                    attempt_count: 1,
                    ..HealthSummary::default()
                },
            )]),
        )
        .unwrap();

        assert_eq!(
            core.trace2_target_conflict().unwrap().as_deref(),
            Some(external_target)
        );
        assert!(core
            .set_acceleration(true, false)
            .unwrap_err()
            .contains("Trace2"));
        assert!(!core.settings().unwrap().acceleration_enabled);

        let enabled = core.set_acceleration(true, true).unwrap();
        assert!(enabled.settings.acceleration_enabled);
        assert!(enabled.environment.include_registered);
        assert!(!enabled.environment.trace2_target_overridden);
        let effective = std::process::Command::new("git")
            .args([
                "config",
                "--global",
                "--includes",
                "--get",
                "trace2.eventTarget",
            ])
            .output()
            .unwrap();
        assert!(effective.status.success());
        assert_eq!(
            String::from_utf8_lossy(&effective.stdout).trim(),
            format!("af_unix:stream:{}", core.paths.trace_socket.display())
        );

        let external_config = directory.path().join("external.gitconfig");
        fs::write(
            &external_config,
            format!("[trace2]\n\teventTarget = {external_target}\n"),
        )
        .unwrap();
        let overridden = std::process::Command::new("git")
            .args([
                "config",
                "--global",
                "--add",
                "include.path",
                external_config.to_string_lossy().as_ref(),
            ])
            .status()
            .unwrap();
        assert!(overridden.success());
        assert!(
            core.snapshot()
                .unwrap()
                .environment
                .trace2_target_overridden
        );

        let reclaimed = core.set_acceleration(true, true).unwrap();
        assert!(!reclaimed.environment.trace2_target_overridden);

        let disabled = core.set_acceleration(false, false).unwrap();
        assert!(!disabled.settings.acceleration_enabled);
        assert!(!disabled.environment.include_registered);
        let restored = std::process::Command::new("git")
            .args([
                "config",
                "--global",
                "--includes",
                "--get",
                "trace2.eventTarget",
            ])
            .output()
            .unwrap();
        assert!(restored.status.success());
        assert_eq!(
            String::from_utf8_lossy(&restored.stdout).trim(),
            external_target
        );
    }

    #[test]
    fn acceleration_requires_privacy_consent_once() {
        const MARKER: &str = "GITBOOST_CONSENT_GATE_STATE_CHILD";
        if rerun_with_isolated_git(
            "core::tests::acceleration_requires_privacy_consent_once",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        // 常规配置操作（添加路由）不能影响同意状态
        core.add_route("openai/codex").unwrap();
        assert!(core.settings().unwrap().consent_acknowledged_at.is_none());

        assert_eq!(
            core.set_acceleration(true, false).unwrap_err(),
            "首次开启加速前，请在 GitBoost 窗口中确认隐私提示"
        );
        assert!(!core.settings().unwrap().acceleration_enabled);

        let acknowledged = core.acknowledge_consent().unwrap();
        assert!(acknowledged.settings.consent_acknowledged_at.is_some());
        assert!(!acknowledged.settings.acceleration_enabled);

        // 同意后不再要求确认，错误回到线路本身
        assert_eq!(
            core.set_acceleration(true, false).unwrap_err(),
            "没有通过真实 Git 检测的可用节点"
        );
    }

    #[test]
    fn startup_clears_a_stale_selection_while_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let inconsistent = Settings {
            current_node_id: Some(FASTGIT_REWRITE_BASE.into()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &inconsistent).unwrap();

        core.refresh_registered_configuration().unwrap();

        let normalized = core.settings().unwrap();
        assert!(!normalized.acceleration_enabled);
        assert!(normalized.current_node_id.is_none());
    }

    #[test]
    fn download_rejects_an_enabled_node_outside_the_automatic_pool() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let error = core
            .prepare_download_batch(
                "https://github.com/ollama/ollama/releases/download/v1/OllamaSetup.exe",
                &[],
            )
            .unwrap_err();
        assert_eq!(error, "自动线路池中没有可用的下载节点");
    }

    #[test]
    fn prepares_download_batch_without_changing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        core.import_nodes("https://one.example\nhttps://two.example")
            .unwrap();
        let nodes = core.custom_nodes().unwrap();
        let first = &nodes[0];
        let second = &nodes[1];
        let mut health = HashMap::new();
        for node in &nodes {
            health.insert(
                node.id.clone(),
                HealthSummary {
                    status: NodeStatus::Available,
                    in_auto_pool: true,
                    success_count: 1,
                    attempt_count: 1,
                    median_latency_ms: Some(if node.id == first.id { 10 } else { 20 }),
                    ..HealthSummary::default()
                },
            );
        }
        atomic_write_json(&core.paths.health, &health).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            current_node_id: Some(first.id.clone()),
            ..Settings::default()
        };
        atomic_write_json(&core.paths.settings, &settings).unwrap();

        let (targets, has_remaining) = core
            .prepare_download_batch(
                "https://github.com/DiscoverBox/gitboost/archive/refs/heads/main.zip",
                &[],
            )
            .unwrap();

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].node_id, first.id);
        assert_eq!(targets[1].node_id, second.id);
        assert!(!has_remaining);
        assert_eq!(
            core.settings().unwrap().current_node_id,
            Some(first.id.clone())
        );
        assert_eq!(
            core.prepare_download_batch(
                "https://github.com/DiscoverBox/gitboost/archive/refs/heads/main.zip",
                &[first.id.clone(), second.id.clone()],
            )
            .unwrap_err(),
            "自动线路池中没有其他可用的下载线路"
        );
    }

    #[test]
    fn download_batch_keeps_unattempted_nodes_for_manual_retry() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let nodes = (1..=5)
            .map(|index| {
                custom_node(
                    &format!("node-{index}"),
                    &format!("Node {index}"),
                    &format!("https://{index}.example/https://github.com/"),
                )
            })
            .collect::<Vec<_>>();
        configure_automatic_nodes(&core, &nodes, &nodes[0].id);

        let (first_batch, has_remaining) = core
            .prepare_download_batch(
                "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
                &[],
            )
            .unwrap();
        assert_eq!(first_batch.len(), DOWNLOAD_BATCH_SIZE);
        assert!(has_remaining);

        let excluded = first_batch
            .iter()
            .map(|target| target.node_id.clone())
            .collect::<Vec<_>>();
        let (second_batch, has_remaining) = core
            .prepare_download_batch(
                "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
                &excluded,
            )
            .unwrap();
        assert_eq!(second_batch.len(), 1);
        assert_eq!(second_batch[0].node_id, nodes[4].id);
        assert!(!has_remaining);
    }

    #[test]
    fn usage_audit_records_route_and_removes_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let recheck = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "clone".into(),
                original_url: Some(
                    "https://secret-token@github.com/octocat/Hello-World.git?access=hidden".into(),
                ),
                effective_url: "https://fastgit.cc/https://github.com/octocat/Hello-World.git"
                    .into(),
                exit_code: 0,
                duration_ms: 321,
            })
            .unwrap();
        assert!(recheck.is_none());
        let log = core.usage_log().unwrap();
        assert_eq!(log.events.len(), 1);
        assert_eq!(log.events[0].route, UsageRoute::Accelerated);
        assert_eq!(log.events[0].connection_host, "fastgit.cc");
        assert_eq!(
            log.events[0].repository,
            "https://github.com/octocat/Hello-World.git"
        );
        assert!(
            !std::fs::read_to_string(directory.path().join("logs/usage.jsonl"))
                .unwrap()
                .contains("secret-token")
        );
    }

    #[test]
    fn failed_accelerated_read_requests_recheck_when_usage_logging_is_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "current",
            "Current",
            "https://current.example/https://github.com/",
        );
        configure_automatic_nodes(&core, std::slice::from_ref(&node), &node.id);

        let recheck = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "clone".into(),
                original_url: Some("https://github.com/openai/codex.git".into()),
                effective_url: format!("{}openai/codex.git", node.rewrite_base),
                exit_code: 128,
                duration_ms: 500,
            })
            .unwrap();

        assert_eq!(recheck.as_deref(), Some("current"));
        assert!(core.usage_log().unwrap().events.is_empty());
    }

    #[test]
    fn failure_observation_ignores_success_push_and_direct_routes() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "current",
            "Current",
            "https://current.example/https://github.com/",
        );
        configure_automatic_nodes(&core, std::slice::from_ref(&node), &node.id);
        let trace = |command: &str, effective_url: String, exit_code: i32| CompletedTrace {
            occurred_at: Utc::now(),
            command: command.into(),
            original_url: Some("https://github.com/openai/codex.git".into()),
            effective_url,
            exit_code,
            duration_ms: 100,
        };

        assert!(core
            .observe_git_operation(trace(
                "clone",
                format!("{}openai/codex.git", node.rewrite_base),
                0,
            ))
            .unwrap()
            .is_none());
        assert!(core
            .observe_git_operation(trace(
                "push",
                format!("{}openai/codex.git", node.rewrite_base),
                128,
            ))
            .unwrap()
            .is_none());
        assert!(core
            .observe_git_operation(trace(
                "clone",
                "https://github.com/openai/codex.git".into(),
                128,
            ))
            .unwrap()
            .is_none());
    }

    #[test]
    fn failure_observation_uses_the_most_specific_matching_node_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let shorter = custom_node(
            "shorter",
            "Shorter",
            "https://proxy.example/service/https://github.com/",
        );
        let longer = custom_node(
            "longer",
            "Longer",
            "https://proxy.example/service/https://github.com/nested/https://github.com/",
        );
        assert_eq!(
            normalize_rewrite_base(&longer.rewrite_base).unwrap(),
            longer.rewrite_base
        );
        configure_automatic_nodes(&core, &[shorter, longer.clone()], &longer.id);
        let mut settings = core.settings().unwrap();
        settings.usage_logging_enabled = true;
        atomic_write_json(&core.paths.settings, &settings).unwrap();

        let failed_node = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "fetch".into(),
                original_url: None,
                effective_url: format!("{}octocat/Hello-World.git", longer.rewrite_base),
                exit_code: 128,
                duration_ms: 10,
            })
            .unwrap();

        assert_eq!(failed_node.as_deref(), Some("longer"));
        let usage = core.usage_log().unwrap();
        assert_eq!(usage.events[0].node_name.as_deref(), Some("Longer"));
        assert_eq!(usage.events[0].repository, TEST_REPOSITORY);
    }

    #[test]
    fn automatic_failover_only_supports_read_commands() {
        for command in ["clone", "fetch", "pull", "ls-remote"] {
            assert!(supports_automatic_failover(command));
        }
        for command in ["push", "status", "submodule", "git"] {
            assert!(!supports_automatic_failover(command));
        }
    }

    #[test]
    fn failed_node_recheck_switches_to_the_next_usable_node() {
        const MARKER: &str = "GITBOOST_FAILED_NODE_SWITCH_CHILD";
        if rerun_with_isolated_git(
            "core::tests::failed_node_recheck_switches_to_the_next_usable_node",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let first = custom_node(
            "first",
            "First",
            "https://first.example/https://github.com/",
        );
        let second = custom_node(
            "second",
            "Second",
            "https://second.example/https://github.com/",
        );
        configure_automatic_nodes(&core, &[first.clone(), second.clone()], &first.id);
        *core.full_node_test_result.lock() = Some(Err("stale result".into()));

        let outcome = core
            .recheck_failed_node_with(&first.id, |_, previous| HealthSummary {
                status: NodeStatus::Unavailable,
                in_auto_pool: previous.in_auto_pool,
                attempt_count: previous.attempt_count + 2,
                consecutive_failures: 2,
                checked_at: Some(Utc::now()),
                failure_reason: Some("连接超时".into()),
                ..previous.clone()
            })
            .unwrap();

        assert_eq!(
            outcome,
            FailoverOutcome::Switched {
                from: "First".into(),
                to: "Second".into()
            }
        );
        assert!(core.full_node_test_result.lock().is_none());
        let settings = core.settings().unwrap();
        assert_eq!(settings.current_node_id.as_deref(), Some("second"));
        assert!(settings.acceleration_enabled);
        assert!(fs::read_to_string(&core.paths.gitconfig)
            .unwrap()
            .contains(&second.rewrite_base));
    }

    #[test]
    fn usable_failed_node_recheck_does_not_reselect_a_faster_node() {
        const MARKER: &str = "GITBOOST_USABLE_RECHECK_CHILD";
        if rerun_with_isolated_git(
            "core::tests::usable_failed_node_recheck_does_not_reselect_a_faster_node",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let first = custom_node(
            "first",
            "First",
            "https://first.example/https://github.com/",
        );
        let second = custom_node(
            "second",
            "Second",
            "https://second.example/https://github.com/",
        );
        configure_automatic_nodes(&core, &[first.clone(), second], &first.id);

        let outcome = core
            .recheck_failed_node_with(&first.id, |_, previous| HealthSummary {
                status: NodeStatus::Available,
                in_auto_pool: true,
                success_count: previous.success_count + 2,
                attempt_count: previous.attempt_count + 2,
                median_latency_ms: Some(1_000),
                consecutive_failures: 0,
                checked_at: Some(Utc::now()),
                failure_reason: None,
                recent_latencies_ms: vec![1_000],
            })
            .unwrap();

        assert_eq!(outcome, FailoverOutcome::Unconfirmed);
        assert_eq!(
            core.settings().unwrap().current_node_id.as_deref(),
            Some("first")
        );
        assert_eq!(core.health().unwrap()["first"].median_latency_ms, Some(20));
    }

    #[test]
    fn usage_log_write_failure_does_not_suppress_failover_request() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "current",
            "Current",
            "https://current.example/https://github.com/",
        );
        configure_automatic_nodes(&core, std::slice::from_ref(&node), &node.id);
        let mut settings = core.settings().unwrap();
        settings.usage_logging_enabled = true;
        atomic_write_json(&core.paths.settings, &settings).unwrap();
        fs::remove_dir_all(&core.paths.logs).unwrap();
        fs::write(&core.paths.logs, b"not a directory").unwrap();

        let failed_node = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "fetch".into(),
                original_url: Some(TEST_REPOSITORY.into()),
                effective_url: format!("{}octocat/Hello-World.git", node.rewrite_base),
                exit_code: 128,
                duration_ms: 10,
            })
            .unwrap();

        assert_eq!(failed_node.as_deref(), Some("current"));
    }

    #[test]
    fn stale_failure_does_not_override_a_newer_line_selection() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let first = custom_node(
            "first",
            "First",
            "https://first.example/https://github.com/",
        );
        let second = custom_node(
            "second",
            "Second",
            "https://second.example/https://github.com/",
        );
        configure_automatic_nodes(&core, &[first.clone(), second.clone()], &first.id);

        let outcome = core
            .recheck_failed_node_with(&first.id, |_, previous| {
                let mut settings = core.settings().unwrap();
                settings.current_node_id = Some(second.id.clone());
                atomic_write_json(&core.paths.settings, &settings).unwrap();
                HealthSummary {
                    status: NodeStatus::Unavailable,
                    consecutive_failures: 2,
                    ..previous.clone()
                }
            })
            .unwrap();

        assert_eq!(outcome, FailoverOutcome::Skipped);
        assert_eq!(
            core.settings().unwrap().current_node_id.as_deref(),
            Some("second")
        );
        assert_eq!(
            core.health().unwrap()["first"].status,
            NodeStatus::Available
        );
    }

    #[test]
    fn failed_node_recheck_skips_without_clearing_results_while_a_test_is_running() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "current",
            "Current",
            "https://current.example/https://github.com/",
        );
        configure_automatic_nodes(&core, std::slice::from_ref(&node), &node.id);
        *core.full_node_test_result.lock() = Some(Err("existing result".into()));
        let active = core.full_node_test_lock.lock();

        let outcome = core
            .recheck_failed_node_with(&node.id, |_, _| {
                panic!("recheck must not probe while another node test is running")
            })
            .unwrap();

        assert_eq!(outcome, FailoverOutcome::Skipped);
        assert!(core
            .full_node_test_result
            .lock()
            .as_ref()
            .is_some_and(|result| result.as_ref().unwrap_err() == "existing result"));
        drop(active);
    }

    #[test]
    fn real_git_recheck_falls_back_to_direct_when_no_node_is_usable() {
        const MARKER: &str = "GITBOOST_REAL_FAILURE_RECHECK_CHILD";
        if rerun_with_isolated_git(
            "core::tests::real_git_recheck_falls_back_to_direct_when_no_node_is_usable",
            MARKER,
        ) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let node = custom_node(
            "unreachable",
            "Unreachable",
            "https://127.0.0.1:1/https://github.com/",
        );
        configure_automatic_nodes(&core, std::slice::from_ref(&node), &node.id);

        let failed_node = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "clone".into(),
                original_url: Some(TEST_REPOSITORY.into()),
                effective_url: format!("{}octocat/Hello-World.git", node.rewrite_base),
                exit_code: 128,
                duration_ms: 10,
            })
            .unwrap()
            .unwrap();
        let outcome = core.recheck_failed_node(&failed_node).unwrap();

        assert_eq!(
            outcome,
            FailoverOutcome::FellBackToDirect {
                from: "Unreachable".into()
            }
        );
        let settings = core.settings().unwrap();
        assert!(!settings.acceleration_enabled);
        assert_eq!(
            core.health().unwrap()["unreachable"].status,
            NodeStatus::Unavailable
        );
        assert!(fs::read_to_string(&core.paths.gitconfig)
            .unwrap()
            .contains("GitHub remains direct"));
    }

    #[test]
    fn diagnostics_reports_an_invalid_repository_as_a_check_failure() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();

        let report = core.diagnostics(Some(directory.path())).unwrap();

        assert_eq!(
            report.repository_error.as_deref(),
            Some("指定路径不是 Git 仓库")
        );
        assert!(report.explicit_push_url.is_none());
        assert!(report.report_text.contains("显式 pushurl: 检查失败"));
    }

    #[test]
    #[ignore = "requires live CDN and GitHub access"]
    fn live_https_client_works_without_external_curl() {
        const CHILD_MARKER: &str = "GITBOOST_HTTP_CLIENT_CHILD";
        if std::env::var_os(CHILD_MARKER).is_none() {
            let empty_path = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "core::tests::live_https_client_works_without_external_curl",
                    "--nocapture",
                ])
                .env(CHILD_MARKER, "1")
                .env("PATH", empty_path.path())
                .status()
                .unwrap();
            assert!(status.success(), "application-owned HTTPS client failed");
            return;
        }

        let catalog = fetch_system_catalog().unwrap();
        assert!(!parse_system_catalog(&catalog).unwrap().is_empty());
        http::probe_range(
            "https://github.com/DiscoverBox/gitboost/archive/refs/heads/main.zip",
            Duration::from_secs(4),
            Duration::from_secs(15),
            65_536,
        )
        .unwrap();
    }

    #[test]
    #[ignore = "requires live HTTPS and GitHub proxy access"]
    fn live_download_batch_falls_back_after_a_real_http_failure() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let unavailable = custom_node(
            "real-http-failure",
            "Real HTTP failure",
            "https://example.com/https://github.com/",
        );
        let fallback = NodeDefinition::fastgit();
        configure_automatic_nodes(
            &core,
            &[unavailable.clone(), fallback.clone()],
            &unavailable.id,
        );

        let (targets, has_remaining) = core
            .prepare_download_batch(
                "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh",
                &[],
            )
            .unwrap();
        assert_eq!(
            targets
                .iter()
                .map(|target| target.node_id.as_str())
                .collect::<Vec<_>>(),
            vec![unavailable.id.as_str(), fallback.id.as_str()]
        );
        assert!(!has_remaining);

        let (winner, attempted_node_ids) = downloads::probe_first_available(targets).unwrap();
        assert_eq!(winner.node_id, fallback.id);
        assert_eq!(
            attempted_node_ids,
            vec![unavailable.id.clone(), fallback.id.clone()]
        );
        assert_eq!(
            core.settings().unwrap().current_node_id,
            Some(unavailable.id)
        );
    }

    #[test]
    #[ignore = "requires live system-node and GitHub access"]
    fn core_workflow_applies_verifies_persists_and_restores_git_routing() {
        const CHILD_MARKER: &str = "GITBOOST_CORE_WORKFLOW_CHILD";
        if std::env::var_os(CHILD_MARKER).is_none() {
            let sandbox = tempfile::tempdir().unwrap();
            let home = sandbox.path().join("home");
            fs::create_dir(&home).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "core::tests::core_workflow_applies_verifies_persists_and_restores_git_routing",
                    "--nocapture",
                ])
                .env(CHILD_MARKER, "1")
                .env("GIT_CONFIG_GLOBAL", sandbox.path().join("global.gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", home)
                .status()
                .unwrap();
            assert!(status.success(), "isolated core workflow failed");
            return;
        }

        fn git(repository: &Path, arguments: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .args(arguments)
                .current_dir(repository)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app-data");
        let core = AppCore::new(root.clone()).unwrap();

        let catalog = fetch_system_catalog().unwrap();
        core.apply_system_catalog(&catalog).unwrap();

        // Production probes must remain anonymous even when the launching user has
        // a more-specific rewrite, repository context, or credential helper configured.
        let clean_global = std::env::var_os("GIT_CONFIG_GLOBAL").unwrap();
        let hostile_global = directory.path().join("hostile.gitconfig");
        fs::write(
            &hostile_global,
            "[credential]\n\thelper = inherited-helper\n",
        )
        .unwrap();
        std::env::set_var("GIT_CONFIG_GLOBAL", &hostile_global);
        std::env::set_var("GIT_CONFIG_COUNT", "2");
        std::env::set_var("GIT_CONFIG_KEY_0", "url.http://127.0.0.1:9/.insteadOf");
        std::env::set_var("GIT_CONFIG_VALUE_0", TEST_REPOSITORY);
        std::env::set_var("GIT_CONFIG_KEY_1", "credential.helper");
        std::env::set_var("GIT_CONFIG_VALUE_1", "dynamic-helper");
        std::env::set_var("GIT_DIR", directory.path().join("missing.git"));
        std::env::set_var("GIT_WORK_TREE", directory.path());

        let progress = Mutex::new(Vec::new());
        let started = std::sync::Barrier::new(2);
        let release = std::sync::Barrier::new(2);
        let owner_result = Mutex::new(None);
        let joined_result = std::thread::scope(|scope| {
            scope.spawn(|| {
                *owner_result.lock() =
                    Some(core.test_all_nodes_with_progress(|completed, total| {
                        progress.lock().push((completed, total));
                        if completed == 0 {
                            started.wait();
                            release.wait();
                        }
                    }));
            });
            started.wait();
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(30));
                release.wait();
            });
            core.test_all_nodes_or_join_with_progress(|_, _| {})
        })
        .unwrap();

        std::env::set_var("GIT_CONFIG_GLOBAL", clean_global);
        for key in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_KEY_1",
            "GIT_CONFIG_VALUE_1",
            "GIT_DIR",
            "GIT_WORK_TREE",
        ] {
            std::env::remove_var(key);
        }

        let owner_result = owner_result.into_inner().unwrap().unwrap();
        assert_eq!(
            owner_result
                .iter()
                .map(|node| (&node.node.id, node.health.status))
                .collect::<Vec<_>>(),
            joined_result
                .iter()
                .map(|node| (&node.node.id, node.health.status))
                .collect::<Vec<_>>()
        );
        let progress = progress.into_inner();
        let usable = owner_result
            .iter()
            .filter(|node| is_usable_health(&node.health))
            .count();
        assert!(usable <= AUTO_POOL_NODE_TARGET);
        assert_eq!(
            progress,
            (0..=usable)
                .map(|completed| (completed, AUTO_POOL_NODE_TARGET))
                .collect::<Vec<_>>()
        );

        core.add_route("openai/codex").unwrap();
        assert_eq!(
            core.set_acceleration(true, false).unwrap_err(),
            "首次开启加速前，请在 GitBoost 窗口中确认隐私提示"
        );
        core.acknowledge_consent().unwrap();
        drop(core);

        // 真实文件持久化和重新加载后，同意状态仍然有效，不依赖进程内 mock 状态。
        let core = AppCore::new(root.clone()).unwrap();
        assert!(core.settings().unwrap().consent_acknowledged_at.is_some());
        let enabled = core.set_acceleration(true, false).unwrap();
        assert!(enabled.settings.acceleration_enabled);
        let node_id = enabled
            .settings
            .current_node_id
            .clone()
            .expect("live integration requires at least one usable system node");
        let node = owner_result
            .iter()
            .find(|node| {
                node.node.id == node_id
                    && matches!(node.health.status, NodeStatus::Available | NodeStatus::Slow)
            })
            .expect("automatic selection must use a verified node");
        let rewrite_base = node.node.rewrite_base.clone();
        assert!(enabled.environment.include_registered);

        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q"]);
        git(
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/openai/codex.git",
            ],
        );
        git(
            repository.path(),
            &[
                "remote",
                "add",
                "unlisted",
                "https://github.com/octocat/Hello-World.git",
            ],
        );
        assert_eq!(
            git(repository.path(), &["remote", "get-url", "origin"]),
            format!("{rewrite_base}openai/codex.git")
        );
        assert_eq!(
            git(
                repository.path(),
                &["remote", "get-url", "--push", "origin"]
            ),
            "https://github.com/openai/codex.git"
        );
        assert_eq!(
            git(repository.path(), &["remote", "get-url", "unlisted"]),
            "https://github.com/octocat/Hello-World.git"
        );

        drop(core);
        let core = AppCore::new(root.clone()).unwrap();
        core.refresh_registered_configuration().unwrap();
        let restarted = core.snapshot().unwrap();
        assert!(restarted.settings.acceleration_enabled);
        assert!(restarted.environment.include_registered);
        assert_eq!(
            git(repository.path(), &["remote", "get-url", "origin"]),
            format!("{rewrite_base}openai/codex.git")
        );

        git(
            repository.path(),
            &[
                "config",
                "remote.origin.pushurl",
                "https://user:secret@github.com/openai/codex.git?token=hidden",
            ],
        );
        let report = core.diagnostics(Some(repository.path())).unwrap();
        assert_eq!(
            report.fetch_url.as_deref(),
            Some(format!("{rewrite_base}openai/codex.git").as_str())
        );
        let explicit_push_url = report.explicit_push_url.as_deref().unwrap();
        assert!(explicit_push_url.contains("redacted"));
        assert!(!report.report_text.contains("secret"));
        assert!(!report.report_text.contains("hidden"));

        let recheck = core
            .observe_git_operation(CompletedTrace {
                occurred_at: Utc::now(),
                command: "fetch".into(),
                original_url: Some(
                    "https://user:secret@github.com/openai/codex.git?token=hidden".into(),
                ),
                effective_url: format!("{rewrite_base}openai/codex.git"),
                exit_code: 0,
                duration_ms: 42,
            })
            .unwrap();
        assert!(recheck.is_none());
        let usage = core.usage_log().unwrap();
        assert_eq!(usage.events.len(), 1);
        assert_eq!(usage.events[0].route, UsageRoute::Accelerated);
        assert_eq!(
            usage.events[0].repository,
            "https://github.com/openai/codex.git"
        );
        let stored_usage =
            fs::read_to_string(directory.path().join("app-data/logs/usage.jsonl")).unwrap();
        assert!(!stored_usage.contains("secret"));
        assert!(!stored_usage.contains("hidden"));

        let route_id = core.snapshot().unwrap().routes[0].id.clone();
        let without_routes = core.delete_route(&route_id).unwrap();
        assert!(without_routes.routes.is_empty());
        assert!(!without_routes.settings.acceleration_enabled);
        assert_eq!(
            git(repository.path(), &["remote", "get-url", "origin"]),
            "https://github.com/openai/codex.git"
        );

        let mut stale_settings = core.settings().unwrap();
        stale_settings.acceleration_enabled = true;
        atomic_write_json(&core.paths.settings, &stale_settings).unwrap();
        drop(core);
        let core = AppCore::new(root).unwrap();
        core.refresh_registered_configuration().unwrap();
        assert!(!core.snapshot().unwrap().settings.acceleration_enabled);

        let restored = core.restore_git_config().unwrap();
        assert!(!restored.settings.acceleration_enabled);
        assert!(!restored.environment.include_registered);
        assert_eq!(
            git(repository.path(), &["remote", "get-url", "origin"]),
            "https://github.com/openai/codex.git"
        );
    }
}
