import { expect, test } from "@playwright/test";
import { createRequire } from "node:module";
import type { AppSnapshot } from "../src/types";

const packageMetadata = createRequire(import.meta.url)("../package.json") as { version: string };

test("core desktop workflow is navigable", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();
  await expect(page.getByRole("button", { name: "自动选择", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "直连", exact: true })).toBeVisible();

  const dragRegion = page.locator("[data-tauri-drag-region]");
  await expect(dragRegion).toBeVisible();
  await expect(dragRegion).toHaveCSS("height", "29px");

  await expect(page.getByRole("navigation", { name: "主要导航" }).getByText("自定义节点", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(page.getByText(`GitBoost ${packageMetadata.version} · macOS / Windows`, { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "自定义节点", exact: true })).toBeVisible();
  const healthCheck = page.locator(".setting-row").filter({ hasText: "后台健康检查" }).locator("select");
  await expect(healthCheck).toHaveValue("1440");
  await expect(healthCheck.locator("option")).toHaveText(["关闭", "每小时", "每 8 小时", "每天", "每周", "每月"]);
  await expect(page.getByText("系统线路 1 个 · 使用公开仓库进行隔离检测，不修改全局 Git 配置", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "刷新系统线路" })).toBeVisible();
  await expect(page.getByText("https://fastgit.cc/https://github.com/", { exact: true })).toBeHidden();

  await page.getByRole("button", { name: "添加节点" }).click();
  await expect(page.getByRole("dialog", { name: "添加节点" })).toBeVisible();
  await expect(page.getByRole("textbox", { name: "代理地址" })).toHaveAttribute("placeholder", "https://proxy.example");
  await expect(page.getByText("应用会自动补全 GitHub 重写路径。", { exact: false })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "添加节点" })).toBeHidden();

  await page.getByRole("button", { name: "路由清单", exact: true }).click();
  await expect(page.getByText("访问私有仓库或不确定时，建议仅加速清单。")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "GitHub 仓库" })).toHaveAttribute("placeholder", "owner/repository 或完整 GitHub 地址");

  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  await expect(page.getByRole("heading", { name: "文件下载", exact: true })).toBeVisible();
  await expect(page.getByLabel("GitHub 地址")).toHaveAttribute("placeholder", "https://github.com/... 或 https://raw.githubusercontent.com/...");
  await expect(page.getByText("支持 github.com 和 raw.githubusercontent.com 下的公开地址。")).toBeVisible();
  await expect(page.getByText("节点失败时不会静默改为 GitHub 直连。")).toBeVisible();

  await page.getByRole("button", { name: "使用日志", exact: true }).click();
  await expect(page.getByRole("heading", { name: "使用日志", exact: true })).toBeVisible();
  await expect(page.getByText("只保存脱敏结果")).toBeVisible();
  expect(errors).toEqual([]);
});

test("settings remain scrollable without showing a scrollbar", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();

  const content = page.locator(".main-content");
  await expect(content).toBeVisible();
  expect(await content.evaluate((element) => element.scrollHeight)).toBeGreaterThan(
    await content.evaluate((element) => element.clientHeight),
  );
  await expect(content).toHaveCSS("scrollbar-width", "none");
  await content.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
  await expect(page.locator(".about-line")).toBeInViewport();
});

