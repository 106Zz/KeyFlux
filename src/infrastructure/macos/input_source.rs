//! TIS 来源枚举、选择及系统通知。只持有 ABC 与选定拼音的原生引用。

use crate::domain::input_source::{InputSource, SourceSnapshot};
use crate::error::{Error, Result};

pub struct InputSourceManager {
    #[cfg(target_os = "macos")]
    catalog: Option<native::Catalog>,
}

impl InputSourceManager {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            catalog: None,
        }
    }

    pub fn initialize(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.catalog = Some(native::Catalog::load()?);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        { Err(Error::UnsupportedPlatform) }
    }

    pub fn current(&self) -> Result<SourceSnapshot> {
        #[cfg(target_os = "macos")]
        { native::current_snapshot() }
        #[cfg(not(target_os = "macos"))]
        { Err(Error::UnsupportedPlatform) }
    }

    pub fn target_for(&self, current: &SourceSnapshot) -> Option<InputSource> {
        #[cfg(target_os = "macos")]
        {
            let catalog = self.catalog.as_ref()?;
            if current.id == catalog.abc.descriptor.id {
                Some(catalog.pinyin.descriptor.clone())
            } else if current.id == catalog.pinyin.descriptor.id {
                Some(catalog.abc.descriptor.clone())
            } else {
                None
            }
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = current; None }
    }

    pub fn select(&self, source: &InputSource) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.catalog.as_ref()
                .ok_or(Error::InputSourcesUnavailable("尚未初始化"))?
                .select(source)
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = source; Err(Error::UnsupportedPlatform) }
    }

    pub fn is_abc(&self, id: &str) -> bool {
        #[cfg(target_os = "macos")]
        { self.catalog.as_ref().is_some_and(|catalog| catalog.abc.descriptor.id == id) }
        #[cfg(not(target_os = "macos"))]
        { let _ = id; false }
    }

    pub fn abc(&self) -> Option<InputSource> {
        #[cfg(target_os = "macos")]
        { self.catalog.as_ref().map(|catalog| catalog.abc.descriptor.clone()) }
        #[cfg(not(target_os = "macos"))]
        None
    }
}

pub struct SourceObserver {
    #[cfg(target_os = "macos")]
    _native: native::Observer,
}

impl SourceObserver {
    pub fn new(handler: impl FnMut() + 'static) -> Result<Self> {
        #[cfg(target_os = "macos")]
        { Ok(Self { _native: native::Observer::new(Box::new(handler))? }) }
        #[cfg(not(target_os = "macos"))]
        { let _ = handler; Err(Error::UnsupportedPlatform) }
    }

