import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { disable as disableAutostart, enable as enableAutostart, isEnabled as autostartEnabled } from "@tauri-apps/plugin-autostart";
import { api, getSnapshot } from "./api";
import type { AppSnapshot, DiagnosticReport, DownloadTarget, ImportResult, LineMode, NodeEntry, PageKey, RouteScope, UsageLogSnapshot } from "./types";
import { currentNode, formatLatency, formatRelativeTime, lineStatusLabel, statusLabel, statusTone, successRate } from "./utils";

const navItems: { key: PageKey; label: string }[] = [
  { key: "overview", label: "总览" },
  { key: "routes", label: "路由清单" },
  { key: "downloads", label: "文件下载" },
  { key: "usage", label: "使用日志" },
  { key: "diagnostics", label: "环境诊断" },
  { key: "settings", label: "设置" },
];
const usesNativeTitleBar = navigator.userAgent.includes("Windows");

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function Button({ children, tone = "secondary", busy, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: "primary" | "secondary" | "danger" | "quiet"; busy?: boolean }) {
  return (
    <button className={`button button--${tone}`} disabled={busy || props.disabled} {...props}>
      {busy ? "处理中…" : children}
    </button>
  );
}

function StatusDot({ status }: { status: NodeEntry["health"]["status"] }) {
  return <span className={`status-dot status-dot--${statusTone(status)}`} aria-hidden="true" />;
}

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (checked: boolean) => void; label: string }) {
  return (
    <button type="button" className={`switch ${checked ? "switch--on" : ""}`} role="switch" aria-checked={checked} aria-label={label} onClick={() => onChange(!checked)}>
      <span />
    </button>
  );
}

function Segmented<T extends string>({ value, options, onChange }: { value: T; options: { value: T; label: string }[]; onChange: (value: T) => void }) {
  return (
    <div className="segmented">
      {options.map((option) => (
        <button key={option.value} className={value === option.value ? "is-active" : ""} onClick={() => onChange(option.value)}>
          {option.label}
        </button>
      ))}
    </div>
  );
}

function PageHeader({ eyebrow, title, description, actions }: { eyebrow: string; title: string; description: string; actions?: React.ReactNode }) {
  return (
    <header className="page-header">
      <div>
        <p className="eyebrow">{eyebrow}</p>
        <h1>{title}</h1>
        <p className="page-description">{description}</p>
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </header>
  );
}

