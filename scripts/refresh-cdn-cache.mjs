import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const sleep = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

export async function refreshCdnCache({
  repository,
  ref,
  filePath = "nodes.enc.json",
  readFileImpl = readFile,
  fetchImpl = fetch,
  wait = sleep,
  maxAttempts = 21,
  pollIntervalMs = 30_000,
  log = console.log,
}) {
  const cdnPath = `/gh/${repository}@${ref}/${filePath}`;
  const purgeUrl = `https://purge.jsdelivr.net${cdnPath}`;
  const jsDelivrUrl = `https://cdn.jsdelivr.net${cdnPath}`;
  const mirrorUrl = `https://cdn.jsdmirror.com${cdnPath}`;
  const expectedContent = await readFileImpl(filePath);

  const purgeResponse = await fetchImpl(purgeUrl);
  if (!purgeResponse.ok) {
    throw new Error(`jsDelivr cache purge failed: HTTP ${purgeResponse.status}`);
  }

  const purgeResult = await purgeResponse.json();
  const providers = Object.values(
    purgeResult.paths?.[cdnPath]?.providers ?? {},
  );
  if (
    purgeResult.status !== "finished" ||
    providers.length === 0 ||
    providers.some((succeeded) => !succeeded)
  ) {
    throw new Error(`jsDelivr cache purge failed: ${JSON.stringify(purgeResult)}`);
  }

  log(`jsDelivr cache purged: ${jsDelivrUrl}`);

  const contentMatches = async (url) => {
    try {
      const response = await fetchImpl(url, { cache: "no-store" });
      if (!response.ok) {
        return false;
      }
      const content = Buffer.from(await response.arrayBuffer());
      return content.equals(expectedContent);
    } catch (error) {
      log(`Cache check failed for ${url}: ${error.message}`);
      return false;
    }
  };

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const [jsDelivrReady, mirrorReady] = await Promise.all([
      contentMatches(jsDelivrUrl),
      contentMatches(mirrorUrl),
    ]);

    if (jsDelivrReady) {
      if (mirrorReady) {
        log(`CDN cache refreshed: ${mirrorUrl}`);
      } else {
        log(
          `::warning::JSDMirror cache is still stale; ` +
            `jsDelivr has refreshed successfully: ${mirrorUrl}`,
        );
      }
      return;
    }

    if (attempt === maxAttempts) {
      throw new Error(
        `jsDelivr cache did not refresh after ${maxAttempts} checks`,
      );
    }

    log(
      `Waiting for CDN refresh (${attempt}/${maxAttempts}): ` +
        `jsDelivr=${jsDelivrReady}, JSDMirror=${mirrorReady}`,
    );
    await wait(pollIntervalMs);
  }
}

async function main() {
  await refreshCdnCache({
    repository: process.env.GITHUB_REPOSITORY ?? "DiscoverBox/gitboost",
    ref: process.env.GITHUB_REF_NAME ?? "main",
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
