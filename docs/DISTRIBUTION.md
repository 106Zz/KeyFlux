# macOS App 与 DMG 发行

当前先使用 GUI 开发版测试，暂停制作新版 DMG。`python3 scripts/package_macos.py --app-only` 只生成项目内的开发 App，不生成 DMG、不安装、不启动；旧 DMG 和已安装 App 不会自动更新。当前启动 / 停止方式以 README 的控制窗口说明为准。

## 启动与权限身份

`KeyFlux.app/Contents/MacOS/KeyFlux` 是同一个 Rust 进程，没有 shell 启动器、Electron、WebView 或额外守护进程。`Info.plist` 保留固定 `CFBundleIdentifier=org.keyflux.KeyFlux`，将 `LSBackgroundOnly` 改为 false，由原生 AppKit 显示控制窗口。打开后为停止状态，只有用户点击“开始监听”才注册键盘相关来源。

直接从 Warp / RustRover 执行裸二进制时，macOS 可能把 TCC 授权归属到终端宿主。分发时让用户把完整 App 放进“应用程序”，双击 App，而不是执行内部文件。第一次仍需用户明确允许 KeyFlux 控制设备 / 监控输入；DMG、Bundle ID 和开源许可都不能绕过系统授权。

窗口显示三项独立权限查询值；缺权限时点击“开始监听”或“检查权限”才调用系统请求 API。启动失败时保留窗口并显示错误。授权提示由系统提供，程序不自动改动系统偏好。

启动时用 `~/Library/Caches/KeyFlux/instance.lock` 的系统文件锁保证同一用户只有一个新版本进程，进程退出后锁自动释放，文件保留不会阻止下次启动。App 的标准错误追加到 `~/Library/Logs/KeyFlux/KeyFlux.log`，默认不输出逐事件诊断。调试日志不记录输入文字，用户结束排查后应退出并正常重开。

## 本机打包

在项目目录运行：

```sh
python3 scripts/package_macos.py
```

要求 macOS、Xcode Command Line Tools、Rust 1.89+、Python 3.9+。使用标准库，无 Python 第三方依赖。脚本构建本机架构，Apple Silicon 生成 arm64 包，Intel 生成 x86_64 包，不把单架构产物宣称为 Universal。最低 macOS 版本从实际 Mach-O 的构建信息写入 Info.plist。

每次在 `dist/KeyFlux-版本-架构-时间-随机后缀/` 创建新产物，不删除旧文件、不改已安装 App、不自动挂载或启动。内容包括：

- `image/KeyFlux.app`：可以独立启动的后台 App。
- `image/Applications`：拖拽安装目标快捷方式。
- `image/安装说明.txt`：权限、系统键盘选项、退出和排查方法。
- `KeyFlux-版本-架构.dmg` 与对应 `.sha256`。
- App 内 `Contents/Resources/` 保留 MIT 和 ShiftIME 的第三方声明。

默认 ad-hoc 签名适合本地验证。脚本检查 App 签名和 DMG 校验值，但不将这视为权限归属、键盘兼容性或 15 MB 验收。

## 公开发行签名与公证

维护者需自行持有有效的 **Developer ID Application** 证书，并提前配置自己的 notarytool 钥匙串凭据 profile。凭据不写入仓库或脚本。已有证书和 profile 后运行：

```sh
python3 scripts/package_macos.py \
  --sign-identity 'Developer ID Application: YOUR NAME (TEAMID)' \
  --notary-profile 'YOUR_KEYCHAIN_PROFILE'
```

脚本启用 hardened runtime 与签名时间戳，先公证、staple App，再生成、签名、公证、staple DMG。也支持环境变量 `KEYFLUX_SIGN_IDENTITY` 和 `KEYFLUX_NOTARY_PROFILE`。公证提交是显式选择的发行步骤，默认本机构建不联网提交。

目前没有可用的开发者证书，故此次本机构建不执行公证。未公证包不应标记为已完成正式发行；互联网下载后的 Gatekeeper 行为需用正式签名包验证。只固定 Bundle ID、不使用稳定证书签名，更新后仍可能遇到 TCC 重新授权。

Fork 可通过 `--bundle-id` 指定自己的身份。正式发布后应保持项目的 Bundle ID 与签名团队稳定，并验证同身份升级的权限行为；不能保证系统永远不要求重新授权。

## 安装后的验收

先停止旧终端版本，再拖拽到应用程序并双击。只授权 KeyFlux，确认无须额外授权 Warp / RustRover。验证 Shift、中/英短按、连续输入、键盘灯与安全输入，以及活动监视器里的独立 KeyFlux 进程。

通过“停止监听”取消接管并保留窗口；“退出程序”、关闭窗口或 ⌘Q 会清理监听并退出。不写入开机启动项。调试启动使用 `open -a /Applications/KeyFlux.app --args --debug`，不直接执行包内二进制；先退出原进程，已有实例不会被重复启动替换为调试模式。

记录 RSS / CPU 时关闭逐事件日志；15 MB 目标尚未实测。此次只进行了源码修复、逻辑检查与打包，没有替用户安装、授权或启动新 App。