function Modal({ title, children, onClose }: { title: string; children: React.ReactNode; onClose: () => void }) {
  useEffect(() => {
    const close = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section className="modal" role="dialog" aria-modal="true" aria-label={title} onMouseDown={(event) => event.stopPropagation()}>
        <header><h2>{title}</h2><button className="close-button" onClick={onClose} aria-label="关闭">×</button></header>
        {children}
      </section>
    </div>
  );
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [page, setPage] = useState<PageKey>("overview");
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<{ kind: "success" | "error"; text: string } | null>(null);
  const [importOpen, setImportOpen] = useState(false);

  const reload = useCallback(async () => setSnapshot(await getSnapshot()), []);
  useEffect(() => { reload().catch((error) => setToast({ kind: "error", text: errorMessage(error) })); }, [reload]);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const snapshotListener = listen<AppSnapshot>("snapshot-updated", (event) => setSnapshot(event.payload));
    const errorListener = listen<string>("operation-error", (event) => setToast({ kind: "error", text: event.payload }));
    return () => { snapshotListener.then((unlisten) => unlisten()); errorListener.then((unlisten) => unlisten()); };
  }, []);
  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 4200);
    return () => window.clearTimeout(timer);
  }, [toast]);

  const run = useCallback(async <T,>(key: string, operation: () => Promise<T>, message?: string | ((result: T) => string | undefined)) => {
    setBusy(key);
    try {
      const result = await operation();
      if (result && typeof result === "object" && "settings" in result && "nodes" in result && "routes" in result && "environment" in result) {
        setSnapshot(result as unknown as AppSnapshot);
      }
      else await reload();
      const successMessage = typeof message === "function" ? message(result) : message;
      if (successMessage) setToast({ kind: "success", text: successMessage });
      return result;
    } catch (error) {
      setToast({ kind: "error", text: errorMessage(error) });
      throw error;
    } finally {
      setBusy(null);
    }
  }, [reload]);

  if (!snapshot) return <div className="splash"><div className="wordmark">GitBoost</div><p>正在读取 Git 环境…</p></div>;

  return (
    <div className={`app-shell ${usesNativeTitleBar ? "app-shell--native-titlebar" : ""}`}>
      {!usesNativeTitleBar && <div className="window-drag" data-tauri-drag-region />}
      <aside className="sidebar">
        <div className="brand-block">
          <div className="wordmark">GitBoost</div>
          <span>GitHub 读取线路</span>
        </div>
        <nav aria-label="主要导航">
          {navItems.map((item) => <button key={item.key} className={page === item.key ? "is-active" : ""} onClick={() => setPage(item.key)}>{item.label}</button>)}
        </nav>
        <div className="sidebar-status">
          <div className="line-glyph" data-on={snapshot.settings.accelerationEnabled}><span /><span /><span /></div>
          <div><strong>{snapshot.settings.accelerationEnabled ? "加速已开启" : "当前为直连"}</strong><small>{lineStatusLabel(snapshot.settings.accelerationEnabled, snapshot.settings.lineMode)}</small></div>
        </div>
      </aside>
      <main className="main-content">
        {page === "overview" && <Overview snapshot={snapshot} busy={busy} run={run} go={setPage} />}
        {page === "routes" && <Routes snapshot={snapshot} busy={busy} run={run} />}
        {page === "downloads" && <Downloads busy={busy} run={run} />}
        {page === "usage" && <UsageLogs snapshot={snapshot} busy={busy} run={run} />}
        {page === "diagnostics" && <Diagnostics snapshot={snapshot} busy={busy} />}
        {page === "settings" && <Settings snapshot={snapshot} busy={busy} run={run} onImport={() => setImportOpen(true)} />}
      </main>
      {toast && <div className={`toast toast--${toast.kind}`}>{toast.text}</div>}
      {importOpen && <ImportNodes busy={busy} run={run} onClose={() => setImportOpen(false)} />}
    </div>
  );
}

type Runner = <T>(key: string, operation: () => Promise<T>, message?: string | ((result: T) => string | undefined)) => Promise<T>;

