#!/usr/bin/env node

// Isolated macOS experiment. The normal Cargo profile and release workflow do not load this plugin.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { inspectReleaseBinary } from "./verify-release-binary.mjs";

const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const commit = "ad26aa21b90cc5c5118d32114b674d1455f54168";
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const capture = (command, args, options = {}) => execFileSync(command, args,
  { cwd: project, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, ...options }).trim();

function logged(command, args, logPath, env) {
  const log = openSync(logPath, "w");
  try {
    execFileSync(command, args, { cwd: project, env, stdio: ["ignore", log, log] });
  } finally { closeSync(log); }
}

function sections(bytes) {
  const result = [];
  let command = 32;
  for (let index = 0; index < bytes.readUInt32LE(16); index++) {
    if (bytes.readUInt32LE(command) === 0x19) {
      for (let i = 0; i < bytes.readUInt32LE(command + 64); i++) {
        const start = command + 72 + i * 80;
        const name = bytes.toString("ascii", start, start + 16).replace(/\0.*$/, "");
        const segment = bytes.toString("ascii", start + 16, start + 32).replace(/\0.*$/, "");
        const address = bytes.readBigUInt64LE(start + 32).toString(16);
        const size = Number(bytes.readBigUInt64LE(start + 40));
        const offset = bytes.readUInt32LE(start + 48);
        const type = bytes.readUInt32LE(start + 64) & 0xff;
        const zeroFill = [1, 0xc, 0x12].includes(type);
        assert.ok(zeroFill || offset + size <= bytes.length, "Mach-O section 越界");
        result.push({ segment, name, address, size,
          sha256: zeroFill ? null : sha256(bytes.subarray(offset, offset + size)) });
      }
    }
    command += bytes.readUInt32LE(command + 4);
  }
  return result;
}

function compilerWrapper(directory, compiler, plugin) {
  mkdirSync(directory, { recursive: true });
  const source = path.join(directory, "rustc-wrapper.rs");
  const wrapper = path.join(directory, "rustc-wrapper");
  writeFileSync(source, `use std::{env,process::{Command,exit}};
fn main() {
 let mut args:Vec<_>=env::args_os().skip(1).collect();
 let compiler=args.remove(0);
 let selected=args.windows(2).any(|pair|pair[0]=="--crate-name" && pair[1]=="gitboost_lib");
 let mut command=Command::new(compiler);
 if selected {
  args.extend(["-C".into(),"codegen-units=1".into()]);
  let plugin=${JSON.stringify(plugin)};
  if !plugin.is_empty() {
   args.extend(["-Z".into(),format!("llvm-plugins={plugin}").into()]);
   command.env("RUSTC_BOOTSTRAP","1");
  }
 }
 match command.args(args).status() {
  Ok(status)=>exit(status.code().unwrap_or(1)),
  Err(error)=>{eprintln!("rustc wrapper: {error}");exit(1)}
 }
}
`);
  capture(compiler, ["--edition=2021", "-O", source, "-o", wrapper]);
  return wrapper;
}

