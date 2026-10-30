# 单函数 LLVM 混淆实验（2026-10-30）

这次实际变换了 `gitboost_lib::importer::normalize_repository_url` 的控制流，Mac 与 Windows 的目标函数都变复杂了。不过 Ghidra 仍能找到并反编译它，不能据此声称已经获得强保护或显著延长逆向时间。实验使用免费开源代码，没有采购 VMProtect，也没有接入正式发布。

## 变更与范围

- 上游为 [eShard obfuscator-llvm](https://github.com/eshard/obfuscator-llvm/tree/ad26aa21b90cc5c5118d32114b674d1455f54168)，固定提交 `ad26aa21b90cc5c5118d32114b674d1455f54168`，保留原许可证。
- 采用 bogus control flow 与 instruction substitution，由 [单函数适配器](../scripts/ollvm-pass.cpp) 只选择上述解析函数，不处理它的闭包和其他函数。固定随机种子便于对照。
- [最小上游补丁](../scripts/ollvm-eshard.patch) 限制最后处理阶段只修改被选函数，修正条件类型检查，并让辅助全局变量保持私有。
- 没有启用控制流平坦化：目标函数有异常处理路径，上游实现会跳过这类函数。保留原有 `panic=unwind`，不为混淆改变错误处理语义。
- 对同一份完整 LLVM 模块统一使用 `opt` 序列化后逐函数比较：6,164 个函数定义中，只有目标函数的定义改变。该检查对应 Mac 原始模块，不代表两个平台的所有机器码字节都保持不变。
- 业务运行逻辑未修改；新增一项通过 `black_box` 函数指针执行解析器的边界测试，覆盖 2,321 个输入，避免优化器把测试中的调用直接折叠掉。

源码基线为 `62a9dd3df2ce6fb51757d7d5b1ba7eed84092704`。Mac 构建报告记录的是当时 HEAD `b586734` 加上尚未提交的实验文件；报告中的 importer、适配器和补丁 SHA-256 与 `62a9dd3` 一致。两组样本都关闭 BLA、去除符号，并对业务 crate 使用 `codegen-units=1`，仅保护样本加载插件。

## 实际结果

| 平台 | 样本 | 文件大小 | 目标函数基本块 | 出边数 | 反编译 C 字符数 | Ghidra 反编译 |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| macOS ARM64 | 对照 | 10,742,672 | 70 | 154 | 13,802 | 成功 |
| macOS ARM64 | 混淆 | 10,742,688 | 134 | 268 | 22,694 | 成功 |
| Windows x64 | 对照 | 15,124,992 | 62 | 131 | 13,153 | 成功 |
| Windows x64 | 混淆 | 15,126,016 | 112 | 219 | 17,648 | 成功 |

这里统计的是 Ghidra `BasicBlockModel` 对定位到的目标函数的结果，出边数包含模型返回的目标边。编译器 IR 的块数另为 Mac **87→147**、Windows **82→133**，不能与反汇编后的基本块数混用。文件大小差异也不能单独用于判断保护强度。

Mac 两个优化测试程序均为 **96 passed / 0 failed / 5 ignored**，包含上述边界测试。保护版 `.app` 通过严格 ad hoc 签名检查；其加载代码与数据 section 的地址、长度和散列均与已测试的保护版裸二进制一致。打包重新签名会改变签名区和文件长度，不能把整个文件散列差异误判成保护被移除。未进行 GUI、实时代理和 Apple 公证验证。

Windows 使用同一 Homebrew Rust/LLVM 插件，通过 cargo-xwin 交叉编译 MSVC 二进制；标准库由相同编译器从官方源码重建，未绕过 Rust 编译器版本检查。在 [Windows 原生验证任务](https://github.com/DiscoverBox/gitboost/actions/runs/41074750441) 中，对照和保护版优化测试程序均为 **92 passed / 0 failed / 4 ignored**，两次边界测试均通过；应用和测试 EXE 都通过 x64、COFF 符号表、CodeView/PDB 及 BLA 检查。下载 CI 产物后，四个 EXE 的 SHA-256 均与本地原始样本一致。临时转运草稿已删除，测试记录与产物保留在该 Actions 任务中（产物保留 7 天）。未验证保护版 Windows 安装器和 GUI。

[机器可读报告](ollvm-audit.json) 包含四个样本的 SHA-256、函数地址、工具版本、测试结果与上游散列。既有正式流水线 [CI](https://github.com/DiscoverBox/gitboost/actions/runs/41067339251) 和 [Native builds](https://github.com/DiscoverBox/gitboost/actions/runs/41067339249) 均通过；这两条流水线构建的是常规版本，不能当作实验混淆版的运行证明。

## 如何解释保护效果

这比原来的去符号和 BLA 填充更进一步：变换发生在一个真实业务函数里，反编译结果也出现了更多分支和表达式。但四个样本仍能从相同的中文错误字符串直接定位目标函数，全部成功反编译。没有字符串加密、密钥隐藏、虚拟机解释执行，也没有保护其他业务函数。已知 OLLVM 不透明谓词存在可被简化的模式。

本项目源码公开，本实验主要验证技术是否能改变编译产物。未做独立盲测、人工逆向计时、自动去混淆对抗或性能基准，不能把块数或 C 字符数的增长换算成“破解难度提高多少倍”。测试通过也不保证所有合法与非法输入都等价。

适合把它作为免费方案的有限验证；当前证据不足以默认用于全项目。正式 `v0.3.3` 及普通构建流程保持原状，实验产物未作为正式版本发布。

## 复现 Mac

实际环境为 macOS ARM64、Homebrew Rust **1.97.1**、LLVM **22.1.8**。Rust 必须动态链接插件使用的同一个 `libLLVM.dylib`；普通 rustup 工具链不满足这个前提。脚本会检查而非忽略 ABI 差异。

```sh
npm ci
node scripts/ollvm-native.mjs /private/tmp/gitboost-ollvm-reproduce
```

[构建脚本](../scripts/ollvm-native.mjs) 固定上游提交，编译插件，分别构建并运行完整优化测试，检查二进制和 `.app`，输出 `build-report.json`。需要现有 Homebrew Rust/LLVM、Xcode Command Line Tools 与网络，不会安装或替换系统工具链。插件和 `RUSTC_BOOTSTRAP` 只用于实验命令。

## 复现 Windows 交叉构建

Windows 实验尚未成为一键正式发布流程。下面记录本次实际使用的工具链与命令，供复核；需要先完成上面的 Mac 插件构建。

| 组件 | 固定版本 | 下载归档 SHA-256 |
| --- | --- | --- |
| [cargo-xwin](https://github.com/rust-cross/cargo-xwin/releases/tag/v0.23.1) | universal2 Apple Darwin 0.23.1 | `1d74345d23ec744728124a0244696e9f21340fbb4b8a59cc1e39ba4ab18d8113` |
| [官方 rust-src](https://static.rust-lang.org/dist/rust-src-1.97.1.tar.xz) | 1.97.1 | `e9a1e616d04c6845895c827a178b9227f7c7199f3f4a80af81ab3aff7b80156b` |
| rust-lld | rustup 1.96.0 自带的 LLD 22.1.2 | 使用该工具链自带文件 |

在独立目录安装 rust-src（其归档内 `install.sh --prefix=<目录>/sysroot --disable-ldconfig`）；将实际 Homebrew sysroot 下的 `lib/rustlib/aarch64-apple-darwin` 链接到这个独立 sysroot 的同名位置，供主机 build script 使用。在工具目录放置 cargo-xwin，将 `lld-link` 链接到上述 `rust-lld`，并把 LLVM 22.1.8 的工具目录加入 PATH。cargo-xwin 自动下载 Windows SDK/CRT，首次需要接受其对应许可。

不要使用预编译 Windows std 替代重建：即便官方 Rust 1.97.1 与 Homebrew 版 commit 相同，本次仍报 `E0514`，完整编译器版本不同。最终使用 `-Z build-std`，没有关闭版本检查。

需要两个 Cargo wrapper。外层 `RUSTC_WRAPPER` 用同一 Rust 编译以下程序，并把独立 sysroot 的绝对路径替换到字符串里：

```rust
use std::{env, process::{Command, exit}};
fn main() {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    let compiler = args.remove(0);
    if !args.iter().any(|a| a.to_string_lossy().starts_with("--sysroot")) {
        args.push("--sysroot=/absolute/experiment/sysroot".into());
    }
    exit(Command::new(compiler).args(args).status().unwrap().code().unwrap_or(1));
}
```

内层 `RUSTC_WORKSPACE_WRAPPER` 分别编译到两个不同路径，对照版的 `plugin` 为空字符串，保护版替换为前面已验证的 `.dylib` 绝对路径：

```rust
use std::{env, process::{Command, exit}};
fn main() {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    let compiler = args.remove(0);
    let selected = args.windows(2).any(|p| p[0] == "--crate-name" && p[1] == "gitboost_lib");
    let mut command = Command::new(compiler);
    if selected {
        args.extend(["-C".into(), "codegen-units=1".into()]);
        let plugin = ""; // 保护版在此填入插件绝对路径
        if !plugin.is_empty() {
            args.extend(["-Z".into(), format!("llvm-plugins={plugin}").into()]);
            command.env("RUSTC_BOOTSTRAP", "1");
        }
        if args.iter().any(|a| a == "--test") {
            args.extend(["-C".into(), "link-arg=/DEBUG:NONE".into()]);
        }
    }
    exit(command.args(args).status().unwrap().code().unwrap_or(1));
}
```

在独立源码 worktree 安装前端依赖并运行 `TAURI_ENV_PLATFORM=windows npm run build`。对两个 wrapper 分别执行以下构建，执行后立即把应用和测试 EXE 复制到对应的样本目录，避免下次构建覆盖：

```sh
# 以下环境变量均使用自己的独立实验目录，不能修改系统 sysroot。
export RUSTC_BOOTSTRAP=1
export RUSTC_WRAPPER=/absolute/experiment/tools/sysroot-wrapper
export RUSTC_WORKSPACE_WRAPPER=/absolute/experiment/baseline/rustc-wrapper
export XWIN_CACHE_DIR=/absolute/experiment/xwin
export XWIN_ARCH=x86_64
export CARGO_TARGET_DIR=/absolute/experiment/target
export TAURI_ENV_PLATFORM=windows
cargo-xwin build --release --bin GitBoost --locked \
  --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc \
  --features tauri/custom-protocol -Z build-std=std,panic_unwind,test
cargo-xwin rustc --profile bench --lib --message-format=json --locked \
  --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc \
  --features tauri/custom-protocol -Z build-std=std,panic_unwind,test
```

应用路径为 `<target>/x86_64-pc-windows-msvc/release/GitBoost.exe`；测试路径读取 Cargo JSON 中 `target.name=gitboost_lib`、`profile.test=true` 的 `executable`。保护版日志应只出现一次目标函数变换，对照版不应出现。

把两个目录的应用与测试 EXE（测试程序重命名为 `gitboost-tests.exe`）及构建散列报告压缩。独立 [Windows 验证工作流](../.github/workflows/ollvm-windows-check.yml) 手动运行完整测试、检查实际 EXE，并保存结果。它使用资产的短期只读链接和指定 SHA-256，不使用仓库写权限。短期链接只允许 `release-assets.githubusercontent.com` 的 HTTPS 地址。临时草稿用于转运，验证完成后删除；不能把这个步骤改成公开发布正式版本。

## 复现反编译

四个样本使用同一 Ghidra **11.4.3**、JDK **21.0.10**、2 GiB heap、最多 4 个 CPU、单文件自动分析上限 600 秒和单函数反编译上限 60 秒。使用以下命令分别导入，不需要业务符号：

```sh
analyzeHeadless /absolute/audit-project sample \
  -import /absolute/sample/GitBoost \
  -scriptPath /absolute/gitboost/scripts \
  -preScript PrepareOllvmAudit.java \
  -postScript AuditRepository.java /absolute/audit-output \
  -max-cpu 4 -analysisTimeoutPerFile 600 -deleteProject
```

Windows 把导入路径改为对应 `GitBoost.exe`。预脚本对两个样本统一禁用 `Decompiler Switch Analysis` 和 `Non-Returning Functions - Discovered`；所以这里与早先默认分析/BLA 报告的数字不可直接比较。四次均报告 `Analysis succeeded`，目标函数的反编译错误字段为空，但四份 C 结果都有类型传播未收敛和全局符号重叠警告。Mac 两个样本的其他位置还有 `VarnodeContext: out of address spaces` 及函数/指令重叠警告，不能据此宣称整份 Rust 程序都已准确恢复。

本次本地实验产物在 `/private/tmp/gitboost-ollvm-verified/GitBoost-experimental.app` 和 `/private/tmp/gitboost-ollvm-windows/obfuscated/GitBoost.exe`。它们是临时实验文件，不是新的正式安装包。
