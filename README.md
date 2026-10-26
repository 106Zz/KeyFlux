<p align="center">
  <img src="packaging/macos/icons/KeyFlux.png" width="128" height="128" alt="KeyFlux App 图标">
</p>

<h1 align="center">KeyFlux</h1>

<p align="center">单按 Shift 切换中英文，短按中/英键切换大写。</p>
<p align="center">Rust 编写的 macOS 原生菜单栏工具。</p>

<p align="center">
  <a href="https://github.com/106Zz/KeyFlux/releases/latest">下载最新版本</a> ·
  <a href="#安装与首次设置">安装说明</a> ·
  <a href="#使用方式">使用方式</a> ·
  <a href="https://github.com/106Zz/KeyFlux/issues">反馈问题</a>
</p>

## 下载

<p align="center">
  <a href="https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg">
    <img src="packaging/macos/icons/KeyFluxDMG.png" width="88" height="88" alt="下载 KeyFlux DMG 安装包">
  </a><br>
  <a href="https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg"><strong>下载 KeyFlux v0.1.0 · Apple Silicon</strong></a>
</p>

| 安装包 | 适用设备 | 校验文件 |
| --- | --- | --- |
| [KeyFlux-0.1.0-arm64.dmg](https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg) | Apple Silicon Mac（M 系列芯片） | [SHA-256](https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg.sha256) |

