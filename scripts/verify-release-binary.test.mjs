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

test("accepts PE linker metadata but rejects CodeView symbols and invalid directory bounds", () => {
  const bytes = Buffer.alloc(1024);
  bytes.write("MZ");
  bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE(0x8664, 68);
  bytes.writeUInt16LE(1, 70);
  bytes.writeUInt16LE(240, 84);
  bytes.writeUInt16LE(0x20b, 88);
  bytes.writeUInt32LE(16, 88 + 108);
  bytes.writeUInt32LE(0x1000, 88 + 160);
  bytes.writeUInt32LE(28, 88 + 164);
  bytes.write(".rdata", 328);
  bytes.writeUInt32LE(0x1000, 328 + 12);
  bytes.writeUInt32LE(512, 328 + 16);
  bytes.writeUInt32LE(512, 328 + 20);
  for (const type of [12, 13, 16, 20]) {
    bytes.writeUInt32LE(type, 512 + 12);
    assert.deepEqual(inspectReleaseBinary(bytes).debugTypes, [type]);
  }
  bytes.writeUInt32LE(2, 512 + 12);
  assert.throws(() => inspectReleaseBinary(bytes), /CodeView PDB/);
  bytes.writeUInt32LE(0x5000, 88 + 160);
  assert.throws(() => inspectReleaseBinary(bytes), /越界/);
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
  // Request debug info to test rejection on both native toolchains.
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
