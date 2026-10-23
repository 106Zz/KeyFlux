//! 可取消、可重新安排的 RunLoop 计时器；空闲时从 RunLoop 移除。

use std::time::Duration;

use crate::error::{Error, Result};

pub struct Timer {
    #[cfg(target_os = "macos")]
    native: native::Registration,
}

#[derive(Clone)]
pub struct Wakeup {
    #[cfg(target_os = "macos")]
    native: std::rc::Rc<native::Handle>,
}

impl Timer {
    pub fn new(handler: impl FnMut() + 'static) -> Result<Self> {
        #[cfg(target_os = "macos")]
        { Ok(Self { native: native::Registration::new(Box::new(handler))? }) }
        #[cfg(not(target_os = "macos"))]
        { let _ = handler; Err(Error::UnsupportedPlatform) }
    }

    pub fn handle(&self) -> Wakeup {
        Wakeup {
            #[cfg(target_os = "macos")]
            native: std::rc::Rc::clone(&self.native.handle),
        }
    }

    pub fn completion_result(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        if self.native.failed.get() { return Err(Error::MonitorStopped("切换计时器回调发生 panic")); }
        Ok(())
    }
}

impl Wakeup {
    pub fn arm(&self, delay: Duration) {
        #[cfg(target_os = "macos")]
        self.native.arm(delay);
        #[cfg(not(target_os = "macos"))]
        let _ = delay;
    }

    pub fn pause(&self) {
        #[cfg(target_os = "macos")]
        self.native.pause();
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::rc::Rc;

    use super::*;
    use crate::infrastructure::macos::ffi::{self, NativeRef, OwnedRef};

    pub(super) struct Registration {
        pub handle: Rc<Handle>,
        pub failed: Rc<Cell<bool>>,
        _context: Box<Context>,
    }

    pub(super) struct Handle {
        timer: OwnedRef,
        run_loop: NativeRef,
        active: Cell<bool>,
        valid: Cell<bool>,
    }

    struct Context {
        handler: Box<dyn FnMut()>,
        handle: Option<Rc<Handle>>,
        failed: Rc<Cell<bool>>,
    }

    impl Registration {
        pub fn new(handler: Box<dyn FnMut()>) -> Result<Self> {
            let failed = Rc::new(Cell::new(false));
            let mut context = Box::new(Context { handler, handle: None, failed: Rc::clone(&failed) });
            let mut native_context = ffi::TimerContext {
                version: 0,
                info: (&mut *context as *mut Context).cast(),
                retain: None,
                release: None,
                copy_description: None,
            };
            // SAFETY：context 是稳定 Box；创建时不注册到 RunLoop，不会空闲唤醒。
            let timer = unsafe {
                OwnedRef::from_created(ffi::CFRunLoopTimerCreate(ptr::null(), ffi::CFAbsoluteTimeGetCurrent(), 0.02, 0, 0, callback, &mut native_context))
            }.ok_or(Error::NativeApi("CFRunLoopTimerCreate"))?;
            let handle = Rc::new(Handle {
                timer,
                // SAFETY：仅在当前线程创建和调度此计时器。
                run_loop: unsafe { ffi::CFRunLoopGetCurrent() },
                active: Cell::new(false),
                valid: Cell::new(true),
            });
            context.handle = Some(Rc::clone(&handle));
            Ok(Self { handle, failed, _context: context })
        }
    }

    impl Handle {
        pub fn arm(&self, delay: Duration) {
            if !self.valid.get() { return; }
            // SAFETY：引用保持有效；先安排下次时间，再注册来源。
            unsafe {
                ffi::CFRunLoopTimerSetNextFireDate(self.timer.as_ptr(), ffi::CFAbsoluteTimeGetCurrent() + delay.as_secs_f64());
                if !self.active.replace(true) {
                    ffi::CFRunLoopAddTimer(self.run_loop, self.timer.as_ptr(), ffi::kCFRunLoopCommonModes);
                }
            }
        }

        pub fn pause(&self) {
            if !self.valid.get() || !self.active.replace(false) { return; }
            // SAFETY：移除注册但保留有效 timer，后续可以重新调度。
            unsafe { ffi::CFRunLoopRemoveTimer(self.run_loop, self.timer.as_ptr(), ffi::kCFRunLoopCommonModes) };
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            self.handle.pause();
            self.handle.valid.set(false);
            // SAFETY：先禁止原生回调再释放 Context；遗留 Wakeup 只会执行 no-op。
            unsafe { ffi::CFRunLoopTimerInvalidate(self.handle.timer.as_ptr()) };
        }
    }

    unsafe extern "C" fn callback(_timer: NativeRef, info: *mut c_void) {
        if info.is_null() { return; }
        // SAFETY：Registration 保持 Box 地址和有效期，单线程回调。
        let context = unsafe { &mut *info.cast::<Context>() };
        if let Some(handle) = &context.handle { handle.pause(); }
        if catch_unwind(AssertUnwindSafe(&mut context.handler)).is_err() {
            context.failed.set(true);
            // 延后通知应用层清理队列，panic 不跨 FFI 边界。
            crate::infrastructure::macos::run_loop::stop();
        }
    }
}