当前提供 **arm64** 安装包，未提供 Intel 或 Universal 安装包。构建最低系统版本为 macOS 11.0，日常使用验证环境为 macOS 27.0.1 / Apple Silicon；其他系统版本尚未逐一验证。版本说明和后续下载见 [GitHub Releases](https://github.com/106Zz/KeyFlux/releases)。

> 当前版本采用本地 ad-hoc 签名，**尚未经过 Apple 公证**。首次打开可能被 macOS 拦截，处理方法见下文。

## 功能

- **Shift 切换输入法**：单按并松开 Shift，在系统 ABC 与 Apple 拼音之间切换；Shift + 字母等组合输入不会触发这个切换。
- **中/英键切换大写**：短按 Caps Lock / 中英键进入大写 ABC，再按一次回到小写 ABC。
- **菜单栏常驻**：顶部显示 **⇧**，不显示主窗口或 Dock 图标；可以随时暂停、重新启用或退出。
- **可选登录自启**：从菜单手动开启，默认关闭。
- **监听故障恢复**：遇到可恢复的监听错误时自动尝试重建；状态和原因保留在菜单及日志中。

## 安装与首次设置

### 1. 安装 App

1. 下载并打开 DMG。
2. 把 **KeyFlux.app** 拖入 **Applications（应用程序）**。
3. 从“应用程序”中打开 KeyFlux，顶部菜单栏会出现 **⇧**。

请先退出旧版 KeyFlux，再安装和打开新版。不要直接从 DMG 中运行，也不要执行 App 内部的二进制文件。

如果 macOS 提示无法验证开发者，请先确认下载来源及校验值，再按 [Apple 的官方说明](https://support.apple.com/zh-cn/102445)，在“系统设置 → 隐私与安全性”中处理本次打开提示。公司管理的设备可能不允许设置例外。

### 2. 授予键盘权限

在“系统设置 → 隐私与安全性”中允许 **KeyFlux**：

- **辅助功能**：部分新版系统显示为“设备控制和数据访问”。
- **输入监控**。

可从 **⇧ → 检查权限并启用…** 检查权限，并打开对应的系统设置。若系统要求退出并重新打开，请从 **⇧ → 退出 KeyFlux** 退出后，再从“应用程序”打开。

权限齐全时会自动启用键盘切换；缺少权限时保留菜单栏等待授权。

### 3. 设置系统输入法

1. 在“系统设置 → 键盘 → 文字输入”中启用 **ABC** 和 **Apple 拼音**。
2. 在输入法编辑页面，关闭系统的 **使用大写锁定键 / 中英键切换输入法** 选项，避免同一个按键被系统和 KeyFlux 同时处理。

KeyFlux 不会替你修改这个系统选项。不同 macOS 版本的文字和入口可能略有区别。监听期间若更改启用的输入法列表，请先暂停，再重新启用键盘切换。

## 使用方式

| 当前状态 | 操作 | 结果 |
| --- | --- | --- |
| 中文拼音 | 单按 Shift | 小写 ABC |
| 小写 ABC | 单按 Shift | 中文拼音 |
| 中文拼音或小写 ABC | 短按中/英键 | 大写 ABC |
| 大写 ABC | 再短按中/英键 | 小写 ABC |
| 大写 ABC | 单按 Shift | 关闭大写，留在小写 ABC；再按一次 Shift 切入拼音 |
| 大写 ABC | Shift + 字母 | 临时输入小写字母 |

当前仅处理系统 **ABC / Apple 拼音** 组合，其他输入法不在切换目标中。切入拼音后，应以输入框能够正常出现中文候选词为准，不能只看输入法图标。

点击菜单栏 **⇧**：

| 菜单 | 作用 |
| --- | --- |
| 启用键盘切换 / 暂停键盘切换 | 接管或交回键盘，暂停后保留菜单栏 |
| 检查权限并启用… | 检查权限，权限齐全后启用 |
| 打开键盘设置… | 打开系统设置，手动调整中/英键选项 |
| 登录时自动启动 | 开启或关闭下次登录时启动；不重启当前进程 |
| 状态与权限 | 查看当前状态、最近消息及权限查询结果 |
| 使用说明 | 查看系统设置方法 |
| 退出 KeyFlux | 停止监听并退出程序 |

登录自启请在 App 放入“应用程序”后开启。移动 App 后，菜单可能提示更新启动路径；若 macOS 禁用了后台项目，还需在系统登录项设置中允许。

## 常见问题

**菜单栏有 ⇧，按键却没有切换？**

先查看“状态与权限”，确认已启用、权限齐全，且 ABC / Apple 拼音都已添加。若缺少权限，完成授权后重新检查；系统要求重启 App 时，先退出再打开。

**外接键盘或唤醒后偶尔短暂失效？**

目前日常使用中仍观察到低频、随后自行恢复的情况，尚未完全定位。若持续失效，可以从菜单暂停后重新启用。反馈时请提供发生时间、macOS 版本、键盘型号、连接方式、应用名称，以及是 Shift 还是中/英键失效。

**为什么密码框或部分应用不接管？**

系统安全输入期间会放行相关操作，避免干扰受保护的输入。远程桌面、游戏和特殊输入框尚未全面验证；必要时可先暂停 KeyFlux。

**内存占用大约多少？**

一次 macOS 27.0.1 / Apple Silicon 的日常采样中，预热后物理 Footprint 平均约 **21.18 MB**，结束约 **22.37 MB**，采样峰值约 **23.45 MB**。该次记录包含睡眠或采样空档，仅供参考，不代表所有设备或连续活跃使用的结果；RSS 与 Footprint 是不同口径。新包加入图标后的资源占用尚未重新采样。

**性能脚本为什么 24 小时后结束？**

`monitor_keyflux.py` 默认最多采样 24 小时。达到时长或按 Ctrl+C 只结束采样并保存报告，不会停止 KeyFlux；当前不支持无限时长采样。开发和采样方法见 [开发与性能记录](docs/DEVELOPMENT.md)。

## 日志与反馈

基础运行日志保存在：

```text
~/Library/Logs/KeyFlux/KeyFlux.log
```

默认关闭逐按键诊断。排查时，先从菜单退出已有 KeyFlux，再执行：

```sh
open -a /Applications/KeyFlux.app --args --debug
```

结束排查后退出并正常打开，恢复普通日志模式。诊断记录控制状态和输入来源，不记录输入文字。分享日志前请检查路径、设备信息等内容。

请在 [Issues](https://github.com/106Zz/KeyFlux/issues) 中描述问题；不要提交输入内容、密码或其他敏感信息。

## 从源码构建

需要 macOS、Xcode Command Line Tools 和 Rust **1.89+**：

```sh
git clone https://github.com/106Zz/KeyFlux.git
cd KeyFlux
cargo build --release
```

运行开发二进制、性能采样和项目结构见 [开发与性能记录](docs/DEVELOPMENT.md)。生成带图标的 App / DMG、签名及公证方法见 [发行说明](docs/DISTRIBUTION.md)。普通安装用户直接下载上方 DMG 即可。

## 致谢与许可

Shift 手势识别参考了 [TW527E/ShiftIME](https://github.com/TW527E/ShiftIME) 的设计，感谢原作者。

KeyFlux 使用 [MIT License](LICENSE)。原项目版权及许可保留在 [第三方声明](docs/THIRD_PARTY_NOTICES.md) 中。
