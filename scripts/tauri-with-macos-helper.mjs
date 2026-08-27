#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import {
  access,
  readFile,
  readdir,
  rename,
  rm,
} from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const defaultProjectRoot = path.resolve(scriptDirectory, "..");

export const dmgHelperName = "无法打开时请双击.command";

export function bundleRootForBuild(projectRoot, buildArgs) {
  const targetIndex = buildArgs.indexOf("--target");
  const target = targetIndex === -1 ? null : buildArgs[targetIndex + 1];
  if (targetIndex !== -1 && (!target || target.startsWith("-"))) {
    throw new Error("--target 后缺少 Rust target");
  }

  return path.join(
    projectRoot,
    "src-tauri",
    "target",
    ...(target ? [target] : []),
    "release",
    "bundle",
  );
}

export function createDmgArguments({
  appName,
  bundleRoot,
  helperPath,
  outputPath,
}) {
  const dmgDirectory = path.join(bundleRoot, "dmg");
  return [
    "--volname",
    appName,
    "--volicon",
    path.join(dmgDirectory, "icon.icns"),
    "--window-size",
    "660",
    "400",
    "--icon-size",
    "128",
    "--icon",
    `${appName}.app`,
    "180",
    "170",
    "--hide-extension",
    `${appName}.app`,
    "--app-drop-link",
    "480",
    "170",
    "--add-file",
    dmgHelperName,
    helperPath,
    "330",
    "300",
    "--hide-extension",
    dmgHelperName,
    outputPath,
    path.join(bundleRoot, "macos"),
  ];
}

async function assertExists(targetPath, label) {
  try {
    await access(targetPath);
  } catch {
    throw new Error(`${label}不存在：${targetPath}`);
  }
}

export async function addHelperToDmg({
  projectRoot = defaultProjectRoot,
  buildArgs = [],
  runCommand = (command, args, options) => execFileSync(command, args, options),
} = {}) {
  const config = JSON.parse(
    await readFile(path.join(projectRoot, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  const appName = config.productName;
  if (!appName) throw new Error("tauri.conf.json 缺少 productName");

  const bundleRoot = bundleRootForBuild(projectRoot, buildArgs);
  const dmgDirectory = path.join(bundleRoot, "dmg");
  const bundleScript = path.join(dmgDirectory, "bundle_dmg.sh");
  const helperPath = path.join(projectRoot, "scripts", "open-gitboost.command");
  const appPath = path.join(bundleRoot, "macos", `${appName}.app`);

  await Promise.all([
    assertExists(bundleScript, "Tauri DMG 打包脚本"),
    assertExists(helperPath, "macOS 安装助手"),
    assertExists(appPath, "macOS 应用包"),
  ]);

  const dmgFiles = (await readdir(dmgDirectory)).filter(
    (name) => name.endsWith(".dmg") && !name.startsWith("."),
  );
  if (dmgFiles.length !== 1) {
    throw new Error(`预期找到一个 Tauri DMG，实际找到 ${dmgFiles.length} 个`);
  }

  const originalPath = path.join(dmgDirectory, dmgFiles[0]);
  const temporaryPath = path.join(dmgDirectory, `.${dmgFiles[0]}.with-helper.dmg`);
  const backupPath = path.join(dmgDirectory, `.${dmgFiles[0]}.tauri-backup`);
  await Promise.all([
    rm(temporaryPath, { force: true }),
    rm(backupPath, { force: true }),
  ]);

  await runCommand(
    "/bin/bash",
    [
      bundleScript,
      ...createDmgArguments({ appName, bundleRoot, helperPath, outputPath: temporaryPath }),
    ],
    { cwd: projectRoot, stdio: "inherit" },
  );
  await assertExists(temporaryPath, "包含安装助手的新 DMG");

  await rename(originalPath, backupPath);
  try {
    await rename(temporaryPath, originalPath);
  } catch (error) {
    await rename(backupPath, originalPath);
    throw error;
  }
  await rm(backupPath, { force: true });

  console.log(`已将“${dmgHelperName}”加入 ${originalPath}`);
  return originalPath;
}

export async function runTauriWithMacosHelper(
  args,
  {
    projectRoot = defaultProjectRoot,
    platform = process.platform,
    runTauri = (command, commandArgs, options) => execFileSync(command, commandArgs, options),
  } = {},
) {
  const tauriCli = path.join(
    projectRoot,
    "node_modules",
    ".bin",
    platform === "win32" ? "tauri.cmd" : "tauri",
  );
  runTauri(tauriCli, args, { cwd: projectRoot, stdio: "inherit" });

  if (platform === "darwin" && args[0] === "build") {
    await addHelperToDmg({ projectRoot, buildArgs: args.slice(1) });
  }
}

const invokedDirectly = process.argv[1]
  && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (invokedDirectly) {
  runTauriWithMacosHelper(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
