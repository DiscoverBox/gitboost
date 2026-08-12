# GitBoost

GitBoost 是一个 macOS 桌面工具：用户继续使用原始 `https://github.com/...` 地址，应用通过 Git 原生 URL 重写，把公开仓库的读取临时切到用户选择的外部节点。push 默认保持 GitHub 直连；显式 `pushurl` 会在诊断中告警。

当前交付仅支持 Apple Silicon（arm64）macOS，不构建 Intel 或 Universal 版本。

## 当前实现

- Tauri 2 + Rust 核心，React + TypeScript 界面。
- 预置 `https://fastgit.cc/https://github.com/`，首次状态为“未检测”。
- 粘贴 / JSON 导入、规范化、去重、凭据与查询参数拒绝。
- 隔离的 `git ls-remote` 双次检测、失败分类、有限健康历史与自动选路。
- 全局加速 / 仅加速清单、直连清单、固定节点与直连模式。
- 独立 `gitboost.gitconfig`、候选配置验证、原子替换、精确 include 注册与恢复。
- Git 冲突、有效 fetch/push 地址、显式 `pushurl` 的脱敏诊断。
- 基于 Git Trace2 Unix Socket 的实际连接日志，区分 FastGit、GitHub 直连和其他重写；不落盘原始参数或凭据。
- macOS 托盘控制、定时健康检查、登录时启动。

## 开发

```bash
npm install
npm test
npm run test:ui
npm run tauri dev
```

Rust 单元测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

构建 macOS App / DMG：

```bash
npm run tauri -- build --target aarch64-apple-darwin
```

数据保存在 macOS 的应用数据目录 `pro.gitboost.desktop` 下。恢复操作只删除 GitBoost 自己注册的 `include.path` 并清空自己的重写规则，不修改任何仓库的 remote。

“使用日志”要求 GitBoost 正在运行。应用退出后加速配置仍然有效，但没有本地 Socket 接收 Trace2 事件，因此退出期间的 Git 操作不会补记；Socket 不可用不会阻断 Git 命令。