test("Windows uses stable native typography without changing desktop geometry", async ({ browser }) => {
  const context = await browser.newContext({
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36",
    viewport: { width: 1100, height: 760 },
  });
  const page = await context.newPage();
  await page.goto("/");

  const shell = page.locator(".app-shell");
  await expect(shell).toHaveClass(/app-shell--native-titlebar/);
  await expect(page.locator("[data-tauri-drag-region]")).toHaveCount(0);
  await expect(page.locator(".sidebar")).toHaveCSS("padding-top", "25px");
  await expect(page.locator(".main-content")).toHaveCSS("padding-top", "0px");
  await expect(page.locator(".status-board")).toHaveCSS("min-height", "175px");

  const typography = await shell.evaluate((element) => {
    const shellStyles = getComputedStyle(element);
    const headingStyles = getComputedStyle(element.querySelector(".page-header h1")!);
    const authorMetaStyles = getComputedStyle(element.querySelector(".author-link small")!);
    const statusDetailStyles = getComputedStyle(element.querySelector(".status-primary p")!);
    const footnoteStyles = getComputedStyle(element.querySelector(".page-footnote")!);
    return {
      ui: shellStyles.fontFamily,
      display: headingStyles.fontFamily,
      mono: shellStyles.getPropertyValue("--font-mono").trim(),
      textMeta: shellStyles.getPropertyValue("--text-meta").trim(),
      textLabel: shellStyles.getPropertyValue("--text-label").trim(),
      textBodySmall: shellStyles.getPropertyValue("--text-body-small").trim(),
      authorMetaSize: authorMetaStyles.fontSize,
      statusDetailSize: statusDetailStyles.fontSize,
      footnoteSize: footnoteStyles.fontSize,
      sidebarWidth: element.querySelector(".sidebar")!.getBoundingClientRect().width,
    };
  });

  expect(typography.ui).toContain("Segoe UI Variable Text");
  expect(typography.display).toContain("Segoe UI Variable Display");
  expect(typography.mono).toContain("Cascadia Mono");
  expect(typography.textMeta).toBe("10px");
  expect(typography.textLabel).toBe("11px");
  expect(typography.textBodySmall).toBe("12px");
  expect(typography.authorMetaSize).toBe("10px");
  expect(typography.statusDetailSize).toBe("12px");
  expect(typography.footnoteSize).toBe("11px");
  expect(typography.sidebarWidth).toBe(228);

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(page.locator(".setting-row p").first()).toHaveCSS("font-size", "12px");
  await expect(page.locator(".about-line")).toHaveCSS("font-size", "10px");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(1100);
  await context.close();
});

test("system node refresh reports whether the catalog changed", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes: [{ id: "system", name: "system", rewriteBase: "https://proxy.example/https://github.com/", enabled: true, builtIn: true, health: { status: "untested", inAutoPool: false, successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null } }],
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    let refreshCount = 0;
    Object.assign(window, { __TAURI_INTERNALS__: { invoke: async (command: string) => {
      if (command === "plugin:app|version") return "9.8.7";
      if (command === "refresh_system_nodes") return refreshCount++ === 0;
      return snapshot;
    } } });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(page.getByText("GitBoost 9.8.7 · macOS / Windows", { exact: true })).toBeVisible();
  const refresh = page.getByRole("button", { name: "刷新系统线路" });

  await refresh.click();
  await expect(page.locator(".toast")).toHaveText("系统线路已更新");
  await refresh.click();
  await expect(page.locator(".toast")).toHaveText("系统线路已是最新");
});

