# GitBoost 使用教程

GitBoost 用来加速公开 GitHub 仓库的 HTTPS 读取。你仍然使用原来的 `https://github.com/owner/repository.git` 地址，GitBoost 会在本机通过 Git 配置选择可用线路，不会修改仓库中保存的 `origin`。push 默认仍直连 GitHub。

> 只建议用 GitBoost 访问公开仓库。第三方加速节点可以看到仓库路径和传输内容；访问私有仓库或不确定仓库属性时，请保持“仅加速清单”，不要把私有仓库加入清单。

## 1. 安装前准备

GitBoost 当前支持：

- Apple Silicon（M 系列芯片）Mac，macOS 12 或更高版本；
- 64 位 Windows 10/11；
- GitHub HTTPS 地址。`git@github.com:owner/repository.git` 这类 SSH 地址不会被改写。

Windows 用户需要先安装 [Git for Windows](https://git-scm.com/download/win)。macOS 用户如果在终端执行 `git --version` 能看到版本号，就可以直接使用。

从项目的 [Releases 页面](https://github.com/DiscoverBox/gitboost/releases)下载当前正式版本：

请只从 GitBoost 官方项目下载应用，并在继续安装前核对下载来源和文件名。

- macOS：打开 DMG，把 GitBoost 拖到“应用程序”。当前 macOS 版本未使用 Apple Developer ID 证书签名，也未经过 Apple 公证，因此首次启动前需要手动移除系统添加的隔离标记。确认安装包来自上述官方 Releases 页面后，打开终端执行：

  ```bash
  sudo xattr -rd com.apple.quarantine /Applications/GitBoost.app
  ```

  该命令仅针对 `/Applications/GitBoost.app` 及其内部文件移除隔离标记。输入管理员账户密码时，终端不会显示字符；命令执行完成后，再从“应用程序”中启动 GitBoost。
- Windows：运行 EXE 或 MSI 安装包，按提示完成安装。

## 2. 第一次启用加速

### 第一步：确认 Git 环境

打开 GitBoost 后先看“总览”。正常状态下，“Git 配置”会显示是否已经接入，“连接质量”会显示当前线路及延迟。

![GitBoost 启用成功后的总览示例](images/guide-overview.png)

如果页面提示“未检测到系统 Git”，请先安装 Git，再重新打开 GitBoost。

### 第二步：设置路由范围

进入“路由清单”，建议保留默认的“仅加速清单”。在输入框中填写公开仓库，以下两种格式都可以：

```text
owner/repository
https://github.com/owner/repository.git
```

点击“加入清单”后，该仓库会出现在列表中。

![设置公开仓库路由清单](images/guide-routes.png)

“全局加速”会让所有 GitHub HTTPS 读取经过第三方节点。Git 无法自动判断仓库是否公开，因此只有在确定不会访问私有仓库时才使用。

清单按 URL 前缀匹配。例如，清单中的 `owner/repo` 也可能匹配名称以它开头的 `owner/repo-private`。如果公开仓库名可能与私有仓库形成这种关系，请不要把它加入清单。

### 第三步：开启加速

回到“总览”，点击“开启加速”或选择“自动选择”。第一次开启时，应用会说明第三方节点可见的信息；确认后，GitBoost 会写入并验证自己的独立 Git 配置。

启用成功后可以继续照常使用原始地址：

```bash
git clone https://github.com/owner/repository.git
git -C repository fetch
git -C repository pull
```

不需要把 remote 改成代理地址。

### 第四步：确认实际线路

保持 GitBoost 在后台运行，完成一次 `clone`、`fetch` 或 `pull`，再打开“使用日志”。“实际线路”会显示本次操作是经过加速节点、GitHub 直连，还是其他 URL 重写。

![查看实际 Git 使用线路](images/guide-usage.png)

使用日志只保存在本机，自动保留最近 7 天，不记录原始命令、Token、用户名、查询参数或环境变量。关闭“记录 Git 使用”不会关闭加速，也不会关闭自动故障复检。

## 3. 日常使用

### 自动选路和重新测速

“自动选择”会使用已经通过真实 Git 检测的可用线路。需要立即刷新线路状态时，在“总览”点击“重新测速”，或在“设置”中点击“检测线路”。

当前 Git 操作失败并确认线路异常后，GitBoost 会为下一次命令选择其他线路。它不会接管或自动重试已经开始的 `clone`，因此请手动重新执行失败的命令。

### 临时恢复 GitHub 直连

在“总览”点击“关闭加速”，或把线路模式切换为“直连”。切换只影响之后启动的 Git 操作。

直接退出应用不会自动关闭加速：最后一次成功写入的 Git 配置仍然有效。如果你希望退出后保持 GitHub 直连，请先在“总览”关闭加速。

### 下载公开 GitHub 文件

“文件下载”支持 `github.com` 下的公开页面或文件地址。粘贴地址后点击“开始下载”，GitBoost 会先用当前线路进行小流量探测，再交给系统默认浏览器打开。

![通过当前线路打开 GitHub 下载地址](images/guide-download.png)

下载功能独立于 Git 路由清单。它不接受带用户名、密码、Token、查询参数或片段的地址；当前节点失败时也不会静默改成 GitHub 直连，可按页面提示换下一条线路重试。

## 4. 环境诊断

遇到配置冲突、fetch 与 push 地址异常，或需要提交问题时，打开“环境诊断”：

1. 先检查系统 Git、独立配置和重写冲突状态；
2. 如需检查某个仓库，在“仓库本地路径”中填写该仓库目录；
3. 点击“运行诊断”；
4. 使用“复制脱敏报告”取得可用于排查的结果。

![GitBoost 环境诊断](images/guide-diagnostics.png)

> 截图中的本机路径已隐去。GitBoost 不会覆盖其他应用或用户已有的 URL 重写配置；发现冲突时，请先根据诊断报告确认来源。

诊断中的几个地址含义如下：

- “保存值”：仓库中原本保存的 remote；
- “fetch 有效地址”：Git 实际用于读取的地址；
- “push 有效地址”：Git 实际用于推送的地址；
- “显式 pushurl”：仓库单独设置的推送地址。存在时，push 不一定仍直连 GitHub，需要人工确认。

## 5. 设置与恢复

“设置”页包含后台健康检查、登录时启动、日志级别、自定义节点和恢复工具。

![GitBoost 设置](images/guide-settings.png)

- **后台健康检查**：默认每天维护可用线路；找到 10 条后停止继续检测。
- **登录时启动**：建议开启，以便应用在后台接收 Git 使用结果，并在节点失效后重新选路。
- **日志级别**：日常使用选“信息”即可；排查问题时再临时改为“调试”。
- **自定义节点**：通常无需添加。需要时，每行输入一个 HTTPS 代理地址，再运行线路检测。
- **导出自定义节点**：只导出你自己添加的节点。
- **清理日志**：清除 GitBoost 的本地日志。
- **恢复 Git 配置**：删除 GitBoost 自己注册的包含项并清空自己的重写规则，不修改任何仓库的 remote。

## 6. 常见问题

### 开启加速后仍然很慢

先点击“重新测速”。如果当前命令已经失败或卡住，请结束该命令，等待选路完成后手动重试。自动切换只对下一次 Git 操作生效。

### 提示没有可用线路

到“设置”点击“刷新系统线路”，然后点击“检测线路”。线路检测需要网络，并会对公开仓库执行隔离的真实 Git 探测。

### 使用日志没有记录

确认以下三点：

1. GitBoost 正在运行；
2. “记录 Git 使用”已经开启；
3. 执行的是 GitHub HTTPS 的 `clone`、`fetch` 或 `pull`，而不是 SSH 操作。

应用退出期间无法接收 Git 事件，因此不会补记日志，也无法根据那段时间的实际 Git 失败自动换线。

### push 会经过第三方节点吗

默认不会，push 保持 GitHub 直连。但仓库如果显式设置了 `pushurl`，实际行为以该配置为准；可在“环境诊断”中检查。

### 如何彻底停用 GitBoost 配置

如果只是临时不用，在“总览”切换为“直连”。如果要移除 GitBoost 写入的 Git 配置，到“设置”点击“恢复 Git 配置”。该操作不会删除仓库，也不会修改仓库中保存的 remote。
