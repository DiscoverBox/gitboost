import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

import { assertReleaseTagMatches } from "./version.mjs";

const defaultProjectRoot = fileURLToPath(new URL("../", import.meta.url));

export const githubRepository = "DiscoverBox/gitboost";

export function releaseAssetPlan(version) {
  return [
    {
      sourceName: `GitBoost_${version}_aarch64.app.tar.gz`,
    },
    {
      sourceName: `GitBoost_${version}_aarch64.dmg`,
      aliasName: "GitBoost-macOS-arm64.dmg",
    },
    {
      sourceName: `GitBoost_${version}_x64-setup.exe`,
      aliasName: "GitBoost-Windows-x64-setup.exe",
    },
    {
      sourceName: `GitBoost_${version}_x64_en-US.msi`,
      aliasName: "GitBoost-Windows-x64.msi",
    },
  ];
}

function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

export async function prepareCnbReleaseAssets({
  tag,
  projectRoot = defaultProjectRoot,
  outputDirectory = path.join(projectRoot, "dist", "cnb-release"),
  fetchImpl = fetch,
  logger = console.log,
} = {}) {
  const packageJson = JSON.parse(
    await readFile(path.join(projectRoot, "package.json"), "utf8"),
  );
  const releaseTag = tag || `v${packageJson.version}`;
  const version = await assertReleaseTagMatches(releaseTag, projectRoot);
  const plan = releaseAssetPlan(version);

  await rm(outputDirectory, { recursive: true, force: true });
  await mkdir(outputDirectory, { recursive: true });

  const writtenFiles = [];
  for (const asset of plan) {
    const url = `https://github.com/${githubRepository}/releases/download/${releaseTag}/${asset.sourceName}`;
    logger(`正在下载 ${asset.sourceName}`);
    const response = await fetchImpl(url, {
      headers: { "user-agent": "GitBoost-CNB-Release-Sync" },
      redirect: "follow",
      signal: AbortSignal.timeout(10 * 60 * 1000),
    });
    if (!response.ok) {
      throw new Error(`下载失败：${asset.sourceName}（HTTP ${response.status}）`);
    }

    const contents = Buffer.from(await response.arrayBuffer());
    if (contents.length === 0) {
      throw new Error(`下载失败：${asset.sourceName} 内容为空`);
    }

    await writeFile(path.join(outputDirectory, asset.sourceName), contents);
    writtenFiles.push({ name: asset.sourceName, contents });

    if (asset.aliasName) {
      await writeFile(path.join(outputDirectory, asset.aliasName), contents);
      writtenFiles.push({ name: asset.aliasName, contents });
    }
    logger(`已下载 ${asset.sourceName}（${contents.length} bytes）`);
  }

  writtenFiles.sort((left, right) => left.name.localeCompare(right.name, "en"));
  const checksums = writtenFiles
    .map(({ name, contents }) => `${sha256(contents)}  ${name}`)
    .join("\n");
  await writeFile(path.join(outputDirectory, "SHA256SUMS"), `${checksums}\n`);

  return {
    releaseTag,
    outputDirectory,
    files: [...writtenFiles.map(({ name }) => name), "SHA256SUMS"],
  };
}

async function main() {
  const result = await prepareCnbReleaseAssets({
    tag: process.env.RELEASE_TAG || process.argv[2],
  });
  console.log(`已准备 ${result.files.length} 个 CNB Release 附件：${result.outputDirectory}`);
  console.log(`##[set-output release_tag=${result.releaseTag}]`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
