//! 纯 Rust Shift 手势规则，不调用 macOS API。
//!
//! 参考 ShiftIME 的手势识别设计，许可说明见 docs/THIRD_PARTY_NOTICES.md。

#[derive(Clone, Copy, Debug)]
pub enum ShiftKey {
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(state: &mut ShiftGestureStateMachine, key: ShiftKey) -> ShiftAction {
        state.shift_changed(key, true, false, 0, false)
    }

    fn release(state: &mut ShiftGestureStateMachine, key: ShiftKey) -> ShiftAction {
        state.shift_changed(key, false, false, 0, false)
    }

    #[test]
    fn overlapping_left_and_right_shift_toggle_only_once() {
        let mut state = ShiftGestureStateMachine::new();
        press(&mut state, ShiftKey::Left);
        press(&mut state, ShiftKey::Right);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::Pass);
        assert_eq!(release(&mut state, ShiftKey::Right), ShiftAction::ToggleInputSource);
        assert_eq!(release(&mut state, ShiftKey::Right), ShiftAction::Pass);
    }

    #[test]
    fn letter_before_shift_and_letter_during_shift_do_not_toggle() {
        let mut state = ShiftGestureStateMachine::new();
        state.key_down(0);
        press(&mut state, ShiftKey::Left);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::Pass);
        state.key_up(0);
        press(&mut state, ShiftKey::Left);
        state.key_down(0);
        state.key_up(0);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::Pass);
        press(&mut state, ShiftKey::Left);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::ToggleInputSource);
    }

    #[test]
    fn repeated_press_cannot_erase_combination_or_modifier_use() {
        let mut state = ShiftGestureStateMachine::new();
        press(&mut state, ShiftKey::Left);
        state.other_modifier_changed();
        press(&mut state, ShiftKey::Left);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::Pass);
    }

    #[test]
    fn mouse_activity_suppresses_toggle_but_caps_shift_returns_to_lowercase() {
        let mut state = ShiftGestureStateMachine::new();
        press(&mut state, ShiftKey::Left);
        assert_eq!(state.shift_changed(ShiftKey::Left, false, false, 1, false), ShiftAction::Pass);
        state.shift_changed(ShiftKey::Right, true, false, 1, true);
        assert_eq!(state.shift_changed(ShiftKey::Right, false, false, 1, true), ShiftAction::UseLowercaseEnglish);
    }

    #[test]
    fn caps_shift_letter_is_not_a_bare_shift_and_next_bare_shift_clears_caps() {
        let mut state = ShiftGestureStateMachine::new();
        state.shift_changed(ShiftKey::Left, true, false, 0, true);
        state.key_down(0);
        state.key_up(0);
        assert_eq!(state.shift_changed(ShiftKey::Left, false, false, 0, true), ShiftAction::Pass);
        state.shift_changed(ShiftKey::Left, true, false, 0, true);
        assert_eq!(state.shift_changed(ShiftKey::Left, false, false, 0, true), ShiftAction::UseLowercaseEnglish);
        // 下一次 Shift 才回到普通中英切换，不能在关闭大写的同一次手势中切中文。
        press(&mut state, ShiftKey::Left);
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::ToggleInputSource);
    }

    #[test]
    fn overlapping_shifts_only_request_lowercase_after_the_last_release() {
        let mut state = ShiftGestureStateMachine::new();
        state.shift_changed(ShiftKey::Left, true, false, 0, true);
        state.shift_changed(ShiftKey::Right, true, false, 0, true);
        assert_eq!(state.shift_changed(ShiftKey::Left, false, false, 0, true), ShiftAction::Pass);
        assert_eq!(state.shift_changed(ShiftKey::Right, false, false, 0, true), ShiftAction::UseLowercaseEnglish);
    }

    #[test]
    fn reset_invalidates_unfinished_gesture() {
        let mut state = ShiftGestureStateMachine::new();
        press(&mut state, ShiftKey::Left);
        state.reset();
        assert_eq!(release(&mut state, ShiftKey::Left), ShiftAction::Pass);
    }
}

#[derive(Default)]
pub struct ShiftGestureStateMachine {
    left_pressed: bool,
    right_pressed: bool,
    used_with_other_input: bool,
    pointer_count_at_press: u32,
    /// 固定大小位图，记录 Shift 按下之前已经按住的普通键。
    pressed_keys: [u64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftAction {
    Pass,
    ToggleInputSource,
    UseLowercaseEnglish,
}

impl ShiftGestureStateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn shift_changed(
        &mut self,
        key: ShiftKey,
        is_down: bool,
        has_other_modifiers: bool,
        pointer_count: u32,
        caps_lock: bool,
    ) -> ShiftAction {
        let was_down = match key {
            ShiftKey::Left => self.left_pressed,
            ShiftKey::Right => self.right_pressed,
        };

        if is_down {
            // 重复的按下事件不能抹掉本次已经参与组合输入的记录。
            if was_down {
                self.used_with_other_input |= has_other_modifiers;
                return ShiftAction::Pass;
            }
            if !self.is_pressed() {
                self.pointer_count_at_press = pointer_count;
                self.used_with_other_input = self.pressed_keys.iter().any(|bits| *bits != 0);
            }
            self.set_pressed(key, true);
            self.used_with_other_input |= has_other_modifiers;
            return ShiftAction::Pass;
        }

        // 没观察到按下的松开事件，不构成一次完整手势。
        if !was_down {
            return ShiftAction::Pass;
        }
        self.set_pressed(key, false);
        self.used_with_other_input |= has_other_modifiers
            || pointer_count != self.pointer_count_at_press;

        if self.is_pressed() {
            return ShiftAction::Pass;
        }

        let should_toggle = !self.used_with_other_input;
        self.used_with_other_input = false;
        if should_toggle && caps_lock {
            ShiftAction::UseLowercaseEnglish
        } else if should_toggle {
            ShiftAction::ToggleInputSource
        } else {
            ShiftAction::Pass
        }
    }

    pub fn key_down(&mut self, key_code: u16) {
        if self.is_pressed() {
            self.used_with_other_input = true;
        }
        if let Some(bits) = self.pressed_keys.get_mut(usize::from(key_code / 64)) {
            *bits |= 1_u64 << (key_code % 64);
        }
    }

    pub fn key_up(&mut self, key_code: u16) {
        // 即使按下事件丢失，Shift 期间收到普通键松开也不能算单按。
        if self.is_pressed() {
            self.used_with_other_input = true;
        }
        if let Some(bits) = self.pressed_keys.get_mut(usize::from(key_code / 64)) {
            *bits &= !(1_u64 << (key_code % 64));
        }
    }

    pub fn other_modifier_changed(&mut self) {
        if self.is_pressed() {
            self.used_with_other_input = true;
        }
    }

    fn is_pressed(&self) -> bool {
        self.left_pressed || self.right_pressed
    }

    fn set_pressed(&mut self, key: ShiftKey, pressed: bool) {
        match key {
            ShiftKey::Left => self.left_pressed = pressed,
            ShiftKey::Right => self.right_pressed = pressed,
        }
    }
}
