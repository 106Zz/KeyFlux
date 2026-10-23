//! 切换时序与确认策略。时间由调用方传入，测试不需要睡眠或切换真实输入法。

use std::time::{Duration, Instant};

use crate::domain::input_source::{InputSource, SourceSnapshot};

pub const SELECTION_DELAY: Duration = Duration::from_millis(50);
pub const CONFIRM_INTERVAL: Duration = Duration::from_millis(20);
pub const SWITCH_TIMEOUT: Duration = Duration::from_millis(600);
const POLLS_PER_ATTEMPT: u8 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPhase {
    Idle,
    Selecting,
    Confirming,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchOutcome {
    Confirmed,
    TimedOut,
    SourceChanged,
    SelectionFailed,
    Cancelled,
}

#[derive(Debug)]
pub enum SwitchAction {
    Wait(Duration),
    Select(InputSource),
    Finish(SwitchOutcome),
}

pub struct SwitchController {
    phase: SwitchPhase,
    pending: Option<PendingSwitch>,
}

struct PendingSwitch {
    origin: String,
    target: InputSource,
    next_at: Instant,
    deadline: Instant,
    polls: u8,
    reselections: u8,
}

impl SwitchController {
    pub fn new() -> Self {
        Self {
            phase: SwitchPhase::Idle,
            pending: None,
        }
    }

    pub fn begin(&mut self, origin: String, target: InputSource, now: Instant) -> bool {
        if self.is_active() || origin == target.id {
            return false;
        }
        self.pending = Some(PendingSwitch {
            origin,
            target,
            next_at: now + SELECTION_DELAY,
            deadline: now + SWITCH_TIMEOUT,
            polls: 0,
            reselections: 2,
        });
        self.phase = SwitchPhase::Selecting;
        true
    }

    pub fn is_active(&self) -> bool {
        self.pending.is_some()
    }

    pub fn phase(&self) -> SwitchPhase { self.phase }

    /// 通知若显示当前来源已经变成第三方选择，不允许旧任务继续抢回来源。
    pub fn notification_conflicts(&self, current: &SourceSnapshot) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            // 来源通知不携带本次操作编号，查询仍读到 origin 可能只是系统尚未激活目标。
            // 只有第三个来源能确定是外部选择；旧 origin 交给有限确认 / 超时处理。
            current.id != pending.target.id && current.id != pending.origin
        })
    }

    pub fn advance(&mut self, now: Instant, current: &SourceSnapshot) -> SwitchAction {
        let Some(pending) = self.pending.as_mut() else {
            return SwitchAction::Finish(SwitchOutcome::Cancelled);
        };
        if now >= pending.deadline {
            return self.finish(SwitchOutcome::TimedOut);
        }
        if current.id == pending.target.id && current.selected {
            return self.finish(SwitchOutcome::Confirmed);
        }
        if current.id != pending.origin && current.id != pending.target.id {
            return self.finish(SwitchOutcome::SourceChanged);
        }
        if current.id == pending.target.id {
            // 即使尚未发起首次选择，外部已激活目标来源时也不能重选它。
            self.phase = SwitchPhase::Confirming;
            pending.next_at = now + CONFIRM_INTERVAL;
            return SwitchAction::Wait(CONFIRM_INTERVAL);
        }
        if now < pending.next_at {
            return SwitchAction::Wait(pending.next_at - now);
        }

        if self.phase == SwitchPhase::Selecting {
            self.phase = SwitchPhase::Confirming;
            pending.next_at = now + CONFIRM_INTERVAL;
            return SwitchAction::Select(pending.target.clone());
        }

        // 目标 ID 已经正确但仍在激活时，只等候，不重复选择正在激活的拼音。
        pending.polls += 1;
        if current.id != pending.target.id && pending.polls >= POLLS_PER_ATTEMPT {
            if pending.reselections == 0 {
                return self.finish(SwitchOutcome::SelectionFailed);
            }
            pending.reselections -= 1;
            pending.polls = 0;
            pending.next_at = now + CONFIRM_INTERVAL;
            return SwitchAction::Select(pending.target.clone());
        }
        pending.next_at = now + CONFIRM_INTERVAL;
        SwitchAction::Wait(CONFIRM_INTERVAL)
    }

    pub fn cancel(&mut self) {
        self.finish(SwitchOutcome::Cancelled);
    }

    fn finish(&mut self, outcome: SwitchOutcome) -> SwitchAction {
        self.pending = None;
        self.phase = if outcome == SwitchOutcome::Confirmed {
            SwitchPhase::Completed
        } else {
            SwitchPhase::Failed
        };
        SwitchAction::Finish(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, selected: bool) -> SourceSnapshot {
        SourceSnapshot { id: id.into(), selected }
    }

    fn begin(now: Instant) -> SwitchController {
        let mut controller = SwitchController::new();
        assert!(controller.begin("ABC".into(), InputSource { id: "Pinyin".into() }, now));
        controller
    }

    #[test]
    fn shift_release_gets_time_to_reach_old_source() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(matches!(controller.advance(now, &snapshot("ABC", true)), SwitchAction::Wait(_)));
        assert!(matches!(controller.advance(now + SELECTION_DELAY, &snapshot("ABC", true)), SwitchAction::Select(_)));
    }

    #[test]
    fn current_target_still_activating_is_never_reselected() {
        let now = Instant::now();
        let mut controller = begin(now);
        controller.advance(now + SELECTION_DELAY, &snapshot("ABC", true));
        for tick in 1..=20 {
            assert!(matches!(controller.advance(now + SELECTION_DELAY + CONFIRM_INTERVAL * tick, &snapshot("Pinyin", false)), SwitchAction::Wait(_)));
        }
    }

    #[test]
    fn timeout_releases_operation_and_stale_tick_cannot_select() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(matches!(controller.advance(now + SWITCH_TIMEOUT, &snapshot("ABC", true)), SwitchAction::Finish(SwitchOutcome::TimedOut)));
        assert!(!controller.is_active());
        assert!(matches!(controller.advance(now + SWITCH_TIMEOUT, &snapshot("ABC", true)), SwitchAction::Finish(SwitchOutcome::Cancelled)));
    }

    #[test]
    fn duplicate_request_is_coalesced_and_external_source_cancels() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(!controller.begin("ABC".into(), InputSource { id: "Pinyin".into() }, now));
        assert!(controller.notification_conflicts(&snapshot("Other", true)));
        assert!(matches!(controller.advance(now, &snapshot("Other", true)), SwitchAction::Finish(SwitchOutcome::SourceChanged)));
    }

    #[test]
    fn only_matching_selected_target_confirms() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(matches!(controller.advance(now, &snapshot("Pinyin", true)), SwitchAction::Finish(SwitchOutcome::Confirmed)));
        assert!(!controller.is_active());
    }

    #[test]
    fn externally_activating_target_before_first_selection_is_not_reselected() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(matches!(controller.advance(now + SELECTION_DELAY, &snapshot("Pinyin", false)), SwitchAction::Wait(_)));
    }

    #[test]
    fn failed_selection_has_bounded_retry_count() {
        let now = Instant::now();
        let mut controller = begin(now);
        let mut selects = 0;
        for tick in 0..=30 {
            if matches!(controller.advance(now + SELECTION_DELAY + CONFIRM_INTERVAL * tick, &snapshot("ABC", true)), SwitchAction::Select(_)) {
                selects += 1;
            }
        }
        assert_eq!(selects, 3);
        assert!(!controller.is_active());
    }

    #[test]
    fn wakeup_from_cancelled_operation_cannot_select_new_target_too_early() {
        let now = Instant::now();
        let mut controller = begin(now);
        controller.cancel();
        assert!(controller.begin("Pinyin".into(), InputSource { id: "ABC".into() }, now + CONFIRM_INTERVAL));
        assert!(matches!(controller.advance(now + SELECTION_DELAY, &snapshot("Pinyin", true)), SwitchAction::Wait(_)));
    }

    #[test]
    fn old_origin_notification_after_select_does_not_cancel_reverse_switch() {
        let now = Instant::now();
        let mut controller = begin(now);
        assert!(matches!(controller.advance(now + SELECTION_DELAY, &snapshot("ABC", true)), SwitchAction::Select(_)));
        assert!(!controller.notification_conflicts(&snapshot("ABC", true)));
        assert!(matches!(controller.advance(now + SELECTION_DELAY + CONFIRM_INTERVAL, &snapshot("ABC", true)), SwitchAction::Wait(_)));
        assert!(matches!(controller.advance(now + SELECTION_DELAY + CONFIRM_INTERVAL * 2, &snapshot("Pinyin", true)), SwitchAction::Finish(SwitchOutcome::Confirmed)));
    }
}
