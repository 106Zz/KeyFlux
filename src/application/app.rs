//! 应用生命周期与模块协调；依赖由 bootstrap 显式传入。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use crate::application::switch_control::{SwitchAction, SwitchController, SwitchOutcome, CONFIRM_INTERVAL, SELECTION_DELAY};
use crate::application::caps_control::{CapsReconciler, CapsSourceRequest, CapsSyncAction};
use crate::domain::caps_state::{CapsInput, CapsPresentation, CapsState};
use crate::domain::deferred_input::DeferredInput;
use crate::domain::input_source::SourceSnapshot;
use crate::domain::keyboard::{EventDisposition, KeyboardEvent, KeyboardEventKind};
use crate::domain::shift_state::{ShiftAction, ShiftGestureStateMachine};
use crate::error::{Error, Result};
use crate::infrastructure::macos::caps_lock::{self, CapsLockController, CapsOutput};
use crate::infrastructure::macos::caps_input::CapsInputMonitor;
use crate::infrastructure::macos::event_tap::{self, EventTap, InputBuffer};
use crate::infrastructure::macos::input_source::{InputSourceManager, SourceObserver};
use crate::infrastructure::macos::permissions::PermissionStatus;
use crate::infrastructure::macos::run_loop;
use crate::infrastructure::macos::timer::{Timer, Wakeup};
use crate::infrastructure::macos::diagnostics::record;

pub(crate) struct KeyFlux {
    keyboard: EventTap,
    processor: Rc<RefCell<KeyboardProcessor>>,
    session: Option<MonitoringSession>,
}

/// 生命周期由控制窗口管理；停止时立即注销全部原生回调。
struct MonitoringSession {
    timer: Timer,
    caps_timer: Timer,
    observer: SourceObserver,
    caps_monitor: CapsInputMonitor,
}

/// 由单个事件循环访问；共享所有权用于回调生命周期，不引入线程或锁。
struct KeyboardProcessor {
    shift: ShiftGestureStateMachine,
    caps_lock: CapsLockController,
    input_sources: InputSourceManager,
    switching: SwitchController,
    current_source: Option<SourceSnapshot>,
    source_dirty: Rc<Cell<bool>>,
    buffer: InputBuffer,
    wakeup: Option<Wakeup>,
    was_secure_input: bool,
    debug: bool,
    caps_state: CapsState,
    caps_output: CapsOutput,
    caps_source_request: Option<CapsSourceRequest>,
    caps_passthrough: bool,
    caps_switch_active: bool,
    caps_sync: CapsReconciler,
    caps_wakeup: Option<Wakeup>,
    fatal: Option<Error>,
    caps_trace: [Option<CapsTrace>; 24],
    caps_trace_next: usize,
}

#[derive(Clone, Copy, Debug)]
struct CapsTrace {
    at: Instant,
    stage: &'static str,
    desired: bool,
    pressed: bool,
    observation: Option<crate::domain::caps_state::CapsObservation>,
    input: Option<CapsInput>,
}

impl KeyFlux {
    pub(crate) fn new(
        keyboard: EventTap,
        shift: ShiftGestureStateMachine,
        caps_lock: CapsLockController,
        input_sources: InputSourceManager,
        switching: SwitchController,
    ) -> Self {
        Self {
            keyboard,
            session: None,
            processor: Rc::new(RefCell::new(KeyboardProcessor {
                shift,
                caps_lock,
                input_sources,
                switching,
                current_source: None,
                source_dirty: Rc::new(Cell::new(false)),
                buffer: Rc::new(RefCell::new(DeferredInput::new())),
                wakeup: None,
                was_secure_input: false,
                debug: crate::infrastructure::macos::launch::debug_enabled(),
                caps_state: CapsState::new(),
                caps_output: Rc::new(Cell::new(CapsPresentation::default())),
                caps_source_request: None,
                caps_passthrough: false,
                caps_switch_active: false,
                caps_sync: CapsReconciler::new(),
                caps_wakeup: None,
                fatal: None,
                caps_trace: [None; 24],
                caps_trace_next: 0,
            })),
        }
    }

