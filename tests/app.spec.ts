import { expect, test } from "@playwright/test";

test("core desktop workflow is navigable", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();

  const dragRegion = page.locator("[data-tauri-drag-region]");
  await expect(dragRegion).toBeVisible();
  await expect(dragRegion).toHaveCSS("height", "29px");

  await expect(page.getByRole("navigation", { name: "主要导航" }).getByText("自定义节点", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(page.getByRole("heading", { name: "自定义节点", exact: true })).toBeVisible();
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
  await expect(page.getByLabel("GitHub 文件地址")).toHaveAttribute("placeholder", "https://github.com/owner/repo/releases/download/v1.0/file.zip");
  await expect(page.getByText("节点失败时不会静默改为 GitHub 直连。")).toBeVisible();

  await page.getByRole("button", { name: "使用日志", exact: true }).click();
  await expect(page.getByRole("heading", { name: "使用日志", exact: true })).toBeVisible();
  await expect(page.getByText("只保存脱敏结果")).toBeVisible();
  expect(errors).toEqual([]);
});

test("system node refresh reports whether the catalog changed", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", lineMode: "automatic", fixedNodeId: null, currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null },
      nodes: [{ id: "system", name: "system", rewriteBase: "https://proxy.example/https://github.com/", enabled: true, builtIn: true, health: { status: "untested", successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null } }],
      routes: [],
      environment: { gitAvailable: true, gitPath: "/usr/bin/git", gitVersion: "git version 2.51.1", includeRegistered: false, configPath: "/tmp/gitboost.gitconfig", conflicts: 0, conflictScanError: null },
    };
    let refreshCount = 0;
    Object.assign(window, { __TAURI_INTERNALS__: { invoke: async (command: string) => {
      if (command === "refresh_system_nodes") return refreshCount++ === 0;
      return snapshot;
    } } });
  });

  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const refresh = page.getByRole("button", { name: "刷新系统线路" });

  await refresh.click();
  await expect(page.locator(".toast")).toHaveText("系统线路已更新");
  await refresh.click();
  await expect(page.locator(".toast")).toHaveText("系统线路已是最新");
});

test("full node detection shows determinate progress", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", lineMode: "automatic", fixedNodeId: null, currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null },
      nodes: [{ id: "system", name: "system", rewriteBase: "https://proxy.example/https://github.com/", enabled: true, builtIn: true, health: { status: "untested", successCount: 0, attemptCount: 0, medianLatencyMs: null, consecutiveFailures: 0, checkedAt: null, failureReason: null } }],
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
            listeners.get("node-test-progress")?.({ event: "node-test-progress", id: 1, payload: { completed: 7, total: 18 } });
            return new Promise((resolve) => { completeNodeTest = () => resolve(snapshot.nodes); });
          }
          return snapshot;
        },
      },
      completeNodeTest: () => completeNodeTest?.(),
    });
  });

  await page.goto("/");
  const lineControl = page.locator(".section-title").filter({ has: page.getByRole("heading", { name: "线路控制" }) });
  await lineControl.getByRole("button", { name: "重新测速" }).click();

  await expect(lineControl.getByRole("button", { name: "检测中 7/18" })).toBeDisabled();
  const progress = page.getByRole("progressbar", { name: "线路检测进度 7/18" });
  await expect(progress).toHaveAttribute("aria-valuenow", "7");
  const fillRatio = await progress.locator("span").evaluate((fill) => fill.clientWidth / fill.parentElement!.clientWidth);
  expect(fillRatio).toBeCloseTo(7 / 18, 2);

  await page.evaluate(() => (window as typeof window & { completeNodeTest: () => void }).completeNodeTest());
  await expect(page.locator(".toast")).toHaveText("节点检测完成");
  await expect(page.getByRole("progressbar")).toHaveCount(0);
  await expect(lineControl.getByRole("button", { name: "重新测速" })).toBeEnabled();
});

