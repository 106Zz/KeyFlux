//! Caps 大小写与英文来源规则，不持有系统对象，也不恢复旧拼音。

use crate::domain::input_source::InputSource;
use crate::domain::caps_state::CapsObservation;
use std::time::{Duration, Instant};

pub const CAPS_SETTLE: Duration = Duration::from_millis(20);
const CAPS_TIMEOUT: Duration = Duration::from_millis(300);
const MAX_CAPS_WRITES: u8 = 4;
const MAX_CAPS_ATTEMPTS: u8 = 3;
const CAPS_RETRY_BACKOFF: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsSourceRequest {
    UppercaseEnglish,
    LowercaseEnglish,
}

impl CapsSourceRequest {
    pub fn from_enabled(enabled: bool) -> Self {
        if enabled { Self::UppercaseEnglish } else { Self::LowercaseEnglish }
    }

    /// Caps 开关都进入 ABC，大小写由独立的 Caps 状态决定。
    pub fn target_for(self, current_id: &str, abc: InputSource) -> Option<InputSource> {
        match self {
            Self::UppercaseEnglish | Self::LowercaseEnglish => {
                if current_id == abc.id { None } else { Some(abc) }
            }
        }
    }
}

/// HID、输入法与 WindowServer 的状态更新不在同一个回调中完成。
/// 只安排后续 RunLoop 确认，不在键盘回调中写入后立即回读。
pub struct CapsReconciler {
    pending: Option<CapsPending>,
}

struct CapsPending {
    desired: bool,
    next_at: Instant,
    deadline: Instant,
    writes: u8,
    total_writes: u8,
    attempt: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapsSyncAction {
    Idle,
    Wait(Duration),
    Write(bool),
    Confirmed(bool),
    Retry { desired: bool, attempt: u8, delay: Duration },
    Failed { desired: bool, actual: bool, writes: u8, attempts: u8 },
}

impl CapsReconciler {
    pub fn new() -> Self { Self { pending: None } }

    pub fn begin(&mut self, desired: bool, now: Instant) -> bool {
        // 普通键携带的旧 flags 不能无限延长本次确认截止时间。
        if self.pending.as_ref().is_some_and(|pending| pending.desired == desired) { return false; }
        self.pending = Some(CapsPending { desired, next_at: now + CAPS_SETTLE,
            deadline: now + CAPS_TIMEOUT, writes: 0, total_writes: 0, attempt: 1 });
        true
    }

    pub fn cancel(&mut self) { self.pending = None; }

    pub fn attempts(&self) -> u8 { self.pending.as_ref().map_or(0, |pending| pending.attempt) }

    pub fn delay(&self, now: Instant) -> Option<Duration> {
        self.pending.as_ref().map(|pending| pending.next_at.min(pending.deadline).saturating_duration_since(now))
    }