    pub fn completion_result(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        if self._native.failed.get() {
            return Err(Error::MonitorStopped("输入来源通知回调发生 panic"));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::{CStr, c_void};
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::rc::Rc;

    use super::*;
    use crate::domain::input_source::is_apple_pinyin;
    use crate::infrastructure::macos::ffi::{self, NativeRef, OwnedRef};

    pub(super) struct Catalog {
        pub abc: Source,
        pub pinyin: Source,
    }

    pub(super) struct Source {
        pub descriptor: InputSource,
        handle: OwnedRef,
    }

    impl Catalog {
        pub fn load() -> Result<Self> {
            let current = current_snapshot()?;
            // SAFETY：false 只枚举启用来源，不强制加载所有已安装输入法。
            let list = unsafe { OwnedRef::from_created(ffi::TISCreateInputSourceList(ptr::null(), 0)) }
                .ok_or(Error::NativeApi("TISCreateInputSourceList"))?;
            let mut abc: Option<Source> = None;
            let mut pinyin: Option<Source> = None;
            // SAFETY：list 是 TIS 返回的有效 CFArray。
            let count = unsafe { ffi::CFArrayGetCount(list.as_ptr()) };
            for index in 0..count {
                // SAFETY：index 位于数组范围内，元素借用在 list 释放前有效。
                let source = unsafe { ffi::CFArrayGetValueAtIndex(list.as_ptr(), index) }.cast_mut();
                if !selectable(source) { continue; }
                let id = source_id(source)?;
                let is_abc = id == "com.apple.keylayout.ABC";
                let is_pinyin = is_apple_pinyin(&id);
                if !is_abc && !is_pinyin { continue; }
                if is_pinyin && pinyin.as_ref().is_some_and(|saved| {
                    saved.descriptor.id == current.id
                        || (id != current.id && !id.eq_ignore_ascii_case("com.apple.inputmethod.SCIM.ITABC"))
                }) { continue; }
                // SAFETY：来源仍由 list 持有，retain 后可在 list 释放后单独使用。
                let handle = unsafe { OwnedRef::from_borrowed(source) }
                    .ok_or(Error::NativeApi("保留 TIS 输入来源"))?;
                let source = Source { descriptor: InputSource { id }, handle };
                if is_abc { abc = Some(source); } else { pinyin = Some(source); }
            }
            Ok(Self {
                abc: abc.ok_or(Error::InputSourcesUnavailable("请先启用系统 ABC"))?,
                pinyin: pinyin.ok_or(Error::InputSourcesUnavailable("请先启用 Apple 拼音"))?,
            })
        }

        pub fn select(&self, target: &InputSource) -> Result<()> {
            let source = if target.id == self.abc.descriptor.id {
                &self.abc
            } else if target.id == self.pinyin.descriptor.id {
                &self.pinyin
            } else {
                return Err(Error::InputSourcesUnavailable("目标不是已配置的 ABC / 拼音"));
            };
            if !selectable(source.handle.as_ptr()) {
                return Err(Error::InputSourcesUnavailable("目标已被禁用或不可选择，请重新启动"));
            }
            // SAFETY：来源持有有效引用，且刚确认可选；OSStatus 必须单独检查。
            let status = unsafe { ffi::TISSelectInputSource(source.handle.as_ptr()) };
            if status == 0 { Ok(()) } else { Err(Error::InputSourceSelection(status)) }
        }
    }

    pub(super) fn current_snapshot() -> Result<SourceSnapshot> {
        // SAFETY：Copy 返回一个拥有引用，由 OwnedRef 恰好释放一次。
        let source = unsafe { OwnedRef::from_created(ffi::TISCopyCurrentKeyboardInputSource()) }
            .ok_or(Error::NativeApi("TISCopyCurrentKeyboardInputSource"))?;
        Ok(SourceSnapshot {
            id: source_id(source.as_ptr())?,
            // SAFETY：使用对应属性键读取 CFBoolean，缺失时按未确认处理。
            selected: unsafe { property_bool(source.as_ptr(), ffi::kTISPropertyInputSourceIsSelected) },
        })
    }

    fn selectable(source: NativeRef) -> bool {
        if source.is_null() { return false; }
        // SAFETY：TIS 来源引用有效；属性类型由 SDK 定义为 CFBoolean。
        unsafe {
            property_bool(source, ffi::kTISPropertyInputSourceIsEnabled)
                && property_bool(source, ffi::kTISPropertyInputSourceIsSelectCapable)
        }
    }

    unsafe fn property_bool(source: NativeRef, key: *const c_void) -> bool {
        let property = unsafe { ffi::TISGetInputSourceProperty(source, key) };
        !property.is_null() && unsafe { ffi::CFBooleanGetValue(property) != 0 }
    }

    fn source_id(source: NativeRef) -> Result<String> {
        let mut bytes = [0_u8; 1024];
        // SAFETY：ID 属性为借用 CFString，不释放它；UTF-8 输出写入有界缓冲区。
        let success = unsafe {
            let id = ffi::TISGetInputSourceProperty(source, ffi::kTISPropertyInputSourceID);
            !id.is_null() && ffi::CFStringGetCString(id, bytes.as_mut_ptr().cast(), bytes.len() as isize, 0x0800_0100) != 0
        };
        if !success { return Err(Error::NativeApi("读取 TIS 输入来源 ID")); }
        CStr::from_bytes_until_nul(&bytes).ok().and_then(|id| id.to_str().ok())
            .map(str::to_owned).ok_or(Error::NativeApi("输入来源 ID 不是有效 UTF-8"))
    }

    pub(super) struct Observer {
        center: NativeRef,
        context: Box<ObserverContext>,
        pub failed: Rc<Cell<bool>>,
    }

    struct ObserverContext {
        handler: Box<dyn FnMut()>,
        failed: Rc<Cell<bool>>,
    }

    impl Observer {
        pub fn new(handler: Box<dyn FnMut()>) -> Result<Self> {
            // SAFETY：通知中心是系统管理的借用，不需要释放。
            let center = unsafe { ffi::CFNotificationCenterGetDistributedCenter() };
            if center.is_null() { return Err(Error::NativeApi("CFNotificationCenterGetDistributedCenter")); }
            let failed = Rc::new(Cell::new(false));
            let mut context = Box::new(ObserverContext { handler, failed: Rc::clone(&failed) });
            // SAFETY：Box 提供稳定的薄指针，保留到注销 observer 之后。
            unsafe {
                ffi::CFNotificationCenterAddObserver(center, (&mut *context as *mut ObserverContext).cast(), callback, ffi::kTISNotifySelectedKeyboardInputSourceChanged, ptr::null(), 4);
            }
            Ok(Self { center, context, failed })
        }
    }

    impl Drop for Observer {
        fn drop(&mut self) {
            // SAFETY：先取消原生注册，再由 Rust 释放回调 context。
            unsafe { ffi::CFNotificationCenterRemoveEveryObserver(self.center, (&*self.context as *const ObserverContext).cast()) };
        }
    }

    unsafe extern "C" fn callback(_center: NativeRef, observer: *mut c_void, _name: *const c_void, _object: *const c_void, _info: *const c_void) {
        if observer.is_null() { return; }
        // SAFETY：Observer 持有稳定 Box，通知在主线程 RunLoop 中投递。
        let context = unsafe { &mut *observer.cast::<ObserverContext>() };
        if catch_unwind(AssertUnwindSafe(&mut context.handler)).is_err() {
            context.failed.set(true);
            // 不跨 C 边界传播 panic；延后通知应用层停止监听并释放待补发事件。
            crate::infrastructure::macos::run_loop::stop();
        }
    }
}
