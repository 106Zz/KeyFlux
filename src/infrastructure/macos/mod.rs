//! macOS 原生适配器。后续 FFI、unsafe 与原生引用管理限定在此目录。

pub(crate) mod caps_lock;
pub(crate) mod event_tap;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod hid_ffi;
pub(crate) mod caps_input;
pub(crate) mod input_source;
pub(crate) mod permissions;
pub(crate) mod run_loop;
pub(crate) mod timer;
pub(crate) mod launch;
pub(crate) mod control_window;
pub(crate) mod diagnostics;