    pub fn advance(&mut self, now: Instant, observation: CapsObservation) -> CapsSyncAction {
        let Some(pending) = self.pending.as_mut() else { return CapsSyncAction::Idle; };
        if now < pending.next_at && now < pending.deadline { return CapsSyncAction::Wait(pending.next_at - now); }
        let desired = pending.desired;
        // 会话 flags 与 setter 实际写入的键盘服务属性必须一致。
        // 旧 IOHIDSystem eventFlags 不是 setter 的对称回读，仅保留用于诊断。
        let actual = observation.session_enabled;
        if actual == desired && observation.service_enabled == Some(desired) {
            self.pending = None;
            return CapsSyncAction::Confirmed(desired);
        }
        if now >= pending.deadline {
            if pending.attempt < MAX_CAPS_ATTEMPTS {
                let delay = CAPS_RETRY_BACKOFF * u32::from(pending.attempt);
                pending.attempt += 1;
                pending.writes = 0;
                pending.next_at = now + delay;
                pending.deadline = pending.next_at + CAPS_TIMEOUT;
                return CapsSyncAction::Retry { desired, attempt: pending.attempt, delay };
            }
            let writes = pending.total_writes;
            let attempts = pending.attempt;
            self.pending = None;
            return CapsSyncAction::Failed { desired, actual, writes, attempts };
        }
        if pending.writes >= MAX_CAPS_WRITES {
            // 写入次数耗尽后仍给系统留出已约定的确认窗口，不增加写入次数。
            pending.next_at = pending.deadline;
            return CapsSyncAction::Wait(pending.deadline - now);
        }
        pending.writes += 1;
        pending.total_writes += 1;
        pending.next_at = now + CAPS_SETTLE;
        CapsSyncAction::Write(desired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observed(enabled: bool) -> CapsObservation { CapsObservation { io_enabled: enabled, session_enabled: enabled, service_enabled: Some(enabled) } }

    #[test]
    fn short_external_caps_does_not_require_legacy_iohid_flags_to_change() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(true, now);
        // 原故障记录：旧 IO=false、会话=true。服务尚未开启时仍需要真实写入。
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation {
            io_enabled: false, session_enabled: true, service_enabled: Some(false),
        }), CapsSyncAction::Write(true));
        // 服务和会话确认以后，旧 IO=false 不应触发四次写入后误报失败。
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, CapsObservation {
            io_enabled: false, session_enabled: true, service_enabled: Some(true),
        }), CapsSyncAction::Confirmed(true));
        sync.begin(false, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation {
            io_enabled: true, session_enabled: false, service_enabled: Some(false),
        }), CapsSyncAction::Confirmed(false));
    }

    #[test]
    fn unknown_or_mixed_keyboard_services_cannot_be_confirmed_from_session_alone() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(false, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation {
            io_enabled: false, session_enabled: false, service_enabled: None,
        }), CapsSyncAction::Write(false));
    }
    #[test]
    fn caps_from_pinyin_enters_uppercase_then_lowercase_abc_without_restoring_pinyin() {
        use crate::domain::caps_state::{CapsInput, CapsState};
        let mut caps = CapsState::new();
        let down = CapsInput::Key { device: 1, down: true, initial: false };
        let up = CapsInput::Key { device: 1, down: false, initial: false };
        let enabled = caps.input(down, false).unwrap();
        assert!(enabled);
        let request = CapsSourceRequest::from_enabled(enabled);
        assert_eq!(request, CapsSourceRequest::UppercaseEnglish);
        let current = request.target_for("Pinyin", InputSource { id: "ABC".into() }).unwrap();
        assert_eq!(current.id, "ABC");
        caps.input(up, false);
        let enabled = caps.input(down, false).unwrap();
        assert!(!enabled);
        let request = CapsSourceRequest::from_enabled(enabled);
        assert_eq!(request, CapsSourceRequest::LowercaseEnglish);
        assert!(request.target_for(&current.id, InputSource { id: "ABC".into() }).is_none());
    }

    #[test]
    fn caps_requests_always_choose_abc_even_after_an_external_source_change() {
        for enabled in [true, false] {
            let request = CapsSourceRequest::from_enabled(enabled);
            for current in ["Pinyin", "Other"] {
                assert_eq!(request.target_for(current, InputSource { id: "ABC".into() }).unwrap().id, "ABC");
            }
        }
    }

    #[test]
    fn caps_requests_do_not_reselect_abc_when_only_case_changes() {
        for enabled in [false, true, false, true] {
            assert!(CapsSourceRequest::from_enabled(enabled).target_for("ABC", InputSource { id: "ABC".into() }).is_none());
        }
    }

    #[test]
    fn caps_write_and_confirmation_are_separate_ticks() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        assert_eq!(sync.advance(now, observed(false)), CapsSyncAction::Wait(CAPS_SETTLE));
        assert_eq!(sync.advance(now + CAPS_SETTLE, observed(false)), CapsSyncAction::Write(true));
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, observed(false)), CapsSyncAction::Write(true));
        assert_eq!(sync.advance(now + CAPS_SETTLE * 3, observed(true)), CapsSyncAction::Confirmed(true));
        assert_eq!(sync.delay(now), None);
    }

    #[test]
    fn repeated_old_flags_do_not_reset_the_retry_budget() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        for attempt in 1..=MAX_CAPS_WRITES {
            let at = now + CAPS_SETTLE * u32::from(attempt);
            sync.begin(true, at);
            assert_eq!(sync.advance(at, observed(false)), CapsSyncAction::Write(true));
        }
        assert_eq!(sync.advance(now + CAPS_SETTLE * 5, observed(false)), CapsSyncAction::Wait(CAPS_TIMEOUT - CAPS_SETTLE * 5));
        assert_eq!(sync.advance(now + CAPS_TIMEOUT, observed(false)), CapsSyncAction::Retry { desired: true, attempt: 2, delay: CAPS_RETRY_BACKOFF });
        assert!(!sync.begin(true, now + CAPS_TIMEOUT));
        assert_eq!(sync.attempts(), 2);
    }

    #[test]
    fn new_toggle_replaces_old_target_and_secure_input_cancels_it() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, observed(false)), CapsSyncAction::Write(true));
        sync.begin(false, now + CAPS_SETTLE);
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, observed(true)), CapsSyncAction::Write(false));
        sync.cancel();
        assert_eq!(sync.advance(now + CAPS_SETTLE * 3, observed(true)), CapsSyncAction::Idle);
    }

    #[test]
    fn late_run_loop_tick_does_not_write_past_deadline() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        assert_eq!(sync.advance(now + CAPS_TIMEOUT, observed(false)), CapsSyncAction::Retry { desired: true, attempt: 2, delay: CAPS_RETRY_BACKOFF });
    }

    #[test]
    fn lowercase_session_alone_cannot_skip_turning_off_service_caps() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(false, now);
        let real_log = CapsObservation { io_enabled: true, session_enabled: false, service_enabled: Some(true) };
        assert_eq!(sync.advance(now + CAPS_SETTLE, real_log), CapsSyncAction::Write(false));
        assert!(sync.delay(now + CAPS_SETTLE).is_some());
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, observed(false)), CapsSyncAction::Confirmed(false));
        assert_eq!(sync.delay(now), None);
        assert_eq!(sync.advance(now + CAPS_SETTLE * 5, real_log), CapsSyncAction::Idle);
    }

    #[test]
    fn uppercase_session_alone_cannot_skip_turning_on_service_caps() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation { io_enabled: false, session_enabled: true, service_enabled: Some(false) }), CapsSyncAction::Write(true));
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, observed(true)), CapsSyncAction::Confirmed(true));
    }

    #[test]
    fn matching_service_cannot_confirm_an_incorrect_input_session() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(true, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation { io_enabled: true, session_enabled: false, service_enabled: Some(true) }), CapsSyncAction::Write(true));
    }

    #[test]
    fn persistent_service_caps_cannot_be_reported_as_successful_lowercase() {
        let mut sync = CapsReconciler::new();
        let now = Instant::now();
        sync.begin(false, now);
        let mismatched = CapsObservation { io_enabled: true, session_enabled: false, service_enabled: Some(true) };
        for attempt in 1..=MAX_CAPS_WRITES {
            assert_eq!(sync.advance(now + CAPS_SETTLE * u32::from(attempt), mismatched), CapsSyncAction::Write(false));
        }
        assert_eq!(sync.advance(now + CAPS_SETTLE * 5, mismatched), CapsSyncAction::Wait(CAPS_TIMEOUT - CAPS_SETTLE * 5));
        assert_eq!(sync.advance(now + CAPS_TIMEOUT, mismatched), CapsSyncAction::Retry { desired: false, attempt: 2, delay: CAPS_RETRY_BACKOFF });
    }

    #[test]
    fn first_failed_round_keeps_monitoring_and_can_recover_after_backoff() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(true, now);
        let mismatch = CapsObservation { io_enabled: false, session_enabled: true, service_enabled: Some(false) };
        for tick in 1..=4 {
            assert_eq!(sync.advance(now + CAPS_SETTLE * tick, mismatch), CapsSyncAction::Write(true));
        }
        assert_eq!(sync.advance(now + CAPS_TIMEOUT, mismatch),
            CapsSyncAction::Retry { desired: true, attempt: 2, delay: CAPS_RETRY_BACKOFF });
        assert!(matches!(sync.advance(now + CAPS_TIMEOUT + CAPS_SETTLE, mismatch), CapsSyncAction::Wait(_)));
        assert_eq!(sync.advance(now + CAPS_TIMEOUT + CAPS_RETRY_BACKOFF, observed(true)), CapsSyncAction::Confirmed(true));
        assert_eq!(sync.delay(now), None);
    }

    #[test]
    fn persistent_failure_has_three_rounds_and_at_most_twelve_writes() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(true, now);
        let mut round_start = now;
        let mut writes = 0;
        for round in 1..=MAX_CAPS_ATTEMPTS {
            let first_delay = if round == 1 { CAPS_SETTLE } else { Duration::ZERO };
            for tick in 0..MAX_CAPS_WRITES {
                assert_eq!(sync.advance(round_start + first_delay + CAPS_SETTLE * u32::from(tick), observed(false)), CapsSyncAction::Write(true));
                writes += 1;
            }
            let deadline = round_start + CAPS_TIMEOUT;
            assert!(!sync.begin(true, deadline));
            let result = sync.advance(deadline, observed(false));
            if round < MAX_CAPS_ATTEMPTS {
                let delay = CAPS_RETRY_BACKOFF * u32::from(round);
                assert_eq!(result, CapsSyncAction::Retry { desired: true, attempt: round + 1, delay });
                round_start = deadline + delay;
            } else {
                assert_eq!(result, CapsSyncAction::Failed { desired: true, actual: false, writes: 12, attempts: 3 });
            }
        }
        assert_eq!(writes, 12);
        assert_eq!(sync.delay(now), None);
        assert_eq!(sync.advance(now + Duration::from_secs(10), observed(false)), CapsSyncAction::Idle);
    }

    #[test]
    fn new_target_and_cancel_invalidate_old_retry_backoff() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(true, now);
        assert!(matches!(sync.advance(now + CAPS_TIMEOUT, observed(false)), CapsSyncAction::Retry { .. }));
        let changed = now + CAPS_TIMEOUT + CAPS_SETTLE;
        assert!(sync.begin(false, changed));
        assert_eq!(sync.attempts(), 1);
        assert_eq!(sync.advance(changed + CAPS_SETTLE, observed(false)), CapsSyncAction::Confirmed(false));
        sync.begin(true, changed);
        sync.advance(changed + CAPS_TIMEOUT, observed(false));
        sync.cancel();
        assert_eq!(sync.advance(changed + Duration::from_secs(1), observed(false)), CapsSyncAction::Idle);
    }

    #[test]
    fn system_may_confirm_after_four_writes_without_a_fifth_write() {
        let now = Instant::now();
        let mut sync = CapsReconciler::new();
        sync.begin(true, now);
        for attempt in 1..=MAX_CAPS_WRITES {
            assert_eq!(sync.advance(now + CAPS_SETTLE * u32::from(attempt), observed(false)), CapsSyncAction::Write(true));
        }
        assert!(matches!(sync.advance(now + CAPS_SETTLE * 5, observed(false)), CapsSyncAction::Wait(_)));
        assert_eq!(sync.advance(now + CAPS_TIMEOUT, observed(true)), CapsSyncAction::Confirmed(true));
    }

    #[test]
    fn shift_after_leaving_uppercase_can_still_select_pinyin() {
        use crate::application::switch_control::{SwitchAction, SwitchController, SwitchOutcome, SELECTION_DELAY, CONFIRM_INTERVAL};
        use crate::domain::caps_state::CapsState;
        use crate::domain::input_source::SourceSnapshot;
        use crate::domain::shift_state::{ShiftAction, ShiftGestureStateMachine, ShiftKey};
        let now = Instant::now();
        let mut shift = ShiftGestureStateMachine::new();
        let mut caps = CapsState::new();
        caps.synchronize(true);
        shift.shift_changed(ShiftKey::Left, true, false, 0, caps.enabled());
        assert_eq!(shift.shift_changed(ShiftKey::Left, false, false, 0, caps.enabled()), ShiftAction::UseLowercaseEnglish);
        caps.synchronize(false);
        let mut sync = CapsReconciler::new();
        sync.begin(false, now);
        assert_eq!(sync.advance(now + CAPS_SETTLE, CapsObservation { io_enabled: true, session_enabled: false, service_enabled: Some(true) }), CapsSyncAction::Write(false));
        assert_eq!(sync.advance(now + CAPS_SETTLE * 2, observed(false)), CapsSyncAction::Confirmed(false));
        shift.shift_changed(ShiftKey::Left, true, false, 0, caps.enabled());
        assert_eq!(shift.shift_changed(ShiftKey::Left, false, false, 0, caps.enabled()), ShiftAction::ToggleInputSource);
        let mut switching = SwitchController::new();
        assert!(switching.begin("ABC".into(), InputSource { id: "Pinyin".into() }, now));
        assert!(matches!(switching.advance(now + SELECTION_DELAY, &SourceSnapshot { id: "ABC".into(), selected: true }), SwitchAction::Select(_)));
        assert!(matches!(switching.advance(now + SELECTION_DELAY + CONFIRM_INTERVAL, &SourceSnapshot { id: "Pinyin".into(), selected: true }), SwitchAction::Finish(SwitchOutcome::Confirmed)));
    }
}