    pub(crate) fn start(&mut self) -> Result<()> {
        self.start_monitoring(None, true)
    }

    /// 自动恢复只检查权限，不弹出权限申请；故障前尚未确认的 Caps 目标可延续。
    pub(crate) fn resume(&mut self, caps_target: Option<bool>) -> Result<()> {
        self.start_monitoring(caps_target, false)
    }

    fn start_monitoring(&mut self, caps_target: Option<bool>, request_permissions: bool) -> Result<()> {
        if self.is_running() { return Ok(()); }
        if !cfg!(target_os = "macos") {
            return Err(Error::UnsupportedPlatform);
        }

        let permissions = PermissionStatus::check()?;
        if !permissions.is_granted() {
            record(format_args!(
                "辅助功能信任：{}；事件发送权限：{}；输入监控权限：{}",
                permissions.accessibility, permissions.post_event, permissions.input_monitoring
            ));
            if request_permissions { permissions.request_missing()?; }
            return Err(Error::MissingPermissions);
        }

        {
            let mut processor = self.processor.borrow_mut();
            processor.fatal = None;
            processor.caps_trace = [None; 24];
            processor.caps_trace_next = 0;
            processor.shift.reset();
            processor.caps_state = CapsState::new();
            processor.caps_passthrough = false;
            processor.was_secure_input = false;
            processor.source_dirty.set(false);
            processor.caps_lock.disconnect();
            processor.caps_lock.synchronize()?;
            processor.input_sources.initialize()?;
            processor.current_source = Some(processor.input_sources.current()?);
            let enabled = caps_target.filter(|_| !run_loop::secure_input_enabled() && !caps_lock::shortcut_pressed())
                .or(processor.caps_lock.enabled()).ok_or(Error::NativeApi("Caps 初始状态未知"))?;
            processor.caps_state.synchronize(enabled);
            processor.refresh_caps_output();
            if processor.debug {
                record(format_args!("开始监听：source={:?}，caps={:?}，逻辑Caps={enabled}", processor.current_source, processor.caps_lock.observation()));
            }
        }
        let processor = Rc::clone(&self.processor);
        let timer = Timer::new(move || processor.borrow_mut().tick())?;
        self.processor.borrow_mut().wakeup = Some(timer.handle());
        let processor = Rc::clone(&self.processor);
        let caps_timer = Timer::new(move || processor.borrow_mut().tick_caps())?;
        self.processor.borrow_mut().caps_wakeup = Some(caps_timer.handle());
        let dirty = Rc::clone(&self.processor.borrow().source_dirty);
        let wakeup = timer.handle();
        // TISSelectInputSource 可能同步通知：这里只改 Cell，不重入 processor。
        let observer = SourceObserver::new(move || {
            dirty.set(true);
            wakeup.arm(std::time::Duration::ZERO);
        })?;
        let processor = Rc::clone(&self.processor);
        let caps_monitor = CapsInputMonitor::new(move |input| processor.borrow_mut().handle_caps_input(input))?;
        caps_monitor.completion_result()?;
        let buffer = Rc::clone(&self.processor.borrow().buffer);
        let caps_output = Rc::clone(&self.processor.borrow().caps_output);
        let processor = Rc::clone(&self.processor);
        self.keyboard.start(move |event| {
            processor.borrow_mut().handle_event(event)
        }, buffer, caps_output)?;
        self.session = Some(MonitoringSession { timer, caps_timer, observer, caps_monitor });
        let caps_enabled = self.processor.borrow().caps_state.enabled();
        if caps_enabled || caps_target.is_some() {
            // 恢复时重新确认旧目标，不能只重建 tap 后丢失用户刚按下的 Caps。
            let mut processor = self.processor.borrow_mut();
            if !run_loop::secure_input_enabled() && !caps_lock::shortcut_pressed() {
                processor.queue_caps_sync("monitor-resumed");
                processor.queue_caps_source(caps_enabled);
            }
        }

        record(format_args!("KeyFlux：Shift 切换与中/英键短按大写已接入。"));
        record(format_args!("实机使用前请关闭系统的 Caps Lock / 中英键切换输入法选项，避免双重处理。"));
        record(format_args!("诊断 Shift / 中英键状态可设置 KEYFLUX_DEBUG=1；不记录输入文字。"));

        Ok(())
    }

