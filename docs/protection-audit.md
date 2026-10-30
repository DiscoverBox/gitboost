# 二进制保护对照实测（2026-10-30）

本次 macOS 定点逆向任务没有测到 BLA 在去符号之上的额外效果。因此正式构建保留符号剥离，移除 BLA 注入；原实验脚本保留供复现。不能据此声称 BLA 对所有分析工具都无效，也不能把去符号称为代码混淆或虚拟化。

机器可读证据见 [protection-audit.json](protection-audit.json)，定位与反编译脚本见 [AuditRepository.java](../scripts/AuditRepository.java)。

## 样本与方法

三个样本来自提交 `636823c88fe082b150b011bb8fe68372f9e27b92` 的同一业务源码、同一前端构建，使用本地 Rust 1.97.1 / LLVM 22.1.8，目标 `aarch64-apple-darwin`，均启用 `tauri/custom-protocol`。仅改变 release 的 strip 设置及 BLA 链接参数。后续 `build.rs` 的 `/DEBUG:NONE` 仅作用于 MSVC，不影响这批 macOS 样本。

工具为官方 Ghidra 11.4.3，下载 ZIP 的 SHA-256 为 `b22a8baebb14339f19903c54363ba7e01781930cc43ab430539aabce0d58547a`，使用 Microsoft OpenJDK 21.0.10 ARM64。三个样本均使用默认分析器、2 GiB Java heap、`-max-cpu 4`、`-analysisTimeoutPerFile 600`；单函数反编译上限为 60 秒。

BLA 固定提交 `b2d643600774d62e562081129e55b13b0d9e8a80`，强度 `bankrupt`，seed 为 `gitboost-bla-native-v1`。使用前次发布采用的合法 Mach-O `__TEXT,__bla` section 注入方式，不使用会破坏签名的文件尾追加方式。

任务是定位并查看 GitHub 仓库 URL 规范化/校验逻辑。脚本在二进制内搜索 UTF-8 错误提示 `仓库地址必须是 github.com/owner/repository`，沿引用找到所属函数，再调用 Ghidra 反编译器。定位过程不读取业务函数名，也没有专门跳过 BLA section。

这不是盲测：已知项目源码用于核对结果，也事先知道目标错误提示。它测量同一定位路径是否仍然可用，不测独立分析员发现目标所需的时间。

## 结果

| 项目 | 原版 | 去符号 | 去符号＋BLA |
| --- | ---: | ---: | ---: |
| 二进制字节数 | 19,396,280 | 12,604,832 | 13,777,184 |
| 已定义符号 | 42,590 | 1 | 1 |
| 名称含 gitboost 的符号 | 14,398 | 0 | 0 |
| BLA payload 字节数 | 0 | 0 | 1,166,242 |
| 错误提示定位到的函数数 | 1 | 1 | 1 |
| 目标函数反编译完成 | 是 | 是 | 是 |
| BasicBlockModel 基本块数 | 70 | 70 | 70 |
| 基本块目标引用边数 | 149 | 149 | 149 |
| 伪代码字符数 | 14,046 | 12,874 | 12,874 |

去符号确实移除了函数名：原版目标名称为 `normalize_repository_url`，去符号后为 `FUN_1001e2c1c`。剩余的一个已定义符号是 Mach-O header 符号 `__mh_execute_header`，动态导入仍然保留。

去符号版和 BLA 版的目标伪代码只存在地址差异。按相同 Mach-O `(segment, section)` 的相对位置把 BLA 版地址映射回去符号版后，两份 C 文本逐字一致。此过程保留所有非地址文本、常量和被调函数的对应关系，没有把所有十六进制常量或函数名统一抹掉。两份文本 SHA-256 均为：

```text
a2c48dadcd5e577ffd2c18d6efaa0bddecd503571356ea4b1b04fd790b4e3012
```

去符号版中仍可直接看到 `https` 的比较、`github.com` 的字节常量、端口 `0x1bb`（443）、路径段数量 `2` 的分支，以及规范化输出的 GitHub HTTPS URL 格式模板。BLA 没有改变这些语句。基本块目标引用边按 Ghidra `getDestinations()` 计数，不应等同于独立的源码分支数量。

源码辅助的字节匹配还确认，三个样本中都保留了一份完整目录解密密钥。这只证明密钥没有被隐藏，不等于通过盲测从零恢复了密钥。

## 限制与原始告警

- 原版全局自动分析报告 536 秒；两份去符号样本均触发 600 秒全局分析上限，分析器结束当前工作后报告总时间分别为 643、640 秒。不能称为“三份完整自动分析均通过”。三份目标函数的后置反编译均返回 `decompileCompleted=true`，错误字段为空。两份去符号样本并行运行且均被截断，这些时间不用于估算 BLA 的性能开销。
- 三份全局分析均出现共享 Rust 代码的函数重叠 / `VarnodeContext: out of address spaces` 等告警。目标伪代码也有类型传播告警，原版另有 Rust calling convention 告警。未证明能恢复完整 Rust 类型或全部源代码。
- 没有测试 REA 的 LLM 分析路径、独立分析员盲测、动态调试或虚拟化保护；本次不能给出这些方面的效果结论。
- 三样本逆向对照仅覆盖 macOS ARM64。Windows 的验证独立覆盖构建、运行夹具与发布二进制元数据。
- 本次没有启动实际 GUI 或连接真实代理做端到端测试。

