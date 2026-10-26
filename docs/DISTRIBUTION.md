# macOS App 与 DMG 发行

KeyFlux 的安装包通过 [GitHub Releases](https://github.com/106Zz/KeyFlux/releases) 分发。用户安装和首次设置见 [README](../README.md)。

## 打包内容

App 直接运行 Rust 可执行文件，使用原生 AppKit 菜单栏，不包含 Electron、WebView 或额外守护进程。正常启动时，权限齐全会启用键盘切换；缺少权限时保留菜单栏等待授权。`--paused` 可用于开发时采集暂停基线。

- 固定 Bundle ID：`org.keyflux.KeyFlux`。
- `LSUIElement=true`，不显示 Dock 图标。
- `CFBundleIconFile=KeyFlux.icns`，应用图标由仓库中的 PNG 生成。
- DMG 包含独立的磁盘图标：挂载卷使用 `.VolumeIcon.icns` 和自定义图标标记；DMG 文件的数据区嵌入 `icns` 资源，随安装包一起分发。
- App 内附 `LICENSE` 和 `THIRD_PARTY_NOTICES.md`。
- 最低 macOS 版本从构建产物的 Mach-O 信息读取，写入 Info.plist；不以模板中的版本替代实际构建要求。

图标源文件位于 [`packaging/macos/icons/`](../packaging/macos/icons/)。应用图标与 DMG 图标使用相同的 Shift 标记；菜单栏继续使用轻量文字 ⇧，不加载彩色 App 图标。

## 本机打包

要求 macOS、Xcode Command Line Tools、Rust 1.89+ 和 Python 3.9+。Python 仅使用标准库；图标转换使用系统 `sips`、`iconutil`，卷属性使用 Command Line Tools 的 `SetFile`。

在项目根目录执行：

```sh
python3 scripts/package_macos.py
```

脚本构建当前源码的本机架构：Apple Silicon 为 arm64，Intel 为 x86_64。单架构包不能标记为 Universal。

每次在 `dist/KeyFlux-版本-架构-时间-随机后缀/` 中创建独立产物，不覆盖旧包，不安装或启动 KeyFlux，不修改用户权限、自启配置或系统键盘选项。为了设置卷图标，打包期间会短暂挂载本次新建的工作镜像，设置完成后卸载，再生成压缩 DMG。

主要产物：

```text
image/
  KeyFlux.app/
  Applications -> /Applications
  安装说明.txt
  .VolumeIcon.icns
KeyFlux-版本-架构.dmg
KeyFlux-版本-架构.dmg.sha256
```

输出目录同时保留图标转换中间文件、卷配置工作镜像和嵌入图标的资源描述。上传 GitHub Release 时仅选择最终 DMG 及对应 SHA-256 文件。

只生成开发 App：

```sh
python3 scripts/package_macos.py --app-only
```

产物位于 `target/dev-apps/`，不创建 DMG、不提交公证、不启动或安装 App。

## 签名与 Apple 公证

默认使用本地 ad-hoc 签名，**没有 Developer ID 签名或 Apple 公证**。这类包可明确标注后用于早期版本分发，但不能宣称通过 Apple 公证，或保证下载后直接打开。首次打开的处理方法参考 [Apple 官方说明](https://support.apple.com/zh-cn/102445)。

脚本检查 App 签名及 DMG 数据校验；这些检查不代表 Gatekeeper、键盘权限、实际输入或所有系统版本兼容性已验收。

需要 Developer ID 签名与公证时，维护者需自行准备证书和已有的 notarytool 钥匙串凭据 profile：

```sh
python3 scripts/package_macos.py \
  --sign-identity 'Developer ID Application: YOUR NAME (TEAMID)' \
  --notary-profile 'YOUR_KEYCHAIN_PROFILE'
```

脚本为 App 启用 hardened runtime 和签名时间戳，公证并 staple App 后再生成 DMG，随后签名、公证并 staple DMG。也支持 `KEYFLUX_SIGN_IDENTITY`、`KEYFLUX_NOTARY_PROFILE` 环境变量；凭据不得写入仓库。

Fork 可以使用 `--bundle-id` 设置自己的身份。固定 Bundle ID 不能代替稳定签名证书，本地签名更新后仍可能需要重新授权。不要通过关闭系统安全保护来解决发行问题。

## 上传 GitHub Release

在确认发布源码已提交并推送后，为对应提交创建版本标签及 Release。v0.1.0 使用的资源名为：

```text
KeyFlux-0.1.0-arm64.dmg
KeyFlux-0.1.0-arm64.dmg.sha256
```

README 的下载链接指向这些精确名称。新版本发布时需要同步更新版本、架构和下载链接；缺少对应资源时不应只更新文档中的下载声明。

已登录 GitHub CLI 的维护者可以执行：

```sh
gh release create v0.1.0 \
  dist/实际输出目录/KeyFlux-0.1.0-arm64.dmg \
  dist/实际输出目录/KeyFlux-0.1.0-arm64.dmg.sha256 \
  --repo 106Zz/KeyFlux \
  --target main \
  --title 'KeyFlux v0.1.0' \
  --notes-file /绝对路径/发行说明.md
```

也可以在 GitHub 网页创建 Release 并上传两个文件。发布前后检查：标签所指提交、资源架构、App 与 DMG 图标、SHA-256、README 下载链接及签名说明一致。DMG 留在 Release 资源中，`dist/` 不进入 Git 源码仓库。

## 安装后的人工确认

从下载后的 DMG 把 App 拖入“应用程序”，通过 Finder / Launch Services 打开完整 App，不执行 `Contents/MacOS/KeyFlux`。确认 App 以自身身份申请权限，并检查：

- 单按 Shift 切换 ABC / 拼音，拼音能够实际出现中文候选词。
- 中/英键开关大写、大写下 Shift 行为与连续组合输入。
- 外接键盘重连、睡眠唤醒及系统安全输入。
- 暂停后键盘交回系统，退出后不再监听。
- App 图标、挂载卷图标及下载文件图标。
- 手动开启 / 关闭登录自启后的下次登录行为。

不以构建成功代替这些人工确认。当前已知外接键盘仍可能低频短暂失效；新包加入图标后的内存占用需单独采样，不宣称达到 15 MB。
