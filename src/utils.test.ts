import { describe, expect, it } from "vitest";
import { formatLatency, friendlyError, statusLabel, successRate } from "./utils";

describe("display utilities", () => {
  it("formats node timing without inventing data", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(286.4)).toBe("286 ms");
  });

  it("calculates success rate from real attempts", () => {
    expect(successRate({ status: "available", inAutoPool: true, successCount: 3, attemptCount: 4, medianLatencyMs: 200, consecutiveFailures: 0, checkedAt: null, failureReason: null })).toBe("75%");
    expect(successRate({ status: "untested", inAutoPool: false, successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null })).toBe("—");
  });

  it("has explicit copy for every state", () => {
    expect(Object.keys(statusLabel)).toEqual(["untested", "available", "slow", "incompatible", "unavailable"]);
  });

  it("keeps plain errors untouched", () => {
    expect(friendlyError(new Error("默认浏览器未能打开地址"))).toEqual({ text: "默认浏览器未能打开地址", detail: null });
    expect(friendlyError("仅支持 https://github.com/ 下的地址")).toEqual({ text: "仅支持 https://github.com/ 下的地址", detail: null });
    expect(friendlyError("x".repeat(80))).toEqual({ text: "x".repeat(80), detail: null });
  });

  it("uses the backend error contract without parsing its wording", () => {
    const detail = "HTTPS 探测失败：error sending request for url (https://fastgit.cc/x): client error (Connect)";
    expect(friendlyError({ message: "无法通过 fastgit.cc 获取此文件", detail })).toEqual({
      text: "无法通过 fastgit.cc 获取此文件",
      detail,
    });
  });

  it("ignores empty or duplicate structured details", () => {
    expect(friendlyError({ message: "操作失败", detail: "操作失败" })).toEqual({ text: "操作失败", detail: null });
    expect(friendlyError({ message: "操作失败", detail: "  " })).toEqual({ text: "操作失败", detail: null });
  });

  it("falls back to a generic message for invalid input", () => {
    expect(friendlyError(null)).toEqual({ text: "操作失败，请重试", detail: null });
    expect(friendlyError({ message: "", detail: "raw" })).toEqual({ text: "操作失败，请重试", detail: null });
  });
});
