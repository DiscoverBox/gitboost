import type { HealthSummary, NodeEntry, NodeStatus } from "./types";

export const statusLabel: Record<NodeStatus, string> = {
  untested: "未检测",
  available: "可用",
  slow: "较慢",
  incompatible: "不兼容",
  unavailable: "不可用",
};

export function formatLatency(value: number | null): string {
  return value == null ? "—" : `${Math.round(value)} ms`;
}

export function formatRelativeTime(value: string | null): string {
  if (!value) return "尚未检测";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间未知";
  const elapsed = Date.now() - date.getTime();
  if (elapsed < 60_000) return "刚刚";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)} 分钟前`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)} 小时前`;
  return date.toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" });
}

export function currentNode(nodes: NodeEntry[], id: string | null): NodeEntry | null {
  return nodes.find((node) => node.id === id) ?? null;
}

export function successRate(health: HealthSummary): string {
  if (!health.attemptCount) return "—";
  return `${Math.round((health.successCount / health.attemptCount) * 100)}%`;
}

export function statusTone(status: NodeStatus): "neutral" | "success" | "warning" | "danger" {
  if (status === "available") return "success";
  if (status === "slow") return "warning";
  if (status === "unavailable" || status === "incompatible") return "danger";
  return "neutral";
}
