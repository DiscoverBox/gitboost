import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot, DiagnosticReport, DownloadTarget, ImportResult, NodeEntry, RouteScope, LineMode, UsageLogSnapshot } from "./types";

const browserMock: AppSnapshot = {
  settings: {
    schemaVersion: 1,
    accelerationEnabled: false,
    routeScope: "allowlist",
    lineMode: "automatic",
    fixedNodeId: null,
    currentNodeId: null,
    healthCheckMinutes: 30,
    launchAtLogin: false,
    logLevel: "info",
    usageLoggingEnabled: true,
    lastAppliedAt: null,
  },
  nodes: [
    {
      id: "https://fastgit.cc/https://github.com/",
      name: "fastgit.cc",
      rewriteBase: "https://fastgit.cc/https://github.com/",
      enabled: true,
      builtIn: true,
      health: {
        status: "untested",
        successCount: 0,
        attemptCount: 0,
        medianLatencyMs: null,
        consecutiveFailures: 0,
        checkedAt: null,
        failureReason: null,
      },
    },
  ],
  routes: [],
  environment: {
    gitAvailable: true,
    gitPath: "/usr/bin/git",
    gitVersion: "git version 2.51.1",
    includeRegistered: false,
    configPath: "~/Library/Application Support/pro.gitboost.desktop/gitboost.gitconfig",
    conflicts: 0,
    conflictScanError: null,
  },
};

const browserUsageMock: UsageLogSnapshot = {
  enabled: true,
  listening: true,
  configured: false,
  events: [],
  storagePath: "~/Library/Application Support/pro.gitboost.desktop/logs/usage.jsonl",
};

function inTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

async function call<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!inTauri()) throw new Error(`“${command}”只能在 GitBoost 桌面应用中执行。`);
  return invoke<T>(command, args);
}

export async function getSnapshot(): Promise<AppSnapshot> {
  return inTauri() ? call<AppSnapshot>("get_snapshot") : structuredClone(browserMock);
}

export const api = {
  importNodes: (text: string) => call<ImportResult>("import_nodes", { text }),
  importNodeFile: (path: string) => call<ImportResult>("import_node_file", { path }),
  exportNodes: (path: string) => call<string>("export_nodes", { path }),
  testNode: (nodeId: string) => call<NodeEntry>("test_node", { nodeId }),
  testAllNodes: () => call<NodeEntry[]>("test_all_nodes"),
  refreshSystemNodes: () => call<boolean>("refresh_system_nodes"),
  renameNode: (nodeId: string, name: string) => call<AppSnapshot>("rename_node", { nodeId, name }),
  setNodeEnabled: (nodeId: string, enabled: boolean) => call<AppSnapshot>("set_node_enabled", { nodeId, enabled }),
  deleteNode: (nodeId: string) => call<AppSnapshot>("delete_node", { nodeId }),
  setAcceleration: (enabled: boolean) => call<AppSnapshot>("set_acceleration", { enabled }),
  setLineMode: (mode: LineMode, nodeId?: string | null) => call<AppSnapshot>("set_line_mode", { mode, nodeId }),
  setRouteScope: (scope: RouteScope) => call<AppSnapshot>("set_route_scope", { scope }),
  addRoute: (repositoryUrl: string) => call<AppSnapshot>("add_route", { repositoryUrl }),
  deleteRoute: (routeId: string) => call<AppSnapshot>("delete_route", { routeId }),
  runDiagnostics: (repositoryPath?: string) => call<DiagnosticReport>("run_diagnostics", { repositoryPath: repositoryPath || null }),
  updateSettings: (healthCheckMinutes: number, logLevel: string) =>
    call<AppSnapshot>("update_settings", { healthCheckMinutes, logLevel }),
  updateLaunchAtLogin: (enabled: boolean) => call<AppSnapshot>("update_launch_at_login", { enabled }),
  restoreGitConfig: () => call<AppSnapshot>("restore_git_config"),
  clearLogs: () => call<AppSnapshot>("clear_logs"),
  getUsageLog: () => inTauri() ? call<UsageLogSnapshot>("get_usage_log") : Promise.resolve(structuredClone(browserUsageMock)),
  setUsageLogging: (enabled: boolean) => call<AppSnapshot>("set_usage_logging", { enabled }),
  prepareDownload: (originalUrl: string, excludedNodeIds: string[]) => call<DownloadTarget>("prepare_download", { originalUrl, excludedNodeIds }),
  openDownload: (originalUrl: string, nodeId: string) => call<DownloadTarget>("open_download", { originalUrl, nodeId }),
};
