import assert from "node:assert/strict";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { mkdtemp } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  addHelperToDmg,
  bundleRootForBuild,
  dmgHelperName,
} from "./tauri-with-macos-helper.mjs";

test("install helper only removes quarantine from the expected GitBoost app", async () => {
  const helper = await readFile(
    path.join(import.meta.dirname, "open-gitboost.command"),
    "utf8",
  );
  assert.match(helper, /app_path="\/Applications\/GitBoost\.app"/);
  assert.match(helper, /pro\.gitboost\.desktop/);
  assert.match(
    helper,
    /\/usr\/bin\/sudo \/usr\/bin\/xattr -rd com\.apple\.quarantine "\$app_path"/,
  );
});

test("resolves native and explicit-target bundle directories", () => {
  assert.equal(
    bundleRootForBuild("/project", []),
    path.join("/project", "src-tauri", "target", "release", "bundle"),
  );
  assert.equal(
    bundleRootForBuild("/project", ["--target", "aarch64-apple-darwin"]),
    path.join(
      "/project",
      "src-tauri",
      "target",
      "aarch64-apple-darwin",
      "release",
      "bundle",
    ),
  );
  assert.throws(() => bundleRootForBuild("/project", ["--target"]), /缺少 Rust target/);
});

test("replaces the Tauri DMG with one containing the install helper", async (t) => {
  const projectRoot = await mkdtemp(path.join(os.tmpdir(), "gitboost-dmg-helper-test-"));
  t.after(() => rm(projectRoot, { recursive: true, force: true }));

  const bundleRoot = bundleRootForBuild(projectRoot, []);
  const dmgDirectory = path.join(bundleRoot, "dmg");
  const appDirectory = path.join(bundleRoot, "macos", "GitBoost.app");
  await Promise.all([
    mkdir(dmgDirectory, { recursive: true }),
    mkdir(appDirectory, { recursive: true }),
    mkdir(path.join(projectRoot, "scripts"), { recursive: true }),
    mkdir(path.join(projectRoot, "src-tauri"), { recursive: true }),
  ]);
  await Promise.all([
    writeFile(
      path.join(projectRoot, "src-tauri", "tauri.conf.json"),
      `${JSON.stringify({ productName: "GitBoost" })}\n`,
    ),
    writeFile(path.join(projectRoot, "scripts", "open-gitboost.command"), "#!/bin/zsh\n"),
    writeFile(path.join(dmgDirectory, "bundle_dmg.sh"), "#!/bin/bash\n"),
    writeFile(path.join(dmgDirectory, "icon.icns"), "icon"),
    writeFile(path.join(dmgDirectory, "GitBoost_0.2.6_aarch64.dmg"), "original"),
  ]);

  let invocation;
  const result = await addHelperToDmg({
    projectRoot,
    runCommand: (_command, args) => {
      invocation = args;
      const outputPath = args.at(-2);
      return writeFile(outputPath, "with-helper");
    },
  });

  assert.equal(await readFile(result, "utf8"), "with-helper");
  assert.ok(invocation.includes("--add-file"));
  assert.ok(invocation.includes(dmgHelperName));
  assert.equal(invocation.at(-1), path.join(bundleRoot, "macos"));
});
