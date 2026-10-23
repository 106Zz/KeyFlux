//! 最小原生声明，按本机 SDK 头文件核对。仅此层暴露原生引用。

use std::ffi::{c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

pub(super) type NativeRef = *mut c_void;
pub(super) type EventCallback = unsafe extern "C" fn(
    NativeRef,
    u32,
    NativeRef,
    *mut c_void,
) -> NativeRef;

pub(super) const KEY_DOWN: u32 = 10;
pub(super) const KEY_UP: u32 = 11;
pub(super) const FLAGS_CHANGED: u32 = 12;
pub(super) const TAP_DISABLED_TIMEOUT: u32 = 0xffff_fffe;
pub(super) const TAP_DISABLED_USER: u32 = 0xffff_ffff;
pub(super) const CAPS_LOCK: u64 = 1 << 16;
pub(super) const SHIFT: u64 = 1 << 17;
pub(super) const OTHER_MODIFIERS: u64 = (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23);
pub(super) const COMBINED_SESSION: i32 = 0;
pub(super) const SESSION_EVENT_TAP: u32 = 1;

pub(super) type NotificationCallback = unsafe extern "C" fn(
    NativeRef, *mut c_void, *const c_void, *const c_void, *const c_void,
);

#[repr(C)]
pub(super) struct TimerContext {
    pub version: isize,
    pub info: *mut c_void,
    pub retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    pub release: Option<unsafe extern "C" fn(*const c_void)>,
    pub copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
}

#[repr(C)]
pub(super) struct SourceContext {
    pub version: isize,
    pub info: *mut c_void,
    pub retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    pub release: Option<unsafe extern "C" fn(*const c_void)>,
    pub copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    pub equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
    pub hash: Option<unsafe extern "C" fn(*const c_void) -> usize>,
    pub schedule: Option<unsafe extern "C" fn(*mut c_void, NativeRef, *const c_void)>,
    pub cancel: Option<unsafe extern "C" fn(*mut c_void, NativeRef, *const c_void)>,
    pub perform: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CGPreflightListenEventAccess() -> bool;
    pub(super) fn CGPreflightPostEventAccess() -> bool;
    pub(super) fn CGRequestPostEventAccess() -> bool;
    pub(super) fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events: u64,
        callback: Option<EventCallback>,
        user_info: *mut c_void,
    ) -> NativeRef;
    pub(super) fn CGEventTapEnable(tap: NativeRef, enabled: bool);
    pub(super) fn CGEventGetFlags(event: NativeRef) -> u64;
    pub(super) fn CGEventGetType(event: NativeRef) -> u32;
    pub(super) fn CGEventSetFlags(event: NativeRef, flags: u64);
    pub(super) fn CGEventKeyboardSetUnicodeString(event: NativeRef, length: usize, text: *const u16);
    pub(super) fn CGEventGetIntegerValueField(event: NativeRef, field: u32) -> i64;
    pub(super) fn CGEventCreateCopy(event: NativeRef) -> NativeRef;
    pub(super) fn CGEventSetIntegerValueField(event: NativeRef, field: u32, value: i64);
    pub(super) fn CGEventPost(tap: u32, event: NativeRef);
    pub(super) fn CGEventSourceFlagsState(state: i32) -> u64;
    pub(super) fn CGEventSourceCounterForEventType(state: i32, event_type: u32) -> u32;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub(super) fn AXIsProcessTrusted() -> u8;
    pub(super) fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    pub(super) static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    // Carbon 的 Boolean 是 UInt8，与 CGEvent 使用的 C bool 分开声明。
    pub(super) fn IsSecureEventInputEnabled() -> u8;
    pub(super) static kTISPropertyInputSourceID: *const c_void;
    pub(super) static kTISPropertyInputSourceIsEnabled: *const c_void;
    pub(super) static kTISPropertyInputSourceIsSelectCapable: *const c_void;
    pub(super) static kTISPropertyInputSourceIsSelected: *const c_void;
    pub(super) static kTISNotifySelectedKeyboardInputSourceChanged: *const c_void;
    pub(super) fn TISCreateInputSourceList(properties: *const c_void, all: u8) -> NativeRef;
    pub(super) fn TISCopyCurrentKeyboardInputSource() -> NativeRef;
    pub(super) fn TISGetInputSourceProperty(source: NativeRef, key: *const c_void) -> NativeRef;
    pub(super) fn TISSelectInputSource(source: NativeRef) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCFBooleanTrue: *const c_void;
    pub(super) static kCFRunLoopCommonModes: *const c_void;
    pub(super) fn CFRelease(object: *const c_void);
    pub(super) fn CFRetain(object: *const c_void) -> *const c_void;
    pub(super) fn CFArrayGetCount(array: *const c_void) -> isize;
    pub(super) fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    pub(super) fn CFBooleanGetValue(value: *const c_void) -> u8;
    pub(super) fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    pub(super) fn CFNotificationCenterGetDistributedCenter() -> NativeRef;
    pub(super) fn CFNotificationCenterAddObserver(center: NativeRef, observer: *const c_void, callback: NotificationCallback, name: *const c_void, object: *const c_void, behavior: isize);
    pub(super) fn CFNotificationCenterRemoveEveryObserver(center: NativeRef, observer: *const c_void);
    pub(super) fn CFAbsoluteTimeGetCurrent() -> f64;
    pub(super) fn CFRunLoopTimerCreate(allocator: *const c_void, fire_date: f64, interval: f64, flags: usize, order: isize, callback: unsafe extern "C" fn(NativeRef, *mut c_void), context: *mut TimerContext) -> NativeRef;
    pub(super) fn CFRunLoopAddTimer(run_loop: NativeRef, timer: NativeRef, mode: *const c_void);
    pub(super) fn CFRunLoopRemoveTimer(run_loop: NativeRef, timer: NativeRef, mode: *const c_void);
    pub(super) fn CFRunLoopTimerSetNextFireDate(timer: NativeRef, fire_date: f64);
    pub(super) fn CFRunLoopTimerInvalidate(timer: NativeRef);
    pub(super) fn CFMachPortInvalidate(port: NativeRef);
    pub(super) fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: NativeRef,
        order: isize,
    ) -> NativeRef;
    pub(super) fn CFRunLoopGetCurrent() -> NativeRef;
    pub(super) fn CFRunLoopAddSource(
        run_loop: NativeRef,
        source: NativeRef,
        mode: *const c_void,
    );
    pub(super) fn CFRunLoopRemoveSource(
        run_loop: NativeRef,
        source: NativeRef,
        mode: *const c_void,
    );
    pub(super) fn CFRunLoopStop(run_loop: NativeRef);
    pub(super) fn CFRunLoopSourceCreate(allocator: *const c_void, order: isize, context: *mut SourceContext) -> NativeRef;
    pub(super) fn CFRunLoopSourceSignal(source: NativeRef);
    pub(super) fn CFRunLoopSourceInvalidate(source: NativeRef);
    pub(super) fn CFRunLoopWakeUp(run_loop: NativeRef);
}

/// 接管 Create 系列返回的拥有引用；禁止跨线程移动原生事件资源。
pub(super) struct OwnedRef {
    pointer: NonNull<c_void>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl OwnedRef {
    /// 把借用的有效 CF 引用保留为独立拥有引用。
    pub(super) unsafe fn from_borrowed(pointer: NativeRef) -> Option<Self> {
        if pointer.is_null() { return None; }
        // SAFETY：调用方保证引用有效，retain 新增一次释放责任。
        unsafe { CFRetain(pointer) };
        unsafe { Self::from_created(pointer) }
    }

    /// 调用方必须传入拥有一次释放责任的 CF 引用，不能传入 Get 返回的借用。
    pub(super) unsafe fn from_created(pointer: NativeRef) -> Option<Self> {
        NonNull::new(pointer).map(|pointer| Self {
            pointer,
            _thread_bound: PhantomData,
        })
    }

    pub(super) fn as_ptr(&self) -> NativeRef {
        self.pointer.as_ptr()
    }
}

impl Drop for OwnedRef {
    fn drop(&mut self) {
        // SAFETY：引用由 from_created 接管且不提供 Clone，恰好释放一次。
        unsafe { CFRelease(self.pointer.as_ptr()) };
    }
}