    pub(crate) fn is_running(&self) -> bool { self.session.is_some() }

    pub(crate) fn recovery_caps_target(&self) -> Option<bool> {
        let processor = self.processor.borrow();
        (processor.fatal.is_some() || processor.caps_source_request.is_some()
            || processor.caps_sync.delay(Instant::now()).is_some()).then(|| processor.caps_state.enabled())
    }

    /// 在按钮、关闭窗口和错误通知中调用；没有嵌套 RunLoop，不阻塞窗口。
    pub(crate) fn stop(&mut self) -> Result<()> {
        run_loop::clear_stop_request();
        let Some(session) = self.session.take() else {
            self.processor.borrow_mut().caps_lock.disconnect();
            return Ok(());
        };
        let result = self.keyboard.completion_result()
            .and(session.timer.completion_result())
            .and(session.caps_timer.completion_result())
            .and(session.observer.completion_result())
            .and(session.caps_monitor.completion_result());
        {
            let mut processor = self.processor.borrow_mut();
            processor.cancel_and_release();
            processor.caps_sync.cancel();
            if let Some(wakeup) = &processor.caps_wakeup { wakeup.pause(); }
        }
        self.keyboard.stop();
        drop(session);
        let mut processor = self.processor.borrow_mut();
        processor.shift.reset();
        processor.caps_state = CapsState::new();
        processor.wakeup = None;
        processor.caps_wakeup = None;
        processor.caps_output.set(CapsPresentation::default());
        processor.caps_lock.disconnect();
        let fatal = processor.fatal.take().map_or(Ok(()), Err);
        run_loop::clear_stop_request();
        record(format_args!("KeyFlux：监听已停止，键盘已交回系统。"));
        result.and(fatal)
    }
}

impl Drop for KeyFlux {
    fn drop(&mut self) { let _ = self.stop(); }
}

