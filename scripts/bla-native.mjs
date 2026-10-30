#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, openSync, closeSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { addHelperToDmg } from "./tauri-with-macos-helper.mjs";

const blaCommit = "b2d643600774d62e562081129e55b13b0d9e8a80";
const marker = Buffer.from("\0[BLA-NATIVE-SHIELD-V1]\0");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

export async function preparePayload(blaRoot, outputDirectory) {
  assert.ok((process.platform === "darwin" && process.arch === "arm64")
    || (process.platform === "win32" && process.arch === "x64"), "仅支持 macOS ARM64 和 Windows x64");
  mkdirSync(outputDirectory, { recursive: true });
  const probe = path.join(outputDirectory, process.platform === "win32" ? "probe.exe" : "probe");
  const probeSource = path.join(outputDirectory, "probe.rs");
  writeFileSync(probeSource, "fn main() {}\n");
  execFileSync("rustc", ["--edition=2021", "-O", probeSource, "-o", probe]);

  const { protectNativeBinary } = await import(pathToFileURL(
    path.resolve(blaRoot, "dist/shields/formats/native.js"),
  ).href);
  const original = readFileSync(probe);
  const protectedBytes = protectNativeBinary(original, {
    intensity: "bankrupt",
    seed: "gitboost-bla-native-v1",
  }).buffer;
  assert.ok(protectedBytes.subarray(0, original.length).equals(original),
    "BLA 改变了输入二进制，不能按尾部数据提取");
  const payload = protectedBytes.subarray(original.length);
  assert.ok(payload.subarray(0, marker.length).equals(marker), "缺少 BLA 数据标记");
  writeFileSync(path.join(outputDirectory, "payload.bin"), payload);
  if (process.platform === "win32") {
    const source = path.join(outputDirectory, "payload.rs");
    writeFileSync(source, `#![no_std]\n#[used]\n#[no_mangle]\n#[link_section = ".bla"]\npub static GITBOOST_BLA_PAYLOAD: [u8; ${payload.length}] = *include_bytes!(${JSON.stringify(path.resolve(outputDirectory, "payload.bin"))});\n`);
    execFileSync("rustc", ["--edition=2021", "--crate-type=lib", "--emit=obj", source,
      "-o", path.join(outputDirectory, "payload.obj")]);
  }
  const metadata = {
    blaRepository: "qybaihe/bla",
    blaCommit,
    blaVersion: JSON.parse(readFileSync(path.join(blaRoot, "package.json"), "utf8")).version,
    intensity: "bankrupt",
    platform: process.platform,
    payloadBytes: payload.length,
    payloadSha256: sha256(payload),
  };
  writeFileSync(path.join(outputDirectory, "payload.json"), `${JSON.stringify(metadata, null, 2)}\n`);
  return metadata;
}

export function linkerArguments(platform, directory) {
  if (platform === "darwin") {
    return ["-C", `link-arg=-Wl,-sectcreate,__TEXT,__bla,${path.join(directory, "payload.bin")}`];
  }
  assert.equal(platform, "win32", "不支持的平台");
  return ["-C", `link-arg=${path.join(directory, "payload.obj")}`,
    "-C", "link-arg=/INCLUDE:GITBOOST_BLA_PAYLOAD", "-C", "link-arg=/SECTION:.bla,R"];
}

