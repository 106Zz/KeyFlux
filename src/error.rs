//! 项目统一错误类型，不依赖具体系统框架。

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    UnsupportedPlatform,
    MissingPermissions,
    NativeApi(&'static str),
    MonitorStopped(&'static str),
    InputSourcesUnavailable(&'static str),
    InputSourceSelection(i32),
    HidOperation { operation: &'static str, status: i32 },
    CapsSyncFailed { desired: bool, actual: bool, io_actual: bool, service_actual: Option<bool>, writes: u8, attempts: u8 },
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(formatter, "目前只计划支持 macOS"),
            Self::MissingPermissions => write!(
                formatter,
                "缺少权限，请在隐私与安全性中授权设备控制和数据访问（旧版为辅助功能）及输入监控，再重新打开 KeyFlux"
            ),
            Self::NativeApi(api) => write!(formatter, "系统 API 调用失败：{api}"),
            Self::MonitorStopped(reason) => write!(formatter, "键盘监听已停止：{reason}"),
            Self::InputSourcesUnavailable(reason) => write!(formatter, "输入来源不可用：{reason}"),
            Self::InputSourceSelection(status) => write!(formatter, "选择输入来源失败，OSStatus={status}"),
            Self::HidOperation { operation, status } => write!(formatter, "HID 调用失败：{operation}，status=0x{:08x}", *status as u32),
            Self::CapsSyncFailed { desired, actual, io_actual, service_actual, writes, attempts } => write!(formatter, "Caps 自动重试耗尽：确认轮次={attempts}，期望={desired}，会话实际={actual}，键盘服务={service_actual:?}，旧IOHIDSystem={io_actual}，累计写入次数={writes}"),
            Self::Io(error) => write!(formatter, "文件或进程初始化失败：{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self { Self::Io(error) }
}