impl KeyboardProcessor {
    fn handle_event(&mut self, event: KeyboardEvent) -> EventDisposition {
        if self.fatal.is_some() { return EventDisposition::Pass; }
        if matches!(event.kind, KeyboardEventKind::MonitorReset) {
            self.shift.reset();
            self.was_secure_input = false;
            self.cancel_and_release();
            return EventDisposition::Pass;
        }

        // 安全输入期间普通键可能不可见，不能把缺失的组合输入推断为单按。
        if event.secure_input {
            self.shift.reset();
            self.was_secure_input = true;
            let pending = self.switching.is_active() || self.caps_source_request.is_some();
            self.suspend_caps();
            if pending {
                self.cancel();
                return EventDisposition::FlushBeforeCurrent;
            }
            return EventDisposition::Pass;
        }
        if self.was_secure_input {
            self.shift.reset();
            self.was_secure_input = false;
        }
        if matches!(event.kind, KeyboardEventKind::CapsLockKeyChanged) && event.has_shortcut_modifiers {
            let pending = self.switching.is_active() || self.caps_source_request.is_some();
            self.suspend_caps();
            if pending {
                self.cancel();
                return EventDisposition::FlushBeforeCurrent;
            }
            return EventDisposition::Pass;
        }
        if self.caps_passthrough && event.has_shortcut_modifiers { return EventDisposition::Pass; }
        self.resume_caps();
        if event.caps_lock != self.caps_state.enabled() {
            // 回调中的 flags 可能属于系统更新前的快照，不同步写入 / 回读。
            self.queue_caps_sync("event-flags-mismatch");
        }
        if (self.switching.is_active() || self.caps_source_request.is_some()) && event.has_shortcut_modifiers {
            // 将已暂存输入排在当前快捷键之前补发，避免快捷键和输入顺序颠倒。
            self.shift.reset();
            self.cancel();
            return EventDisposition::FlushBeforeCurrent;
        }

        let action = match event.kind {
            KeyboardEventKind::ShiftChanged {
                key,
                is_down,
                has_other_modifiers,
                pointer_count,
            } => {
                let action = self.shift.shift_changed(
                    key,
                    is_down,
                    has_other_modifiers,
                    pointer_count,
                    self.caps_state.enabled(),
                );
                if self.debug {
                    record(format_args!("Shift {key:?} down={is_down} caps={} other={has_other_modifiers} pointer={pointer_count} action={action:?} source={:?} switching={} target_pid={:?}",
                        self.caps_state.enabled(), self.current_source, self.switching.is_active(), event.target_pid));
                }
                action
            }
            KeyboardEventKind::KeyDown { key_code } => {
                self.shift.key_down(key_code);
                ShiftAction::Pass
            }
            KeyboardEventKind::KeyUp { key_code } => {
                self.shift.key_up(key_code);
                ShiftAction::Pass
            }
            KeyboardEventKind::CapsLockKeyChanged => {
                self.trace_caps(if event.caps_lock { "native-caps-on" } else { "native-caps-off" }, None);
                if self.debug {
                    record(format_args!("收到中/英键状态变化；Caps Lock={}", event.caps_lock));
                }
                self.shift.other_modifier_changed();
                // HID 负责一次物理按压的逻辑翻转；flagsChanged 只负责传递状态。
                // 适配器按逻辑目标修正 Caps 位后放行，避免吞掉会话 / 输入框的状态通知。
                return EventDisposition::Pass;
            }
            KeyboardEventKind::OtherModifierChanged => {
                self.shift.other_modifier_changed();
                ShiftAction::Pass
            }
            KeyboardEventKind::MonitorReset => ShiftAction::Pass,
        };

        if action == ShiftAction::UseLowercaseEnglish {
            // 一次完整的单按只负责退出大写、保留 ABC；不能顺便恢复之前的拼音。
            self.cancel_and_release();
            self.caps_state.synchronize(false);
            self.refresh_caps_output();
            self.queue_caps_sync("shift-to-lowercase");
            self.queue_caps_request(CapsSourceRequest::LowercaseEnglish);
            if self.debug { record(format_args!("单按 Shift：退出大写，目标为小写 ABC，不恢复拼音")); }
            return EventDisposition::Pass;
        }
        if action == ShiftAction::ToggleInputSource {
            if self.switching.is_active() || self.caps_source_request.is_some() {
                if self.debug { record(format_args!("单按 Shift：前一次切换仍在进行，合并本次手势")); }
                return EventDisposition::DiscardShiftTap;
            }
            let Some(current) = &self.current_source else { return EventDisposition::Pass; };
            let Some(target) = self.input_sources.target_for(current) else { return EventDisposition::Pass; };
            if self.debug { record(format_args!("请求输入来源切换：{} -> {}", current.id, target.id)); }
            if self.switching.begin(current.id.clone(), target, Instant::now()) {
                self.buffer.borrow_mut().begin();
                if let Some(wakeup) = &self.wakeup { wakeup.arm(SELECTION_DELAY); }
                // 关键：触发切换的 Shift-up 不暂存，留给旧来源完整接收。
                return EventDisposition::Pass;
            }
        }
        EventDisposition::DeferIfSwitching
    }

