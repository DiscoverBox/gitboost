# GitBoost

GitBoost 是一个 macOS / Windows 桌面工具：用户继续使用原始 `https://github.com/...` 地址，应用通过 Git 原生 URL 重写，把公开仓库的读取临时切到本机自动选择的加速线路。push 默认保持 GitHub 直连；显式 `pushurl` 会在诊断中告警。

当前交付支持 Apple Silicon（arm64）macOS，以及 64 位 Windows 10/11（x86_64）。macOS 不构建 Intel 或 Universal 版本。

## 当前实现

- Tauri 2 + Rust 核心，React + TypeScript 界面。
- 从官方静态 URL 列表更新系统节点，本地保留 Last Known Good 缓存；远程不可用不影响已有线路。
- 系统节点与用户自定义节点合并后统一规范化、去重和检测；凭据、查询参数与片段会被拒绝。
- 隔离的 `git ls-remote` 双次检测、最多 4 路并发、全量任务互斥、失败分类、有限健康历史与自动选路。
- 全局加速 / 基于 URL 前缀的仅加速清单、公开仓库清单、固定节点与直连模式。
- GitHub 地址校验、节点小流量探测和浏览器打开。
- 独立 `gitboost.gitconfig`、候选配置验证、原子替换、精确 include 注册与恢复。
- Git 冲突、有效 fetch/push 地址、显式 `pushurl` 的脱敏诊断。
- 基于 Git Trace2 Unix Stream Socket 的实际连接日志，区分加速线路、GitHub 直连和其他重写；不落盘原始参数或凭据。
- 自动模式下，实际 Git 读取失败会触发当前节点的真实 Git 复检；确认节点异常后为下一次命令切换线路，没有候选节点时恢复 GitHub 直连。
- macOS / Windows 托盘控制、定时健康检查、登录时启动。

## 路由清单的匹配边界

Git 的 `insteadOf` 按 URL 前缀匹配。为同时支持带或不带 `.git` 的仓库地址，GitBoost 会用去掉 `.git` 的清单地址作为匹配前缀。因此，清单中的 `https://github.com/owner/repo.git` 也会匹配 `https://github.com/owner/repo-private.git` 等以同一地址开头的仓库。

这是当前接受的实现边界。请勿将名称可能与私有仓库形成前缀关系的公开仓库加入清单；存在这种情况时，请使用直连模式，避免私有仓库读取经过外部节点。push 仍默认保持 GitHub 直连，但显式 `pushurl` 不受此保证。

## 开发

```bash
npm install
npm test
npm run test:ui
npm run tauri dev
```

核心链路集成验证会通过真实系统节点执行 `git ls-remote`，并使用隔离的临时 Git 全局配置串联验证路由、配置注册、Git 实际地址解析、持久化、诊断脱敏、使用日志和恢复直连。该测试需要网络，但不会修改开发机的 `~/.gitconfig`：

```bash
npm run test:integration
```

提交前运行全部前端单元测试、Rust 测试和桌面界面测试：

```bash
npm run test:all
```

Rust 单元测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

构建 macOS App / DMG：

```bash
npm run tauri -- build --target aarch64-apple-darwin
```

在 64 位 Windows 10/11 上构建 NSIS / MSI 安装包：

```powershell
npm ci
npm run build:windows
```

Windows 需要先安装 Git for Windows。安装包默认按当前用户安装；WebView2 缺失时由安装器静默引导安装。产物位于 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/`。

系统节点目录源文件为仓库根目录的 `nodes.enc.json`，主发布地址为 `https://cdn.jsdelivr.net/gh/DiscoverBox/gitboost@main/nodes.enc.json`，不可用时依次回退到 JSDMirror、Bili33 CDN 和 JSDMirror.com。文件使用 AES-256-GCM 加密，不直接包含代理域名；客户端解密并校验后才会更新本地缓存。由于解密密钥随开源客户端分发，这项措施用于避免静态目录直接暴露节点，并不用于抵抗客户端逆向。

更新系统节点时，先准备一个不提交到仓库的明文 URL 数组，再生成发布文件：

```bash
node scripts/encrypt-nodes.mjs /path/to/plain-nodes.json nodes.enc.json
```

数据保存在系统的应用数据目录 `pro.gitboost.desktop` 下。`system-nodes.json` 保存最近一次有效的系统节点目录，`nodes.json` 只保存用户自定义节点。恢复操作只删除 GitBoost 自己注册的 `include.path` 并清空自己的重写规则，不修改任何仓库的 remote。

“使用日志”要求 GitBoost 正在运行，只保留最近 7 天的脱敏记录。关闭使用日志不会关闭自动故障复检，也不会写入使用记录。应用退出后加速配置仍然有效，但没有本地 Socket 接收 Trace2 事件，因此退出期间的 Git 操作不会补记，也无法根据实际 Git 失败自动换线；Socket 不可用不会阻断 Git 命令。
