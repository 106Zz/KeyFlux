//! IOKit / HID 声明，与键盘事件 FFI 分开管理。

use std::ffi::{c_char, c_void};
use super::ffi::NativeRef;

pub(super) type ValueCallback = unsafe extern "C" fn(*mut c_void, i32, NativeRef, NativeRef);
pub(super) type DeviceCallback = unsafe extern "C" fn(*mut c_void, i32, NativeRef, NativeRef);

#[repr(C)]
pub(super) struct DictionaryKeys {
    version: isize,
    retain: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void, *const c_void)>,
    description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
    hash: Option<unsafe extern "C" fn(*const c_void) -> usize>,
}

#[repr(C)]
pub(super) struct DictionaryValues {
    version: isize,
    retain: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void, *const c_void)>,
    description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
}

#[repr(C)]
pub(super) struct ProcessSerialNumber { pub high: u32, pub low: u32 }

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub(super) fn IOServiceMatching(name: *const c_char) -> NativeRef;
    pub(super) fn IOServiceGetMatchingService(port: u32, matching: NativeRef) -> u32;
    pub(super) fn IOServiceOpen(service: u32, task: u32, kind: u32, connection: *mut u32) -> i32;
    pub(super) fn IOServiceClose(connection: u32) -> i32;
    pub(super) fn IOObjectRelease(object: u32) -> i32;
    pub(super) fn IOHIDGetModifierLockState(connection: u32, selector: i32, state: *mut bool) -> i32;
    pub(super) fn IOHIDSetModifierLockState(connection: u32, selector: i32, state: bool) -> i32;
    // IOKit 导出的 SPI；类型来自 WebKit IOKitSPIMac.h，枚举 Passive=2。
    // IOHIDLib.c 的公开 Caps setter 本身使用同一服务接口。
    pub(super) fn IOHIDEventSystemClientCreateWithType(allocator: *const c_void, kind: i32, attributes: *const c_void) -> NativeRef;
    pub(super) fn IOHIDEventSystemClientCopyServices(client: NativeRef) -> NativeRef;
    pub(super) fn IOHIDServiceClientConformsTo(service: NativeRef, page: u32, usage: u32) -> bool;
    pub(super) fn IOHIDServiceClientCopyProperty(service: NativeRef, key: *const c_void) -> NativeRef;
    pub(super) fn IOHIDRequestAccess(request_type: u32) -> bool;
    pub(super) fn IOHIDManagerCreate(allocator: *const c_void, options: u32) -> NativeRef;
    pub(super) fn IOHIDManagerSetDeviceMatching(manager: NativeRef, matching: NativeRef);
    pub(super) fn IOHIDManagerSetInputValueMatching(manager: NativeRef, matching: NativeRef);
    pub(super) fn IOHIDManagerOpen(manager: NativeRef, options: u32) -> i32;
    pub(super) fn IOHIDManagerClose(manager: NativeRef, options: u32) -> i32;
    pub(super) fn IOHIDManagerScheduleWithRunLoop(manager: NativeRef, run_loop: NativeRef, mode: *const c_void);
    pub(super) fn IOHIDManagerUnscheduleFromRunLoop(manager: NativeRef, run_loop: NativeRef, mode: *const c_void);
    pub(super) fn IOHIDManagerRegisterInputValueCallback(manager: NativeRef, callback: Option<ValueCallback>, context: *mut c_void);
    pub(super) fn IOHIDManagerRegisterDeviceRemovalCallback(manager: NativeRef, callback: Option<DeviceCallback>, context: *mut c_void);
    pub(super) fn IOHIDValueGetElement(value: NativeRef) -> NativeRef;
    pub(super) fn IOHIDValueGetIntegerValue(value: NativeRef) -> isize;
    pub(super) fn IOHIDValueGetTimeStamp(value: NativeRef) -> u64;
    pub(super) fn IOHIDElementGetDevice(element: NativeRef) -> NativeRef;
    pub(super) fn IOHIDElementGetUsagePage(element: NativeRef) -> u32;
    pub(super) fn IOHIDElementGetUsage(element: NativeRef) -> u32;
    pub(super) fn IOHIDElementGetType(element: NativeRef) -> u32;
}

#[link(name = "System")]
unsafe extern "C" {
    pub(super) static mach_task_self_: u32;
    pub(super) fn mach_absolute_time() -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCFTypeDictionaryKeyCallBacks: DictionaryKeys;
    pub(super) static kCFTypeDictionaryValueCallBacks: DictionaryValues;
    pub(super) fn CFStringCreateWithCString(allocator: *const c_void, value: *const c_char, encoding: u32) -> NativeRef;
    pub(super) fn CFGetTypeID(value: *const c_void) -> usize;
    pub(super) fn CFBooleanGetTypeID() -> usize;
    pub(super) fn CFNumberCreate(allocator: *const c_void, kind: isize, value: *const c_void) -> NativeRef;
    pub(super) fn CFDictionaryCreate(allocator: *const c_void, keys: *const *const c_void, values: *const *const c_void, count: isize, key_callbacks: *const DictionaryKeys, value_callbacks: *const DictionaryValues) -> NativeRef;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub(super) fn GetFrontProcess(process: *mut ProcessSerialNumber) -> i16;
    pub(super) fn GetProcessPID(process: *const ProcessSerialNumber, pid: *mut i32) -> i32;
}
