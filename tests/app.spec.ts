import { expect, test } from "@playwright/test";

test("core desktop workflow is navigable", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "使用 GitHub 原地址，按需加速" })).toBeVisible();

  const dragRegion = page.locator("[data-tauri-drag-region]");
  await expect(dragRegion).toBeVisible();
  await expect(dragRegion).toHaveCSS("height", "29px");

  await page.getByRole("button", { name: "节点", exact: true }).click();
  await expect(page.getByRole("heading", { name: "节点", exact: true })).toBeVisible();
  await expect(page.getByText("https://fastgit.cc/https://github.com/", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "导入节点" }).click();
  await expect(page.getByRole("dialog", { name: "导入节点" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "导入节点" })).toBeHidden();

  await page.getByRole("button", { name: "路由清单", exact: true }).click();
  await expect(page.getByText("访问私有仓库或不确定时，建议仅加速清单。")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "GitHub 仓库" })).toHaveAttribute("placeholder", "anthropics/skills.git");

  await page.getByRole("button", { name: "文件下载", exact: true }).click();
  await expect(page.getByRole("heading", { name: "文件下载", exact: true })).toBeVisible();
  await expect(page.getByLabel("GitHub 文件地址")).toHaveAttribute("placeholder", "https://github.com/owner/repo/releases/download/v1.0/file.zip");
  await expect(page.getByText("节点失败时不会静默改为 GitHub 直连。")).toBeVisible();

  await page.getByRole("button", { name: "使用日志", exact: true }).click();
  await expect(page.getByRole("heading", { name: "使用日志", exact: true })).toBeVisible();
  await expect(page.getByText("只保存脱敏结果")).toBeVisible();
  expect(errors).toEqual([]);
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
