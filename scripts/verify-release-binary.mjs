#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Check the shipped executable, so a profile override cannot silently undo stripping.
export function inspectReleaseBinary(bytes) {
  assert.ok(bytes.length >= 64, "二进制 header 不完整");
  if (bytes.readUInt32LE(0) === 0xfeedfacf) {
    assert.equal(bytes.readUInt32LE(4), 0x100000c, "预期 macOS ARM64");
    const end = 32 + bytes.readUInt32LE(20);
    assert.ok(end <= bytes.length, "Mach-O load commands 越界");
    let position = 32;
    let symbols;
    for (let index = 0; index < bytes.readUInt32LE(16); index++) {
      assert.ok(position + 8 <= end, "Mach-O load command 不完整");
      const command = bytes.readUInt32LE(position);
      const size = bytes.readUInt32LE(position + 4);
      assert.ok(size >= 8 && position + size <= end, "Mach-O load command 越界");
      if (command === 0x19) {
        assert.ok(size >= 72, "Mach-O segment 不完整");
        const segment = bytes.toString("ascii", position + 8, position + 24).replace(/\0.*$/, "");
        assert.notEqual(segment, "__DWARF", "发布二进制保留了调试信息");
        const count = bytes.readUInt32LE(position + 64);
        assert.ok(72 + count * 80 <= size, "Mach-O section headers 不完整");
        for (let i = 0; i < count; i++) {
          const start = position + 72 + i * 80;
          const name = bytes.toString("ascii", start, start + 16).replace(/\0.*$/, "");
          assert.notEqual(name, "__bla", "正式构建不应包含 BLA 数据");
        }
      } else if (command === 0x2) {
        assert.ok(size >= 24, "Mach-O symbol table command 不完整");
        symbols = {
          offset: bytes.readUInt32LE(position + 8),
          count: bytes.readUInt32LE(position + 12),
          strings: bytes.readUInt32LE(position + 16),
          stringBytes: bytes.readUInt32LE(position + 20),
        };
      }
      position += size;
    }
    assert.equal(position, end, "Mach-O load commands 长度不匹配");
    assert.ok(symbols, "缺少 Mach-O symbol table command");
    assert.ok(symbols.offset + symbols.count * 16 <= bytes.length
      && symbols.strings + symbols.stringBytes <= bytes.length, "Mach-O symbol table 越界");
    const defined = [];
    for (let i = 0; i < symbols.count; i++) {
      const start = symbols.offset + i * 16;
      const type = bytes[start + 4];
      assert.equal(type & 0xe0, 0, "发布二进制保留了调试符号");
      if ((type & 0x0e) === 0) continue; // Dynamic imports are required at runtime.
      const offset = bytes.readUInt32LE(start);
      assert.ok(offset < symbols.stringBytes, "Mach-O symbol name 越界");
      const nameEnd = bytes.indexOf(0, symbols.strings + offset);
      assert.ok(nameEnd >= symbols.strings + offset
        && nameEnd < symbols.strings + symbols.stringBytes, "Mach-O symbol name 不完整");
      defined.push(bytes.toString("utf8", symbols.strings + offset, nameEnd));
    }
    assert.ok(defined.every((name) => name === "__mh_execute_header"),
      `发布二进制未剥离符号：保留 ${defined.length} 个已定义符号`);
    return { format: "Mach-O ARM64", bytes: bytes.length, definedSymbols: defined.length, stripped: true, bla: false };
  }

  assert.equal(bytes.toString("ascii", 0, 2), "MZ", "不支持的二进制格式");
  const pe = bytes.readUInt32LE(0x3c);
  assert.ok(pe >= 64 && pe + 24 <= bytes.length, "PE header 越界");
  assert.equal(bytes.readUInt32LE(pe), 0x4550, "PE signature 无效");
  assert.equal(bytes.readUInt16LE(pe + 4), 0x8664, "预期 Windows x64");
  assert.equal(bytes.readUInt32LE(pe + 12), 0, "发布二进制未剥离 COFF symbol table");
  assert.equal(bytes.readUInt32LE(pe + 16), 0, "发布二进制未剥离 COFF symbols");
  const optionalSize = bytes.readUInt16LE(pe + 20);
  const optional = pe + 24;
  assert.ok(optionalSize >= 168 && optional + optionalSize <= bytes.length, "PE optional header 不完整");
  assert.equal(bytes.readUInt16LE(optional), 0x20b, "预期 PE32+");
  assert.ok(bytes.readUInt32LE(optional + 108) > 6, "PE data directories 不完整");
  const debugRva = bytes.readUInt32LE(optional + 112 + 6 * 8);
  const debugSize = bytes.readUInt32LE(optional + 112 + 6 * 8 + 4);
  const sections = bytes.readUInt16LE(pe + 6);
  const table = optional + optionalSize;
  assert.ok(table + sections * 40 <= bytes.length, "PE section table 越界");
  let debugOffset;
  for (let i = 0; i < sections; i++) {
    const start = table + i * 40;
    const name = bytes.toString("ascii", start, start + 8).replace(/\0.*$/, "");
    assert.notEqual(name, ".bla", "正式构建不应包含 BLA 数据");
    assert.ok(!name.startsWith(".debug"), "发布二进制保留了调试 section");
    const address = bytes.readUInt32LE(start + 12);
    const rawSize = bytes.readUInt32LE(start + 16);
    if (debugRva >= address && debugRva - address + debugSize <= rawSize) {
      debugOffset = bytes.readUInt32LE(start + 20) + debugRva - address;
    }
  }
  const debugTypes = [];
  if (debugSize) {
    assert.ok(debugRva && debugSize % 28 === 0 && debugOffset !== undefined
      && debugOffset + debugSize <= bytes.length, "PE debug directory 越界或不完整");
    for (let offset = debugOffset; offset < debugOffset + debugSize; offset += 28) {
      const type = bytes.readUInt32LE(offset + 12);
      // VC feature counts, POGO section groups, reproducible-build hashes and
      // security flags are linker metadata. COFF/CodeView symbol records are not allowed.
      assert.ok([12, 13, 16, 20].includes(type),
        `发布二进制保留了调试符号 / CodeView PDB 引用（type=${type}）`);
      const size = bytes.readUInt32LE(offset + 16);
      const pointer = bytes.readUInt32LE(offset + 24);
      assert.ok(pointer + size <= bytes.length, "PE debug data 越界");
      debugTypes.push(type);
    }
  } else {
    assert.equal(debugRva, 0, "PE debug directory 长度无效");
  }
  return { format: "PE x64", bytes: bytes.length, coffSymbols: 0, pdbReference: false, debugTypes, stripped: true, bla: false };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const binary = process.argv[2];
    assert.ok(binary, "用法：node scripts/verify-release-binary.mjs <executable>");
    const report = inspectReleaseBinary(readFileSync(binary));
    if (report.format === "Mach-O ARM64") {
      execFileSync("codesign", ["--verify", "--strict", binary], { stdio: "inherit" });
    }
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