    fn tick(&mut self) {
        let dirty = self.source_dirty.replace(false);
        if !dirty && !self.switching.is_active() && self.caps_source_request.is_none() { return; }
        if run_loop::secure_input_enabled() {
            self.suspend_caps();
            self.cancel_and_release();
            return;
        }
        let current = match self.input_sources.current() {
            Ok(current) => current,
            Err(error) => {
                record(format_args!("KeyFlux: {error}"));
                self.cancel_and_release();
                return;
            }
        };
        self.current_source = Some(current.clone());
        self.refresh_caps_output();
        if self.debug {
            record(format_args!("来源确认：source={} selected={} dirty={dirty} phase={:?} caps={} request={:?}",
                current.id, current.selected, self.switching.phase(), self.caps_state.enabled(), self.caps_source_request));
        }
        // 长按期间不启动来源切换的 600ms 截止；松开回调会唤醒此计时器。
        if self.caps_state.is_pressed() { return; }
        // Caps 恢复可能跨越多轮；状态确认后再启动对应来源切换的截止时间。
        // Caps 计时器成功后会唤醒此流程，无需在退避期间额外轮询。
        if self.caps_source_request.is_some() && self.caps_sync.delay(Instant::now()).is_some() { return; }
        if let Some(request) = self.caps_source_request.take() {
            self.process_caps_source(request, &current);
            return;
        }
        if !self.switching.is_active() { return; }
        if run_loop::secure_input_enabled()
            || (dirty && self.switching.notification_conflicts(&current)) {
            if self.debug { record(format_args!("取消输入来源切换：安全输入或第三个来源，current={}", current.id)); }
            self.cancel_and_release();
            return;
        }
        // 不能在底层 Caps 仍打开时切入拼音，也不能提前补发暂存字母。
        if self.caps_sync.delay(Instant::now()).is_some() {
            // Caps 成功确认会主动唤醒来源流程；等待期间不再每 20ms 查询 TIS。
            return;
        }
        match self.switching.advance(Instant::now(), &current) {
            SwitchAction::Select(target) => {
                if self.debug { record(format_args!("执行 TISSelectInputSource：{} -> {}", current.id, target.id)); }
                if let Err(error) = self.input_sources.select(&target) {
                    record(format_args!("KeyFlux: {error}"));
                    self.cancel_and_release();
                } else if let Some(wakeup) = &self.wakeup {
                    wakeup.arm(CONFIRM_INTERVAL);
                }
            }
            SwitchAction::Wait(delay) => {
                if let Some(wakeup) = &self.wakeup { wakeup.arm(delay); }
            }
            SwitchAction::Finish(outcome) => {
                let was_caps_switch = self.caps_switch_active;
                self.caps_switch_active = false;
                if outcome != SwitchOutcome::Confirmed {
                    record(format_args!("KeyFlux: 输入来源切换结束：{outcome:?}"));
                } else if self.debug {
                    record(format_args!("输入来源已确认：{}", current.id));
                }
                // 普通中英切换不主动写 Caps；只对 Caps 相关来源切换重新同步。
                if outcome == SwitchOutcome::Confirmed && was_caps_switch { self.queue_caps_sync("caps-source-confirmed"); }
                self.release_if_ready();
            }
        }
    }

    fn cancel(&mut self) {
        self.switching.cancel();
        self.caps_source_request = None;
        self.caps_switch_active = false;
        if let Some(wakeup) = &self.wakeup { wakeup.pause(); }
    }

    fn cancel_and_release(&mut self) {
        self.cancel();
        self.release_input();
    }

    fn release_input(&mut self) {
        let events = self.buffer.borrow_mut().take();
        event_tap::replay(events);
    }

    fn release_if_ready(&mut self) {
        if !self.switching.is_active() && self.caps_source_request.is_none()
            && !self.caps_state.is_pressed() && self.caps_sync.delay(Instant::now()).is_none() {
            self.release_input();
        }
    }

    fn refresh_caps_output(&self) {
        self.caps_output.set(CapsPresentation {
            enabled: if self.caps_passthrough { None } else { Some(self.caps_state.enabled()) },
            abc_active: self.current_source.as_ref().is_some_and(|source| source.selected && self.input_sources.is_abc(&source.id)),
        });
    }

    fn resume_caps(&mut self) {
        if !self.caps_passthrough { return; }
        // 键盘回调只继承会话快照；跨服务查询留到后续 timer，避免扩大 tap 延迟。
        self.caps_state.synchronize(caps_lock::session_caps_enabled());
        self.caps_passthrough = false;
        self.refresh_caps_output();
        self.queue_caps_sync("resume-after-passthrough");
    }

    fn suspend_caps(&mut self) {
        self.caps_sync.cancel();
        if let Some(wakeup) = &self.caps_wakeup { wakeup.pause(); }
        self.caps_passthrough = true;
        self.caps_output.set(CapsPresentation::default());
        self.caps_source_request = None;
    }