test("raw file download keeps unattempted lines after the first line succeeds", async ({ page }) => {
  await page.addInitScript(() => {
    const nodes: AppSnapshot["nodes"] = [
      { id: "node-one", name: "Node One", rewriteBase: "https://one.example/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 20, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } },
      { id: "node-two", name: "Node Two", rewriteBase: "https://two.example/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 30, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } },
      { id: "node-three", name: "Node Three", rewriteBase: "https://three.example/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 40, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } },
      { id: "node-four", name: "Node Four", rewriteBase: "https://four.example/https://github.com/", enabled: true, builtIn: false, health: { status: "slow", inAutoPool: true, successCount: 1, attemptCount: 2, medianLatencyMs: 80, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } },
    ];
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes,
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const originalUrl = "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh";
    const target = (node: AppSnapshot["nodes"][number]) => ({ originalUrl, acceleratedUrl: `${node.rewriteBase.replace("https://github.com/", "")}${originalUrl}`, fileName: "install.sh", nodeId: node.id, nodeName: node.name });
    let attempt = 0;
    Object.assign(window, {
      __downloadCalls: calls,
      __TAURI_INTERNALS__: { invoke: async (command: string, args: Record<string, unknown> = {}) => {
        if (command.startsWith("plugin:event|")) return undefined;
        calls.push({ command, args });
        if (command === "get_snapshot") return structuredClone(snapshot);
        if (command === "open_download") {
          await new Promise((resolve) => setTimeout(resolve, 80));
          const node = nodes[attempt++];
          return { target: target(node), attemptedNodeIds: [node.id], hasRemaining: attempt < nodes.length, failure: null };
        }
        return structuredClone(snapshot);
      } },
    });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  const input = page.getByLabel("GitHub 地址");
  await input.fill("https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh");
  await page.getByRole("button", { name: "开始下载" }).click();

  await expect(input).toBeDisabled();
  await expect(page.getByRole("button", { name: "检测线路…" })).toBeDisabled();
  await expect(page.locator(".toast")).toHaveText("已通过 Node One 在浏览器中打开地址");
  await expect(input).toBeEnabled();
  await expect(page.locator(".download-target").getByText("Node One", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "换线路重试" }).click();
  await expect(page.locator(".toast")).toHaveText("已通过 Node Two 在浏览器中打开地址");
  await expect(page.locator(".download-target").getByText("Node Two", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as typeof window & { __downloadCalls: { command: string; args: Record<string, unknown> }[] }).__downloadCalls);
  expect(calls.filter(({ command }) => command === "open_download")).toEqual([
    { command: "open_download", args: { originalUrl: "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh", excludedNodeIds: [] } },
    { command: "open_download", args: { originalUrl: "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh", excludedNodeIds: ["node-one"] } },
  ]);
});

test("raw file download keeps remaining lines after a failed fallback batch", async ({ page }) => {
  await page.addInitScript(() => {
    const nodes: AppSnapshot["nodes"] = Array.from({ length: 5 }, (_, index) => ({
      id: `node-${index + 1}`,
      name: `Node ${index + 1}`,
      rewriteBase: `https://${index + 1}.example/https://github.com/`,
      enabled: true,
      builtIn: false,
      health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 20 + index, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null },
    }));
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes,
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const originalUrl = "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh";
    const target = (node: AppSnapshot["nodes"][number]) => ({ originalUrl, acceleratedUrl: `${node.rewriteBase.replace("https://github.com/", "")}${originalUrl}`, fileName: "install.sh", nodeId: node.id, nodeName: node.name });
    let attempt = 0;
    Object.assign(window, {
      __downloadCalls: calls,
      __TAURI_INTERNALS__: { invoke: async (command: string, args: Record<string, unknown> = {}) => {
        if (command.startsWith("plugin:event|")) return undefined;
        calls.push({ command, args });
        if (command === "get_snapshot") return structuredClone(snapshot);
        if (command === "open_download" && attempt++ === 0) return {
          target: target(nodes[3]),
          attemptedNodeIds: nodes.slice(0, 4).map((node) => node.id),
          hasRemaining: true,
          failure: { message: "无法通过 Node 4 获取此文件", detail: "HTTP 500" },
        };
        if (command === "open_download") return { target: target(nodes[4]), attemptedNodeIds: ["node-5"], hasRemaining: false, failure: null };
        return structuredClone(snapshot);
      } },
    });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  await page.getByLabel("GitHub 地址").fill("https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh");
  await page.getByRole("button", { name: "开始下载" }).click();

  await expect(page.locator(".toast")).toContainText("无法通过 Node 4 获取此文件");
  await expect(page.getByText("本次检测的线路均未能获取此文件，可继续检测剩余线路。")).toBeVisible();
  await page.getByRole("button", { name: "换线路重试" }).click();
  await expect(page.locator(".toast")).toHaveText("已通过 Node 5 在浏览器中打开地址");
  await expect(page.locator(".download-target").getByText("Node 5", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "换线路重试" })).toHaveCount(0);

  const calls = await page.evaluate(() => (window as typeof window & { __downloadCalls: { command: string; args: Record<string, unknown> }[] }).__downloadCalls);
  expect(calls.filter(({ command }) => command === "open_download")).toEqual([
    { command: "open_download", args: { originalUrl: "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh", excludedNodeIds: [] } },
    { command: "open_download", args: { originalUrl: "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh", excludedNodeIds: ["node-1", "node-2", "node-3", "node-4"] } },
  ]);
});

test("another operation cannot replace an active download busy state", async ({ page }) => {
  await page.addInitScript(() => {
    const node: AppSnapshot["nodes"][number] = { id: "node-one", name: "Node One", rewriteBase: "https://one.example/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 20, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } };
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes: [node],
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const originalUrl = "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh";
    const target = { originalUrl, acceleratedUrl: `https://one.example/${originalUrl}`, fileName: "install.sh", nodeId: node.id, nodeName: node.name };
    let settingsUpdates = 0;
    let autostartEnabled = false;
    let autostartEnables = 0;
    let launchAtLoginUpdates = 0;
    Object.assign(window, {
      __settingsUpdates: () => settingsUpdates,
      __autostartCalls: () => ({ autostartEnables, launchAtLoginUpdates }),
      __finishDownload: undefined,
      __TAURI_INTERNALS__: { invoke: async (command: string) => {
        if (command.startsWith("plugin:event|")) return undefined;
        if (command === "plugin:autostart|is_enabled") return autostartEnabled;
        if (command === "plugin:autostart|enable") {
          autostartEnabled = true;
          autostartEnables += 1;
          return undefined;
        }
        if (command === "get_snapshot") return structuredClone(snapshot);
        if (command === "update_settings") {
          settingsUpdates += 1;
          return structuredClone(snapshot);
        }
        if (command === "update_launch_at_login") {
          launchAtLoginUpdates += 1;
          return structuredClone(snapshot);
        }
        if (command === "open_download") return new Promise((resolve) => {
          (window as typeof window & { __finishDownload?: () => void }).__finishDownload = () => resolve({ target, attemptedNodeIds: [node.id], hasRemaining: false, failure: null });
        });
        return structuredClone(snapshot);
      } },
    });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  await page.getByLabel("GitHub 地址").fill("https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh");
  await page.getByRole("button", { name: "开始下载" }).click();
  await expect(page.getByLabel("GitHub 地址")).toBeDisabled();

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: "保存设置" }).click();
  await expect(page.locator(".toast")).toContainText("当前操作尚未完成，请稍候");
  expect(await page.evaluate(() => (window as typeof window & { __settingsUpdates: () => number }).__settingsUpdates())).toBe(0);
  await page.getByRole("switch", { name: "登录时启动" }).click();
  await expect(page.locator(".toast")).toContainText("当前操作尚未完成，请稍候");
  expect(await page.evaluate(() => (window as typeof window & { __autostartCalls: () => { autostartEnables: number; launchAtLoginUpdates: number } }).__autostartCalls())).toEqual({ autostartEnables: 0, launchAtLoginUpdates: 0 });

  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  await expect(page.getByLabel("GitHub 地址")).toBeDisabled();
  await page.evaluate(() => (window as typeof window & { __finishDownload?: () => void }).__finishDownload?.());
  await expect(page.getByLabel("GitHub 地址")).toBeEnabled();
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("switch", { name: "登录时启动" }).click();
  await expect(page.locator(".toast")).toHaveText("已设为登录时启动");
  expect(await page.evaluate(() => (window as typeof window & { __autostartCalls: () => { autostartEnables: number; launchAtLoginUpdates: number } }).__autostartCalls())).toEqual({ autostartEnables: 1, launchAtLoginUpdates: 1 });
});

test("node detection shows usable-node target progress", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes: [{ id: "system", name: "system", rewriteBase: "https://proxy.example/https://github.com/", enabled: true, builtIn: true, health: { status: "untested", inAutoPool: false, successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null } }],
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, (event: unknown) => void>();
    let callbackId = 0;
    let completeNodeTest: (() => void) | undefined;
    Object.assign(window, {
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
      __TAURI_INTERNALS__: {
        transformCallback: (callback: (event: unknown) => void) => {
          const id = ++callbackId;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, args: Record<string, unknown>) => {
          if (command === "plugin:event|listen") {
            listeners.set(String(args.event), callbacks.get(Number(args.handler))!);
            return callbackId;
          }
          if (command === "plugin:event|unlisten") return undefined;
          if (command === "test_all_nodes") {
            listeners.get("node-test-progress")?.({ event: "node-test-progress", id: 1, payload: { completed: 7, total: 10, finished: false } });
            return new Promise((resolve) => { completeNodeTest = () => {
              listeners.get("node-test-progress")?.({ event: "node-test-progress", id: 1, payload: { completed: 0, total: 0, finished: true } });
              resolve(snapshot.nodes);
            }; });
          }
          return snapshot;
        },
      },
      completeNodeTest: () => completeNodeTest?.(),
      emitNodeTestProgress: (completed: number, total: number) => {
        listeners.get("node-test-progress")?.({ event: "node-test-progress", id: 1, payload: { completed, total, finished: false } });
      },
      emitNodeTestFinished: () => {
        listeners.get("node-test-progress")?.({ event: "node-test-progress", id: 1, payload: { completed: 0, total: 0, finished: true } });
      },
    });
  });

  await page.goto("/");
  const lineControl = page.locator(".section-title").filter({ has: page.getByRole("heading", { name: "线路控制" }) });

  await page.evaluate(() => (window as typeof window & { emitNodeTestProgress: (completed: number, total: number) => void }).emitNodeTestProgress(3, 10));
  await expect(lineControl.getByRole("button", { name: "可用 3/10" })).toBeDisabled();
  await expect(page.getByRole("progressbar", { name: "可用线路 3/10" })).toBeVisible();
  await page.evaluate(() => (window as typeof window & { emitNodeTestProgress: (completed: number, total: number) => void }).emitNodeTestProgress(10, 10));
  await expect(page.getByRole("progressbar", { name: "可用线路 10/10" })).toBeVisible();
  await page.evaluate(() => (window as typeof window & { emitNodeTestFinished: () => void }).emitNodeTestFinished());
  await expect(page.getByRole("progressbar")).toHaveCount(0);
  await expect(lineControl.getByRole("button", { name: "重新测速" })).toBeEnabled();

  await lineControl.getByRole("button", { name: "重新测速" }).click();

  await expect(lineControl.getByRole("button", { name: "可用 7/10" })).toBeDisabled();
  const progress = page.getByRole("progressbar", { name: "可用线路 7/10" });
  await expect(progress).toHaveAttribute("aria-valuenow", "7");
  const fillRatio = await progress.locator("span").evaluate((fill) => fill.clientWidth / fill.parentElement!.clientWidth);
  expect(fillRatio).toBeCloseTo(7 / 10, 2);

  await page.evaluate(() => (window as typeof window & { completeNodeTest: () => void }).completeNodeTest());
  await expect(page.locator(".toast")).toHaveText("节点检测完成");
  await expect(page.getByRole("progressbar")).toHaveCount(0);
  await expect(lineControl.getByRole("button", { name: "重新测速" })).toBeEnabled();
});

test("UI issues commands in workflow order and reflects their results", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null as string | null, consentAcknowledgedAt: null as string | null },
      nodes: [{ id: "verified-node", name: "Verified Node", rewriteBase: "https://proxy.integration.test/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 25, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } }],
      routes: [] as { id: string; repositoryUrl: string; createdAt: string }[],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const copy = () => structuredClone(snapshot);
    Object.assign(window, {
      __workflowCalls: calls,
      __TAURI_INTERNALS__: { invoke: async (command: string, args: Record<string, unknown> = {}) => {
        if (command.startsWith("plugin:event|")) return undefined;
        calls.push({ command, args });
        if (command === "get_snapshot") return copy();
        if (command === "add_route") {
          const repository = String(args.repositoryUrl).replace(/\.git$/, "");
          const repositoryUrl = `${repository.startsWith("https://github.com/") ? repository : `https://github.com/${repository}`}.git`;
          snapshot.routes.push({ id: "route-1", repositoryUrl, createdAt: "2026-08-14T00:00:00Z" });
          return copy();
        }
        if (command === "acknowledge_consent") {
          snapshot.settings.consentAcknowledgedAt = "2026-08-14T00:00:00Z";
          return copy();
        }
        if (command === "get_trace2_target_conflict") return null;
        if (command === "set_acceleration") {
          if (args.enabled && snapshot.routes.length === 0) throw new Error("仅加速清单为空，请先加入至少一个公开仓库");
          snapshot.settings.accelerationEnabled = Boolean(args.enabled);
          snapshot.settings.currentNodeId = args.enabled ? "verified-node" : null;
          snapshot.settings.lastAppliedAt = "2026-08-14T00:00:01Z";
          snapshot.environment.includeRegistered = true;
          return copy();
        }
        if (command === "restore_git_config") {
          snapshot.settings.accelerationEnabled = false;
          snapshot.settings.currentNodeId = null;
          snapshot.environment.includeRegistered = false;
          return copy();
        }
        return copy();
      } },
    });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "开启加速" }).click();
  const firstConsent = page.getByRole("dialog", { name: "首次开启加速" });
  await expect(firstConsent).toBeVisible();
  await firstConsent.getByRole("button", { name: "了解并开启加速" }).click();
  await expect(page.locator(".toast")).toContainText("仅加速清单为空，请先加入至少一个公开仓库");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();

  await page.getByRole("button", { name: "路由清单", exact: true }).click();
  await page.getByRole("textbox", { name: "GitHub 仓库" }).fill("openai/codex");
  await page.getByRole("button", { name: "加入清单" }).click();
  await expect(page.getByText("https://github.com/openai/codex.git", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "总览", exact: true }).click();
  await page.getByRole("button", { name: "开启加速" }).click();
  // 同意已持久化，再次开启不再弹确认
  await expect(page.getByRole("dialog", { name: "首次开启加速" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "读取线路已接入" })).toBeVisible();
  await expect(page.getByText("独立配置已注册", { exact: true })).toBeVisible();
  await expect(page.getByText("加速已开启", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "直连", exact: true }).click();
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();
  await expect(page.getByText("当前为直连", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "自动选择", exact: true }).click();
  await expect(page.getByRole("heading", { name: "读取线路已接入" })).toBeVisible();
  await expect(page.getByText("加速已开启", { exact: true })).toBeVisible();
  await expect(page.locator(".toast")).toHaveText("已切换为自动选择，加速已开启");

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: "恢复 Git 配置" }).click();
  await expect(page.locator(".toast")).toHaveText("GitBoost 配置已恢复为直连");

  const calls = await page.evaluate(() => (window as typeof window & { __workflowCalls: { command: string; args: Record<string, unknown> }[] }).__workflowCalls);
  expect(calls.filter(({ command }) => command === "get_snapshot").length).toBeGreaterThan(0);
  expect(calls.filter(({ command }) => ["acknowledge_consent", "set_acceleration", "add_route", "restore_git_config"].includes(command))).toEqual([
    { command: "acknowledge_consent", args: {} },
    { command: "set_acceleration", args: { enabled: true, replaceTrace2Target: false } },
    { command: "add_route", args: { repositoryUrl: "openai/codex" } },
    { command: "set_acceleration", args: { enabled: true, replaceTrace2Target: false } },
    { command: "set_acceleration", args: { enabled: false, replaceTrace2Target: false } },
    { command: "set_acceleration", args: { enabled: true, replaceTrace2Target: false } },
    { command: "restore_git_config", args: {} },
  ]);
});

