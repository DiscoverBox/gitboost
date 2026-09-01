import assert from "node:assert/strict";
import test from "node:test";

import { refreshCdnCache } from "./refresh-cdn-cache.mjs";

const repository = "DiscoverBox/gitboost";
const ref = "main";
const cdnPath = `/gh/${repository}@${ref}/nodes.enc.json`;

test("purges jsDelivr and waits for its content to refresh", async () => {
  const expectedContent = '{"version":1}\n';

  let jsDelivrChecks = 0;
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
    if (url.startsWith("https://cdn.jsdmirror.com/")) {
      mirrorChecks += 1;
      return new Response(mirrorChecks === 1 ? "stale" : expectedContent);
    }
    jsDelivrChecks += 1;
    return new Response(jsDelivrChecks === 1 ? "stale" : expectedContent);
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

test("warns without failing when only JSDMirror remains stale", async () => {
  const expectedContent = '{"version":1}\n';
  const logs = [];

  await refreshCdnCache({
    repository,
    ref,
    readFileImpl: async () => Buffer.from(expectedContent),
    fetchImpl: async (url) => {
      if (url.startsWith("https://purge.jsdelivr.net/")) {
        return Response.json({
          status: "finished",
          paths: { [cdnPath]: { providers: { CF: true, FY: true } } },
        });
      }
      if (url.startsWith("https://cdn.jsdmirror.com/")) {
        return new Response("stale");
      }
      return new Response(expectedContent);
    },
    log: (message) => logs.push(message),
  });

  assert.ok(logs.some((message) => message.startsWith("::warning::")));
});

test("fails when jsDelivr content remains stale", async () => {
  const expectedContent = '{"version":1}\n';

  await assert.rejects(
    refreshCdnCache({
      repository,
      ref,
      readFileImpl: async () => Buffer.from(expectedContent),
      fetchImpl: async (url) => {
        if (url.startsWith("https://purge.jsdelivr.net/")) {
          return Response.json({
            status: "finished",
            paths: { [cdnPath]: { providers: { CF: true, FY: true } } },
          });
        }
        if (url.startsWith("https://cdn.jsdelivr.net/")) {
          return new Response("stale");
        }
        return new Response(expectedContent);
      },
      wait: async () => {},
      maxAttempts: 2,
      pollIntervalMs: 0,
      log: () => {},
    }),
    /jsDelivr cache did not refresh after 2 checks/,
  );
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