test("@integration core workflow preserves state across command boundaries", async ({ page }) => {
  await page.addInitScript(() => {
    const snapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", lineMode: "automatic", fixedNodeId: null, currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null as string | null },
      nodes: [{ id: "verified-node", name: "Verified Node", rewriteBase: "https://proxy.integration.test/https://github.com/", enabled: true, builtIn: false, health: { status: "available", successCount: 2, attemptCount: 2, medianLatencyMs: 25, consecutiveFailures: 0, checkedAt: "2026-08-14T00:00:00Z", failureReason: null } }],
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
        if (command === "set_acceleration") {
          if (args.enabled && snapshot.routes.length === 0) throw new Error("仅加速清单为空，请先加入至少一个公开仓库");
          snapshot.settings.accelerationEnabled = Boolean(args.enabled);
          snapshot.settings.lineMode = args.enabled ? "automatic" : snapshot.settings.lineMode;
          snapshot.settings.currentNodeId = args.enabled ? "verified-node" : null;
          snapshot.settings.lastAppliedAt = "2026-08-14T00:00:01Z";
          snapshot.environment.includeRegistered = true;
          return copy();
        }
        if (command === "set_line_mode") {
          snapshot.settings.lineMode = String(args.mode) as "automatic" | "direct";
          if (args.mode === "direct") {
            snapshot.settings.accelerationEnabled = false;
            snapshot.settings.currentNodeId = null;
          }
          return copy();
        }
        if (command === "restore_git_config") {
          snapshot.settings.accelerationEnabled = false;
          snapshot.settings.lineMode = "direct";
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
  await expect(page.locator(".toast")).toHaveText("仅加速清单为空，请先加入至少一个公开仓库");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();

  await page.getByRole("button", { name: "路由清单", exact: true }).click();
  await page.getByRole("textbox", { name: "GitHub 仓库" }).fill("openai/codex");
  await page.getByRole("button", { name: "加入清单" }).click();
  await expect(page.getByText("https://github.com/openai/codex.git", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "总览", exact: true }).click();
  await page.getByRole("button", { name: "开启加速" }).click();
  await expect(page.getByRole("heading", { name: "读取线路已接入" })).toBeVisible();
  await expect(page.getByText("独立配置已注册", { exact: true })).toBeVisible();
  await expect(page.getByText("加速已开启", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "直连", exact: true }).click();
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();
  await expect(page.getByText("当前为直连", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: "恢复 Git 配置" }).click();
  await expect(page.locator(".toast")).toHaveText("GitBoost 配置已恢复为直连");

  const calls = await page.evaluate(() => (window as typeof window & { __workflowCalls: { command: string; args: Record<string, unknown> }[] }).__workflowCalls);
  expect(calls.filter(({ command }) => command === "get_snapshot").length).toBeGreaterThan(0);
  expect(calls.filter(({ command }) => ["set_acceleration", "add_route", "set_line_mode", "restore_git_config"].includes(command))).toEqual([
    { command: "set_acceleration", args: { enabled: true } },
    { command: "add_route", args: { repositoryUrl: "openai/codex" } },
    { command: "set_acceleration", args: { enabled: true } },
    { command: "set_line_mode", args: { mode: "direct", nodeId: null } },
    { command: "restore_git_config", args: {} },
  ]);
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
    canvas: "#f7f9fc",
    sidebar: "#edf4fb",
    surface: "#ffffff",
    accent: "#1769c2",
  });
  await expect(page.locator(".status-board")).toHaveCSS("background-color", "rgb(255, 255, 255)");
  await expect(page.getByRole("navigation", { name: "主要导航" }).getByRole("button", { name: "总览" })).toHaveCSS("color", "rgb(23, 105, 194)");
});

test("node import keeps failures open and reports actual results", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("button", { name: "添加节点" }).click();

  await page.evaluate(() => {
    const snapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", lineMode: "automatic", fixedNodeId: null, currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null },
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
    const snapshot = {
      settings: { schemaVersion: 1, accelerationEnabled: false, routeScope: "allowlist", lineMode: "automatic", fixedNodeId: null, currentNodeId: null, healthCheckMinutes: 30, launchAtLogin: false, logLevel: "info", usageLoggingEnabled: true, lastAppliedAt: null },
      nodes: [],
      routes: [
        { id: "long", repositoryUrl, createdAt: "2026-08-13T00:00:00Z" },
        { id: "codex", repositoryUrl: "https://github.com/openai/codex.git", createdAt: "2026-08-13T00:00:00Z" },
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

  const search = page.getByRole("searchbox", { name: "搜索已添加仓库" });
  await search.fill("OPENAI/CODEX");
  await expect(page.getByText("https://github.com/openai/codex.git", { exact: true })).toBeVisible();
  await expect(fullAddress).toBeHidden();
  await expect(page.getByText("1 / 2 个", { exact: true })).toBeVisible();

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
