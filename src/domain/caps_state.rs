//! 物理 Caps 按压规则：每个设备一次按下只翻转一次，不受系统重复报告影响。

#[derive(Clone, Copy, Debug)]
pub enum CapsInput {
    Key { device: usize, down: bool, initial: bool },
    Removed { device: usize },
}

/// 服务属性是 IOHIDSetModifierLockState 实际写入的状态；旧 IOHIDSystem
/// 读数仅用于诊断。会话 flags 可被事件重写，不能单独作为确认依据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapsObservation {
    pub io_enabled: bool,
    pub session_enabled: bool,
    /// 所有键盘服务一致时为 Some；未读到、类型不符或服务间不一致时为 None。
    pub service_enabled: Option<bool>,
}

pub struct CapsState {
    enabled: bool,
    pressed_devices: Vec<usize>,
}

impl CapsState {
    pub fn new() -> Self { Self { enabled: false, pressed_devices: Vec::new() } }

    pub fn enabled(&self) -> bool { self.enabled }

    pub fn is_pressed(&self) -> bool { !self.pressed_devices.is_empty() }

    /// 退出安全输入或快捷键透传时继承真实系统状态，保留未松开的物理键记录。
    pub fn synchronize(&mut self, enabled: bool) { self.enabled = enabled; }

    pub fn input(&mut self, input: CapsInput, blocked: bool) -> Option<bool> {
        match input {
            CapsInput::Removed { device } => {
                self.pressed_devices.retain(|pressed| *pressed != device);
                None
            }
            CapsInput::Key { device, down: false, .. } => {
                self.pressed_devices.retain(|pressed| *pressed != device);
                None
            }
            CapsInput::Key { device, down: true, initial } => {
                if self.pressed_devices.contains(&device) { return None; }
                self.pressed_devices.push(device);
                if blocked || initial { return None; }
                self.enabled = !self.enabled;
                Some(self.enabled)
            }
        }
    }
}

/// 共享给原生事件适配器的输出策略，None 表示安全输入 / 组合键期间原样透传。
#[derive(Clone, Copy, Default)]
pub struct CapsPresentation {
    pub enabled: Option<bool>,
    pub abc_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(device: usize, down: bool) -> CapsInput { CapsInput::Key { device, down, initial: false } }

    #[test]
    fn long_press_and_repeated_reports_only_toggle_once() {
        let mut state = CapsState::new();
        assert_eq!(state.input(key(1, true), false), Some(true));
        for _ in 0..20 { assert_eq!(state.input(key(1, true), false), None); }
        assert_eq!(state.input(key(1, false), false), None);
        assert_eq!(state.input(key(1, true), false), Some(false));
    }

    #[test]
    fn overlapping_keyboards_and_device_removal_do_not_leave_a_stuck_press() {
        let mut state = CapsState::new();
        assert_eq!(state.input(key(1, true), false), Some(true));
        assert_eq!(state.input(key(2, true), false), Some(false));
        state.input(CapsInput::Removed { device: 1 }, false);
        assert_eq!(state.input(key(1, true), false), Some(true));
    }

    #[test]
    fn secure_or_initial_press_must_be_released_before_it_can_toggle() {
        let mut state = CapsState::new();
        assert_eq!(state.input(key(1, true), true), None);
        state.synchronize(true);
        assert_eq!(state.input(key(1, true), false), None);
        state.input(key(1, false), false);
        assert_eq!(state.input(key(1, true), false), Some(false));
        assert_eq!(state.input(CapsInput::Key { device: 2, down: true, initial: true }, false), None);
    }
}