function Overview({ snapshot, busy, run, go }: { snapshot: AppSnapshot; busy: string | null; run: Runner; go: (page: PageKey) => void }) {
  const node = currentNode(snapshot.nodes, snapshot.settings.currentNodeId);
  const usable = snapshot.nodes.filter((item) => item.enabled && ["available", "slow"].includes(item.health.status));
  const lineModeOptions: { value: LineMode; label: string }[] = [
    { value: "automatic", label: "自动选择" },
    ...(snapshot.settings.lineMode === "fixed" ? [{ value: "fixed" as const, label: "固定节点" }] : []),
    { value: "direct", label: "直连" },
  ];
  const toggle = () => run("toggle", () => api.setAcceleration(!snapshot.settings.accelerationEnabled), snapshot.settings.accelerationEnabled ? "已恢复 GitHub 直连" : "加速配置已写入并验证").catch(() => undefined);
  return (
    <div className="page">
      <PageHeader eyebrow="运行状态" title={snapshot.settings.accelerationEnabled ? "读取线路已接入" : "使用 GitHub 原地址，按需加速"} description="GitBoost 只改写公开仓库的读取地址；仓库中保存的 origin 保持不变。" actions={<Button tone={snapshot.settings.accelerationEnabled ? "secondary" : "primary"} busy={busy === "toggle"} onClick={toggle}>{snapshot.settings.accelerationEnabled ? "关闭加速" : "开启加速"}</Button>} />

      <section className="status-board">
        <div className="status-primary">
          <div className={`status-ring ${snapshot.settings.accelerationEnabled ? "is-on" : ""}`}><span /></div>
          <div><span className="label">Git 配置</span><strong>{snapshot.settings.accelerationEnabled ? "已生效" : "未接入"}</strong><p>{snapshot.environment.includeRegistered ? "独立配置已注册" : "首次开启时注册独立配置"}</p></div>
        </div>
        <dl className="status-facts">
          <div><dt>路由范围</dt><dd>{snapshot.settings.routeScope === "allowlist" ? "仅加速清单" : "全局加速"}</dd></div>
          <div><dt>线路模式</dt><dd>{snapshot.settings.lineMode === "automatic" ? "自动选择" : snapshot.settings.lineMode === "fixed" ? "固定节点" : "GitHub 直连"}</dd></div>
          <div><dt>连接质量</dt><dd>{node ? `${statusLabel[node.health.status]} · ${formatLatency(node.health.medianLatencyMs)}` : "尚未选路"}</dd></div>
          <div><dt>最近检测</dt><dd>{node ? formatRelativeTime(node.health.checkedAt) : "尚未检测"}</dd></div>
        </dl>
      </section>

      {!snapshot.environment.gitAvailable && <div className="notice notice--danger"><strong>未检测到系统 Git</strong><p>请先安装 Git（Windows 推荐 Git for Windows），然后再开启加速。</p></div>}
      {snapshot.environment.conflictScanError ? <div className="notice notice--danger"><strong>URL 重写冲突检查失败</strong><p>{snapshot.environment.conflictScanError}</p><Button tone="quiet" onClick={() => go("diagnostics")}>查看诊断</Button></div> : snapshot.environment.conflicts > 0 && <div className="notice notice--warning"><strong>发现 {snapshot.environment.conflicts} 条 URL 重写冲突</strong><p>GitBoost 不会覆盖它们。请先到环境诊断查看来源。</p><Button tone="quiet" onClick={() => go("diagnostics")}>查看诊断</Button></div>}
      {!usable.length && <div className="notice"><strong>还没有通过验证的线路</strong><p>系统线路和自定义节点都必须先通过真实 Git 检测。</p><Button tone="quiet" busy={busy === "test-all"} onClick={() => run("test-all", api.testAllNodes, "线路检测完成").catch(() => undefined)}>立即检测</Button></div>}

      <section className="section-block">
        <div className="section-title"><div><h2>线路控制</h2><p>切换只影响下一次 Git 操作；已开始的 clone 需要手动重试。</p></div><Button busy={busy === "test-all"} onClick={() => run("test-all", api.testAllNodes, "节点检测完成").catch(() => undefined)}>重新测速</Button></div>
        <div className="control-row">
          <label>线路模式</label>
          <Segmented<LineMode> value={snapshot.settings.lineMode} options={lineModeOptions} onChange={(mode) => run("line-mode", () => api.setLineMode(mode, mode === "fixed" ? snapshot.settings.fixedNodeId : null), "线路模式已更新").catch(() => undefined)} />
        </div>
      </section>

      <footer className="page-footnote">应用退出后，最后一次成功写入的 Git 配置仍会生效。第三方节点可看到被访问的公开仓库路径和传输内容。</footer>
    </div>
  );
}

function CustomNodeSettings({ snapshot, busy, run, onImport }: { snapshot: AppSnapshot; busy: string | null; run: Runner; onImport: () => void }) {
  const [editing, setEditing] = useState<NodeEntry | null>(null);
  const customNodes = snapshot.nodes.filter((node) => !node.builtIn);
  const systemCount = snapshot.nodes.length - customNodes.length;
  return (
    <section className="section-block node-settings" aria-labelledby="custom-nodes-title">
      <div className="section-title">
        <div><h2 id="custom-nodes-title">自定义节点</h2><p>添加并管理自己的代理地址；系统节点仍由 GitBoost 自动维护。</p></div>
        <div className="section-actions"><Button onClick={onImport}>添加节点</Button><Button busy={busy === "test-all"} onClick={() => run("test-all", api.testAllNodes, "全部线路检测完成").catch(() => undefined)}>检测全部</Button></div>
      </div>
      <div className="node-settings-list">
        {customNodes.map((node) => (
          <div className={`node-setting-row ${!node.enabled ? "is-disabled" : ""}`} key={node.id}>
            <div className="node-name"><StatusDot status={node.health.status} /><div><strong>{node.name}</strong><code>{node.rewriteBase}</code></div></div>
            <div className="node-health">
              <strong className={`status-text status-text--${statusTone(node.health.status)}`}>{statusLabel[node.health.status]}</strong>
              <span>{successRate(node.health)} 成功 · {formatLatency(node.health.medianLatencyMs)}</span>
              <span>{formatRelativeTime(node.health.checkedAt)}</span>
            </div>
            <div className="row-actions"><Button tone="quiet" busy={busy === `test-${node.id}`} onClick={() => run(`test-${node.id}`, () => api.testNode(node.id), `${node.name} 检测完成`).catch(() => undefined)}>检测</Button><button className="more-button" onClick={() => setEditing(node)} aria-label={`管理 ${node.name}`}>•••</button></div>
            {node.health.failureReason && <p className="row-detail">{node.health.failureReason}</p>}
          </div>
        ))}
        {!customNodes.length && <div className="empty-state compact"><strong>还没有自定义节点</strong><p>通常无需添加，GitBoost 会自动使用系统线路。</p></div>}
      </div>
      <p className="node-settings-note">系统线路 {systemCount} 个 · 使用公开仓库进行隔离检测，不修改全局 Git 配置</p>
      {editing && <ManageNode node={editing} busy={busy} run={run} onClose={() => setEditing(null)} />}
    </section>
  );
}

