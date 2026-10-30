import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { inspectBlaSection, inspectPeBlaSection, installerExecutableBytes, linkerArguments, verifyBlaBinary } from "./bla-native.mjs";

test("rejects missing and truncated Mach-O headers", () => {
  assert.throws(() => inspectBlaSection(Buffer.alloc(0), Buffer.alloc(0)), /Mach-O/);
  assert.throws(() => inspectBlaSection(Buffer.from("not a binary"), Buffer.alloc(0)), /Mach-O/);
});

test("rejects invalid PE files and forces the Windows payload to remain linked", () => {
  assert.throws(() => inspectPeBlaSection(Buffer.alloc(0), Buffer.alloc(0)), /PE/);
  assert.throws(() => inspectPeBlaSection(Buffer.alloc(64), Buffer.alloc(0)), /PE/);
  const args = linkerArguments("win32", "payload");
  assert.ok(args.includes("link-arg=/INCLUDE:GITBOOST_BLA_PAYLOAD"));
  assert.ok(args.includes("link-arg=/SECTION:.bla,R"));
});

test("allows only Tauri's documented installer-type patch when checking embedded executables", () => {
  const original = Buffer.from("code-before\0__TAURI_BUNDLE_TYPE_VAR_UNK\0code-after");
  assert.deepEqual(installerExecutableBytes(original, "nsis"),
    Buffer.from("code-before\0__TAURI_BUNDLE_TYPE_VAR_NSS\0code-after"));
  assert.deepEqual(installerExecutableBytes(original, "msi"),
    Buffer.from("code-before\0__TAURI_BUNDLE_TYPE_VAR_MSI\0code-after"));
  assert.ok(original.includes(Buffer.from("__TAURI_BUNDLE_TYPE_VAR_UNK")));
  assert.throws(() => installerExecutableBytes(Buffer.from("missing"), "nsis"), /缺少/);
  assert.throws(() => installerExecutableBytes(original, "other"), /不支持/);
});

test("links a real Windows read-only PE section and executes the resulting program", {
  skip: process.platform !== "win32" || process.arch !== "x64",
}, (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "gitboost-bla-pe-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const payload = Buffer.from("\0[BLA-NATIVE-SHIELD-V1]\0fixture payload\0");
  const data = Array.from(payload).join(",");
  const source = path.join(directory, "probe.rs");
  const binary = path.join(directory, "probe.exe");
  writeFileSync(source, `#[used]\n#[no_mangle]\n#[link_section=".bla"]\nstatic GITBOOST_BLA_PAYLOAD: [u8; ${payload.length}] = [${data}];\nfn main() { println!("BLA fixture OK"); }\n`);
  execFileSync("rustc", ["--edition=2021", "-O", source, "-o", binary,
    "-C", "link-arg=/INCLUDE:GITBOOST_BLA_PAYLOAD", "-C", "link-arg=/SECTION:.bla,R"]);
  assert.match(execFileSync(binary, { encoding: "utf8" }), /BLA fixture OK/);
  const bytes = readFileSync(binary);
  const result = inspectPeBlaSection(bytes, payload);
  assert.equal(result.section.size, payload.length);
  assert.throws(() => inspectPeBlaSection(bytes, Buffer.from("wrong")), /长度不匹配/);
  assert.throws(() => inspectPeBlaSection(Buffer.concat([bytes, Buffer.from("tail")]), payload), /段外存在追加数据/);
  assert.throws(() => inspectPeBlaSection(bytes.subarray(0, 64), payload), /header 无效/);
  const tampered = Buffer.from(bytes);
  tampered[result.section.offset] ^= 1;
  assert.throws(() => inspectPeBlaSection(tampered, payload), /数据不匹配/);
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
