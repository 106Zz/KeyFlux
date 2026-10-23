//! 应用层：组织启动与切换流程，不放原生 FFI 声明。

pub(crate) mod app;
pub(crate) mod switch_control;
pub(crate) mod caps_control;
