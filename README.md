<p align="center">
  <img src="packaging/macos/icons/KeyFlux.png" width="96" height="96" alt="KeyFlux">
</p>

<h1 align="center">KeyFlux</h1>

<p align="center">为 macOS 带来顺手的中英文与大写切换。</p>
<p align="center">
  <a href="https://github.com/106Zz/KeyFlux/releases/latest">下载</a> ·
  <a href="#安装">安装</a> ·
  <a href="#使用">使用</a> ·
  <a href="https://github.com/106Zz/KeyFlux/issues">反馈</a>
</p>

KeyFlux 是使用 Rust 编写的 macOS 原生菜单栏工具。单按 Shift 切换 ABC 与 Apple 拼音，短按中/英键切换大写，让常用输入操作保持简单。

## 功能

- **Shift 切换中英文**：单按并松开 Shift 切换输入法，保留 Shift + 字母等组合操作。
- **中/英键控制大写**：短按 Caps Lock / 中英键进入大写 ABC，再按一次回到小写 ABC。
- **菜单栏操作**：通过顶部 **⇧** 启用、暂停或退出，无需打开主窗口。
- **按需登录自启**：从菜单开启，默认关闭。

## 下载

<a href="https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg"><img src="packaging/macos/icons/KeyFluxDMG.png" width="56" height="56" align="right" alt="KeyFlux DMG"></a>

| 版本 | 平台 | 下载 |
| --- | --- | --- |
| v0.1.0 | macOS · Apple Silicon（M 系列） | [下载 DMG](https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg) · [SHA-256](https://github.com/106Zz/KeyFlux/releases/download/v0.1.0/KeyFlux-0.1.0-arm64.dmg.sha256) |

[查看版本说明](https://github.com/106Zz/KeyFlux/releases) · 构建最低系统版本：macOS 11.0 · 当前提供 arm64 安装包。

## 安装

### 1. 安装应用

打开 DMG，将 **KeyFlux.app** 拖入 **Applications（应用程序）**，然后从“应用程序”打开。顶部菜单栏出现 **⇧** 即表示应用已运行。更新前请先退出旧版。

当前安装包采用 ad-hoc 签名，未经 Apple 公证。首次打开若被系统拦截，请在确认下载来源后，按照 [Apple 官方说明](https://support.apple.com/zh-cn/102445) 在“系统设置 → 隐私与安全性”中处理打开提示。

### 2. 允许键盘权限

在“系统设置 → 隐私与安全性”中，为 **KeyFlux** 开启：

- **辅助功能**（部分新版系统显示为“设备控制和数据访问”）。
- **输入监控**。

随后选择 **⇧ → 检查权限并启用…**。如果系统提示需要重新打开应用，请先从菜单退出，再从“应用程序”启动。

### 3. 设置输入法

在“系统设置 → 键盘 → 文字输入”中：

1. 添加 **ABC** 与 **Apple 拼音**。
2. 关闭系统的 **使用大写锁定键 / 中英键切换输入法** 选项，避免重复响应。

权限齐全时，KeyFlux 会自动启用键盘切换。不同 macOS 版本的设置名称可能略有差异。

## 使用

| 当前状态 | 按键 | 切换结果 |
| --- | --- | --- |
| 中文拼音 | 单按 Shift | 小写 ABC |
| 小写 ABC | 单按 Shift | 中文拼音 |
| 中文拼音或小写 ABC | 短按中/英键 | 大写 ABC |
| 大写 ABC | 短按中/英键或单按 Shift | 小写 ABC |
| 大写 ABC | Shift + 字母 | 临时输入小写字母 |

大写状态下单按 Shift 会先关闭大写，再按一次 Shift 才切入拼音。

点击菜单栏 **⇧** 可以暂停或重新启用键盘切换、查看状态与权限、设置登录自启，或退出应用。暂停后按键交由系统处理，菜单栏仍然保留。

登录自启请在 App 移入“应用程序”后开启。移动 App 或更改系统输入法列表后，请检查自启路径，并重新启用键盘切换。

## 兼容性

- 输入法切换适用于系统 **ABC / Apple 拼音** 组合。
- 日常使用验证环境为 **macOS 27.0.1 / Apple Silicon**，其他系统版本尚未逐一验证。
- 系统安全输入期间会放行相关操作；远程桌面、游戏和特殊输入框的行为可能不同。
- 外接键盘偶有短暂失效后恢复的情况；持续异常时，可从菜单暂停后重新启用。

## 问题反馈

请通过 [GitHub Issues](https://github.com/106Zz/KeyFlux/issues) 提交问题，附上 macOS 版本、应用名称、键盘型号及连接方式、发生时间和失效按键。

基础日志位于 `~/Library/Logs/KeyFlux/KeyFlux.log`。分享前请检查其中的个人路径和设备信息，不要提交输入内容或敏感信息。

## 开发

需要 macOS、Xcode Command Line Tools 和 Rust 1.89+：

```sh
git clone https://github.com/106Zz/KeyFlux.git
cd KeyFlux
cargo build --release
```

[开发指南](docs/DEVELOPMENT.md) · [打包与发行](docs/DISTRIBUTION.md)

## 致谢与许可

Shift 手势识别参考了 [TW527E/ShiftIME](https://github.com/TW527E/ShiftIME) 的设计。

采用 [MIT License](LICENSE)，第三方版权及许可见 [第三方声明](docs/THIRD_PARTY_NOTICES.md)。