    fn fail_caps(&mut self, error: Error) {
        if self.fatal.is_some() { return; }
        self.trace_caps("failure", None);
        let now = Instant::now();
        for offset in 0..self.caps_trace.len() {
            if let Some(trace) = self.caps_trace[(self.caps_trace_next + offset) % self.caps_trace.len()] {
                record(format_args!("Caps 故障前记录：距失败={}ms stage={} desired={} pressed={} cached_observation={:?} input={:?}",
                    now.saturating_duration_since(trace.at).as_millis(), trace.stage, trace.desired,
                    trace.pressed, trace.observation, trace.input));
            }
        }
        self.suspend_caps();
        self.cancel_and_release();
        self.fatal = Some(error);
        run_loop::stop();
    }

    fn handle_caps_input(&mut self, input: CapsInput) -> Result<()> {
        if self.fatal.is_some() { return Ok(()); }
        self.trace_caps("hid-input", Some(input));
        if matches!(input, CapsInput::Removed { .. }) {
            self.caps_state.input(input, true);
            self.queue_caps_sync("device-removed");
            return Ok(());
        }
        let blocked = run_loop::secure_input_enabled() || caps_lock::shortcut_pressed();
        if self.debug { record(format_args!("Caps HID：{input:?}，放行={blocked}")); }
        if blocked {
            self.caps_state.input(input, true);
            self.suspend_caps();
            self.cancel_and_release();
            return Ok(());
        }
        self.resume_caps();
        let changed = self.caps_state.input(input, false);
        if let Some(enabled) = changed {
            self.cancel_and_release();
            self.shift.reset();
            self.refresh_caps_output();
            self.queue_caps_sync("physical-caps");
            self.queue_caps_source(enabled);
            if self.debug { record(format_args!("物理中/英键：目标 Caps Lock={enabled}，等待系统确认")); }
        } else if matches!(input, CapsInput::Key { down: false, .. }) {
            self.queue_caps_sync("physical-caps-release");
        }
        Ok(())
    }

    fn queue_caps_sync(&mut self, reason: &'static str) {
        if self.caps_passthrough { return; }
        if self.caps_state.is_pressed() {
            // 物理状态仍由键盘驱动更新；写入与失败计时都从最后一个 Caps 松开后开始。
            self.caps_sync.cancel();
            if let Some(wakeup) = &self.caps_wakeup { wakeup.pause(); }
            self.trace_caps("waiting-for-release", None);
            return;
        }
        let now = Instant::now();
        let started = self.caps_sync.begin(self.caps_state.enabled(), now);
        if self.debug && started { record(format_args!("安排 Caps 同步：reason={reason} desired={}", self.caps_state.enabled())); }
        if let (Some(delay), Some(wakeup)) = (self.caps_sync.delay(now), &self.caps_wakeup) { wakeup.arm(delay); }
        // 来源切换可能曾因 Caps 仍按住而暂停。
        if (self.switching.is_active() || self.caps_source_request.is_some())
            && let Some(wakeup) = &self.wakeup { wakeup.arm(CONFIRM_INTERVAL); }
    }

    fn trace_caps(&mut self, stage: &'static str, input: Option<CapsInput>) {
        self.caps_trace[self.caps_trace_next] = Some(CapsTrace { at: Instant::now(), stage,
            desired: self.caps_state.enabled(), pressed: self.caps_state.is_pressed(),
            observation: self.caps_lock.observation(), input });
        self.caps_trace_next = (self.caps_trace_next + 1) % self.caps_trace.len();
    }

