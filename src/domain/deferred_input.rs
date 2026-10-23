//! 有界输入暂存策略，泛型事件载荷让测试无需创建或发送真实键盘事件。

use crate::domain::shift_state::ShiftKey;

pub const MAX_DEFERRED_EVENTS: usize = 128;

#[derive(Clone, Copy, Debug)]
pub struct DeferredKey {
    pub code: u16,
    pub is_up: bool,
    pub shift: Option<ShiftKey>,
}

pub struct DeferredInput<T> {
    active: bool,
    events: Vec<(DeferredKey, T)>,
    held_keys: [u64; 4],
    shift_start: Option<usize>,
    held_shifts: u8,
}

impl<T> DeferredInput<T> {
    pub fn new() -> Self {
        Self { active: false, events: Vec::new(), held_keys: [0; 4], shift_start: None, held_shifts: 0 }
    }

    pub fn begin(&mut self) {
        self.active = true;
    }

    pub fn should_hold(&self, key: DeferredKey, shortcut: bool) -> bool {
        if !self.active || key.code >= 256 {
            return false;
        }
        if key.is_up {
            self.held(key.code)
        } else {
            !shortcut
        }
    }

    pub fn is_full(&self) -> bool {
        self.events.len() >= MAX_DEFERRED_EVENTS
    }

    pub fn push(&mut self, key: DeferredKey, event: T) -> std::result::Result<(), T> {
        if !self.active || self.is_full() || key.code >= 256 {
            return Err(event);
        }
        if key.shift.is_some() && !key.is_up && self.held_shifts == 0 {
            self.shift_start = Some(self.events.len());
        }
        let bits = &mut self.held_keys[usize::from(key.code / 64)];
        let mask = 1_u64 << (key.code % 64);
        if key.is_up { *bits &= !mask; } else { *bits |= mask; }
        if let Some(shift) = key.shift {
            let bit = match shift { ShiftKey::Left => 1, ShiftKey::Right => 2 };
            if key.is_up { self.held_shifts &= !bit; } else { self.held_shifts |= bit; }
        }
        self.events.push((key, event));
        if key.shift.is_some() && key.is_up && self.held_shifts == 0 {
            self.shift_start = None;
        }
        Ok(())
    }

    /// 合并切换期间第二次单按 Shift，防止补发后被拼音当作内部中英切换。
    /// 调用时最后一次 Shift-up 还没有入队。
    pub fn discard_shift_tap(&mut self) -> bool {
        let Some(start) = self.shift_start else { return false; };
        if !self.events[start..].iter().all(|(key, _)| key.shift.is_some()) {
            return false;
        }
        for (key, _) in &self.events[start..] {
            self.held_keys[usize::from(key.code / 64)] &= !(1_u64 << (key.code % 64));
        }
        self.events.truncate(start);
        self.held_shifts = 0;
        self.shift_start = None;
        true
    }

    pub fn take(&mut self) -> Vec<T> {
        self.active = false;
        self.held_keys = [0; 4];
        self.shift_start = None;
        self.held_shifts = 0;
        std::mem::take(&mut self.events).into_iter().map(|(_, event)| event).collect()
    }

    fn held(&self, code: u16) -> bool {
        self.held_keys[usize::from(code / 64)] & (1_u64 << (code % 64)) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: u16, is_up: bool) -> DeferredKey {
        DeferredKey { code, is_up, shift: match code { 56 => Some(ShiftKey::Left), 60 => Some(ShiftKey::Right), _ => None } }
    }

    #[test]
    fn releases_original_order_and_does_not_hold_unmatched_key_up() {
        let mut buffer = DeferredInput::new();
        buffer.begin();
        assert!(!buffer.should_hold(key(0, true), false));
        buffer.push(key(0, false), 1).unwrap();
        assert!(buffer.should_hold(key(0, true), true));
        buffer.push(key(0, true), 2).unwrap();
        assert_eq!(buffer.take(), vec![1, 2]);
        assert!(!buffer.should_hold(key(0, false), false));
    }

    #[test]
    fn cap_is_enforced_and_rejected_payload_is_returned() {
        let mut buffer = DeferredInput::new();
        buffer.begin();
        for n in 0..MAX_DEFERRED_EVENTS { buffer.push(key(0, false), n).unwrap(); }
        assert!(buffer.is_full());
        assert_eq!(buffer.push(key(0, true), 999), Err(999));
        assert_eq!(buffer.take().len(), MAX_DEFERRED_EVENTS);
    }

    #[test]
    fn coalescing_bare_shift_preserves_previous_letter_shift_release() {
        let mut buffer = DeferredInput::new();
        buffer.begin();
        for (event, id) in [(key(56, false), 1), (key(0, false), 2), (key(0, true), 3), (key(56, true), 4), (key(60, false), 5)] {
            buffer.push(event, id).unwrap();
        }
        assert!(buffer.discard_shift_tap());
        assert_eq!(buffer.take(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn shift_used_with_letter_cannot_be_discarded() {
        let mut buffer = DeferredInput::new();
        buffer.begin();
        buffer.push(key(56, false), 1).unwrap();
        buffer.push(key(0, false), 2).unwrap();
        assert!(!buffer.discard_shift_tap());
        assert!(!buffer.should_hold(key(4, false), true));
    }

    #[test]
    fn releasing_mid_gesture_cannot_leave_shift_held_in_next_operation() {
        let mut buffer = DeferredInput::new();
        buffer.begin();
        buffer.push(key(56, false), 1).unwrap();
        assert_eq!(buffer.take(), vec![1]);
        buffer.begin();
        buffer.push(key(60, false), 2).unwrap();
        assert!(buffer.discard_shift_tap());
        assert!(buffer.take().is_empty());
    }
}