export function inspectPeBlaSection(bytes, payload) {
  assert.ok(bytes.length >= 64 && bytes.toString("ascii", 0, 2) === "MZ", "不是完整的 PE 文件");
  const pe = bytes.readUInt32LE(0x3c);
  assert.ok(pe + 24 <= bytes.length && bytes.toString("ascii", pe, pe + 4) === "PE\0\0",
    "PE header 无效");
  assert.equal(bytes.readUInt16LE(pe + 4), 0x8664, "不是 Windows x64 二进制");
  const count = bytes.readUInt16LE(pe + 6);
  const optional = pe + 24;
  const optionalSize = bytes.readUInt16LE(pe + 20);
  const sections = optional + optionalSize;
  assert.ok(optionalSize >= 112 && sections + count * 40 <= bytes.length, "PE section headers 越界");
  assert.equal(bytes.readUInt16LE(optional), 0x20b, "不是 PE32+ 文件");
  const imageSize = bytes.readUInt32LE(optional + 56);
  const headersSize = bytes.readUInt32LE(optional + 60);
  let end = headersSize;
  let section;
  for (let index = 0; index < count; index++) {
    const start = sections + index * 40;
    const name = bytes.toString("ascii", start, start + 8).replace(/\0.*$/, "");
    const virtualSize = bytes.readUInt32LE(start + 8);
    const address = bytes.readUInt32LE(start + 12);
    const rawSize = bytes.readUInt32LE(start + 16);
    const offset = bytes.readUInt32LE(start + 20);
    const flags = bytes.readUInt32LE(start + 36);
    assert.ok(offset + rawSize <= bytes.length && address + virtualSize <= imageSize,
      "PE section 越界");
    end = Math.max(end, offset + rawSize);
    if (name !== ".bla") continue;
    assert.ok(!section, "存在重复 BLA section");
    assert.ok(offset >= headersSize && rawSize >= virtualSize, "BLA section 布局无效");
    assert.ok((flags & 0x40000000) !== 0 && (flags & 0xa0000000) === 0,
      "BLA section 必须只读且不可执行");
    assert.equal(virtualSize, payload.length, "BLA section 长度不匹配");
    assert.ok(bytes.subarray(offset, offset + virtualSize).equals(payload), "BLA section 数据不匹配");
    assert.ok(bytes.subarray(offset + virtualSize, offset + rawSize).every((byte) => byte === 0),
      "BLA section 填充无效");
    section = { name, offset, size: virtualSize, rawSize, address, flags };
  }
  assert.ok(section, "缺少 .bla section");
  assert.equal(end, bytes.length, "PE 段外存在追加数据");
  return { bytes: bytes.length, sha256: sha256(bytes), section };
}

export function inspectBlaSection(bytes, payload) {
  assert.ok(bytes.length >= 32 && bytes.readUInt32LE(0) === 0xfeedfacf,
    "不是完整的 64 位 little-endian Mach-O");
  assert.equal(bytes.readUInt32LE(4), 0x0100000c, "不是 ARM64 二进制");
  const commandEnd = 32 + bytes.readUInt32LE(20);
  assert.ok(commandEnd <= bytes.length, "Mach-O load commands 越界");
  let position = 32;
  let section;
  let signature;
  let segmentEnd = 0;
  for (let index = 0; index < bytes.readUInt32LE(16); index++) {
    assert.ok(position + 8 <= commandEnd, "Mach-O load command 不完整");
    const command = bytes.readUInt32LE(position);
    const size = bytes.readUInt32LE(position + 4);
    assert.ok(size >= 8 && position + size <= commandEnd, "Mach-O load command 大小无效");
    if (command === 0x19) {
      assert.ok(size >= 72, "Mach-O segment 不完整");
      const offset = Number(bytes.readBigUInt64LE(position + 40));
      const length = Number(bytes.readBigUInt64LE(position + 48));
      assert.ok(Number.isSafeInteger(offset + length) && offset + length <= bytes.length,
        "Mach-O segment 越界");
      segmentEnd = Math.max(segmentEnd, offset + length);
      const count = bytes.readUInt32LE(position + 64);
      assert.ok(72 + count * 80 <= size, "Mach-O section headers 不完整");
      for (let index = 0; index < count; index++) {
        const start = position + 72 + index * 80;
        const name = bytes.toString("ascii", start, start + 16).replace(/\0.*$/, "");
        const segment = bytes.toString("ascii", start + 16, start + 32).replace(/\0.*$/, "");
        if (name !== "__bla" || segment !== "__TEXT") continue;
        assert.ok(!section, "存在重复 BLA section");
        const sectionSize = Number(bytes.readBigUInt64LE(start + 40));
        const sectionOffset = bytes.readUInt32LE(start + 48);
        assert.ok(Number.isSafeInteger(sectionSize) && sectionOffset >= offset
          && sectionOffset + sectionSize <= offset + length, "BLA section 越界");
        section = { segment, name, offset: sectionOffset, size: sectionSize };
      }
    } else if (command === 0x1d) {
      assert.ok(size >= 16, "Mach-O 签名命令不完整");
      signature = {
        offset: bytes.readUInt32LE(position + 8),
        size: bytes.readUInt32LE(position + 12),
      };
    }
    position += size;
  }
  assert.equal(position, commandEnd, "Mach-O load commands 长度不匹配");
  assert.ok(section, "缺少 __TEXT,__bla section");
  assert.ok(bytes.subarray(section.offset, section.offset + section.size).equals(payload),
    "BLA section 数据不匹配");
  assert.equal(segmentEnd, bytes.length, "Mach-O 段外存在追加数据");
  assert.ok(signature?.size > 0 && signature.offset + signature.size === bytes.length,
    "Mach-O 签名区域无效");
  assert.ok(section.offset + section.size <= signature.offset, "BLA 数据位于签名之后");
  return { bytes: bytes.length, sha256: sha256(bytes), section, signature };
}

