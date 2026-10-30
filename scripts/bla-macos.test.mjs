import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { inspectBlaSection, verifyBlaBinary } from "./bla-macos.mjs";

test("rejects missing and truncated Mach-O headers", () => {
  assert.throws(() => inspectBlaSection(Buffer.alloc(0), Buffer.alloc(0)), /Mach-O/);
  assert.throws(() => inspectBlaSection(Buffer.from("not a binary"), Buffer.alloc(0)), /Mach-O/);
});

test("validates a real signed ARM64 section and rejects changed payloads and EOF trailers", {
  skip: process.platform !== "darwin" || process.arch !== "arm64",
}, (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "gitboost-bla-section-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const payload = Buffer.from("\0[BLA-NATIVE-SHIELD-V1]\0fixture payload\0");
  const payloadPath = path.join(directory, "payload.bin");
  const binary = path.join(directory, "probe");
  writeFileSync(payloadPath, payload);
  execFileSync("/usr/bin/clang", ["-arch", "arm64", "-x", "c", "-", "-o", binary,
    `-Wl,-sectcreate,__TEXT,__bla,${payloadPath}`], {
    input: "int main(void) { return 0; }\n",
  });
  execFileSync("/usr/bin/codesign", ["--force", "--sign", "-", binary]);
  const result = verifyBlaBinary(binary, payload);
  assert.equal(result.section.size, payload.length);
  const bytes = readFileSync(binary);
  assert.throws(() => inspectBlaSection(bytes, Buffer.from("wrong payload")), /数据不匹配/);
  assert.throws(() => inspectBlaSection(Buffer.concat([bytes, Buffer.from("trailer")]), payload),
    /段外存在追加数据/);
  assert.throws(() => inspectBlaSection(bytes.subarray(0, 40), payload), /越界/);

  const modified = Buffer.from(bytes);
  const alteredPayload = Buffer.from(payload);
  modified[result.section.offset + payload.length - 2] ^= 1;
  alteredPayload[payload.length - 2] ^= 1;
  writeFileSync(binary, modified);
  // Matching payload bytes alone must not hide a broken signature.
  assert.throws(() => verifyBlaBinary(binary, alteredPayload), /codesign/);
});
