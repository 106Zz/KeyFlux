//! 监听、有限暂存与补发键盘事件。原生对象不进入领域层。

use std::cell::RefCell;
use std::rc::Rc;

use crate::domain::deferred_input::DeferredInput;
use crate::domain::keyboard::{EventDisposition, KeyboardEvent};
use crate::error::{Error, Result};
use super::caps_lock::CapsOutput;

pub struct EventTap {
    #[cfg(target_os = "macos")]
    registration: Option<native::Registration>,
}

pub struct OwnedEvent {
    #[cfg(target_os = "macos")]
    handle: super::ffi::OwnedRef,
    caps: CapsOutput,
}

pub type InputBuffer = Rc<RefCell<DeferredInput<OwnedEvent>>>;

pub fn replay(events: Vec<OwnedEvent>) {
    for event in events { event.post(); }
}

impl OwnedEvent {
    fn post(&self) {
        #[cfg(target_os = "macos")]
        // SAFETY：独立拥有事件引用，系统发送调用后才释放；标记防止再次拦截。
        unsafe {
            apply_caps(self.handle.as_ptr(), &self.caps);
            super::ffi::CGEventSetIntegerValueField(self.handle.as_ptr(), 42, 0x4b4559464c5558);
            super::ffi::CGEventPost(super::ffi::SESSION_EVENT_TAP, self.handle.as_ptr());
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_caps(event: super::ffi::NativeRef, output: &CapsOutput) {
    let presentation = output.get();
    if presentation.enabled.is_none() { return; }
    // SAFETY：事件在回调内借用或由 OwnedEvent 持有，所有读写都在主线程。
    unsafe {
        if super::ffi::IsSecureEventInputEnabled() != 0 { return; }
        let original = super::ffi::CGEventGetFlags(event);
        let kind = super::ffi::CGEventGetType(event);
        let code = super::ffi::CGEventGetIntegerValueField(event, 9) as u16;
        let (flags, letter) = super::caps_lock::adjusted_case(original, code,
            kind == super::ffi::KEY_DOWN || kind == super::ffi::KEY_UP, presentation);
        if flags != original { super::ffi::CGEventSetFlags(event, flags); }
        if let Some(letter) = letter { super::ffi::CGEventKeyboardSetUnicodeString(event, 1, &letter); }
    }
}

impl EventTap {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            registration: None,
        }
    }

    pub fn start(&mut self, handler: impl FnMut(KeyboardEvent) -> EventDisposition + 'static, buffer: InputBuffer, caps: CapsOutput) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            if self.registration.is_some() {
                return Err(Error::MonitorStopped("重复启动监听"));
            }
            self.registration = Some(native::Registration::start(Box::new(handler), buffer, caps)?);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (handler, buffer, caps);
            Err(Error::UnsupportedPlatform)
        }
    }

    pub fn completion_result(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        if let Some(registration) = &self.registration {
            return registration.completion_result();
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            // Registration::drop 先注销回调来源，再释放回调状态。
            self.registration = None;
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::domain::keyboard::KeyboardEventKind;
    use crate::domain::deferred_input::DeferredKey;
    use crate::domain::shift_state::ShiftKey;
    use crate::infrastructure::macos::ffi::{self, NativeRef, OwnedRef};
    use crate::infrastructure::macos::permissions::PermissionStatus;

    const LEFT_SHIFT: u16 = 56;
    const RIGHT_SHIFT: u16 = 60;
    const KEY_CODE_FIELD: u32 = 9;
    const USER_DATA_FIELD: u32 = 42;
    const SYNTHETIC_MARKER: i64 = 0x4b4559464c5558; // "KEYFLUX"

    pub(super) struct Registration {
        source: OwnedRef,
        tap: OwnedRef,
        run_loop: NativeRef,
        failure: Rc<Cell<Option<&'static str>>>,
        // 保持地址稳定；原生回调只在对应 source 注册且运行时访问它。
        _context: Box<CallbackContext>,
    }

    struct CallbackContext {
        handler: Box<dyn FnMut(KeyboardEvent) -> EventDisposition>,
        buffer: InputBuffer,
        caps: CapsOutput,
        tap: NativeRef,
        failure: Rc<Cell<Option<&'static str>>>,
        recovery_window: Instant,
        recoveries: u8,
    }

    impl Registration {
        pub(super) fn start(handler: Box<dyn FnMut(KeyboardEvent) -> EventDisposition>, buffer: InputBuffer, caps: CapsOutput) -> Result<Self> {
            // SAFETY：注册和处理回调均在此线程的 RunLoop 上进行。
            let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
            let failure = Rc::new(Cell::new(None));
            let mut context = Box::new(CallbackContext {
                handler,
                buffer,
                caps,
                tap: ptr::null_mut(),
                failure: Rc::clone(&failure),
                recovery_window: Instant::now(),
                recoveries: 0,
            });
            let mask = (1_u64 << ffi::KEY_DOWN)
                | (1_u64 << ffi::KEY_UP)
                | (1_u64 << ffi::FLAGS_CHANGED);

            // SAFETY：context 位于 Box 的稳定地址，保留到 tap 注销之后。
            let tap = unsafe {
                ffi::CGEventTapCreate(
                    ffi::SESSION_EVENT_TAP,
                    0,
                    0,
                    mask,
                    Some(callback),
                    (&mut *context as *mut CallbackContext).cast(),
                )
            };
            // SAFETY：Create 返回值的唯一拥有引用交给 RAII 包装。
            let tap = unsafe { OwnedRef::from_created(tap) }
                .ok_or(Error::NativeApi("CGEventTapCreate，请检查系统权限"))?;
            context.tap = tap.as_ptr();

            // SAFETY：tap 有效，使用默认分配器创建该 tap 的事件 source。
            let source = unsafe {
                ffi::CFMachPortCreateRunLoopSource(ptr::null(), tap.as_ptr(), 0)
            };
            // SAFETY：同样接管 Create 返回的拥有引用。
            let Some(source) = (unsafe { OwnedRef::from_created(source) }) else {
                // SAFETY：在释放 context 前使原生端不再访问 user_info。
                unsafe { ffi::CFMachPortInvalidate(tap.as_ptr()) };
                return Err(Error::NativeApi("CFMachPortCreateRunLoopSource"));
            };

            // SAFETY：所有引用有效且由返回对象持有，回调在进入 RunLoop 后发生。
            unsafe {
                ffi::CFRunLoopAddSource(run_loop, source.as_ptr(), ffi::kCFRunLoopCommonModes);
                ffi::CGEventTapEnable(tap.as_ptr(), true);
            }

            Ok(Self {
                source,
                tap,
                run_loop,
                failure,
                _context: context,
            })
        }

        pub(super) fn completion_result(&self) -> Result<()> {
            match self.failure.get() {
                Some(reason) => Err(Error::MonitorStopped(reason)),
                None => Ok(()),
            }
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY：资源在注册线程销毁，注销后再释放 source、tap 和 context。
            unsafe {
                ffi::CGEventTapEnable(self.tap.as_ptr(), false);
                ffi::CFRunLoopRemoveSource(
                    self.run_loop,
                    self.source.as_ptr(),
                    ffi::kCFRunLoopCommonModes,
                );
                ffi::CFMachPortInvalidate(self.tap.as_ptr());
            }
        }
    }

    impl CallbackContext {
        fn handle(&mut self, event_type: u32, event: NativeRef) -> NativeRef {
            if event_type == ffi::TAP_DISABLED_TIMEOUT || event_type == ffi::TAP_DISABLED_USER {
                (self.handler)(KeyboardEvent {
                    kind: KeyboardEventKind::MonitorReset,
                    caps_lock: false,
                    secure_input: false,
                    has_shortcut_modifiers: false,
                    target_pid: None,
                });
                if event_type == ffi::TAP_DISABLED_USER {
                    self.stop("系统或用户停用了监听，请检查权限后重新启动");
                    return event;
                }
                if !PermissionStatus::check().is_ok_and(|status| status.is_granted()) {
                    self.stop("键盘监听权限已失效");
                    return event;
                }
                if self.recovery_window.elapsed() >= Duration::from_secs(60) {
                    self.recovery_window = Instant::now();
                    self.recoveries = 0;
                }
                self.recoveries += 1;
                if self.recoveries > 3 {
                    self.stop("一分钟内多次回调超时，需要重建监听");
                    return event;
                }
                // SAFETY：tap 仍由 Registration 持有，超时后允许有限重新启用。
                unsafe { ffi::CGEventTapEnable(self.tap, true) };
                return event;
            }
            if event.is_null() {
                return event;
            }

            // SAFETY：此事件借用仅在系统回调期间有效，不保存或释放它。
            let (key_code, flags, marker, secure_input, target_pid) = unsafe {
                (
                    ffi::CGEventGetIntegerValueField(event, KEY_CODE_FIELD) as u16,
                    ffi::CGEventGetFlags(event),
                    ffi::CGEventGetIntegerValueField(event, USER_DATA_FIELD),
                    ffi::IsSecureEventInputEnabled() != 0,
                    ffi::CGEventGetIntegerValueField(event, 40),
                )
            };
            if marker == SYNTHETIC_MARKER {
                return event;
            }

            let kind = match event_type {
                ffi::FLAGS_CHANGED if key_code == LEFT_SHIFT || key_code == RIGHT_SHIFT => {
                    let device_bits = flags & 0x6;
                    let key_bit = if key_code == LEFT_SHIFT { 0x2 } else { 0x4 };
                    let is_down = if device_bits == 0 {
                        flags & ffi::SHIFT != 0
                    } else {
                        device_bits & key_bit != 0
                    };
                    KeyboardEventKind::ShiftChanged {
                        key: if key_code == LEFT_SHIFT { ShiftKey::Left } else { ShiftKey::Right },
                        is_down,
                        has_other_modifiers: flags & ffi::OTHER_MODIFIERS != 0,
                        pointer_count: pointer_count(),
                    }
                }
                ffi::FLAGS_CHANGED if key_code == 57 => KeyboardEventKind::CapsLockKeyChanged,
                ffi::FLAGS_CHANGED => KeyboardEventKind::OtherModifierChanged,
                ffi::KEY_DOWN => KeyboardEventKind::KeyDown { key_code },
                ffi::KEY_UP => KeyboardEventKind::KeyUp { key_code },
                _ => return event,
            };
            let disposition = (self.handler)(KeyboardEvent {
                kind,
                caps_lock: flags & ffi::CAPS_LOCK != 0,
                secure_input,
                has_shortcut_modifiers: flags & ffi::OTHER_MODIFIERS != 0,
                target_pid: i32::try_from(target_pid).ok().filter(|pid| *pid > 0),
            });
            match disposition {
                EventDisposition::Pass => return self.forward(event),
                EventDisposition::FlushBeforeCurrent => return self.flush_before_current(event),
                EventDisposition::DiscardShiftTap => {
                    if self.buffer.borrow_mut().discard_shift_tap() { return ptr::null_mut(); }
                }
                EventDisposition::DeferIfSwitching => {}
            }
            let key = match kind {
                KeyboardEventKind::KeyDown { key_code } => DeferredKey { code: key_code, is_up: false, shift: None },
                KeyboardEventKind::KeyUp { key_code } => DeferredKey { code: key_code, is_up: true, shift: None },
                KeyboardEventKind::ShiftChanged { key, is_down, .. } => DeferredKey {
                    code: match key { ShiftKey::Left => LEFT_SHIFT, ShiftKey::Right => RIGHT_SHIFT },
                    is_up: !is_down,
                    shift: Some(key),
                },
                _ => return self.forward(event),
            };
            if !self.buffer.borrow().should_hold(key, flags & ffi::OTHER_MODIFIERS != 0) { return self.forward(event); }
            if self.buffer.borrow().is_full() {
                // 停止本次切换但保留先前输入，与当前事件一起有序补发。
                (self.handler)(KeyboardEvent { kind: KeyboardEventKind::MonitorReset, caps_lock: false, secure_input: false, has_shortcut_modifiers: false, target_pid: None });
                let forwarded = self.flush_before_current(event);
                self.stop("切换期间输入队列达到上限，已释放输入并停止监听");
                return forwarded;
            }
            // SAFETY：只在决定暂存时复制回调借用事件，副本由 OwnedEvent 释放。
            let copy = unsafe { OwnedRef::from_created(ffi::CGEventCreateCopy(event)) };
            let Some(handle) = copy else {
                (self.handler)(KeyboardEvent { kind: KeyboardEventKind::MonitorReset, caps_lock: false, secure_input: false, has_shortcut_modifiers: false, target_pid: None });
                let forwarded = self.flush_before_current(event);
                self.stop("复制键盘事件失败");
                return forwarded;
            };
            if self.buffer.borrow_mut().push(key, OwnedEvent { handle, caps: Rc::clone(&self.caps) }).is_err() {
                (self.handler)(KeyboardEvent { kind: KeyboardEventKind::MonitorReset, caps_lock: false, secure_input: false, has_shortcut_modifiers: false, target_pid: None });
                let forwarded = self.flush_before_current(event);
                self.stop("输入暂存状态发生冲突");
                return forwarded;
            }
            ptr::null_mut()
        }

        fn flush_before_current(&mut self, event: NativeRef) -> NativeRef {
            // 将当前事件也补发，使其排在之前的暂存事件之后，而非抢先放行。
            let events = self.buffer.borrow_mut().take();
            replay(events);
            apply_caps(event, &self.caps);
            // SAFETY：回调借用在此函数返回前有效；Post 序列化事件后返回，
            // 无需为当前事件再分配副本。返回 null 防止原始事件重复送达。
            unsafe {
                ffi::CGEventSetIntegerValueField(event, USER_DATA_FIELD, SYNTHETIC_MARKER);
                ffi::CGEventPost(ffi::SESSION_EVENT_TAP, event);
            }
            ptr::null_mut()
        }

        fn forward(&self, event: NativeRef) -> NativeRef {
            apply_caps(event, &self.caps);
            event
        }

        fn stop(&mut self, reason: &'static str) {
            self.failure.set(Some(reason));
            // SAFETY：先停用 tap；发送延后停止通知后，由窗口协调器清理资源。
            unsafe {
                ffi::CGEventTapEnable(self.tap, false);
            }
            crate::infrastructure::macos::run_loop::stop();
        }
    }

    unsafe extern "C" fn callback(
        _proxy: NativeRef,
        event_type: u32,
        event: NativeRef,
        user_info: *mut c_void,
    ) -> NativeRef {
        if user_info.is_null() {
            return event;
        }
        // SAFETY：Registration 持有 Box，回调与销毁都发生在注册线程。
        let context = unsafe { &mut *user_info.cast::<CallbackContext>() };
        match catch_unwind(AssertUnwindSafe(|| context.handle(event_type, event))) {
            Ok(forwarded) => forwarded,
            Err(_) => {
                // Rust panic 不能穿过 C 回调边界；停止监听并由应用层报告失败。
                context.stop("事件处理发生 panic");
                event
            }
        }
    }

    fn pointer_count() -> u32 {
        // 仅在 Shift 状态变化时读取点击与滚轮计数，不监听所有鼠标事件。
        [1, 3, 25, 22].into_iter().fold(0_u32, |count, event_type| {
            // SAFETY：读取窗口服务器计数，无原生引用或输入内容被持有。
            count.wrapping_add(unsafe {
                ffi::CGEventSourceCounterForEventType(ffi::COMBINED_SESSION, event_type)
            })
        })
    }
}