test("Trace2 conflict asks the user to keep the existing target or switch to GitBoost", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: "2026-08-14T00:00:00Z" },
      nodes: [{ id: "verified-node", name: "Verified Node", rewriteBase: "https://proxy.integration.test/https://github.com/", enabled: true, builtIn: false, health: { status: "available", inAutoPool: true, successCount: 2, attemptCount: 2, medianLatencyMs: 25, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } }],
      routes: [{ id: "route-1", repositoryUrl: "https://github.com/openai/codex.git", createdAt: "2026-08-14T00:00:00Z" }],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: true, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const copy = () => structuredClone(snapshot);
    Object.assign(window, {
      __trace2ChoiceCalls: calls,
      __TAURI_INTERNALS__: { invoke: async (command: string, args: Record<string, unknown> = {}) => {
        if (command.startsWith("plugin:event|")) return undefined;
        calls.push({ command, args });
        if (command === "get_snapshot") return copy();
        if (command === "get_trace2_target_conflict") return "af_unix:stream:/Users/example/.git-ai/trace2.sock";
        if (command === "restore_git_config") {
          snapshot.settings.accelerationEnabled = false;
          snapshot.settings.currentNodeId = null;
          snapshot.environment.includeRegistered = false;
          snapshot.environment.trace2TargetOverridden = false;
          return copy();
        }
        if (command === "set_acceleration") {
          snapshot.settings.accelerationEnabled = Boolean(args.enabled);
          snapshot.settings.currentNodeId = args.enabled ? "verified-node" : null;
          snapshot.environment.includeRegistered = Boolean(args.enabled);
          snapshot.environment.trace2TargetOverridden = Boolean(args.enabled && args.replaceTrace2Target);
          return copy();
        }
        return copy();
      } },
    });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "开启加速" }).click();
  const conflict = page.getByRole("dialog", { name: "Git Trace2 已被其他工具使用" });
  await expect(conflict).toBeVisible();
  await expect(conflict).toContainText(".git-ai/trace2.sock");
  await conflict.getByRole("button", { name: "保留现有 Trace2" }).click();
  await expect(page.locator(".toast")).toHaveText("已保留现有 Trace2 配置");
  await expect(page.getByText("当前为直连", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "开启加速" }).click();
  await page.getByRole("dialog", { name: "Git Trace2 已被其他工具使用" }).getByRole("button", { name: "切换到 GitBoost" }).click();
  await expect(page.getByText("加速已开启", { exact: true })).toBeVisible();
  await expect(page.getByText("GitBoost 的 Trace2 接入已失效", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "恢复现有 Trace2" }).click();
  await expect(page.getByText("当前为直连", { exact: true })).toBeVisible();

  const calls = await page.evaluate(() => (window as typeof window & { __trace2ChoiceCalls: { command: string; args: Record<string, unknown> }[] }).__trace2ChoiceCalls);
  expect(calls.filter(({ command }) => command === "set_acceleration")).toEqual([
    { command: "set_acceleration", args: { enabled: true, replaceTrace2Target: true } },
    { command: "set_acceleration", args: { enabled: false, replaceTrace2Target: false } },
  ]);
});

