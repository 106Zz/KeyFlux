# 开发指南

KeyFlux 是一个 Rust 单二进制项目，系统适配基于 macOS 原生 API。用户安装和功能说明见 [README](../README.md)，App / DMG 构建见 [打包与发行](DISTRIBUTION.md)。

## 开发环境

- macOS。
- Xcode Command Line Tools。
- Rust 1.89+。
- Python 3.9+，用于打包和可选的性能诊断。

在项目根目录构建：

```sh
cargo build --release
```

运行开发二进制：

```sh
./target/release/KeyFlux
```

直接运行二进制时，系统权限可能归属于终端宿主。验证 App 的独立权限身份时，应生成开发 App，并通过 Finder 打开完整的 `KeyFlux.app`：

```sh
python3 scripts/package_macos.py --app-only
```

启动时使用 `--paused` 可保留菜单栏并暂停键盘切换。默认启动会在权限齐全时启用键盘切换。

## 项目结构

| 路径 | 职责 |
| --- | --- |
| `src/main.rs` | 进程入口与退出码 |
| `src/bootstrap.rs` | 对象创建和依赖组装 |
| `src/application/` | 输入切换流程与生命周期协调 |
| `src/domain/` | 按键规则、状态机与有界输入暂存 |
| `src/presentation/` | 菜单操作与恢复流程 |
| `src/infrastructure/macos/` | 键盘事件、输入法、权限和原生资源管理 |
| `packaging/macos/` | App 配置、图标与安装说明 |
| `scripts/` | 打包和开发诊断工具 |

领域规则不依赖 macOS 原生对象。FFI、`unsafe` 与原生引用管理集中在系统适配目录；应用层通过显式对象组装协调各模块。

## 开发工具

| 脚本 | 用途 | 安装用户是否需要 |
| --- | --- | --- |
| `scripts/package_macos.py` | 构建 App / DMG，生成图标及校验文件，支持可选签名和公证 | 否 |
| `scripts/monitor_keyflux.py` | 对已运行的开发二进制进行只读性能采样 | 否 |
| `scripts/profile_memory.py` | 按人工操作阶段采集系统内存分类快照 | 否 |

这些脚本作为源码维护工具保留在 Git 仓库中，不进入 App 或 DMG 的安装内容。Rust 源码中的 `#[cfg(test)]` 测试模块也不参与普通 Release 构建。

### 性能采样

先确认开发二进制已运行、键盘切换已启用，并关闭详细诊断及分配跟踪。将基础运行日志保存在采样器默认识别的项目路径：

```sh
./target/release/KeyFlux >> target/keyflux-runtime.log 2>&1
```

若进程已经运行，不要重复启动；现有日志使用其他路径时，可通过 `--runtime-log` 指定。

在另一个终端执行：

```sh
python3 -B scripts/monitor_keyflux.py --interval 30 --hours 24 --follow-restarts --tag normal-day
```

采样器默认每 30 秒记录一次，最长 24 小时，前 120 秒标记为预热。`--follow-restarts` 只等待并附加同一路径的新进程，不会启动或重启 KeyFlux。达到时长或按 Ctrl+C 只结束采样，保留目标进程；当前不支持无限时长采样。

输出位于 `target/performance/`：原始样本、控制事件、小时汇总及摘要。RSS、物理 Footprint 与区间 CPU 分开记录，睡眠或长采样空档不视为连续活跃使用。日志状态不能替代实际输入效果确认，单次内存增长也不能直接判定泄漏。

采样器默认附加本项目的 `target/release/KeyFlux`，不自动匹配安装版 App 的可执行路径。

### 内存分类快照

```sh
python3 scripts/profile_memory.py --help
```

该工具按交互提示采集暂停、监听和操作后的快照。输出位于 `target/memory-profile/`；使用同一进程比较不同阶段。诊断工具可能短暂影响响应，其结果用于定位，不作为普通运行的资源指标。

## 日志

安装版基础日志位于 `~/Library/Logs/KeyFlux/KeyFlux.log`。开发二进制可以将标准错误重定向至项目 `target/`。

需要详细诊断时，先退出当前进程，再以 `KEYFLUX_DEBUG=1` 启动开发二进制，或通过 Launch Services 启动 App：

```sh
open -a /Applications/KeyFlux.app --args --debug
```

结束诊断后退出并普通启动。详细日志记录控制状态和输入来源，不记录输入文字；发布日志前检查个人路径和设备信息。

## 源码与发行产物

- Git 仓库保留源码、开发工具、图标、构建配置及许可文件。
- GitHub Release 提供最终 DMG 和对应 SHA-256 文件。
- `target/`、`dist/` 与本机性能日志不进入源码提交。
- App / DMG 的安装内容由打包脚本显式组装，不复制开发脚本或测试源文件。
