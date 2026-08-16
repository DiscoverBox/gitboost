import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const versionPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;
const defaultProjectRoot = fileURLToPath(new URL("../", import.meta.url));

export function parseVersion(value) {
  const match = versionPattern.exec(value ?? "");
  const hasInvalidNumericPrerelease = match?.[4]
    ?.split(".")
    .some((identifier) => /^\d+$/.test(identifier) && identifier.length > 1 && identifier.startsWith("0"));
  if (!match || hasInvalidNumericPrerelease) {
    throw new Error(`无效版本“${value ?? ""}”，请使用 X.Y.Z 或 X.Y.Z-预发布标识。`);
  }
  return {
    value,
    numbers: match.slice(1, 4).map(Number),
    prerelease: match[4] ?? null,
  };
}

export function parseStableVersion(value) {
  const parsed = parseVersion(value);
  if (parsed.prerelease) {
    throw new Error(`正式发布版本不能包含预发布标识：${value}`);
  }
  return parsed;
}

export function compareStableVersions(left, right) {
  const leftNumbers = parseStableVersion(left).numbers;
  const rightNumbers = parseStableVersion(right).numbers;
  for (let index = 0; index < leftNumbers.length; index += 1) {
    if (leftNumbers[index] !== rightNumbers[index]) {
      return leftNumbers[index] - rightNumbers[index];
    }
  }
  return 0;
}

export async function readProjectVersion(projectRoot = defaultProjectRoot) {
  const packageJson = JSON.parse(
    await readFile(path.join(projectRoot, "package.json"), "utf8"),
  );
  return parseVersion(packageJson.version).value;
}

export async function setProjectVersion(version, projectRoot = defaultProjectRoot) {
  const normalized = parseVersion(version).value;
  const packagePath = path.join(projectRoot, "package.json");
  const lockPath = path.join(projectRoot, "package-lock.json");
  const [packageJson, packageLock] = await Promise.all([
    readFile(packagePath, "utf8").then(JSON.parse),
    readFile(lockPath, "utf8").then(JSON.parse),
  ]);

  packageJson.version = normalized;
  packageLock.version = normalized;
  if (!packageLock.packages?.[""]) {
    throw new Error("package-lock.json 缺少根包信息。");
  }
  packageLock.packages[""].version = normalized;

  await Promise.all([
    writeFile(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`),
    writeFile(lockPath, `${JSON.stringify(packageLock, null, 2)}\n`),
  ]);
  return normalized;
}

export async function assertReleaseTagMatches(tag, projectRoot = defaultProjectRoot) {
  if (!tag?.startsWith("v")) {
    throw new Error(`无效发布 tag：${tag ?? ""}`);
  }
  const tagVersion = parseStableVersion(tag.slice(1)).value;
  const projectVersion = await readProjectVersion(projectRoot);
  if (tagVersion !== projectVersion) {
    throw new Error(
      `发布版本不一致：tag=${tagVersion}，package.json=${projectVersion}`,
    );
  }
  return tagVersion;
}

async function main() {
  const [command, value] = process.argv.slice(2);
  if (command === "set") {
    if (!value) throw new Error("用法：npm run version:set -- <版本>");
    const version = await setProjectVersion(value);
    console.log(`本地应用版本已更新为 ${version}`);
    return;
  }
  if (command === "check-tag") {
    const version = await assertReleaseTagMatches(process.env.RELEASE_TAG);
    console.log(`发布版本校验通过：${version}`);
    return;
  }
  throw new Error("未知命令，请使用 set 或 check-tag。");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
