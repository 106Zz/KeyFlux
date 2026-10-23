//! macOS 事件循环适配器。

use crate::error::Error;
use crate::error::Result;

/// 将停止请求延后到当前键盘回调返回后处理，避免释放正在执行的回调对象。
pub struct StopSignal {
    #[cfg(target_os = "macos")]
    _native: native::Registration,
}

impl StopSignal {
    pub fn new(handler: impl FnMut() + 'static) -> Result<Self> {
        #[cfg(target_os = "macos")]
        { Ok(Self { _native: native::Registration::new(Box::new(handler))? }) }
        #[cfg(not(target_os = "macos"))]
        { let _ = handler; Err(Error::UnsupportedPlatform) }
    }
}

pub fn secure_input_enabled() -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY：由主线程事件循环按需查询 Carbon 安全输入状态。
    unsafe { super::ffi::IsSecureEventInputEnabled() != 0 }
    #[cfg(not(target_os = "macos"))]
    false
}

pub fn stop() {
    #[cfg(target_os = "macos")]
    native::request_stop();
}

/// 手动停止或重建完成后丢弃旧会话尚未送达的故障通知。
pub fn clear_stop_request() {
    #[cfg(target_os = "macos")]
    native::clear_stop_request();
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::ptr;
    use super::*;
    use crate::infrastructure::macos::ffi::{self, NativeRef, OwnedRef};

    thread_local! {
        static STOP_SOURCE: Cell<NativeRef> = const { Cell::new(ptr::null_mut()) };
        static STOP_PENDING: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) struct Registration {
        source: OwnedRef,
        run_loop: NativeRef,
        _context: Box<Context>,
    }

    struct Context { handler: Box<dyn FnMut()> }

    impl Registration {
        pub fn new(handler: Box<dyn FnMut()>) -> Result<Self> {
            if STOP_SOURCE.with(|source| !source.get().is_null()) { return Err(Error::NativeApi("停止信号重复注册")); }
            let mut context = Box::new(Context { handler });
            let mut options = ffi::SourceContext {
                version: 0, info: (&mut *context as *mut Context).cast(), retain: None,
                release: None, copy_description: None, equal: None, hash: None,
                schedule: None, cancel: None, perform: Some(perform),
            };
            // SAFETY：context 的 Box 地址稳定，先注销 source 才释放 context。
            let source = unsafe { OwnedRef::from_created(ffi::CFRunLoopSourceCreate(ptr::null(), 0, &mut options)) }
                .ok_or(Error::NativeApi("创建停止通知"))?;
            // SAFETY：source 与 RunLoop 都在当前主线程使用。
            let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
            unsafe { ffi::CFRunLoopAddSource(run_loop, source.as_ptr(), ffi::kCFRunLoopCommonModes); }
            STOP_SOURCE.with(|slot| slot.set(source.as_ptr()));
            clear_stop_request();
            Ok(Self { source, run_loop, _context: context })
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            STOP_SOURCE.with(|slot| slot.set(ptr::null_mut()));
            clear_stop_request();
            // SAFETY：先断开原生端，再释放持有 handler 的 context。
            unsafe {
                ffi::CFRunLoopRemoveSource(self.run_loop, self.source.as_ptr(), ffi::kCFRunLoopCommonModes);
                ffi::CFRunLoopSourceInvalidate(self.source.as_ptr());
            }
        }
    }

    pub fn request_stop() {
        STOP_SOURCE.with(|slot| {
            // SAFETY：主线程注册期间引用有效；仅发信号，不同步调用 handler。
            unsafe {
                let run_loop = ffi::CFRunLoopGetCurrent();
                if slot.get().is_null() { ffi::CFRunLoopStop(run_loop); }
                else {
                    STOP_PENDING.with(|pending| pending.set(true));
                    ffi::CFRunLoopSourceSignal(slot.get());
                    ffi::CFRunLoopWakeUp(run_loop);
                }
            }
        });
    }

    pub fn clear_stop_request() { STOP_PENDING.with(|pending| pending.set(false)); }

    unsafe extern "C" fn perform(info: *mut c_void) {
        if info.is_null() || !STOP_PENDING.with(|pending| pending.replace(false)) { return; }
        // SAFETY：Registration 保持 context 存活；只在当前主线程调用。
        let context = unsafe { &mut *info.cast::<Context>() };
        if catch_unwind(AssertUnwindSafe(&mut context.handler)).is_err() {
            eprintln!("KeyFlux：停止通知处理异常。");
        }
    }
}
