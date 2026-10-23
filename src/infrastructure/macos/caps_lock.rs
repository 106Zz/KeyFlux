//! 协调中/英键与会话 Caps 状态，同时保留 IOKit 观测值；不模拟永久按住 Shift。

use crate::error::{Error, Result};
use crate::domain::caps_state::{CapsObservation, CapsPresentation};

pub type CapsOutput = std::rc::Rc<std::cell::Cell<CapsPresentation>>;

pub struct CapsLockController {
    /// None 表示还没有从系统获取状态，不能视为已经关闭大写。
    observation: Option<CapsObservation>,
    #[cfg(target_os = "macos")]
    connection: Option<native::Connection>,
}

impl CapsLockController {
    pub fn new() -> Self {
        Self {
            observation: None,
            #[cfg(target_os = "macos")]
            connection: None,
        }
    }

    /// 读取服务属性、旧 IOHIDSystem 与会话状态，不修改系统设置。
    pub fn synchronize(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            if self.connection.is_none() { self.connection = Some(native::Connection::open()?); }
            self.observation = Some(CapsObservation {
                io_enabled: self.connection.as_ref().ok_or(Error::NativeApi("Caps 连接未建立"))?.read()?,
                session_enabled: session_caps_enabled(),
                service_enabled: native::service_caps_enabled()?,
            });
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::UnsupportedPlatform)
        }
    }

    pub fn enabled(&self) -> Option<bool> {
        self.observation.map(|state| state.session_enabled)
    }

    pub fn observation(&self) -> Option<CapsObservation> { self.observation }

    pub fn disconnect(&mut self) {
        self.observation = None;
        #[cfg(target_os = "macos")]
        { self.connection = None; }
    }

    /// 写入只代表系统接受请求；由应用层在后续 RunLoop tick 独立读取确认。
    pub fn write_enabled(&mut self, enabled: bool) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let connection = self.connection.as_ref().ok_or(Error::NativeApi("Caps 连接未建立"))?;
            connection.write(enabled)
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = enabled; Err(Error::UnsupportedPlatform) }
    }
}

pub fn session_caps_enabled() -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY：只读取 WindowServer 的会话标志，用于与 IOKit 回读对照。
    unsafe { super::ffi::CGEventSourceFlagsState(super::ffi::COMBINED_SESSION) & super::ffi::CAPS_LOCK != 0 }
    #[cfg(not(target_os = "macos"))]
    false
}

pub fn shortcut_pressed() -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY：读取会话修饰键快照，不修改系统状态。
    unsafe { super::ffi::CGEventSourceFlagsState(super::ffi::COMBINED_SESSION) & super::ffi::OTHER_MODIFIERS != 0 }
    #[cfg(not(target_os = "macos"))]
    false
}