test("first acceleration enable explains the privacy boundary before running a desktop command", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "开启加速" }).click();
  const consent = page.getByRole("dialog", { name: "首次开启加速" });
  await expect(consent).toBeVisible();
  await expect(consent.getByText("第三方加速节点", { exact: false })).toBeVisible();
  await consent.getByRole("button", { name: "取消" }).click();
  await expect(page.getByRole("dialog", { name: "首次开启加速" })).toHaveCount(0);
  await expect(page.getByText("当前为直连", { exact: true })).toBeVisible();
});

test("desktop command errors persist until dismissed", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "开启加速" }).click();
  await page.getByRole("dialog", { name: "首次开启加速" }).getByRole("button", { name: "了解并开启加速" }).click();

  const errorToast = page.locator(".toast");
  await expect(errorToast).toHaveAttribute("role", "alert");
  await expect(errorToast).toContainText("只能在 GitBoost 桌面应用中执行");
  await page.waitForTimeout(4600);
  await expect(errorToast).toBeVisible();
  await errorToast.getByRole("button", { name: "关闭提示" }).click();
  await expect(page.locator(".toast")).toHaveCount(0);
});

test("uses the bright interface palette", async ({ page }) => {
  await page.goto("/");

  const palette = await page.locator(":root").evaluate((root) => {
    const styles = getComputedStyle(root);
    return {
      canvas: styles.getPropertyValue("--canvas").trim(),
      sidebar: styles.getPropertyValue("--sidebar").trim(),
      surface: styles.getPropertyValue("--surface").trim(),
      accent: styles.getPropertyValue("--accent").trim(),
    };
  });

  expect(palette).toEqual({
    canvas: "#ffffff",
    sidebar: "#e4f4ff",
    surface: "#ffffff",
    accent: "#1377cc",
  });
  await expect(page.locator(".status-board")).toHaveCSS("background-color", "rgb(255, 255, 255)");
  await expect(page.locator(".status-primary")).toHaveCSS("background-color", "rgb(19, 119, 204)");
  await expect(page.locator(".brand-mark").first()).toBeVisible();
  await expect(page.getByRole("navigation", { name: "主要导航" }).getByRole("button", { name: "总览" })).toHaveCSS("background-color", "rgb(19, 119, 204)");
  await expect(page.getByRole("navigation", { name: "主要导航" }).getByRole("button", { name: "总览" })).toHaveCSS("color", "rgb(255, 255, 255)");
});

