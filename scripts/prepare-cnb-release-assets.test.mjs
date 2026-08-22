import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  prepareCnbReleaseAssets,
  releaseAssetPlan,
} from "./prepare-cnb-release-assets.mjs";

async function makeProject(version = "0.2.5") {
  const projectRoot = await mkdtemp(path.join(os.tmpdir(), "gitboost-cnb-assets-test-"));
  await mkdir(path.join(projectRoot, "scripts"), { recursive: true });
  await writeFile(
    path.join(projectRoot, "package.json"),
    `${JSON.stringify({ version }, null, 2)}\n`,
  );
  return projectRoot;
}

test("prepares versioned assets, stable aliases, and checksums", async (t) => {
  const projectRoot = await makeProject();
  t.after(() => rm(projectRoot, { recursive: true, force: true }));
  const requestedUrls = [];

  const result = await prepareCnbReleaseAssets({
    tag: "v0.2.5",
    projectRoot,
    logger: () => {},
    fetchImpl: async (url) => {
      requestedUrls.push(url);
      const name = url.split("/").at(-1);
      return new Response(`contents:${name}`, { status: 200 });
    },
  });

  assert.equal(result.releaseTag, "v0.2.5");
  assert.equal(requestedUrls.length, 4);
  assert.deepEqual(
    requestedUrls.map((url) => url.split("/").at(-1)),
    releaseAssetPlan("0.2.5").map(({ sourceName }) => sourceName),
  );

  for (const { sourceName, aliasName } of releaseAssetPlan("0.2.5")) {
    if (!aliasName) continue;
    const [source, alias] = await Promise.all([
      readFile(path.join(result.outputDirectory, sourceName)),
      readFile(path.join(result.outputDirectory, aliasName)),
    ]);
    assert.deepEqual(alias, source);
  }

  const checksumLines = (await readFile(path.join(result.outputDirectory, "SHA256SUMS"), "utf8"))
    .trim()
    .split("\n");
  assert.equal(checksumLines.length, 7);
  for (const line of checksumLines) {
    const [, expectedHash, name] = /^(\w{64})  (.+)$/.exec(line);
    const contents = await readFile(path.join(result.outputDirectory, name));
    assert.equal(expectedHash, createHash("sha256").update(contents).digest("hex"));
  }
});

test("rejects a release tag that does not match package.json", async (t) => {
  const projectRoot = await makeProject();
  t.after(() => rm(projectRoot, { recursive: true, force: true }));

  await assert.rejects(
    prepareCnbReleaseAssets({
      tag: "v0.2.4",
      projectRoot,
      logger: () => {},
      fetchImpl: () => assert.fail("should not download assets"),
    }),
    /发布版本不一致/,
  );
});

test("fails when a GitHub release asset cannot be downloaded", async (t) => {
  const projectRoot = await makeProject();
  t.after(() => rm(projectRoot, { recursive: true, force: true }));

  await assert.rejects(
    prepareCnbReleaseAssets({
      tag: "v0.2.5",
      projectRoot,
      logger: () => {},
      fetchImpl: async () => new Response("not found", { status: 404 }),
    }),
    /HTTP 404/,
  );
});
