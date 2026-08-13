use crate::{
    downloads, git,
    importer::{
        default_node_name, normalize_repository_url, normalize_rewrite_base, parse_import_text,
    },
    models::*,
    storage::{
        append_log, append_usage_event, atomic_write, atomic_write_json, backup_file, clear_logs,
        ensure_dir, load_json, load_usage_events,
    },
    usage::CompletedTrace,
};
use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tempfile::NamedTempFile;
use uuid::Uuid;

const SYSTEM_NODE_CATALOG_URL: &str =
    "https://cdn.jsdelivr.net/gh/DiscoverBox/gitboost@main/nodes.json";
const MAX_SYSTEM_NODES: usize = 100;
const CATALOG_MAX_BYTES: &str = "262144";
const CATALOG_TIMEOUT_SECONDS: &str = "10";
const NODE_TEST_CONCURRENCY: usize = 4;

#[derive(Debug)]
pub struct AppCore {
    paths: AppPaths,
    lock: Mutex<()>,
    full_node_test_lock: Mutex<()>,
    usage_listening: AtomicBool,
}

struct CatalogUpdate {
    changed: bool,
    recovery_applied_at: Option<chrono::DateTime<Utc>>,
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
            usage_listening: AtomicBool::new(false),
        };
        core.initialize()?;
        Ok(core)
    }

    fn initialize(&self) -> Result<(), String> {
        if !self.paths.settings.exists() {
            atomic_write_json(&self.paths.settings, &Settings::default())?;
        }
        self.initialize_node_files()?;
        if !self.paths.health.exists() {
            atomic_write_json(&self.paths.health, &HashMap::<String, HealthSummary>::new())?;
        }
        if !self.paths.routes.exists() {
            atomic_write_json(&self.paths.routes, &Vec::<RouteEntry>::new())?;
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
        let update = fetch_system_catalog().and_then(|output| self.apply_system_catalog(&output));
        let wait = update.as_ref().is_ok_and(|update| update.changed);
        let Some(_run) = self.acquire_full_node_test(wait) else {
            return update.map(|update| update.changed);
        };
        let tested = self.test_all_nodes_locked();
        let update = update?;
        tested?;
        if let Some(applied_at) = update.recovery_applied_at.as_ref() {
            self.resume_acceleration_after_catalog_refresh(applied_at)?;
        }
        Ok(update.changed)
    }

    fn acquire_full_node_test(&self, wait: bool) -> Option<parking_lot::MutexGuard<'_, ()>> {
        if wait {
            Some(self.full_node_test_lock.lock())
        } else {
            self.full_node_test_lock.try_lock()
        }
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
        let mut settings_changed = false;
        if settings
            .fixed_node_id
            .as_ref()
            .is_some_and(|id| !available_ids.contains(id))
        {
            settings.fixed_node_id = None;
            settings.line_mode = LineMode::Automatic;
            settings_changed = true;
        }
        let current_removed = settings
            .current_node_id
            .as_ref()
            .is_some_and(|id| !available_ids.contains(id));
        if current_removed {
            settings.current_node_id = None;
            settings_changed = true;
        }
        let should_resume = current_removed && settings.acceleration_enabled;
        if should_resume {
            settings.acceleration_enabled = false;
            settings.line_mode = LineMode::Direct;
            self.write_configuration(&mut settings, &[], &[])?;
        }
        atomic_write_json(&self.paths.system_nodes, &urls)?;
        if settings_changed {
            if let Err(error) = atomic_write_json(&self.paths.settings, &settings) {
                let _ = atomic_write_json(&self.paths.settings, &original_settings);
                let _ = atomic_write_json(&self.paths.system_nodes, &current);
                return Err(error);
            }
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
        Ok(AppSnapshot {
            settings,
            nodes,
            routes,
            environment: EnvironmentSummary {
                git_available: git::git_version().is_some(),
                git_path: git::git_path(),
                git_version: git::git_version(),
                include_registered: git::include_registered(&self.paths.gitconfig),
                config_path: self.paths.gitconfig.display().to_string(),
                conflicts,
                conflict_scan_error,
            },
        })
    }

    pub fn prepare_download(&self, original_url: &str) -> Result<DownloadTarget, String> {
        let settings = self.settings()?;
        let pairs = self.node_pairs()?;
        let current = settings.current_node_id.as_deref().and_then(|id| {
            pairs
                .iter()
                .find(|(node, _)| node.id == id && node.enabled)
                .map(|(node, _)| node)
        });
        let node = current
            .or_else(|| git::choose_node(&pairs))
            .or_else(|| {
                pairs
                    .iter()
                    .find(|(node, _)| node.enabled)
                    .map(|(node, _)| node)
            })
            .ok_or_else(|| "没有已启用的下载节点".to_string())?;
        downloads::prepare_target(original_url, node)
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
        let node = self
            .nodes()?
            .into_iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| "节点不存在".to_string())?;
        let previous = self.health()?.remove(node_id).unwrap_or_default();
        let tested = git::test_node(&node, &previous);
        let _guard = self.lock.lock();
        let mut health = self.health()?;
        health.insert(node.id.clone(), tested.clone());
        atomic_write_json(&self.paths.health, &health)?;
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!(
                "node test completed: id={}, status={:?}",
                node.id, tested.status
            ),
        );
        self.reselect_after_health_change()?;
        Ok(NodeEntry {
            node,
            health: tested,
        })
    }

    pub fn test_all_nodes(&self) -> Result<Vec<NodeEntry>, String> {
        let _run = self
            .full_node_test_lock
            .try_lock()
            .ok_or_else(|| "全量线路检测正在进行，请稍后再试".to_string())?;
        self.test_all_nodes_locked()
    }

    fn test_all_nodes_locked(&self) -> Result<Vec<NodeEntry>, String> {
        let nodes: Vec<NodeDefinition> = self
            .nodes()?
            .into_iter()
            .filter(|node| node.enabled)
            .collect();
        let old_health = self.health()?;
        let tested = test_nodes_bounded(&nodes, &old_health, git::test_node)?;
        let _guard = self.lock.lock();
        let mut next_health = self.health()?;
        for (node_id, summary) in tested {
            let is_newer = next_health
                .get(&node_id)
                .is_none_or(|existing| existing.checked_at <= summary.checked_at);
            if is_newer {
                next_health.insert(node_id, summary);
            }
        }
        atomic_write_json(&self.paths.health, &next_health)?;
        self.reselect_after_health_change()?;
        self.node_entries()
    }

    fn resume_acceleration_after_catalog_refresh(
        &self,
        expected_applied_at: &chrono::DateTime<Utc>,
    ) -> Result<(), String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        if settings.acceleration_enabled
            || settings.line_mode != LineMode::Direct
            || settings.last_applied_at.as_ref() != Some(expected_applied_at)
        {
            return Ok(());
        }
        let pairs = self.node_pairs()?;
        let Some(node) = git::choose_node(&pairs) else {
            return Ok(());
        };
        settings.line_mode = LineMode::Automatic;
        settings.fixed_node_id = None;
        settings.current_node_id = Some(node.id.clone());
        settings.acceleration_enabled = true;
        self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
        atomic_write_json(&self.paths.settings, &settings)
    }

    fn reselect_after_health_change(&self) -> Result<(), String> {
        let mut settings = self.settings()?;
        if settings.line_mode == LineMode::Automatic {
            let pairs = self.node_pairs()?;
            settings.current_node_id = git::choose_node(&pairs).map(|node| node.id.clone());
            if settings.acceleration_enabled && settings.current_node_id.is_none() {
                settings.line_mode = LineMode::Direct;
                settings.acceleration_enabled = false;
                self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
            } else if settings.acceleration_enabled {
                self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
            }
            atomic_write_json(&self.paths.settings, &settings)?;
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
        atomic_write_json(&self.paths.nodes, &nodes)?;
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
        if settings.fixed_node_id.as_deref() == Some(node_id) {
            settings.fixed_node_id = None;
            settings.line_mode = LineMode::Automatic;
        }
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

    pub fn set_acceleration(&self, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        if enabled && git::git_version().is_none() {
            return Err("未检测到系统 Git".into());
        }
        if enabled && settings.route_scope == RouteScope::Allowlist && self.routes()?.is_empty() {
            return Err("仅加速清单为空，请先加入至少一个公开仓库".into());
        }
        if enabled && settings.line_mode == LineMode::Direct {
            settings.line_mode = LineMode::Automatic;
        }
        settings.acceleration_enabled = enabled;
        let pairs = self.node_pairs()?;
        if enabled {
            self.select_current(&mut settings, &pairs)?;
        }
        self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    pub fn set_line_mode(
        &self,
        mode: LineMode,
        node_id: Option<&str>,
    ) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        let pairs = self.node_pairs()?;
        settings.line_mode = mode;
        match mode {
            LineMode::Fixed => {
                let id = node_id.ok_or_else(|| "固定模式需要选择节点".to_string())?;
                let usable = pairs.iter().any(|(node, health)| {
                    node.id == id
                        && node.enabled
                        && matches!(health.status, NodeStatus::Available | NodeStatus::Slow)
                });
                if !usable {
                    return Err("只能固定到已通过检测的节点".into());
                }
                settings.fixed_node_id = Some(id.into());
                settings.current_node_id = Some(id.into());
            }
            LineMode::Automatic => {
                settings.fixed_node_id = None;
                self.select_current(&mut settings, &pairs)?;
            }
            LineMode::Direct => {
                settings.current_node_id = None;
                settings.acceleration_enabled = false;
            }
        }
        self.write_configuration(&mut settings, &pairs, &self.routes()?)?;
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    fn select_current(
        &self,
        settings: &mut Settings,
        pairs: &[(NodeDefinition, HealthSummary)],
    ) -> Result<(), String> {
        settings.current_node_id = match settings.line_mode {
            LineMode::Automatic => git::choose_node(pairs).map(|node| node.id.clone()),
            LineMode::Fixed => settings.fixed_node_id.clone(),
            LineMode::Direct => None,
        };
        if settings.acceleration_enabled
            && settings.line_mode != LineMode::Direct
            && settings.current_node_id.is_none()
        {
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
        }
        let pairs = self.node_pairs()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        atomic_write_json(&self.paths.settings, &settings)?;
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
        routes.push(RouteEntry {
            id: Uuid::new_v4().to_string(),
            repository_url: normalized,
            created_at: Utc::now(),
        });
        let mut settings = settings;
        let pairs = self.node_pairs()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        atomic_write_json(&self.paths.routes, &routes)?;
        atomic_write_json(&self.paths.settings, &settings)?;
        self.snapshot()
    }

    pub fn delete_route(&self, route_id: &str) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut routes = self.routes()?;
        if !routes.iter().any(|route| route.id == route_id) {
            return Err("路由不存在".into());
        }
        routes.retain(|route| route.id != route_id);
        let mut settings = self.settings()?;
        let pairs = self.node_pairs()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        atomic_write_json(&self.paths.routes, &routes)?;
        atomic_write_json(&self.paths.settings, &settings)?;
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
                .find(|(node, _)| &node.id == id)
                .map(|(node, _)| node)
        });
        let content =
            git::build_config(settings, selected, routes, Some(&self.paths.trace_socket))?;
        let previous = fs::read(&self.paths.gitconfig).ok();
        let registered_before = git::include_registered(&self.paths.gitconfig);
        let (validation_url, expect_accelerated) = match settings.route_scope {
            RouteScope::Global => (TEST_REPOSITORY.to_string(), true),
            RouteScope::Allowlist => match routes.first() {
                Some(route) => (
                    route
                        .repository_url
                        .strip_suffix(".git")
                        .ok_or_else(|| "清单路由不是规范化的 GitHub HTTPS 仓库地址".to_string())?
                        .to_string(),
                    true,
                ),
                None => (TEST_REPOSITORY.to_string(), false),
            },
        };
        let validate = |config_path: &Path| -> Result<(), String> {
            let (fetch, push) = git::effective_urls(config_path, &validation_url)?;
            if expect_accelerated {
                let node = selected.ok_or_else(|| "没有通过检测的可用节点".to_string())?;
                if !fetch.starts_with(&node.rewrite_base) {
                    return Err("配置未能把 fetch 重写到所选节点".into());
                }
            }
            if !push.starts_with("https://github.com/") {
                return Err("配置的标准 push 未保持 GitHub 直连".into());
            }
            Ok(())
        };
        if settings.acceleration_enabled && settings.line_mode != LineMode::Direct {
            let mut candidate = NamedTempFile::new_in(&self.paths.root)
                .map_err(|error| format!("无法创建配置候选文件：{error}"))?;
            std::io::Write::write_all(&mut candidate, content.as_bytes())
                .map_err(|error| format!("无法写入配置候选：{error}"))?;
            candidate
                .as_file()
                .sync_all()
                .map_err(|error| format!("无法同步配置候选：{error}"))?;
            validate(candidate.path())?;
        }
        let _ = backup_file(
            &self.paths.gitconfig,
            &self.paths.backups,
            "gitboost.gitconfig",
        );
        atomic_write(&self.paths.gitconfig, content.as_bytes())?;
        if settings.acceleration_enabled || registered_before {
            if let Err(error) = git::register_include(&self.paths.gitconfig) {
                if let Some(bytes) = previous {
                    let _ = atomic_write(&self.paths.gitconfig, &bytes);
                }
                return Err(error);
            }
        }
        if settings.acceleration_enabled && settings.line_mode != LineMode::Direct {
            if let Err(error) = validate(&self.paths.gitconfig) {
                if let Some(bytes) = previous {
                    let _ = atomic_write(&self.paths.gitconfig, &bytes);
                }
                if !registered_before {
                    let _ = git::unregister_include(&self.paths.gitconfig);
                }
                return Err(format!("写入后的 Git 配置验证失败：{error}"));
            }
        }
        settings.last_applied_at = Some(Utc::now());
        let _ = append_log(
            &self.paths.logs,
            "INFO",
            &format!(
                "git configuration applied: enabled={}, scope={:?}, mode={:?}",
                settings.acceleration_enabled, settings.route_scope, settings.line_mode
            ),
        );
        Ok(())
    }

    pub fn update_settings(&self, minutes: u32, log_level: &str) -> Result<AppSnapshot, String> {
        if ![0, 15, 30, 60].contains(&minutes) {
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

    pub fn set_usage_logging(&self, enabled: bool) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.usage_logging_enabled = enabled;
        let pairs = self.node_pairs()?;
        let routes = self.routes()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        atomic_write_json(&self.paths.settings, &settings)?;
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
        if !git::include_registered(&self.paths.gitconfig) {
            return Ok(());
        }
        let mut settings = self.settings()?;
        let pairs = self.node_pairs()?;
        let routes = self.routes()?;
        self.write_configuration(&mut settings, &pairs, &routes)?;
        atomic_write_json(&self.paths.settings, &settings)
    }

    pub fn restore_git_config(&self) -> Result<AppSnapshot, String> {
        let _guard = self.lock.lock();
        let mut settings = self.settings()?;
        settings.acceleration_enabled = false;
        settings.line_mode = LineMode::Direct;
        settings.current_node_id = None;
        let content = git::build_config(&settings, None, &[], Some(&self.paths.trace_socket))?;
        let _ = backup_file(
            &self.paths.gitconfig,
            &self.paths.backups,
            "before-restore.gitconfig",
        );
        atomic_write(&self.paths.gitconfig, content.as_bytes())?;
        git::unregister_include(&self.paths.gitconfig)?;
        settings.last_applied_at = Some(Utc::now());
        atomic_write_json(&self.paths.settings, &settings)?;
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

    pub fn record_usage(&self, trace: CompletedTrace) -> Result<(), String> {
        let _guard = self.lock.lock();
        let settings = self.settings()?;
        if !settings.usage_logging_enabled {
            return Ok(());
        }
        let nodes = self.nodes()?;
        let matched_node = nodes
            .iter()
            .find(|node| trace.effective_url.starts_with(&node.rewrite_base));
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
        append_usage_event(
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
        )
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

fn sanitize_repository(raw: &str, nodes: &[NodeDefinition]) -> Option<String> {
    let logical = nodes
        .iter()
        .find_map(|node| raw.strip_prefix(&node.rewrite_base))
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

fn redact_import(input: &str) -> String {
    if input.contains("://") {
        git::sanitize_url(input)
    } else {
        "[无法解析的地址]".into()
    }
}

fn test_nodes_bounded<F>(
    nodes: &[NodeDefinition],
    old_health: &HashMap<String, HealthSummary>,
    test: F,
) -> Result<Vec<(String, HealthSummary)>, String>
where
    F: Fn(&NodeDefinition, &HealthSummary) -> HealthSummary + Sync,
{
    let next = AtomicUsize::new(0);
    let tested = Mutex::new(Vec::with_capacity(nodes.len()));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..NODE_TEST_CONCURRENCY.min(nodes.len()))
            .map(|_| {
                let next = &next;
                let tested = &tested;
                let test = &test;
                scope.spawn(move || loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(node) = nodes.get(index) else {
                        break;
                    };
                    let previous = old_health.get(&node.id).cloned().unwrap_or_default();
                    let summary = test(node, &previous);
                    tested.lock().push((index, node.id.clone(), summary));
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
    let urls: Vec<String> =
        serde_json::from_slice(bytes).map_err(|error| format!("系统节点目录格式错误：{error}"))?;
    normalize_system_urls(urls)
}

fn fetch_system_catalog() -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command.args([
        "--location",
        "--silent",
        "--show-error",
        "--fail",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "4",
        "--max-time",
        CATALOG_TIMEOUT_SECONDS,
        "--max-filesize",
        CATALOG_MAX_BYTES,
        SYSTEM_NODE_CATALOG_URL,
    ]);
    hide_catalog_console(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("无法更新系统节点：{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "无法更新系统节点：{}",
            detail.lines().next().unwrap_or("请求失败")
        ));
    }
    Ok(output.stdout)
}

#[cfg(target_os = "windows")]
fn hide_catalog_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(target_os = "windows"))]
fn hide_catalog_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!snapshot.settings.acceleration_enabled);
        assert!(snapshot.nodes[0].node.built_in);
        assert_eq!(snapshot.nodes[0].node.id, FASTGIT_REWRITE_BASE);
        let cached: Vec<String> = load_json(&directory.path().join("system-nodes.json")).unwrap();
        assert_eq!(cached, vec![FASTGIT_REWRITE_BASE]);
    }

    #[test]
    fn system_catalog_is_a_normalized_url_array() {
        let urls = parse_system_catalog(
            br#"["https://fastgit.cc", "https://fastgit.cc/https://github.com/"]"#,
        )
        .unwrap();
        assert_eq!(urls, vec![FASTGIT_REWRITE_BASE]);
        assert!(parse_system_catalog(br#"[]"#).is_err());
        assert!(parse_system_catalog(br#"["http://proxy.example"]"#).is_err());
        assert!(parse_system_catalog(br#"{"nodes":[]}"#).is_err());
        assert_eq!(
            parse_system_catalog(include_bytes!("../../nodes.json")).unwrap(),
            vec![FASTGIT_REWRITE_BASE]
        );
    }

    #[test]
    fn removing_current_system_node_immediately_writes_direct_git_config() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            line_mode: LineMode::Fixed,
            fixed_node_id: Some(FASTGIT_REWRITE_BASE.into()),
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

        let update = core
            .apply_system_catalog(br#"["https://proxy.example"]"#)
            .unwrap();
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
        assert_eq!(persisted.line_mode, LineMode::Direct);
        assert!(persisted.fixed_node_id.is_none());
        assert!(persisted.current_node_id.is_none());

        drop(core);
        let restarted = AppCore::new(directory.path().to_path_buf()).unwrap();
        restarted.refresh_registered_configuration().unwrap();
        assert_eq!(restarted.settings().unwrap().line_mode, LineMode::Direct);
    }

    #[test]
    fn full_node_tests_allow_only_one_active_run() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();

        let active = core.full_node_test_lock.lock();
        assert_eq!(
            core.test_all_nodes().err().as_deref(),
            Some("全量线路检测正在进行，请稍后再试")
        );
        drop(active);
        assert!(core.full_node_test_lock.try_lock().is_some());
    }

    #[test]
    fn catalog_refresh_waits_for_the_active_full_node_test() {
        use std::{sync::mpsc, time::Duration};

        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let active = core.full_node_test_lock.lock();
        let (waiting, wait_started) = mpsc::channel();
        let (acquired, slot_acquired) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                waiting.send(()).unwrap();
                let _slot = core.acquire_full_node_test(true).unwrap();
                acquired.send(()).unwrap();
            });
            wait_started.recv().unwrap();
            assert!(slot_acquired
                .recv_timeout(Duration::from_millis(30))
                .is_err());
            drop(active);
            slot_acquired.recv_timeout(Duration::from_secs(1)).unwrap();
        });
    }

    #[test]
    fn unchanged_catalog_does_not_queue_behind_an_active_test() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let _active = core.full_node_test_lock.lock();

        assert!(core.acquire_full_node_test(false).is_none());
    }

    #[test]
    fn catalog_recovery_does_not_override_a_newer_user_choice() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let settings = Settings {
            acceleration_enabled: true,
            route_scope: RouteScope::Global,
            line_mode: LineMode::Fixed,
            fixed_node_id: Some(FASTGIT_REWRITE_BASE.into()),
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
        let update = core
            .apply_system_catalog(br#"["https://proxy.example"]"#)
            .unwrap();
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
        assert_eq!(persisted.line_mode, LineMode::Direct);
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

        let tested = test_nodes_bounded(&nodes, &HashMap::new(), |_, _| {
            let running = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(running, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            active.fetch_sub(1, Ordering::SeqCst);
            HealthSummary::default()
        })
        .unwrap();

        assert_eq!(tested.len(), 8);
        assert_eq!(maximum.load(Ordering::SeqCst), NODE_TEST_CONCURRENCY);
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
    fn automatic_mode_can_be_selected_before_nodes_are_verified() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let pairs = vec![(NodeDefinition::fastgit(), HealthSummary::default())];
        let mut settings = Settings::default();
        core.select_current(&mut settings, &pairs).unwrap();
        assert!(settings.current_node_id.is_none());
        settings.acceleration_enabled = true;
        assert!(core.select_current(&mut settings, &pairs).is_err());
    }

    #[test]
    fn prepares_download_with_an_enabled_unverified_node() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        let target = core
            .prepare_download(
                "https://github.com/ollama/ollama/releases/download/v1/OllamaSetup.exe",
            )
            .unwrap();
        assert_eq!(target.node_name, "fastgit.cc");
        assert_eq!(
            target.accelerated_url,
            "https://fastgit.cc/https://github.com/ollama/ollama/releases/download/v1/OllamaSetup.exe"
        );
    }

    #[test]
    fn usage_audit_records_route_and_removes_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let core = AppCore::new(directory.path().to_path_buf()).unwrap();
        core.record_usage(CompletedTrace {
            occurred_at: Utc::now(),
            command: "clone".into(),
            original_url: Some(
                "https://secret-token@github.com/octocat/Hello-World.git?access=hidden".into(),
            ),
            effective_url: "https://fastgit.cc/https://github.com/octocat/Hello-World.git".into(),
            exit_code: 0,
            duration_ms: 321,
        })
        .unwrap();
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
}
