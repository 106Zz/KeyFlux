//! 权限检查与首次系统授权提示；不创建自定义窗口，也不自动授予权限。

#[cfg(not(target_os = "macos"))]
use crate::error::Error;
use crate::error::Result;

pub struct PermissionStatus {
    pub accessibility: bool,
    pub post_event: bool,
    pub input_monitoring: bool,
}

impl PermissionStatus {
    pub fn check() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            use super::ffi;

            // SAFETY：无参数的系统权限查询，不触发授权窗口或修改设置。
            let status = unsafe {
                Self {
                    accessibility: ffi::AXIsProcessTrusted() != 0,
                    post_event: ffi::CGPreflightPostEventAccess(),
                    input_monitoring: ffi::CGPreflightListenEventAccess(),
                }
            };
            Ok(status)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::UnsupportedPlatform)
        }
    }

    pub fn is_granted(&self) -> bool {
        self.accessibility && self.post_event && self.input_monitoring
    }

    pub fn request_missing(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            use super::{ffi, hid_ffi};
            use std::ptr;
            if !self.accessibility {
                // SAFETY：使用系统常量创建只在本次调用有效的 CFDictionary。
                let options = unsafe {
                    let keys = [ffi::kAXTrustedCheckOptionPrompt];
                    let values = [ffi::kCFBooleanTrue];
                    ffi::OwnedRef::from_created(hid_ffi::CFDictionaryCreate(ptr::null(), keys.as_ptr(), values.as_ptr(), 1,
                        &hid_ffi::kCFTypeDictionaryKeyCallBacks, &hid_ffi::kCFTypeDictionaryValueCallBacks))
                }.ok_or(crate::error::Error::NativeApi("创建系统授权选项"))?;
                // SAFETY：只请求系统向用户显示授权提示，返回值不代表提示后已授权。
                unsafe { ffi::AXIsProcessTrustedWithOptions(options.as_ptr()); }
            }
            // SAFETY：仅请求系统权限提示；授权仍必须由用户在系统设置中完成。
            unsafe {
                if !self.post_event { ffi::CGRequestPostEventAccess(); }
                if !self.input_monitoring { hid_ffi::IOHIDRequestAccess(1); }
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        Err(Error::UnsupportedPlatform)
    }
}