function ManageNode({ node, busy, run, onClose }: { node: NodeEntry; busy: string | null; run: Runner; onClose: () => void }) {
  const [name, setName] = useState(node.name);
  const usable = node.enabled && (node.health.status === "available" || node.health.status === "slow");
  return (
    <Modal title="管理节点" onClose={onClose}>
      <div className="modal-body"><label className="field"><span>名称</span><input value={name} onChange={(event) => setName(event.target.value)} /></label><div className="readonly-field"><span>重写前缀</span><code>{node.rewriteBase}</code></div><div className="setting-row"><div><strong>参与自动选择</strong><p>停用后保留检测记录，但不会被选中。</p></div><Switch label="参与自动选择" checked={node.enabled} onChange={(enabled) => run("node-enable", () => api.setNodeEnabled(node.id, enabled), enabled ? "节点已启用" : "节点已停用").catch(() => undefined)} /></div></div>
      <footer className="modal-footer"><Button tone="danger" onClick={() => run("node-delete", () => api.deleteNode(node.id), "节点已删除").then(onClose).catch(() => undefined)}>删除</Button><Button disabled={!usable} onClick={() => run("node-fix", () => api.setLineMode("fixed", node.id), `已固定到 ${node.name}`).then(onClose).catch(() => undefined)}>{usable ? "固定此节点" : "检测通过后可固定"}</Button><span /><Button onClick={onClose}>取消</Button><Button tone="primary" busy={busy === "node-save"} disabled={!name.trim()} onClick={() => run("node-save", () => api.renameNode(node.id, name.trim()), "名称已保存").then(onClose).catch(() => undefined)}>保存</Button></footer>
    </Modal>
  );
}

function ImportNodes({ busy, run, onClose }: { busy: string | null; run: Runner; onClose: () => void }) {
  const [text, setText] = useState("");
  const [result, setResult] = useState<ImportResult | null>(null);
  const finishImport = (next: ImportResult) => {
    if (next.imported > 0 && next.rejected.length === 0) onClose();
    else setResult(next);
  };
  const importMessage = (next: ImportResult) => {
    if (next.imported === 0) return undefined;
    const details = [`已添加 ${next.imported} 个节点`];
    if (next.duplicates > 0) details.push(`跳过 ${next.duplicates} 个重复地址`);
    if (next.rejected.length > 0) details.push(`${next.rejected.length} 个地址未添加`);
    return details.join("，");
  };
  const importFile = async () => {
    const selected = await open({ multiple: false, filters: [{ name: "GitBoost 节点", extensions: ["json"] }] });
    if (typeof selected === "string") {
      setResult(null);
      finishImport(await run("import-file", () => api.importNodeFile(selected), importMessage));
    }
  };
  const importText = async () => {
    setResult(null);
    finishImport(await run("import-text", () => api.importNodes(text), importMessage));
  };
  return (
    <Modal title="添加节点" onClose={onClose}>
      <div className="modal-body"><p className="modal-intro">每行输入一个代理地址，应用会自动补全 GitHub 重写路径。</p><textarea className="import-area" value={text} onChange={(event) => { setText(event.target.value); setResult(null); }} placeholder="https://proxy.example" aria-label="代理地址" spellCheck={false} /><p className="field-help">仅接受 HTTPS。含用户名、密码、Token、查询参数、片段或占位符的地址会被拒绝。</p>{result && <div className={`import-result ${result.rejected.length > 0 || result.imported === 0 ? "import-result--warning" : ""}`} aria-live="polite"><strong>{result.imported > 0 ? `已添加 ${result.imported} 个节点` : "没有添加新节点"}</strong><p>{[result.duplicates > 0 ? `${result.duplicates} 个重复地址已跳过` : "", result.rejected.length > 0 ? `${result.rejected.length} 个地址未添加` : ""].filter(Boolean).join("；") || "输入中没有可添加的节点。"}</p>{result.rejected.length > 0 && <ul>{result.rejected.map((item, index) => <li key={`${item.input}-${index}`}><code>{item.input}</code><span>{item.reason}</span></li>)}</ul>}</div>}</div>
      <footer className="modal-footer"><Button busy={busy === "import-file"} onClick={() => importFile().catch(() => undefined)}>从 JSON 导入</Button><span /><Button onClick={onClose}>取消</Button><Button tone="primary" busy={busy === "import-text"} disabled={!text.trim()} onClick={() => importText().catch(() => undefined)}>添加</Button></footer>
    </Modal>
  );
}

