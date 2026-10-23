//! 与操作系统解耦的键盘事件，不携带文字内容或原生对象指针。

use crate::domain::shift_state::ShiftKey;

#[derive(Clone, Copy, Debug)]
pub enum KeyboardEventKind {
    ShiftChanged {
        key: ShiftKey,
        is_down: bool,
        has_other_modifiers: bool,
        pointer_count: u32,
    },
    KeyDown { key_code: u16 },
    KeyUp { key_code: u16 },
    CapsLockKeyChanged,
    OtherModifierChanged,
    MonitorReset,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyboardEvent {
    pub kind: KeyboardEventKind,
    pub caps_lock: bool,
    pub secure_input: bool,
    pub has_shortcut_modifiers: bool,
    pub target_pid: Option<i32>,
}

/// 应用层决定原始事件如何转发；原生事件的所有权仍留在系统适配层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventDisposition {
    Pass,
    DeferIfSwitching,
    DiscardShiftTap,
    FlushBeforeCurrent,
}
