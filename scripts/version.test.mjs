import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  assertReleaseTagMatches,
  compareStableVersions,
  parseStableVersion,
  parseVersion,
  setProjectVersion,
} from "./version.mjs";

test("accepts development versions and requires stable release versions", () => {
  assert.equal(parseVersion("0.3.0-dev.1").prerelease, "dev.1");
  assert.equal(parseStableVersion("1.2.3").value, "1.2.3");
  assert.throws(() => parseVersion("v1.2.3"), /无效版本/);
  assert.throws(() => parseVersion("1.2.3-01"), /无效版本/);
  assert.throws(() => parseVersion("1.2.3-dev.01"), /无效版本/);
  assert.throws(() => parseStableVersion("1.2.3-dev.1"), /不能包含预发布标识/);
  assert.ok(compareStableVersions("0.10.0", "0.9.9") > 0);
});

test("updates package metadata and validates the matching release tag", async () => {
  const projectRoot = await mkdtemp(path.join(os.tmpdir(), "gitboost-version-"));
  try {
    await writeFile(
      path.join(projectRoot, "package.json"),
      `${JSON.stringify({ name: "gitboost", version: "0.1.0" }, null, 2)}\n`,
    );
    await writeFile(
      path.join(projectRoot, "package-lock.json"),
      `${JSON.stringify({
        name: "gitboost",
        version: "0.1.0",
        lockfileVersion: 3,
        packages: { "": { name: "gitboost", version: "0.1.0" } },
      }, null, 2)}\n`,
    );

    await setProjectVersion("0.2.0", projectRoot);
    const packageJson = JSON.parse(
      await readFile(path.join(projectRoot, "package.json"), "utf8"),
    );
    const packageLock = JSON.parse(
      await readFile(path.join(projectRoot, "package-lock.json"), "utf8"),
    );
    assert.equal(packageJson.version, "0.2.0");
    assert.equal(packageLock.version, "0.2.0");
    assert.equal(packageLock.packages[""].version, "0.2.0");
    assert.equal(await assertReleaseTagMatches("v0.2.0", projectRoot), "0.2.0");
    await assert.rejects(
      assertReleaseTagMatches("v0.3.0", projectRoot),
      /发布版本不一致/,
    );
  } finally {
    await rm(projectRoot, { recursive: true, force: true });
  }
});