function Routes({ snapshot, busy, run }: { snapshot: AppSnapshot; busy: string | null; run: Runner }) {
  const [url, setUrl] = useState("");
  const [query, setQuery] = useState("");
  const global = snapshot.settings.routeScope === "global";
  const normalizedQuery = query.trim().toLowerCase();
  const visibleRoutes = snapshot.routes.filter((route) => route.repositoryUrl.toLowerCase().includes(normalizedQuery));
  return (
    <div className="page">
      <PageHeader eyebrow="安全边界" title="路由清单" description={global ? "所有 GitHub HTTPS 仓库读取都会经过当前加速节点。" : "只有清单中的公开仓库会走外部节点。"} />
      <section className="scope-picker"><div><strong>路由范围</strong><p>访问私有仓库或不确定时，建议仅加速清单。</p></div><Segmented<RouteScope> value={snapshot.settings.routeScope} options={[{ value: "allowlist", label: "仅加速清单" }, { value: "global", label: "全局加速" }]} onChange={(scope) => run("route-scope", () => api.setRouteScope(scope), "路由范围已更新").catch(() => undefined)} /></section>
      {global ? <div className="notice notice--warning"><strong>全局加速不会使用项目清单</strong><p>Git 无法自动区分公开和私有仓库，所有 GitHub HTTPS 读取都会经过当前节点。</p></div> :
        <section className="section-block route-editor">
          <div className="section-title"><div><h2>公开加速仓库</h2><p>输入 owner/repository，或完整的 GitHub HTTPS 地址；可带或不带 .git。</p></div></div>
          <div className="route-input"><input value={url} onChange={(event) => setUrl(event.target.value)} placeholder="owner/repository 或完整 GitHub 地址" aria-label="GitHub 仓库" onKeyDown={(event) => { if (event.key === "Enter" && url.trim()) run("add-route", () => api.addRoute(url), "路由已加入清单").then(() => setUrl("")).catch(() => undefined); }} /><Button tone="primary" busy={busy === "add-route"} disabled={!url.trim()} onClick={() => run("add-route", () => api.addRoute(url), "路由已加入清单").then(() => setUrl("")).catch(() => undefined)}>加入清单</Button></div>
          <div className="route-list-toolbar"><div className="route-list-summary"><strong>已添加仓库</strong><span>{normalizedQuery ? `${visibleRoutes.length} / ${snapshot.routes.length} 个` : `${snapshot.routes.length} 个`}</span></div>{snapshot.routes.length > 0 && <div className="route-search"><input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索仓库地址" aria-label="搜索已添加仓库" /></div>}</div>
          <div className="route-list">
            {visibleRoutes.map((route) => <div key={route.id}><div><code>{route.repositoryUrl}</code><span>加速</span></div><Button tone="quiet" onClick={() => run("delete-route", () => api.deleteRoute(route.id), "路由已删除").catch(() => undefined)}>删除</Button></div>)}
            {!snapshot.routes.length && <div className="empty-state compact"><strong>清单为空</strong><p>添加需要加速的公开仓库后即可启用。</p></div>}
            {snapshot.routes.length > 0 && !visibleRoutes.length && <div className="empty-state compact"><strong>没有匹配的仓库</strong><p>换个关键词试试。</p></div>}
          </div>
        </section>}
    </div>
  );
}

function Downloads({ busy, run }: { busy: string | null; run: Runner }) {
  const [url, setUrl] = useState("");
  const [target, setTarget] = useState<DownloadTarget | null>(null);
  const download = async () => {
    const next = await run("download-open", () => api.openDownload(url), "已通过节点在浏览器中打开下载");
    setTarget(next);
  };
  const changeUrl = (value: string) => {
    setUrl(value);
    setTarget(null);
  };
  return (
    <div className="page">
      <PageHeader eyebrow="Release 文件" title="文件下载" description="粘贴公开 GitHub Release 文件地址；GitBoost 会先通过当前线路做小流量探测，再交给默认浏览器下载。" />
      <section className="download-card">
        <label htmlFor="download-url">GitHub 文件地址</label>
        <div className="download-input">
          <input id="download-url" value={url} onChange={(event) => changeUrl(event.target.value)} placeholder="https://github.com/owner/repo/releases/download/v1.0/file.zip" spellCheck={false} onKeyDown={(event) => { if (event.key === "Enter" && url.trim()) download().catch(() => undefined); }} />
          <Button tone="primary" busy={busy === "download-open"} disabled={!url.trim()} onClick={() => download().catch(() => undefined)}>开始下载</Button>
        </div>
        <p>只检查地址格式，不会直连 GitHub 验证文件；实际可用性由当前线路探测。</p>
      </section>
      <section className="section-block">
        <div className="section-title"><div><h2>下载线路</h2><p>下载操作独立于 Git 路由清单。节点失败时不会静默改为 GitHub 直连。</p></div></div>
        {target ? <dl className="download-target"><div><dt>文件</dt><dd>{target.fileName}</dd></div><div><dt>线路</dt><dd>GitHub 加速</dd></div></dl> : <div className="empty-state compact"><strong>等待下载地址</strong><p>支持 releases/download 和 releases/latest/download。</p></div>}
      </section>
      <footer className="page-footnote">第三方节点可以看到公开仓库路径和文件名。GitBoost 不支持带凭据、Token、查询参数或片段的地址。</footer>
    </div>
  );
}

function Diagnostics({ snapshot, busy }: { snapshot: AppSnapshot; busy: string | null }) {
  const [repositoryPath, setRepositoryPath] = useState("");
  const [report, setReport] = useState<DiagnosticReport | null>(null);
  const [diagnosticError, setDiagnosticError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const diagnose = async () => { setRunning(true); setDiagnosticError(null); setReport(null); try { setReport(await api.runDiagnostics(repositoryPath)); } catch (error) { setDiagnosticError(errorMessage(error)); } finally { setRunning(false); } };
  const copy = () => report && navigator.clipboard.writeText(report.reportText);
  return (
    <div className="page">
      <PageHeader eyebrow="可解释性" title="环境诊断" description="检查 Git 路径、独立配置、URL 重写冲突和显式 pushurl；报告会自动脱敏。" actions={<Button tone="primary" busy={running || busy === "diagnose"} onClick={() => diagnose().catch(() => undefined)}>运行诊断</Button>} />
      <section className="diagnostic-summary">
        <div><span>系统 Git</span><strong>{snapshot.environment.gitAvailable ? snapshot.environment.gitVersion : "未找到"}</strong><code>{snapshot.environment.gitPath ?? "—"}</code></div>
        <div><span>独立配置</span><strong>{snapshot.environment.includeRegistered ? "已注册" : "未注册"}</strong><code>{snapshot.environment.configPath}</code></div>
        <div><span>重写冲突</span><strong className={snapshot.environment.conflicts || snapshot.environment.conflictScanError ? "danger-text" : ""}>{snapshot.environment.conflictScanError ? "检查失败" : snapshot.environment.conflicts ? `${snapshot.environment.conflicts} 条` : "未发现"}</strong><p>{snapshot.environment.conflictScanError ?? "不覆盖其他应用或用户配置。"}</p></div>
      </section>
      <section className="section-block">
        <div className="section-title"><div><h2>仓库检查</h2><p>可选。检查你主动指定的本地仓库是否设置了显式 pushurl。</p></div></div>
        <div className="route-input"><input value={repositoryPath} onChange={(event) => setRepositoryPath(event.target.value)} placeholder="仓库本地路径（可留空）" /><Button onClick={() => setRepositoryPath("")}>清空</Button></div>
      </section>
      {diagnosticError && <div className="notice notice--danger"><strong>诊断运行失败</strong><p>{diagnosticError}</p></div>}
      {report && <section className="report-block"><header><div><h2>诊断结果</h2><p>{new Date(report.generatedAt).toLocaleString("zh-CN")}</p></div><Button onClick={copy}>复制脱敏报告</Button></header><dl><div><dt>保存值</dt><dd><code>{report.originalUrl}</code></dd></div><div><dt>fetch 有效地址</dt><dd><code>{report.fetchUrl ?? "无法解析"}</code></dd></div><div><dt>push 有效地址</dt><dd><code>{report.pushUrl ?? "无法解析"}</code></dd></div><div><dt>显式 pushurl</dt><dd className={report.explicitPushUrl || report.repositoryError ? "danger-text" : ""}><code>{report.repositoryError ? "检查失败" : report.explicitPushUrl ?? "未检测到"}</code></dd></div></dl>{report.warnings.map((warning) => <div className="report-warning" key={warning}>{warning}</div>)}</section>}
    </div>
  );
}

function UsageLogs({ snapshot, busy, run }: { snapshot: AppSnapshot; busy: string | null; run: Runner }) {
  const [usage, setUsage] = useState<UsageLogSnapshot | null>(null);
  const load = useCallback(() => api.getUsageLog().then(setUsage), []);
  useEffect(() => {
    load().catch(() => undefined);
    const timer = window.setInterval(() => load().catch(() => undefined), 5000);
    return () => window.clearInterval(timer);
  }, [load]);
  const copy = () => {
    if (!usage) return;
    const text = usage.events.map((event) => [
      new Date(event.occurredAt).toLocaleString("zh-CN"),
      `git ${event.command}`,
      event.repository,
      event.route === "accelerated" ? `${event.nodeName ?? "加速节点"} (${event.connectionHost})` : event.route === "direct" ? `GitHub 直连 (${event.connectionHost})` : `其他重写 (${event.connectionHost})`,
      event.succeeded ? "成功" : `失败 (${event.exitCode})`,
      `${event.durationMs} ms`,
    ].join("\t")).join("\n");
    navigator.clipboard.writeText(text);
  };
  const auditActive = Boolean(usage?.enabled && usage.listening && usage.configured);
  return (
    <div className="page">
      <PageHeader eyebrow="实际连接" title="使用日志" description="记录 Git 实际启动的 HTTPS 远端连接，用来确认本次 clone、fetch 或 pull 究竟走了加速节点还是 GitHub 直连。" actions={<><Button onClick={copy} disabled={!usage?.events.length}>复制日志</Button><Button busy={busy === "usage-clear"} onClick={() => run("usage-clear", api.clearLogs, "本地日志已清理").then(load).catch(() => undefined)}>清空</Button></>} />
      <section className="audit-summary">
        <div><span className={`audit-light ${auditActive ? "is-on" : ""}`} /><div><small>审计状态</small><strong>{auditActive ? "正在记录" : usage?.enabled ? "等待接入" : "已关闭"}</strong></div></div>
        <div><small>配置状态</small><strong>{usage?.configured ? "已写入 Git 全局包含项" : "开启加速后接入"}</strong></div>
        <div><small>本机记录</small><strong>{usage ? `${usage.events.length} 条` : "读取中…"}</strong></div>
        <Switch label="记录 Git 使用" checked={usage?.enabled ?? snapshot.settings.usageLoggingEnabled} onChange={(enabled) => run("usage-toggle", () => api.setUsageLogging(enabled), enabled ? "使用日志已开启" : "使用日志已关闭").then(load).catch(() => undefined)} />
      </section>
      <div className="audit-note"><strong>只保存脱敏结果</strong><span>不保存原始命令、Token、用户名、查询参数或环境变量。GitBoost 自己的节点检测不会出现在这里。</span></div>
      <section className="usage-table">
        <div className="usage-grid usage-head"><span>时间</span><span>操作 / 仓库</span><span>实际线路</span><span>结果</span><span>耗时</span></div>
        {usage?.events.map((event) => (
          <div className="usage-grid usage-row" key={event.id}>
            <time>{new Date(event.occurredAt).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" })}</time>
            <div className="usage-repo"><strong>git {event.command}</strong><code title={event.repository}>{event.repository}</code></div>
            <div><span className={`route-badge route-badge--${event.route}`}>{event.route === "accelerated" ? event.nodeName ?? "加速" : event.route === "direct" ? "GitHub 直连" : "其他重写"}</span><small>{event.connectionHost}</small></div>
            <span className={event.succeeded ? "success-text" : "danger-text"}>{event.succeeded ? "成功" : `失败 · ${event.exitCode}`}</span>
            <span>{event.durationMs < 1000 ? `${event.durationMs} ms` : `${(event.durationMs / 1000).toFixed(1)} s`}</span>
          </div>
        ))}
        {usage && !usage.events.length && <div className="empty-state"><strong>还没有实际连接记录</strong><p>保持 GitBoost 在后台运行，然后执行一次 git clone、git fetch 或 git pull。</p></div>}
      </section>
      {usage && <footer className="page-footnote">日志仅保存在本机并自动保留最近 7 天：{usage.storagePath}</footer>}
    </div>
  );
}

function Settings({ snapshot, busy, run, onImport }: { snapshot: AppSnapshot; busy: string | null; run: Runner; onImport: () => void }) {
  const [minutes, setMinutes] = useState(snapshot.settings.healthCheckMinutes);
  const [logLevel, setLogLevel] = useState(snapshot.settings.logLevel);
  const [launchAtLogin, setLaunchAtLogin] = useState(snapshot.settings.launchAtLogin);
  useEffect(() => { autostartEnabled().then(setLaunchAtLogin).catch(() => undefined); }, []);
  const saveNodes = async () => { const path = await save({ defaultPath: "gitboost-nodes.json", filters: [{ name: "JSON", extensions: ["json"] }] }); if (path) await run("export", () => api.exportNodes(path), "自定义节点已导出"); };
  const setAutostart = async (enabled: boolean) => {
    enabled ? await enableAutostart() : await disableAutostart();
    setLaunchAtLogin(enabled);
    await run("autostart", () => api.updateLaunchAtLogin(enabled), enabled ? "已设为登录时启动" : "已关闭登录时启动");
  };
  return (
    <div className="page">
      <PageHeader eyebrow="本机偏好" title="设置" description="GitBoost 不上传使用数据；节点、健康状态和诊断日志均保存在本机。" actions={<Button tone="primary" busy={busy === "settings-save"} onClick={() => run("settings-save", () => api.updateSettings(minutes, logLevel), "设置已保存").catch(() => undefined)}>保存设置</Button>} />
      <section className="settings-list">
        <div className="setting-row"><div><strong>后台健康检查</strong><p>应用运行时定期检测，只影响下一次 Git 操作。</p></div><select value={minutes} onChange={(event) => setMinutes(Number(event.target.value))}><option value={0}>关闭</option><option value={15}>每 15 分钟</option><option value={30}>每 30 分钟</option><option value={60}>每小时</option></select></div>
        <div className="setting-row"><div><strong>登录时启动</strong><p>保持托盘运行，以便节点失效后重新选路。</p></div><Switch label="登录时启动" checked={launchAtLogin} onChange={(enabled) => setAutostart(enabled).catch(() => undefined)} /></div>
        <div className="setting-row"><div><strong>日志级别</strong><p>日志会移除凭据、查询参数和命令环境。</p></div><select value={logLevel} onChange={(event) => setLogLevel(event.target.value as "error" | "info" | "debug")}><option value="error">仅错误</option><option value="info">信息</option><option value="debug">调试</option></select></div>
      </section>
      <CustomNodeSettings snapshot={snapshot} busy={busy} run={run} onImport={onImport} />
      <section className="section-block maintenance"><div className="section-title"><div><h2>数据与恢复</h2><p>恢复只清空 GitBoost 自己的重写规则，不修改仓库 remote。</p></div></div><div className="maintenance-actions"><Button busy={busy === "export"} onClick={() => saveNodes().catch(() => undefined)}>导出自定义节点</Button><Button busy={busy === "clear-logs"} onClick={() => run("clear-logs", api.clearLogs, "本地日志已清理").catch(() => undefined)}>清理日志</Button><Button tone="danger" busy={busy === "restore"} onClick={() => run("restore", api.restoreGitConfig, "GitBoost 配置已恢复为直连").catch(() => undefined)}>恢复 Git 配置</Button></div></section>
      <div className="about-line"><span>GitBoost 0.1.0 · macOS / Windows</span><span>本地运行 · 无账号 · 无遥测</span></div>
    </div>
  );
}
