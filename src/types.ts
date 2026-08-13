export type PageKey = "overview" | "nodes" | "routes" | "downloads" | "usage" | "diagnostics" | "settings";
export type RouteScope = "allowlist" | "global";
export type LineMode = "automatic" | "fixed" | "direct";
export type NodeStatus = "untested" | "available" | "slow" | "incompatible" | "unavailable";

export interface Settings {
  schemaVersion: number;
  accelerationEnabled: boolean;
  routeScope: RouteScope;
  lineMode: LineMode;
  fixedNodeId: string | null;
  currentNodeId: string | null;
  healthCheckMinutes: number;
  launchAtLogin: boolean;
  logLevel: "error" | "info" | "debug";
  usageLoggingEnabled: boolean;
  lastAppliedAt: string | null;
}

export interface HealthSummary {
  status: NodeStatus;
  successCount: number;
  attemptCount: number;
  medianLatencyMs: number | null;
  consecutiveFailures: number;
  checkedAt: string | null;
  failureReason: string | null;
}

export interface NodeEntry {
  id: string;
  name: string;
  rewriteBase: string;
  enabled: boolean;
  builtIn: boolean;
  health: HealthSummary;
}

export interface RouteEntry {
  id: string;
  repositoryUrl: string;
  createdAt: string;
}

export interface EnvironmentSummary {
  gitAvailable: boolean;
  gitPath: string | null;
  gitVersion: string | null;
  includeRegistered: boolean;
  configPath: string;
  conflicts: number;
  conflictScanError: string | null;
}

export interface AppSnapshot {
  settings: Settings;
  nodes: NodeEntry[];
  routes: RouteEntry[];
  environment: EnvironmentSummary;
}

export interface DiagnosticReport {
  generatedAt: string;
  gitPath: string | null;
  gitVersion: string | null;
  configPath: string;
  includeRegistered: boolean;
  conflicts: string[];
  conflictScanError: string | null;
  originalUrl: string;
  fetchUrl: string | null;
  pushUrl: string | null;
  explicitPushUrl: string | null;
  repositoryError: string | null;
  warnings: string[];
  reportText: string;
}

export interface ImportResult {
  imported: number;
  duplicates: number;
  rejected: { input: string; reason: string }[];
  nodes: NodeEntry[];
}

export interface DownloadTarget {
  originalUrl: string;
  acceleratedUrl: string;
  fileName: string;
  nodeName: string;
}

export interface UsageEvent {
  id: string;
  occurredAt: string;
  command: string;
  repository: string;
  route: "accelerated" | "direct" | "other";
  nodeName: string | null;
  connectionHost: string;
  succeeded: boolean;
  exitCode: number;
  durationMs: number;
}

export interface UsageLogSnapshot {
  enabled: boolean;
  listening: boolean;
  configured: boolean;
  events: UsageEvent[];
  storagePath: string;
}