/// 只在 Caps 请求时查询前台进程，用于诊断当前输入上下文。
pub fn front_pid() -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        let mut process = super::hid_ffi::ProcessSerialNumber { high: 0, low: 0 };
        let mut pid = 0;
        // SAFETY：使用 SDK 声明的缓冲区与返回类型，不创建 AppKit / GUI 对象。
        let success = unsafe { super::hid_ffi::GetFrontProcess(&mut process) == 0
            && super::hid_ffi::GetProcessPID(&process, &mut pid) == 0 };
        if success && pid > 0 { Some(pid) } else { None }
    }
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn adjusted_case(flags: u64, code: u16, text_key: bool, presentation: CapsPresentation) -> (u64, Option<u16>) {
    const CAPS: u64 = 1 << 16;
    const SHIFT: u64 = 1 << 17;
    const SHORTCUT: u64 = (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23);
    let Some(enabled) = presentation.enabled else { return (flags, None); };
    let effective = if enabled { flags | CAPS } else { flags & !CAPS };
    if !text_key || !enabled || !presentation.abc_active || flags & SHIFT == 0 || flags & SHORTCUT != 0 {
        return (effective, None);
    }
    let letter = match code {
        0 => b'a', 1 => b's', 2 => b'd', 3 => b'f', 4 => b'h', 5 => b'g',
        6 => b'z', 7 => b'x', 8 => b'c', 9 => b'v', 11 => b'b', 12 => b'q',
        13 => b'w', 14 => b'e', 15 => b'r', 16 => b'y', 17 => b't', 31 => b'o',
        32 => b'u', 34 => b'i', 35 => b'p', 37 => b'l', 38 => b'j', 40 => b'k',
        45 => b'n', 46 => b'm', _ => return (effective, None),
    };
    (effective & !(CAPS | SHIFT | 0x6), Some(u16::from(letter)))
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::ptr;
    use crate::infrastructure::macos::ffi::{self, OwnedRef};
    use crate::infrastructure::macos::hid_ffi as hid;

    /// 公开 setter 写入 HIDCapsLockState，公开 getter 却读取旧 eventFlags。
    /// 从 setter 的同一服务属性确认，避免外接键盘短按没有原生 Caps 事件时误报。
    /// 每次重新获取服务快照，支持键盘热插拔；不注册额外事件或空闲轮询。
    pub(super) fn service_caps_enabled() -> Result<Option<bool>> {
        // SAFETY：Passive=2；Create/Copy 所有权均由 OwnedRef 释放。
        let client = unsafe { OwnedRef::from_created(hid::IOHIDEventSystemClientCreateWithType(ptr::null(), 2, ptr::null())) }
            .ok_or(Error::NativeApi("创建 Caps 服务查询客户端"))?;
        let Some(services) = (unsafe { OwnedRef::from_created(hid::IOHIDEventSystemClientCopyServices(client.as_ptr())) }) else {
            return Ok(None);
        };
        let key = unsafe { OwnedRef::from_created(hid::CFStringCreateWithCString(ptr::null(), c"HIDCapsLockState".as_ptr(), 0x0800_0100)) }
            .ok_or(Error::NativeApi("创建 Caps 属性名"))?;
        let mut state = None;
        // SAFETY：数组持有借用的服务，CopyProperty 引用独立释放，先检查 CF 类型。
        unsafe {
            for index in 0..ffi::CFArrayGetCount(services.as_ptr()) {
                let service = ffi::CFArrayGetValueAtIndex(services.as_ptr(), index).cast_mut();
                if service.is_null() || !hid::IOHIDServiceClientConformsTo(service, 1, 6) { continue; }
                let Some(value) = OwnedRef::from_created(hid::IOHIDServiceClientCopyProperty(service, key.as_ptr())) else { return Ok(None); };
                if hid::CFGetTypeID(value.as_ptr()) != hid::CFBooleanGetTypeID() { return Ok(None); }
                let enabled = ffi::CFBooleanGetValue(value.as_ptr()) != 0;
                if state.is_some_and(|previous| previous != enabled) { return Ok(None); }
                state = Some(enabled);
            }
        }
        Ok(state)
    }

    pub(super) struct Connection(u32);

    impl Connection {
        pub fn open() -> Result<Self> {
            // SAFETY：matching 的拥有引用由 IOServiceGetMatchingService 消耗。
            let service = unsafe {
                let matching = hid::IOServiceMatching(c"IOHIDSystem".as_ptr());
                if matching.is_null() { return Err(Error::NativeApi("IOServiceMatching")); }
                hid::IOServiceGetMatchingService(0, matching)
            };
            if service == 0 { return Err(Error::NativeApi("找不到 IOHIDSystem")); }
            let mut connection = 0;
            // SAFETY：仅建立当前进程的参数连接，不独占设备，也不修改持久偏好。
            let status = unsafe {
                let status = hid::IOServiceOpen(service, hid::mach_task_self_, 1, &mut connection);
                hid::IOObjectRelease(service);
                status
            };
            if status != 0 { return Err(Error::HidOperation { operation: "IOServiceOpen", status }); }
            if connection == 0 { return Err(Error::NativeApi("IOServiceOpen 返回空连接")); }
            Ok(Self(connection))
        }

        pub fn read(&self) -> Result<bool> {
            let mut enabled = false;
            // SAFETY：有效连接与 C bool 输出缓冲区，selector=1 为 Caps Lock。
            let status = unsafe { hid::IOHIDGetModifierLockState(self.0, 1, &mut enabled) };
            if status == 0 { Ok(enabled) } else { Err(Error::HidOperation { operation: "IOHIDGetModifierLockState", status }) }
        }

        pub fn write(&self, enabled: bool) -> Result<()> {
            // SAFETY：只设置当前真实 Caps 状态，不使用持久键位映射。
            let status = unsafe { hid::IOHIDSetModifierLockState(self.0, 1, enabled) };
            if status == 0 { Ok(()) } else { Err(Error::HidOperation { operation: "IOHIDSetModifierLockState", status }) }
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            // SAFETY：连接只 Close 一次，不能对连接执行 IOObjectRelease。
            unsafe { hid::IOServiceClose(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CAPS: u64 = 1 << 16;
    const SHIFT: u64 = 1 << 17;
    fn output() -> CapsPresentation { CapsPresentation { enabled: Some(true), abc_active: true } }

    #[test]
    fn caps_shift_letter_is_lowercase_but_digits_and_shortcuts_keep_shift() {
        assert_eq!(adjusted_case(CAPS | SHIFT | 0x2, 0, true, output()), (0, Some(u16::from(b'a'))));
        assert_eq!(adjusted_case(CAPS | SHIFT, 18, true, output()), (CAPS | SHIFT, None));
        let shortcut = CAPS | SHIFT | (1 << 20);
        assert_eq!(adjusted_case(shortcut, 0, true, output()), (shortcut, None));
    }

    #[test]
    fn secure_or_non_abc_context_does_not_force_latin_text() {
        assert_eq!(adjusted_case(SHIFT, 0, true, CapsPresentation::default()), (SHIFT, None));
        assert_eq!(adjusted_case(SHIFT, 0, true, CapsPresentation { enabled: Some(true), abc_active: false }), (CAPS | SHIFT, None));
    }
}
