import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { inspectReleaseBinary } from "./verify-release-binary.mjs";

test("rejects unsupported and truncated release binaries", () => {
  assert.throws(() => inspectReleaseBinary(Buffer.alloc(0)), /header 不完整/);
  assert.throws(() => inspectReleaseBinary(Buffer.alloc(64)), /不支持/);
  const bytes = Buffer.alloc(64);
  bytes.write("MZ");
  bytes.writeUInt32LE(1024, 0x3c);
  assert.throws(() => inspectReleaseBinary(bytes), /PE header 越界/);
});

test("checks actual native Rust executables and detects disabled stripping and BLA", {
  skip: !((process.platform === "darwin" && process.arch === "arm64")
    || (process.platform === "win32" && process.arch === "x64")),
}, (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "gitboost-release-binary-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = path.join(directory, "probe.rs");
  const executable = path.join(directory, process.platform === "win32" ? "probe.exe" : "probe");
  writeFileSync(source, 'fn main() { println!("release fixture OK"); }\n');
  const compile = (strip, extra = []) => {
    const link = process.platform === "win32" && strip === "symbols"
      ? ["-C", "link-arg=/DEBUG:NONE"] : [];
    execFileSync("rustc", ["--edition=2021", "-O", "-C", `strip=${strip}`, source, "-o", executable, ...link, ...extra]);
    return readFileSync(executable);
  };
  assert.equal(inspectReleaseBinary(compile("symbols")).stripped, true);
  assert.match(execFileSync(executable, { encoding: "utf8" }), /release fixture OK/);
  // MSVC release builds ordinarily omit debug info already; request it to test rejection.
  assert.throws(() => inspectReleaseBinary(compile("none", ["-g"])), /未剥离|调试|debug directory/);
  const payload = path.join(directory, "payload.bin");
  writeFileSync(payload, "BLA fixture");
  let extra;
  if (process.platform === "darwin") {
    extra = ["-C", `link-arg=-Wl,-sectcreate,__TEXT,__bla,${payload}`];
  } else {
    writeFileSync(source, '#[used]\n#[no_mangle]\n#[link_section=".bla"]\nstatic GITBOOST_BLA_PAYLOAD: [u8; 3] = *b"BLA";\nfn main() {}\n');
    extra = ["-C", "link-arg=/INCLUDE:GITBOOST_BLA_PAYLOAD", "-C", "link-arg=/SECTION:.bla,R"];
  }
  assert.throws(() => inspectReleaseBinary(compile("symbols", extra)), /不应包含 BLA/);
});
