import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { latestStableVersion, releaseProject } from "./release.mjs";

function git(projectRoot, ...args) {
  return execFileSync("git", args, { cwd: projectRoot, encoding: "utf8" }).trim();
}

test("selects the latest stable tag", () => {
  assert.equal(
    latestStableVersion(["v0.9.0", "preview", "v0.10.0", "v1.0.0-dev.1"]),
    "0.10.0",
  );
});

test("creates a release commit and pushes its annotated tag", async () => {
  const temporaryRoot = await mkdtemp(path.join(os.tmpdir(), "gitboost-release-"));
  const remoteRoot = path.join(temporaryRoot, "remote.git");
  const projectRoot = path.join(temporaryRoot, "work");
  try {
    git(temporaryRoot, "init", "--bare", remoteRoot);
    git(temporaryRoot, "init", "-b", "main", projectRoot);
    git(projectRoot, "config", "user.name", "GitBoost Test");
    git(projectRoot, "config", "user.email", "gitboost@example.test");
    await writeFile(
      path.join(projectRoot, "package.json"),
      `${JSON.stringify({ name: "gitboost", version: "0.2.0-dev.1" }, null, 2)}\n`,
    );
    await writeFile(
      path.join(projectRoot, "package-lock.json"),
      `${JSON.stringify({
        name: "gitboost",
        version: "0.2.0-dev.1",
        lockfileVersion: 3,
        packages: { "": { name: "gitboost", version: "0.2.0-dev.1" } },
      }, null, 2)}\n`,
    );
    git(projectRoot, "add", "package.json", "package-lock.json");
    git(projectRoot, "commit", "-m", "chore: start development");
    git(projectRoot, "tag", "-a", "v0.1.0", "-m", "GitBoost v0.1.0");
    git(projectRoot, "remote", "add", "origin", remoteRoot);
    git(projectRoot, "push", "-u", "origin", "main", "--tags");

    const messages = [];
    assert.equal(
      await releaseProject("0.2.0", { projectRoot, log: (message) => messages.push(message) }),
      "v0.2.0",
    );

    const packageJson = JSON.parse(
      await readFile(path.join(projectRoot, "package.json"), "utf8"),
    );
    assert.equal(packageJson.version, "0.2.0");
    assert.equal(git(projectRoot, "log", "-1", "--format=%s"), "chore: release v0.2.0");
    assert.equal(git(projectRoot, "tag", "--list", "v0.2.0"), "v0.2.0");
    assert.equal(
      git(projectRoot, "rev-list", "-n", "1", "v0.2.0"),
      git(remoteRoot, "rev-parse", "refs/heads/main"),
    );
    assert.deepEqual(messages, [
      "[1/5] 检查分支和工作区状态...",
      "[2/5] 同步远程分支和 tags...",
      "[3/5] 更新项目版本为 0.2.0...",
      "[4/5] 创建发布提交和 tag...",
      "[5/5] 推送 main 和 tag...",
      "GitBoost v0.2.0 已推送，GitHub Release 工作流已触发。",
    ]);
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true });
  }
});
