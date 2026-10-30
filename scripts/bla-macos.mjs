#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const blaCommit = "b2d643600774d62e562081129e55b13b0d9e8a80";
const marker = Buffer.from("\0[BLA-NATIVE-SHIELD-V1]\0");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

export async function preparePayload(blaRoot, outputDirectory) {
  assert.equal(process.platform, "darwin", "BLA 实验需要 macOS");
  assert.equal(process.arch, "arm64", "BLA 实验需要 Apple Silicon");
  mkdirSync(outputDirectory, { recursive: true });
  const probe = path.join(outputDirectory, "probe");
  execFileSync("/usr/bin/clang", ["-arch", "arm64", "-x", "c", "-", "-o", probe], {
    input: "int main(void) { return 0; }\n",
  });

  const { protectNativeBinary } = await import(pathToFileURL(
    path.resolve(blaRoot, "dist/shields/formats/native.js"),
  ).href);
  const original = readFileSync(probe);
  const protectedBytes = protectNativeBinary(original, {
    intensity: "bankrupt",
    seed: "gitboost-bla-macos-v1",
  }).buffer;
  assert.ok(protectedBytes.subarray(0, original.length).equals(original),
    "BLA 改变了输入二进制，不能按尾部数据提取");
  const payload = protectedBytes.subarray(original.length);
  assert.ok(payload.subarray(0, marker.length).equals(marker), "缺少 BLA 数据标记");
  writeFileSync(path.join(outputDirectory, "payload.bin"), payload);
  const metadata = {
    blaRepository: "qybaihe/bla",
    blaCommit,
    blaVersion: JSON.parse(readFileSync(path.join(blaRoot, "package.json"), "utf8")).version,
    intensity: "bankrupt",
    payloadBytes: payload.length,
    payloadSha256: sha256(payload),
  };
  writeFileSync(path.join(outputDirectory, "payload.json"), `${JSON.stringify(metadata, null, 2)}\n`);
  return metadata;
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
  const result = inspectBlaSection(readFileSync(binary), payload);
  execFileSync("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", binary]);
  return result;
}

export function verifyExperiment(payloadPath, app, testBuildLog, reportPath) {
  const payload = readFileSync(payloadPath);
  const metadata = JSON.parse(readFileSync(path.join(path.dirname(payloadPath), "payload.json"), "utf8"));
  assert.equal(sha256(payload), metadata.payloadSha256, "BLA payload 校验失败");
  const appBinary = path.join(app, "Contents", "MacOS", "GitBoost");
  const appResult = verifyBlaBinary(appBinary, payload);
  execFileSync("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);

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
    strictSignatureVerification: "passed",
    signing: "ad hoc; not Developer ID signed or notarized",
    notEvaluated: ["REA/Ghidra resistance", "full GUI and proxy integration", "Windows"],
  };
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  return report;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...args] = process.argv.slice(2);
  try {
    if (command === "prepare" && args.length === 2) {
      console.log(JSON.stringify(await preparePayload(...args), null, 2));
    } else if (command === "verify" && args.length === 4) {
      verifyExperiment(...args);
    } else {
      throw new Error("用法: bla-macos.mjs prepare <BLA目录> <输出目录> | verify <payload.bin> <app> <测试构建JSONL> <report.json>");
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