    fn tick_caps(&mut self) {
        if self.caps_sync.delay(Instant::now()).is_none() { return; }
        if self.caps_state.is_pressed() {
            self.queue_caps_sync("waiting-for-release");
            return;
        }
        if run_loop::secure_input_enabled() || caps_lock::shortcut_pressed() {
            self.suspend_caps();
            self.cancel_and_release();
            return;
        }
        if let Err(error) = self.caps_lock.synchronize() { self.fail_caps(error); return; }
        self.trace_caps("readback", None);
        let Some(observation) = self.caps_lock.observation() else { self.fail_caps(Error::NativeApi("Caps 状态未知")); return; };
        let attempts = self.caps_sync.attempts();
        match self.caps_sync.advance(Instant::now(), observation) {
            CapsSyncAction::Idle => {}
            CapsSyncAction::Wait(delay) => {
                if let Some(wakeup) = &self.caps_wakeup { wakeup.arm(delay); }
            }
            CapsSyncAction::Write(desired) => {
                if self.debug { record(format_args!("Caps 延迟同步：期望={desired}，观测={observation:?}，提交写入请求")); }
                if let Err(error) = self.caps_lock.write_enabled(desired) { self.fail_caps(error); return; }
                self.trace_caps("write-accepted-awaiting-readback", None);
                if let (Some(delay), Some(wakeup)) = (self.caps_sync.delay(Instant::now()), &self.caps_wakeup) { wakeup.arm(delay); }
            }
            CapsSyncAction::Confirmed(enabled) => {
                if attempts > 1 {
                    record(format_args!("KeyFlux：Caps 自动重试恢复成功，目标={enabled}，确认轮次={attempts}，监听继续。"));
                }
                if self.debug { record(format_args!("Caps 服务与会话已确认：目标={enabled}，观测={observation:?}；监听继续")); }
                if (self.caps_source_request.is_some() || self.switching.is_active())
                    && let Some(wakeup) = &self.wakeup { wakeup.arm(std::time::Duration::ZERO); }
                self.release_if_ready();
            }
            CapsSyncAction::Retry { desired, attempt, delay } => {
                self.trace_caps("retry-backoff", None);
                record(format_args!("KeyFlux：Caps 状态尚未同步，{}ms 后自动进入第 {attempt}/3 轮，目标={desired}，观测={observation:?}；监听保留。",
                    delay.as_millis()));
                if let Some(wakeup) = &self.caps_wakeup { wakeup.arm(delay); }
            }
            CapsSyncAction::Failed { desired, actual, writes, attempts } => self.fail_caps(Error::CapsSyncFailed {
                desired, actual, io_actual: observation.io_enabled, service_actual: observation.service_enabled, writes, attempts }),
        }
    }

    fn process_caps_source(&mut self, request: CapsSourceRequest, current: &SourceSnapshot) {
        let front_pid = caps_lock::front_pid();
        if self.debug { record(format_args!("Caps 来源处理：request={request:?}，source={}，front_pid={front_pid:?}", current.id)); }
        let target = self.input_sources.abc().and_then(|abc| request.target_for(&current.id, abc));
        let Some(target) = target else { self.release_if_ready(); return; };
        if self.switching.begin(current.id.clone(), target, Instant::now()) {
            self.caps_switch_active = true;
            if let Some(wakeup) = &self.wakeup { wakeup.arm(SELECTION_DELAY); }
            return;
        }
        self.release_if_ready();
    }

    fn queue_caps_source(&mut self, enabled: bool) {
        self.queue_caps_request(CapsSourceRequest::from_enabled(enabled));
    }

