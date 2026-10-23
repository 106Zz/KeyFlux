//! 非独占物理 Caps 输入监听，仅订阅键盘 UsagePage=7 / Usage=0x39。

use crate::domain::caps_state::CapsInput;
use crate::error::{Error, Result};

pub struct CapsInputMonitor {
    #[cfg(target_os = "macos")]
    native: native::Registration,
}

impl CapsInputMonitor {
    pub fn new(handler: impl FnMut(CapsInput) -> Result<()> + 'static) -> Result<Self> {
        #[cfg(target_os = "macos")]
        { Ok(Self { native: native::Registration::new(Box::new(handler))? }) }
        #[cfg(not(target_os = "macos"))]
        { let _ = handler; Err(Error::UnsupportedPlatform) }
    }

    pub fn completion_result(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        if let Some(error) = self.native.failure.borrow_mut().take() { return Err(error); }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::RefCell;
    use std::ffi::{CStr, c_void};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::rc::Rc;

    use super::*;
    use crate::infrastructure::macos::ffi::{self, NativeRef, OwnedRef};
    use crate::infrastructure::macos::hid_ffi as hid;

    pub(super) struct Registration {
        manager: OwnedRef,
        run_loop: NativeRef,
        pub failure: Rc<RefCell<Option<Error>>>,
        _context: Box<Context>,
    }

    struct Context {
        handler: Box<dyn FnMut(CapsInput) -> Result<()>>,
        failure: Rc<RefCell<Option<Error>>>,
        started_at: u64,
    }

    impl Registration {
        pub fn new(handler: Box<dyn FnMut(CapsInput) -> Result<()>>) -> Result<Self> {
            // SAFETY：0 不启用持久化属性，也不申请独占键盘。
            let manager = unsafe { OwnedRef::from_created(hid::IOHIDManagerCreate(ptr::null(), 0)) }
                .ok_or(Error::NativeApi("IOHIDManagerCreate"))?;
            let devices = matching([c"DeviceUsagePage", c"DeviceUsage"], [1, 6])?;
            let caps = matching([c"UsagePage", c"Usage"], [7, 0x39])?;
            let failure = Rc::new(RefCell::new(None));
            let mut context = Box::new(Context {
                handler,
                failure: Rc::clone(&failure),
                // SAFETY：与 IOHIDValue 时间戳使用同一 Mach 时基，仅用于排除启动前报告。
                started_at: unsafe { hid::mach_absolute_time() },
            });
            let pointer = (&mut *context as *mut Context).cast();
            // SAFETY：所有引用有效，context 保持到注销回调之后。
            let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
            let status = unsafe {
                hid::IOHIDManagerSetDeviceMatching(manager.as_ptr(), devices.as_ptr());
                hid::IOHIDManagerSetInputValueMatching(manager.as_ptr(), caps.as_ptr());
                hid::IOHIDManagerRegisterInputValueCallback(manager.as_ptr(), Some(value_callback), pointer);
                hid::IOHIDManagerRegisterDeviceRemovalCallback(manager.as_ptr(), Some(removal_callback), pointer);
                hid::IOHIDManagerScheduleWithRunLoop(manager.as_ptr(), run_loop, ffi::kCFRunLoopCommonModes);
                hid::IOHIDManagerOpen(manager.as_ptr(), 0)
            };
            if status != 0 {
                // SAFETY：打开失败也解除已注册回调，避免 context 析构后仍被调用。
                unsafe {
                    hid::IOHIDManagerRegisterInputValueCallback(manager.as_ptr(), None, ptr::null_mut());
                    hid::IOHIDManagerRegisterDeviceRemovalCallback(manager.as_ptr(), None, ptr::null_mut());
                    hid::IOHIDManagerUnscheduleFromRunLoop(manager.as_ptr(), run_loop, ffi::kCFRunLoopCommonModes);
                    hid::IOHIDManagerClose(manager.as_ptr(), 0);
                }
                return Err(Error::HidOperation { operation: "IOHIDManagerOpen", status });
            }
            Ok(Self { manager, run_loop, failure, _context: context })
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY：先断开回调与 RunLoop，再关闭 manager，最后释放 Box 和 CF 引用。
            unsafe {
                hid::IOHIDManagerRegisterInputValueCallback(self.manager.as_ptr(), None, ptr::null_mut());
                hid::IOHIDManagerRegisterDeviceRemovalCallback(self.manager.as_ptr(), None, ptr::null_mut());
                hid::IOHIDManagerUnscheduleFromRunLoop(self.manager.as_ptr(), self.run_loop, ffi::kCFRunLoopCommonModes);
                hid::IOHIDManagerClose(self.manager.as_ptr(), 0);
            }
        }
    }

    impl Context {
        fn dispatch(&mut self, input: CapsInput) {
            let result = catch_unwind(AssertUnwindSafe(|| (self.handler)(input)))
                .unwrap_or(Err(Error::MonitorStopped("Caps HID 回调发生 panic")));
            if let Err(error) = result { self.fail(error); }
        }

        fn fail(&mut self, error: Error) {
            if self.failure.borrow().is_some() { return; }
            *self.failure.borrow_mut() = Some(error);
            // 在当前 HID 回调返回后，由窗口协调器释放输入和监听资源。
            crate::infrastructure::macos::run_loop::stop();
        }
    }

    unsafe extern "C" fn value_callback(info: *mut c_void, status: i32, _sender: NativeRef, value: NativeRef) {
        if info.is_null() || value.is_null() { return; }
        // SAFETY：manager 持有回调注册，Context Box 在注销前保持有效。
        let context = unsafe { &mut *info.cast::<Context>() };
        if status != 0 { return context.fail(Error::HidOperation { operation: "HID 输入回调", status }); }
        // SAFETY：value 及其 element / device 均仅在此回调内借用，不释放或保存引用。
        let input = unsafe {
            let element = hid::IOHIDValueGetElement(value);
            if element.is_null() || hid::IOHIDElementGetUsagePage(element) != 7
                || hid::IOHIDElementGetUsage(element) != 0x39
                || !(1..=3).contains(&hid::IOHIDElementGetType(element)) { return; }
            let device = hid::IOHIDElementGetDevice(element);
            if device.is_null() { return; }
            CapsInput::Key {
                device: device as usize,
                down: hid::IOHIDValueGetIntegerValue(value) != 0,
                initial: hid::IOHIDValueGetTimeStamp(value) < context.started_at,
            }
        };
        context.dispatch(input);
    }

    unsafe extern "C" fn removal_callback(info: *mut c_void, _status: i32, _sender: NativeRef, device: NativeRef) {
        if info.is_null() || device.is_null() { return; }
        // SAFETY：使用设备引用地址作身份，不解引用已移除设备，不保留其所有权。
        let context = unsafe { &mut *info.cast::<Context>() };
        context.dispatch(CapsInput::Removed { device: device as usize });
    }

    fn matching(keys: [&CStr; 2], numbers: [i32; 2]) -> Result<OwnedRef> {
        fn string(value: &CStr) -> Result<OwnedRef> {
            // SAFETY：有效 NUL 结尾字符串；Create 引用交由 RAII 释放。
            unsafe { OwnedRef::from_created(hid::CFStringCreateWithCString(ptr::null(), value.as_ptr(), 0x0800_0100)) }
                .ok_or(Error::NativeApi("CFStringCreateWithCString"))
        }
        fn number(value: i32) -> Result<OwnedRef> {
            // SAFETY：kCFNumberSInt32Type=3，输入指针指向有效 i32。
            unsafe { OwnedRef::from_created(hid::CFNumberCreate(ptr::null(), 3, (&value as *const i32).cast())) }
                .ok_or(Error::NativeApi("CFNumberCreate"))
        }
        let keys = [string(keys[0])?, string(keys[1])?];
        let values = [number(numbers[0])?, number(numbers[1])?];
        let key_pointers = keys.each_ref().map(|key| key.as_ptr().cast_const());
        let value_pointers = values.each_ref().map(|value| value.as_ptr().cast_const());
        // SAFETY：标准 CFType 回调会 retain 键和值，局部引用释放后字典仍有效。
        unsafe {
            OwnedRef::from_created(hid::CFDictionaryCreate(ptr::null(), key_pointers.as_ptr(), value_pointers.as_ptr(), 2, &hid::kCFTypeDictionaryKeyCallBacks, &hid::kCFTypeDictionaryValueCallBacks))
        }.ok_or(Error::NativeApi("CFDictionaryCreate"))
    }
}