async function experiment(directory) {
  assert.ok(process.platform === "darwin" && process.arch === "arm64",
    "本实验仅验证 macOS ARM64；未验证 Windows，不应接入正式发布");
  mkdirSync(directory, { recursive: true });
  const compiler = process.env.RUSTC || capture("/usr/bin/which", ["rustc"]);
  const cargo = process.env.CARGO || "cargo";
  const llvmConfig = process.env.LLVM_CONFIG || "/opt/homebrew/opt/llvm/bin/llvm-config";
  const config = (...args) => capture(llvmConfig, args);
  const llvm = config("--version");
  const rust = capture(compiler, ["-Vv"]);
  assert.equal(rust.match(/^LLVM version: (.+)$/m)?.[1], llvm, "Rust / LLVM 版本必须完全一致");
  assert.ok(llvm.startsWith("22."), "目前仅验证 LLVM 22");
  const libDirectory = config("--libdir");
  const linked = capture("otool", ["-L", compiler]);
  const linkedLlvm = linked.split("\n").map((line) => line.trim().split(" ")[0])
    .filter((file) => file.includes("/libLLVM") && existsSync(file));
  assert.ok(linkedLlvm.some((file) => realpathSync(file) === realpathSync(path.join(libDirectory, "libLLVM.dylib"))),
    "需要与 LLVM 工具共享同一动态库的 Rust，例如本实验使用的 Homebrew Rust");

  const archive = path.join(directory, "eshard.tar.gz");
  if (!existsSync(archive)) capture("curl", ["-sSfL", "--retry", "2", "-o", archive,
    `https://codeload.github.com/eshard/obfuscator-llvm/tar.gz/${commit}`]);
  const patch = path.join(project, "scripts/ollvm-eshard.patch");
  const source = path.join(project, "scripts/ollvm-pass.cpp");
  const buildId = sha256(Buffer.concat([readFileSync(patch), readFileSync(source), Buffer.from(llvm + commit)])).slice(0, 12);
  const pluginDirectory = path.join(directory, `plugin-${buildId}`);
  const upstream = path.join(pluginDirectory, "source");
  const plugin = path.join(pluginDirectory, "libGitBoostObfuscator.dylib");
  if (!existsSync(plugin)) {
    mkdirSync(upstream, { recursive: true });
    capture("tar", ["-xzf", archive, "--strip-components=1", "-C", upstream]);
    capture("patch", ["-p1", "-i", patch], { cwd: upstream });
    const flags = config("--cxxflags", "--ldflags", "--libs", "--system-libs").split(/\s+/);
    const sources = [source, "utils/Utils.cpp", "utils/CryptoUtils.cpp",
      "bogus/BogusControlFlow.cpp", "substitution/Substitution.cpp"].map((file, index) => index ? path.join(upstream, file) : file);
    logged(path.join(config("--bindir"), "clang++"),
      ["-shared", "-fPIC", "-O2", `-I${upstream}`, ...sources, ...flags, "-o", plugin],
      path.join(pluginDirectory, "build.log"), process.env);
  }
  const pluginHash = sha256(readFileSync(plugin));
  const baseEnv = { ...process.env, RUSTC: compiler, TAURI_ENV_PLATFORM: "macos",
    CARGO_TARGET_DIR: path.resolve(process.env.CARGO_TARGET_DIR || path.join(directory, "target")) };
  logged("npm", ["run", "build"], path.join(directory, "frontend-build.log"), baseEnv);
  const common = ["--locked", "--manifest-path", path.join(project, "src-tauri/Cargo.toml"),
    "--target", "aarch64-apple-darwin", "--features", "tauri/custom-protocol"];
  const results = [];
  for (const variant of ["baseline", "obfuscated"]) {
    console.log(`Building ${variant}…`);
    const output = path.join(directory, variant);
    mkdirSync(output, { recursive: true });
    // A distinct wrapper path makes Cargo rebuild workspace crates when the plugin changes.
    const wrapper = compilerWrapper(path.join(output, variant === "baseline" ? "compiler" : pluginHash.slice(0, 12)),
      compiler, variant === "baseline" ? "" : plugin);
    const env = { ...baseEnv, RUSTC_WORKSPACE_WRAPPER: wrapper };
    const buildLog = path.join(output, "build.log");
    logged(cargo, ["build", "--release", "--bin", "GitBoost", ...common], buildLog, env);
    const transforms = readFileSync(buildLog, "utf8").split("\n").filter((line) => line.startsWith("GITBOOST_OLLVM transformed "));
    assert.equal(transforms.length, variant === "baseline" ? 0 : 1, "预期只对一个业务函数执行一次变换");
    const executable = path.join(output, "GitBoost");
    copyFileSync(path.join(baseEnv.CARGO_TARGET_DIR, "aarch64-apple-darwin/release/GitBoost"), executable);
    const bytes = readFileSync(executable);
    const native = inspectReleaseBinary(bytes);
    capture("codesign", ["--verify", "--strict", executable]);
    const stderr = openSync(path.join(output, "test-build.log"), "w");
    let testBuild;
    try {
      testBuild = capture(cargo, ["rustc", "--profile", "bench", "--lib", "--message-format=json", ...common],
        { env, stdio: ["ignore", "pipe", stderr] });
    } finally { closeSync(stderr); }
    writeFileSync(path.join(output, "test-build.jsonl"), testBuild + "\n");
    const test = testBuild.split("\n").map(JSON.parse).find((item) => item.profile?.test
      && item.target?.name === "gitboost_lib" && item.executable)?.executable;
    assert.ok(test, "未找到实际 Rust test executable");
    const testLog = capture(test, ["--test-threads=1"]);
    writeFileSync(path.join(output, "tests.log"), testLog + "\n");
    assert.match(testLog, /repository_parser_preserves_boundary_behavior_through_function_pointer \.\.\. ok/);
    results.push({ variant, ...native, sha256: sha256(bytes), transforms,
      tests: [...testLog.matchAll(/test result: ok\..*$/gm)].at(-1)?.[0], signature: "strict ad hoc verification passed" });
  }
  const env = { ...baseEnv, RUSTC_WORKSPACE_WRAPPER: compilerWrapper(
    path.join(directory, "obfuscated", pluginHash.slice(0, 12)), compiler, plugin) };
  logged(process.execPath, [path.join(project, "node_modules/@tauri-apps/cli/tauri.js"), "bundle",
    "--target", "aarch64-apple-darwin", "--features", "tauri/custom-protocol", "--bundles", "app", "--ci"],
    path.join(directory, "bundle.log"), env);
  const app = path.join(baseEnv.CARGO_TARGET_DIR, "aarch64-apple-darwin/release/bundle/macos/GitBoost.app");
  capture("codesign", ["--verify", "--deep", "--strict", app]);
  const appBytes = readFileSync(path.join(app, "Contents/MacOS/GitBoost"));
  const appNative = inspectReleaseBinary(appBytes);
  // Bundle signing can replace __LINKEDIT's code signature. Every loaded section must stay identical.
  assert.deepEqual(sections(appBytes), sections(readFileSync(path.join(directory, "obfuscated/GitBoost"))),
    "app 内代码或数据 section 与已验证的保护版本不一致");
  capture("ditto", [app, path.join(directory, "GitBoost-experimental.app")]);
  const report = { upstream: "eshard/obfuscator-llvm", upstreamCommit: commit,
    archiveSha256: sha256(readFileSync(archive)), llvm, rust, pluginSha256: pluginHash,
    target: "aarch64-apple-darwin", sourceRevision: capture("git", ["rev-parse", "HEAD"]),
    importerSha256: sha256(readFileSync(path.join(project, "src-tauri/src/importer.rs"))),
    pluginSourceSha256: sha256(readFileSync(source)), patchSha256: sha256(readFileSync(patch)),
    targetFunction: "gitboost_lib::importer::normalize_repository_url", codegenUnits: 1,
    panicStrategy: "unchanged (unwind)", appStrictSignatureVerified: true,
    appNative, appSha256: sha256(appBytes), appSectionsMatchProtectedBinary: true, variants: results,
    notEvaluated: ["Windows protection", "reverse-engineering resistance (run the separate Ghidra audit)", "full GUI/proxy integration"] };
  writeFileSync(path.join(directory, "build-report.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(`Saved ${path.join(directory, "build-report.json")}`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const directory = process.argv[2];
  if (!directory) { console.error("用法：node scripts/ollvm-native.mjs <临时输出目录>"); process.exitCode = 1; }
  else experiment(path.resolve(directory)).catch((error) => { console.error(error.message); process.exitCode = 1; });
}
