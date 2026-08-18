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

export function friendlyError(error: unknown): { text: string; detail: string | null } {
  if (error && typeof error === "object" && !(error instanceof Error)) {
    const value = error as { message?: unknown; detail?: unknown };
    if (typeof value.message === "string" && value.message.trim()) {
      const text = value.message.trim();
      const detail = typeof value.detail === "string" && value.detail.trim() && value.detail.trim() !== text
        ? value.detail.trim()
        : null;
      return { text, detail };
    }
  }
  const text = (error instanceof Error ? error.message : typeof error === "string" ? error : "").trim();
  return { text: text || "操作失败，请重试", detail: null };
}
