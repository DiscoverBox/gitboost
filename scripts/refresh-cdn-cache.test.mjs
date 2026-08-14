import assert from "node:assert/strict";
import test from "node:test";

import { refreshCdnCache } from "./refresh-cdn-cache.mjs";

const repository = "DiscoverBox/gitboost";
const ref = "main";
const cdnPath = `/gh/${repository}@${ref}/nodes.enc.json`;

test("purges jsDelivr and waits for JSDMirror to match nodes.enc.json", async () => {
  const expectedContent = '{"version":1}\n';

  let mirrorChecks = 0;
  const requestedUrls = [];
  const fetchImpl = async (url) => {
    requestedUrls.push(url);
    if (url.startsWith("https://purge.jsdelivr.net/")) {
      return Response.json({
        status: "finished",
        paths: { [cdnPath]: { providers: { CF: true, FY: true } } },
      });
    }
    if (url.startsWith("https://cdn.jsdmirror.cn/")) {
      mirrorChecks += 1;
      return new Response(mirrorChecks === 1 ? "stale" : expectedContent);
    }
    return new Response(expectedContent);
  };

  await refreshCdnCache({
    repository,
    ref,
    readFileImpl: async () => Buffer.from(expectedContent),
    fetchImpl,
    wait: async () => {},
    maxAttempts: 2,
    pollIntervalMs: 0,
    log: () => {},
  });

  assert.equal(mirrorChecks, 2);
  assert.equal(requestedUrls[0], `https://purge.jsdelivr.net${cdnPath}`);
});

test("fails when a jsDelivr provider does not purge the cache", async () => {
  await assert.rejects(
    refreshCdnCache({
      repository,
      ref,
      readFileImpl: async () => Buffer.from("[]\n"),
      fetchImpl: async () =>
        Response.json({
          status: "finished",
          paths: { [cdnPath]: { providers: { CF: true, FY: false } } },
        }),
      log: () => {},
    }),
    /jsDelivr cache purge failed/,
  );
});
