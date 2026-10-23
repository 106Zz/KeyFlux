//! 依赖组装入口：显式创建各模块，再交给应用层启动。

use crate::application::app::KeyFlux;
use crate::application::switch_control::SwitchController;
use crate::domain::shift_state::ShiftGestureStateMachine;
use crate::error::Result;
use crate::infrastructure::macos::caps_lock::CapsLockController;
use crate::infrastructure::macos::event_tap::EventTap;
use crate::infrastructure::macos::input_source::InputSourceManager;
use crate::infrastructure::macos::launch::LaunchGuard;

pub(crate) fn run() -> Result<()> {
    if !cfg!(target_os = "macos") { return Err(crate::error::Error::UnsupportedPlatform); }
    let Some(_launch) = LaunchGuard::acquire()? else { return Ok(()); };
    let keyboard = EventTap::new();
    let shift = ShiftGestureStateMachine::new();
    let caps_lock = CapsLockController::new();
    let input_sources = InputSourceManager::new();
    let switching = SwitchController::new();

    let engine = KeyFlux::new(keyboard, shift, caps_lock, input_sources, switching);
    crate::presentation::control_panel::run(engine)
}
