import { expect, test } from "@playwright/test";

test("core desktop workflow is navigable", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();

  const dragRegion = page.locator("[data-tauri-drag-region]");
  await expect(dragRegion).toBeVisible();
  await expect(dragRegion).toHaveCSS("height", "29px");

  await page.getByRole("button", { name: "自定义节点", exact: true }).click();
  await expect(page.getByRole("heading", { name: "自定义节点", exact: true })).toBeVisible();
  await expect(page.getByText("1 个，由远程目录和本地缓存自动维护。", { exact: true })).toBeVisible();
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

test("node import keeps failures open and reports actual results", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "自定义节点", exact: true }).click();
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
