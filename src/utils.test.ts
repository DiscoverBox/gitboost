import { describe, expect, it } from "vitest";
import { formatLatency, statusLabel, successRate } from "./utils";

describe("display utilities", () => {
  it("formats node timing without inventing data", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(286.4)).toBe("286 ms");
  });

  it("calculates success rate from real attempts", () => {
    expect(successRate({ status: "available", successCount: 3, attemptCount: 4, medianLatencyMs: 200, consecutiveFailures: 0, checkedAt: null, failureReason: null })).toBe("75%");
    expect(successRate({ status: "untested", successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null })).toBe("—");
  });

  it("has explicit copy for every state", () => {
    expect(Object.keys(statusLabel)).toEqual(["untested", "available", "slow", "incompatible", "unavailable"]);
  });
});
