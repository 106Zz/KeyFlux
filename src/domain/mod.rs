//! 领域层：输入来源模型与纯按键规则，不依赖应用层或 macOS 系统适配器。

pub(crate) mod input_source;
pub(crate) mod deferred_input;
pub(crate) mod keyboard;
pub(crate) mod shift_state;
pub(crate) mod caps_state;