export function verifyBlaBinary(binary, payload) {
  const bytes = readFileSync(binary);
  if (bytes.toString("ascii", 0, 2) === "MZ") return inspectPeBlaSection(bytes, payload);
  const result = inspectBlaSection(bytes, payload);
  execFileSync("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", binary]);
  return result;
}

export function verifyExperiment(payloadPath, app, testBuildLog, reportPath) {
  const payload = readFileSync(payloadPath);
  const metadata = JSON.parse(readFileSync(path.join(path.dirname(payloadPath), "payload.json"), "utf8"));
  assert.equal(sha256(payload), metadata.payloadSha256, "BLA payload 校验失败");
  const windows = process.platform === "win32";
  const appBinary = windows ? app : path.join(app, "Contents", "MacOS", "GitBoost");
  const appResult = verifyBlaBinary(appBinary, payload);
  if (!windows) execFileSync("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);

  const artifacts = readFileSync(testBuildLog, "utf8").trim().split("\n").map(JSON.parse);
  const testBinaries = artifacts.filter((item) => item.reason === "compiler-artifact"
    && item.profile.test && item.target.name === "gitboost_lib" && item.executable);
  assert.equal(testBinaries.length, 1, "预期一个 GitBoost 测试二进制");
  const testBinary = testBinaries[0].executable;
  const testResult = verifyBlaBinary(testBinary, payload);
  const runtimeChecks = [
    ["importer::tests::", "--test-threads=1"],
    ["--exact", "core::tests::system_catalog_is_encrypted_and_authenticated", "--test-threads=1"],
  ].map((args) => {
    const output = execFileSync(testBinary, args, { encoding: "utf8", timeout: 30_000 });
    process.stdout.write(output);
    const match = output.match(/test result: ok\. (\d+) passed; 0 failed;/);
    assert.ok(match && Number(match[1]) > 0, "运行检查未执行任何测试");
    return { args, passed: Number(match[1]) };
  });
  const report = {
    ...metadata,
    app: appResult,
    testBinary: testResult,
    runtimeChecks,
    strictSignatureVerification: windows ? "not applicable; unsigned Windows build" : "passed",
    signing: windows ? "unsigned; Authenticode signing not configured" : "ad hoc; not Developer ID signed or notarized",
    notEvaluated: ["REA/Ghidra resistance", "full GUI and proxy integration"],
  };
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  return report;
}

export async function buildWithBla(args) {
  const windows = process.platform === "win32";
  const target = windows ? "x86_64-pc-windows-msvc" : "aarch64-apple-darwin";
  assert.deepEqual(args, ["--target", target], "BLA 构建需要当前平台对应的 --target");
  const projectRoot = fileURLToPath(new URL("../", import.meta.url));
  const directory = path.resolve(process.env.BLA_WORK_DIR ?? path.join(projectRoot, ".bla-experiment"));
  await preparePayload(path.join(projectRoot, ".bla-tool"), directory);
  const manifest = path.join(projectRoot, "src-tauri", "Cargo.toml");
  const common = ["--locked", "--target", target, "--features", "tauri/custom-protocol", "--manifest-path", manifest];
  const link = linkerArguments(process.platform, directory);
  execFileSync("cargo", ["rustc", "--release", "--bin", "GitBoost", ...common, "--", ...link],
    { cwd: projectRoot, stdio: "inherit" });
  const logPath = path.join(directory, "test-build.jsonl");
  const log = openSync(logPath, "w");
  try {
    execFileSync("cargo", ["rustc", "--profile", "bench", "--lib", ...common, "--message-format=json", "--", ...link],
      { cwd: projectRoot, stdio: ["ignore", log, "inherit"] });
  } finally {
    closeSync(log);
  }
  const cli = path.join(projectRoot, "node_modules", "@tauri-apps", "cli", "tauri.js");
  execFileSync(process.execPath, [cli, "bundle", "--target", target, "--features", "tauri/custom-protocol",
    "--bundles", ...(windows ? ["nsis", "msi"] : ["app", "dmg"]), "--ci"],
  { cwd: projectRoot, stdio: "inherit" });
  const release = path.join(projectRoot, "src-tauri", "target", target, "release");
  if (!windows) await addHelperToDmg({ projectRoot, buildArgs: ["--target", target] });
  const app = windows ? path.join(release, "GitBoost.exe") : path.join(release, "bundle", "macos", "GitBoost.app");
  const reportPath = path.join(directory, "report.json");
  const report = verifyExperiment(path.join(directory, "payload.bin"), app, logPath, reportPath);
  if (windows) {
    copyFileSync(app, path.join(directory, "GitBoost-bla-x64.exe"));
    report.installers = [];
    for (const [folder, extension] of [["nsis", ".exe"], ["msi", ".msi"]]) {
      const installerDirectory = path.join(release, "bundle", folder);
      const files = readdirSync(installerDirectory).filter((name) => name.endsWith(extension));
      assert.equal(files.length, 1, `预期一个 ${folder} 安装包`);
      const installer = path.join(installerDirectory, files[0]);
      const extracted = path.join(directory, `extracted-${folder}`);
      execFileSync("7z", ["x", "-y", installer, `-o${extracted}`], { stdio: ["ignore", "pipe", "inherit"] });
      for (const entry of readdirSync(extracted, { recursive: true, withFileTypes: true })) {
        if (entry.isFile() && entry.name.toLowerCase().endsWith(".cab")) {
          execFileSync("7z", ["x", "-y", path.join(entry.parentPath, entry.name), `-o${extracted}`],
            { stdio: ["ignore", "pipe", "inherit"] });
        }
      }
      const embedded = readdirSync(extracted, { recursive: true, withFileTypes: true }).some((entry) => {
        if (!entry.isFile()) return false;
        const file = path.join(entry.parentPath, entry.name);
        return statSync(file).size === report.app.bytes && sha256(readFileSync(file)) === report.app.sha256;
      });
      assert.ok(embedded, `${folder} 安装包未包含已验证的 BLA 二进制`);
      copyFileSync(installer, path.join(directory, files[0]));
      report.installers.push({ name: files[0], sha256: sha256(readFileSync(installer)), embeddedExecutableVerified: true });
    }
  } else {
    execFileSync("ditto", ["-c", "-k", "--sequesterRsrc", "--keepParent", app,
      path.join(directory, "GitBoost-bla-arm64.app.zip")]);
    const dmgDirectory = path.join(release, "bundle", "dmg");
    const files = readdirSync(dmgDirectory).filter((name) => name.endsWith(".dmg"));
    assert.equal(files.length, 1, "预期一个 DMG 安装包");
    const dmg = path.join(dmgDirectory, files[0]);
    execFileSync("hdiutil", ["verify", dmg], { stdio: "inherit" });
    copyFileSync(dmg, path.join(directory, files[0]));
  }
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...args] = process.argv.slice(2);
  try {
    if (command === "prepare" && args.length === 2) {
      console.log(JSON.stringify(await preparePayload(...args), null, 2));
    } else if (command === "verify" && args.length === 4) {
      verifyExperiment(...args);
    } else if (command === "build") {
      await buildWithBla(args);
    } else {
      throw new Error("用法: bla-native.mjs build --target <target> | prepare <BLA目录> <输出目录> | verify <payload.bin> <app或exe> <测试构建JSONL> <report.json>");
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