    fn queue_caps_request(&mut self, request: CapsSourceRequest) {
        self.caps_source_request = Some(request);
        self.buffer.borrow_mut().begin();
        if let Some(wakeup) = &self.wakeup { wakeup.arm(std::time::Duration::ZERO); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::caps_state::CapsObservation;
    use crate::domain::deferred_input::DeferredKey;
    use crate::domain::input_source::InputSource;

    fn engine() -> KeyFlux {
        KeyFlux::new(EventTap::new(), ShiftGestureStateMachine::new(), CapsLockController::new(),
            InputSourceManager::new(), SwitchController::new())
    }

    fn letter() -> DeferredKey { DeferredKey { code: 0, is_up: false, shift: None } }

    #[test]
    fn recovery_retains_the_failed_caps_target_before_stop_resets_it() {
        let engine = engine();
        assert_eq!(engine.recovery_caps_target(), None);
        {
            let mut processor = engine.processor.borrow_mut();
            processor.caps_state.synchronize(true);
            // fail_caps 已取消 pending 并切换到系统透传，目标仍由逻辑状态保存。
            processor.caps_passthrough = true;
            processor.fatal = Some(Error::CapsSyncFailed { desired: true, actual: false,
                io_actual: false, service_actual: Some(false), writes: 12, attempts: 3 });
        }
        assert_eq!(engine.recovery_caps_target(), Some(true));
    }

    #[test]
    fn caps_writes_wait_for_the_last_physical_release() {
        let engine = engine();
        let mut processor = engine.processor.borrow_mut();
        let key = |device, down| CapsInput::Key { device, down, initial: false };
        processor.caps_state.input(key(1, true), false);
        processor.queue_caps_sync("test-down");
        assert!(processor.caps_sync.delay(Instant::now()).is_none());
        processor.caps_state.input(key(2, true), false);
        processor.caps_state.input(key(1, false), false);
        processor.queue_caps_sync("test-first-release");
        assert!(processor.caps_sync.delay(Instant::now()).is_none());
        processor.caps_state.input(key(2, false), false);
        processor.queue_caps_sync("test-last-release");
        assert!(processor.caps_sync.delay(Instant::now()).is_some());
        // A new physical press cancels the old write/timeout budget until release.
        processor.caps_state.input(key(1, true), false);
        processor.queue_caps_sync("test-next-down");
        assert!(processor.caps_sync.delay(Instant::now()).is_none());
    }

    #[test]
    fn held_caps_retains_input_and_device_removal_can_resume_sync() {
        let engine = engine();
        let mut processor = engine.processor.borrow_mut();
        processor.caps_state.input(CapsInput::Key { device: 1, down: true, initial: false }, false);
        processor.buffer.borrow_mut().begin();
        processor.release_if_ready();
        assert!(processor.buffer.borrow().should_hold(letter(), false));
        processor.caps_state.input(CapsInput::Removed { device: 1 }, true);
        processor.queue_caps_sync("test-device-removed");
        assert!(processor.caps_sync.delay(Instant::now()).is_some());
    }

    #[test]
    fn already_abc_keeps_input_held_until_both_caps_states_match() {
        let engine = engine();
        let mut processor = engine.processor.borrow_mut();
        let now = Instant::now();
        processor.buffer.borrow_mut().begin();
        processor.caps_sync.begin(false, now);
        processor.release_if_ready();
        assert!(processor.buffer.borrow().should_hold(letter(), false));
        assert_eq!(processor.caps_sync.advance(now + crate::application::caps_control::CAPS_SETTLE,
            CapsObservation { io_enabled: true, session_enabled: false, service_enabled: Some(true) }), CapsSyncAction::Write(false));
        processor.release_if_ready();
        assert!(processor.buffer.borrow().should_hold(letter(), false));
        assert_eq!(processor.caps_sync.advance(now + crate::application::caps_control::CAPS_SETTLE * 2,
            CapsObservation { io_enabled: false, session_enabled: false, service_enabled: Some(false) }), CapsSyncAction::Confirmed(false));
        processor.release_if_ready();
        assert!(!processor.buffer.borrow().should_hold(letter(), false));
    }

    #[test]
    fn matching_caps_cannot_release_input_before_the_english_source_is_confirmed() {
        let engine = engine();
        let mut processor = engine.processor.borrow_mut();
        let now = Instant::now();
        processor.buffer.borrow_mut().begin();
        processor.caps_source_request = Some(CapsSourceRequest::UppercaseEnglish);
        processor.release_if_ready();
        assert!(processor.buffer.borrow().should_hold(letter(), false));
        processor.caps_source_request = None;
        assert!(processor.switching.begin("Pinyin".into(), InputSource { id: "ABC".into() }, now));
        processor.release_if_ready();
        assert!(processor.buffer.borrow().should_hold(letter(), false));
        assert!(matches!(processor.switching.advance(now,
            &SourceSnapshot { id: "ABC".into(), selected: true }), SwitchAction::Finish(SwitchOutcome::Confirmed)));
        processor.release_if_ready();
        assert!(!processor.buffer.borrow().should_hold(letter(), false));
    }
}
