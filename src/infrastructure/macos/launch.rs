//! App 启动支持：同一用户只运行一份，Finder 启动时将错误留在日志中。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use crate::error::{Error, Result};

pub struct LaunchGuard {
    _lock: File,
}

impl LaunchGuard {
    pub fn acquire() -> Result<Option<Self>> {
        let home = std::env::var_os("HOME").ok_or(Error::NativeApi("缺少用户 HOME 路径"))?;
        let cache = Path::new(&home).join("Library/Caches/KeyFlux");
        std::fs::create_dir_all(&cache)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(cache.join("instance.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                eprintln!("KeyFlux 已在运行，不重复启动键盘监听。");
                return Ok(None);
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if is_bundled()? { redirect_log(&Path::new(&home).join("Library/Logs/KeyFlux"))?; }
        else if debug_enabled() {
            super::diagnostics::initialize(&std::env::current_dir()?.join("target/keyflux-debug.log"))?;
        }
        Ok(Some(Self { _lock: lock }))
    }
}

pub fn debug_enabled() -> bool {
    std::env::var_os("KEYFLUX_DEBUG").is_some_and(|value| value == "1")
        || std::env::args_os().skip(1).any(|argument| argument == "--debug")
}

fn is_bundled() -> Result<bool> {
    let executable = std::env::current_exe()?;
    Ok(executable.parent().is_some_and(|macos| {
        macos.file_name().is_some_and(|name| name == "MacOS")
            && macos.parent().is_some_and(|contents| {
                contents.file_name().is_some_and(|name| name == "Contents")
                    && contents.parent().is_some_and(|app| app.extension().is_some_and(|extension| extension == "app"))
            })
    }))
}

fn redirect_log(directory: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(directory)?;
        let log = OpenOptions::new().create(true).append(true).mode(0o600).open(directory.join("KeyFlux.log"))?;
        unsafe extern "C" { fn dup2(source: i32, target: i32) -> i32; }
        // SAFETY：dup2 复制描述符到 stderr，log 离开作用域不会关闭新描述符。
        if unsafe { dup2(log.as_raw_fd(), 2) } == -1 { return Err(std::io::Error::last_os_error().into()); }
        eprintln!("\nKeyFlux {}，进程 {}，App 启动", env!("CARGO_PKG_VERSION"), std::process::id());
    }
    #[cfg(not(target_os = "macos"))]
    let _ = directory;
    Ok(())
}