## 复现定位任务

先用锁定版本的 BLA 源码构建其 `dist`，再准备 payload：

```bash
node scripts/bla-native.mjs prepare "$BLA_SOURCE" "$AUDIT_DIR/payload"
npm run build
```

使用 Cargo 环境覆盖分别构建 `none`、`symbols`，每次完成后单独保存输出文件，避免被下一次构建覆盖：

```bash
CARGO_PROFILE_RELEASE_STRIP=none cargo rustc --release --locked \
  --manifest-path src-tauri/Cargo.toml --bin GitBoost \
  --target aarch64-apple-darwin --features tauri/custom-protocol

CARGO_PROFILE_RELEASE_STRIP=symbols cargo rustc --release --locked \
  --manifest-path src-tauri/Cargo.toml --bin GitBoost \
  --target aarch64-apple-darwin --features tauri/custom-protocol

CARGO_PROFILE_RELEASE_STRIP=symbols cargo rustc --release --locked \
  --manifest-path src-tauri/Cargo.toml --bin GitBoost \
  --target aarch64-apple-darwin --features tauri/custom-protocol \
  -- -C "link-arg=-Wl,-sectcreate,__TEXT,__bla,$AUDIT_DIR/payload/payload.bin"
```

对每个保存后的二进制使用相同命令，仅替换输入路径、项目名和输出目录：

```bash
"$GHIDRA_1143/support/analyzeHeadless" "$AUDIT_DIR/projects" "$VARIANT" \
  -import "$BINARY" -scriptPath "$PWD/scripts" \
  -postScript AuditRepository.java "$AUDIT_DIR/$VARIANT" \
  -max-cpu 4 -analysisTimeoutPerFile 600 -deleteProject
```

审计环境因用户设置目录受到沙箱限制，直接通过官方 `ghidra.Ghidra` Java 入口调用同一 `AnalyzeHeadless` 类，并把设置、缓存和临时目录指向任务临时目录；未改变分析器选项。本次原始 `.c`、日志与二进制保存在 `/private/tmp/gitboost-protection-check`。不同工具链或依赖版本的输出哈希可能变化，应使用同一批次构建的三个样本作比较。

## 发布构建

正式构建使用 Cargo `strip = "symbols"`；MSVC release 的 GitBoost 可执行文件额外传入 `/DEBUG:NONE`。这是因为实测 GitHub Windows 工具链 Rust 1.99.1 仅设置 strip 时仍产生 CodeView/PDB 引用，其 MSVC 链接实现也仍忽略 strip 参数。检查脚本允许正常的 VC feature、POGO section 分组、可复现构建及安全标记元数据，拒绝 COFF 符号、CodeView/PDB 引用、调试 section 与 BLA section。

检查对象是实际生成的可执行文件，不以配置文件本身作为成功证据。安装包兼容性与符号检查的结果独立于上述反编译结论。

代码提交 `0cc79331114b1a81b23ba46e9be83bb382d5b94e` 的 [Native builds](https://github.com/DiscoverBox/gitboost/actions/runs/41034319241) 两端均成功，[常规 CI](https://github.com/DiscoverBox/gitboost/actions/runs/41034319286) 也通过。Windows 原生构建第一次在已有 Git 测试中遇到 5 秒超时，按原配置重跑后通过，没有修改业务代码或放宽产品超时。

- macOS：包内可执行文件为 12,756,960 字节，已定义符号仅剩 1 个，未带入 BLA；app 严格签名及 DMG 完整性检查通过。
- Windows：包内程序为 16,698,880 字节，COFF 符号数为 0，无 CodeView/PDB 引用，无 BLA section。仅保留 type 13 的 POGO section 分组元数据，分组名中未发现 Rust 业务函数名，安装包内没有 PDB 文件。
- 已实际解包 NSIS 和 MSI 并检查其中的 EXE；两包内程序除 Tauri 的 bundle type 标记外逐字节一致。安装包和包内 EXE 的哈希保存在机器可读报告中。

本次审计没有修改公开的 v0.3.3 标签或下载，也没有发布新版本；没有用新 tag 执行正式 Release workflow。以上 CI 安装包是 main 的测试构建。

参考：[Cargo strip 配置](https://doc.rust-lang.org/cargo/reference/profiles.html#strip)、[Rust MSVC 链接实现](https://github.com/rust-lang/rust/blob/1.99.1/compiler/rustc_codegen_ssa/src/back/linker.rs#L1011)、[Microsoft PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#debug-type)、[Ghidra 官方发行](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_11.4.3_build)。
