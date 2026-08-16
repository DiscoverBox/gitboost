import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

import {
  compareStableVersions,
  parseStableVersion,
  setProjectVersion,
} from "./version.mjs";

const gitTimeout = 120_000;

function runGit(args, projectRoot, {
  allowFailure = false,
  showOutput = false,
} = {}) {
  const result = spawnSync("git", args, {
    cwd: projectRoot,
    encoding: "utf8",
    stdio: [
      "ignore",
      showOutput ? "inherit" : "pipe",
      showOutput ? "inherit" : "pipe",
    ],
    timeout: gitTimeout,
  });
  if (result.error) {
    if (result.error.code === "ETIMEDOUT") {
      throw new Error(`git ${args.join(" ")} 超过 2 分钟未完成，已终止。`);
    }
    throw result.error;
  }
  if (result.status !== 0 && !allowFailure) {
    throw new Error(result.stderr?.trim() || `git ${args.join(" ")} 执行失败。`);
  }
  return { status: result.status, output: result.stdout?.trim() ?? "" };
}

export function latestStableVersion(tags) {
  return tags
    .map((tag) => tag.match(/^v(.+)$/)?.[1])
    .filter((version) => {
      try {
        parseStableVersion(version);
        return true;
      } catch {
        return false;
      }
    })
    .sort(compareStableVersions)
    .at(-1) ?? null;
}

export async function releaseProject(version, {
  projectRoot = process.cwd(),
  log = console.log,
} = {}) {
  const stableVersion = parseStableVersion(version).value;
  const tag = `v${stableVersion}`;

  log("[1/5] 检查分支和工作区状态...");
  if (runGit(["branch", "--show-current"], projectRoot).output !== "main") {
    throw new Error("只能从 main 分支发布。");
  }
  if (runGit(["status", "--porcelain"], projectRoot).output) {
    throw new Error("工作区存在未提交修改，请先提交或处理后再发布。");
  }

  log("[2/5] 同步远程分支和 tags...");
  runGit(["fetch", "origin", "main", "--tags"], projectRoot, {
    showOutput: true,
  });
  const head = runGit(["rev-parse", "HEAD"], projectRoot).output;
  const remoteMain = runGit(["rev-parse", "origin/main"], projectRoot).output;
  if (head !== remoteMain) {
    throw new Error("当前 main 与 origin/main 不一致，请先同步并推送代码。");
  }

  const tags = runGit(["tag", "--list"], projectRoot).output
    .split("\n")
    .filter(Boolean);
  if (tags.includes(tag)) {
    throw new Error(`tag ${tag} 已存在，不会覆盖或移动已有 tag。`);
  }
  const latest = latestStableVersion(tags);
  if (latest && compareStableVersions(stableVersion, latest) <= 0) {
    throw new Error(`发布版本 ${stableVersion} 必须高于当前版本 ${latest}。`);
  }

  log(`[3/5] 更新项目版本为 ${stableVersion}...`);
  await setProjectVersion(stableVersion, projectRoot);
  runGit(["add", "package.json", "package-lock.json"], projectRoot);
  const diffResult = runGit(
    ["diff", "--cached", "--quiet"],
    projectRoot,
    { allowFailure: true },
  );
  if (diffResult.status !== 0 && diffResult.status !== 1) {
    throw new Error("无法确认版本文件的暂存状态。");
  }
  const hasVersionChanges = diffResult.status === 1;
  log(`[4/5] ${hasVersionChanges ? "创建发布提交和 tag" : "创建发布 tag"}...`);
  if (hasVersionChanges) {
    runGit(["commit", "-m", `chore: release ${tag}`], projectRoot, {
      showOutput: true,
    });
  }

  runGit(["tag", "-a", tag, "-m", `GitBoost ${tag}`], projectRoot);
  log("[5/5] 推送 main 和 tag...");
  try {
    runGit(
      ["push", "--atomic", "origin", "HEAD:refs/heads/main", `refs/tags/${tag}`],
      projectRoot,
      { showOutput: true },
    );
  } catch (error) {
    throw new Error(
      `${error.message}\n本地已创建 ${tag}，远程未完成发布，请检查后手动推送或处理该 tag。`,
    );
  }

  log(`GitBoost ${tag} 已推送，GitHub Release 工作流已触发。`);
  return tag;
}

async function main() {
  const version = process.argv[2];
  if (!version) throw new Error("用法：npm run release -- <X.Y.Z>");
  await releaseProject(version);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