test("open-source project links stay visible and open the expected GitHub pages", async ({ page }) => {
  await page.addInitScript(() => {
    const opened: string[] = [];
    Object.assign(window, {
      __openedProjectLinks: opened,
      open: (url: string | URL) => {
        opened.push(String(url));
        return null;
      },
    });
  });
  await page.goto("/");

  const author = page.getByRole("region", { name: "GitBoost 项目作者" });
  const projectLink = page.getByRole("button", { name: "查看 GitBoost 项目" });
  const navigation = page.getByRole("navigation", { name: "主要导航" });
  const authorLabel = author.getByText("DiscoverBox", { exact: true });
  await expect(authorLabel).toBeVisible();
  await expect(author.getByText("github.com/DiscoverBox", { exact: true })).toBeVisible();
  const [projectLinkBox, navigationBox] = await Promise.all([
    projectLink.boundingBox(),
    navigation.boundingBox(),
  ]);
  expect(projectLinkBox && projectLinkBox.y + projectLinkBox.height).toBeLessThanOrEqual(navigationBox?.y ?? 0);
  await author.getByRole("button", { name: "DiscoverBox github.com/DiscoverBox" }).click();
  await projectLink.click();

  const opened = await page.evaluate(() => (window as typeof window & { __openedProjectLinks: string[] }).__openedProjectLinks);
  expect(opened).toEqual([
    "https://github.com/DiscoverBox",
    "https://github.com/DiscoverBox/gitboost",
  ]);
});

