//! 调试模式同时记录到终端和项目日志；只收控制状态，不记录输入文字。

use std::cell::RefCell;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::error::Result;

thread_local! { static LOG: RefCell<Option<File>> = const { RefCell::new(None) }; }

pub fn initialize(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    LOG.with(|slot| *slot.borrow_mut() = Some(file));
    record(format_args!("诊断日志：{}（追加保存；不记录输入文字）", path.display()));
    Ok(())
}

pub fn record(message: fmt::Arguments<'_>) {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let line = format!("[{}.{:03} pid={}] {message}", elapsed.as_secs(), elapsed.subsec_millis(), std::process::id());
    eprintln!("{line}");
    LOG.with(|slot| {
        let mut log = slot.borrow_mut();
        if let Some(file) = log.as_mut()
            && let Err(error) = writeln!(file, "{line}") {
            eprintln!("KeyFlux：诊断日志写入失败：{error}");
            *log = None;
        }
    });
}