test("node import keeps failures open and reports actual results", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: "添加节点" }).click();

  await page.evaluate(() => {
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes: [], routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    Object.assign(window, { __TAURI_INTERNALS__: { invoke: async (_command: string, args: { text?: string }) => {
      if (args.text?.startsWith("http://")) return { ...snapshot, imported: 0, duplicates: 0, rejected: [{ input: args.text, reason: "仅接受 HTTPS" }] };
      if (args.text === "https://duplicate.example") return { ...snapshot, imported: 0, duplicates: 1, rejected: [] };
      return { ...snapshot, imported: 1, duplicates: 0, rejected: [] };
    } } });
  });

  const input = page.getByRole("textbox", { name: "代理地址" });
  await input.fill("http://proxy.example");
  await page.getByRole("button", { name: "添加", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "添加节点" })).toBeVisible();
  await expect(page.getByText("没有添加新节点")).toBeVisible();
  await expect(page.getByText("仅接受 HTTPS", { exact: true })).toBeVisible();
  await expect(page.locator(".toast")).toHaveCount(0);

  await input.fill("https://duplicate.example");
  await page.getByRole("button", { name: "添加", exact: true }).click();
  await expect(page.getByText("1 个重复地址已跳过")).toBeVisible();
  await expect(page.getByRole("dialog", { name: "添加节点" })).toBeVisible();

  await input.fill("https://proxy.example");
  await page.getByRole("button", { name: "添加", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "添加节点" })).toBeHidden();
  await expect(page.locator(".toast")).toHaveText("已添加 1 个节点");
});

test("route list shows full addresses and filters existing repositories", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "路由清单", exact: true }).click();

  const longUrl = "https://github.com/organization-with-a-very-long-name/repository-with-a-very-long-name.git";
  await page.evaluate((repositoryUrl) => {
    const extraRoutes = Array.from({ length: 10 }, (_, index) => ({
      id: `extra-${index + 1}`,
      repositoryUrl: `https://github.com/example/repository-${index + 1}.git`,
      createdAt: "2026-08-13T00:00:00Z",
    }));
    const snapshot: AppSnapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null, consentAcknowledgedAt: null },
      nodes: [],
      routes: [
        { id: "long", repositoryUrl, createdAt: "2026-08-13T00:00:00Z" },
        { id: "codex", repositoryUrl: "https://github.com/openai/codex.git", createdAt: "2026-08-13T00:00:00Z" },
        ...extraRoutes,
      ],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    Object.assign(window, { __TAURI_INTERNALS__: { invoke: async () => snapshot } });
  }, longUrl);

  await page.getByRole("textbox", { name: "GitHub 仓库" }).fill("openai/codex");
  await page.getByRole("button", { name: "加入清单" }).click();

  const fullAddress = page.getByText(longUrl, { exact: true });
  await expect(fullAddress).toBeVisible();
  await expect(fullAddress).toHaveCSS("white-space", "normal");
  await expect(fullAddress).not.toHaveCSS("text-overflow", "ellipsis");
  await expect(page.getByText("加速", { exact: true })).toHaveCount(0);

  const pagination = page.getByRole("navigation", { name: "仓库列表分页" });
  await expect(pagination).toContainText("第 1 / 2 页");
  await expect(page.getByText("https://github.com/example/repository-9.git", { exact: true })).toBeHidden();
  await pagination.getByRole("button", { name: "下一页" }).click();
  await expect(pagination).toContainText("第 2 / 2 页");
  await expect(page.getByText("https://github.com/example/repository-9.git", { exact: true })).toBeVisible();

  const search = page.getByRole("searchbox", { name: "搜索已添加仓库" });
  await search.fill("OPENAI/CODEX");
  await expect(page.getByText("https://github.com/openai/codex.git", { exact: true })).toBeVisible();
  await expect(fullAddress).toBeHidden();
  await expect(page.getByText("1 / 12 个", { exact: true })).toBeVisible();
  await expect(pagination).toBeHidden();

  await search.fill("does-not-exist");
  await expect(page.getByText("没有匹配的仓库")).toBeVisible();
});

test("Windows uses the native title bar without duplicate drag spacing", async ({ browser }) => {
  const context = await browser.newContext({
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140 Safari/537.36",
  });
  const page = await context.newPage();
  await page.goto("/");

  await expect(page.locator("[data-tauri-drag-region]")).toHaveCount(0);
  await expect(page.locator(".main-content")).toHaveCSS("padding-top", "0px");
  await expect(page.locator(".sidebar")).toHaveCSS("padding-top", "25px");

  await context.close();
});
