//! 当前用户的登录自启配置；仅菜单操作时读写，不注册常驻辅助进程。

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const LABEL: &str = "org.keyflux.KeyFlux.login";
const MARKER: &str = "<!-- KeyFlux managed login item v1 -->";
const MAX_CONFIG_BYTES: u64 = 16_384;

pub(crate) struct Status {
    pub enabled: bool,
    pub current_location: bool,
}

struct Configuration {
    file: PathBuf,
    arguments: String,
    log: PathBuf,
}

pub(crate) fn status() -> Result<Status, String> {
    let configuration = Configuration::current()?;
    match configuration.read()? {
        Some(contents) => Ok(Status {
            enabled: enabled_value(&contents)?,
            current_location: contents.contains(&configuration.arguments),
        }),
        None => Ok(Status { enabled: false, current_location: true }),
    }
}

pub(crate) fn set_enabled(enabled: bool) -> Result<Status, String> {
    let configuration = Configuration::current()?;
    // 先检查文件归属与格式，绝不覆盖其他工具创建或已被改坏的配置。
    let existing = configuration.read()?;
    let contents = if enabled {
        let contents = configuration.render()?;
        fs::create_dir_all(configuration.log.parent().ok_or("无法确定日志目录")?).map_err(io_error)?;
        contents
    } else {
        let Some(contents) = existing else {
            return Ok(Status { enabled: false, current_location: true });
        };
        // 同时撤销 RunAtLoad；即使系统外部覆盖了 Disabled，也没有登录启动触发器。
        contents.replacen("<key>Disabled</key>\n    <false/>", "<key>Disabled</key>\n    <true/>", 1)
            .replacen("<key>RunAtLoad</key><true/>", "<key>RunAtLoad</key><false/>", 1)
    };
    configuration.write(&contents)?;
    status()
}

impl Configuration {
    fn current() -> Result<Self, String> {
        let home = std::env::var_os("HOME").ok_or("无法确定用户目录")?;
        let library = Path::new(&home).join("Library");
        let executable = std::env::current_exe().map_err(io_error)?;
        // App 通过 Launch Services 打开，保持其权限身份；开发二进制直接启动。
        let arguments = if let Some(app) = app_bundle(&executable) {
            format!("<array><string>/usr/bin/open</string><string>-g</string><string>{}</string></array>", xml_path(app)?)
        } else {
            format!("<array><string>{}</string></array>", xml_path(&executable)?)
        };
        Ok(Self {
            file: library.join("LaunchAgents").join(format!("{LABEL}.plist")),
            arguments: format!("<key>ProgramArguments</key>\n    {arguments}"),
            log: library.join("Logs/KeyFlux/KeyFlux.log"),
        })
    }

    fn read(&self) -> Result<Option<String>, String> {
        let metadata = match fs::symlink_metadata(&self.file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return Err("自启配置不是有效的 KeyFlux 文件，未进行修改".into());
        }
        let mut contents = String::new();
        // O_NOFOLLOW（macOS）：拒绝跟随最终路径上的符号链接。
        OpenOptions::new().read(true).custom_flags(0x100).open(&self.file)
            .map_err(io_error)?.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut contents).map_err(io_error)?;
        if contents.len() as u64 > MAX_CONFIG_BYTES { return Err("自启配置过大，未进行修改".into()); }
        enabled_value(&contents)?;
        Ok(Some(contents))
    }

    fn render(&self) -> Result<String, String> {
        let log = xml_path(&self.log)?;
        Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
{MARKER}\n<plist version=\"1.0\">\n<dict>\n    \
<key>Label</key><string>{LABEL}</string>\n    \
{}\n    \
<key>Disabled</key>\n    <false/>\n    \
<key>RunAtLoad</key><true/>\n    \
<key>KeepAlive</key><false/>\n    \
<key>LimitLoadToSessionType</key><string>Aqua</string>\n    \
<key>StandardOutPath</key><string>{}</string>\n    \
<key>StandardErrorPath</key><string>{}</string>\n\
</dict>\n</plist>\n", self.arguments, log, log))
    }

    fn write(&self, contents: &str) -> Result<(), String> {
        let directory = self.file.parent().ok_or("无法确定自启配置目录")?;
        fs::create_dir_all(directory).map_err(io_error)?;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?.as_nanos();
        let temporary = directory.join(format!(".{LABEL}.{}.{nonce}.tmp", std::process::id()));
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(&temporary).map_err(io_error)?;
        let result = file.write_all(contents.as_bytes()).and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&temporary, &self.file));
        if let Err(error) = result {
            // 只清理本次 create_new 成功创建的临时文件，不删除任何已有配置。
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        Ok(())
    }
}

fn enabled_value(contents: &str) -> Result<bool, String> {
    if !contents.starts_with(&format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{MARKER}\n<plist version=\"1.0\">\n<dict>\n"))
        || !contents.ends_with("</dict>\n</plist>\n")
        || !contents.contains(&format!("<key>Label</key><string>{LABEL}</string>"))
        || contents.matches("<key>").count() != 8
        || contents.matches("<key>Disabled</key>").count() != 1
        || !contents.contains("<key>ProgramArguments</key>\n    <array><string>")
        || contents.matches("<key>RunAtLoad</key>").count() != 1
        || !contents.contains("<key>KeepAlive</key><false/>")
        || !contents.contains("<key>LimitLoadToSessionType</key><string>Aqua</string>")
        || !contents.contains("<key>StandardOutPath</key><string>")
        || !contents.contains("<key>StandardErrorPath</key><string>") {
        return Err("自启配置不属于 KeyFlux 或格式已变化，未进行修改".into());
    }
    if contents.contains("<key>Disabled</key>\n    <false/>")
        && contents.contains("<key>RunAtLoad</key><true/>") { Ok(true) }
    else if contents.contains("<key>Disabled</key>\n    <true/>")
        && contents.contains("<key>RunAtLoad</key><false/>") { Ok(false) }
    else { Err("无法识别自启配置状态，未进行修改".into()) }
}

fn app_bundle(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then_some(app)
}

fn xml_path(path: &Path) -> Result<String, String> {
    let value = path.to_str().ok_or("自启路径必须为有效 UTF-8")?;
    if value.chars().any(char::is_control) { return Err("自启路径不能包含控制字符".into()); }
    Ok(value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&apos;"))
}

fn io_error(error: std::io::Error) -> String { format!("无法更新登录自启：{error}") }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_and_ambiguous_configs_are_rejected() {
        let valid = Configuration { file: PathBuf::new(),
            arguments: "<key>ProgramArguments</key>\n    <array><string>/project/KeyFlux</string></array>".into(),
            log: PathBuf::from("/user/Library/Logs/KeyFlux/KeyFlux.log") }.render().unwrap();
        assert_eq!(enabled_value(&valid), Ok(true));
        let disabled = valid.replacen("<key>Disabled</key>\n    <false/>", "<key>Disabled</key>\n    <true/>", 1)
            .replacen("<key>RunAtLoad</key><true/>", "<key>RunAtLoad</key><false/>", 1);
        assert_eq!(enabled_value(&disabled), Ok(false));
        assert!(enabled_value(&valid.replacen("<key>RunAtLoad</key><true/>", "<key>RunAtLoad</key><false/>", 1)).is_err());
        assert!(enabled_value(&valid.replace(MARKER, "")).is_err());
        assert!(enabled_value(&valid.replace(LABEL, "another.application")).is_err());
        assert!(enabled_value(&format!("{valid}\n<key>Disabled</key>\n    <true/>")).is_err());
        assert!(enabled_value(&valid.replace("<false/>", "not-a-boolean")).is_err());
    }

    #[test]
    fn special_paths_cannot_inject_plist_keys() {
        assert_eq!(xml_path(Path::new("/Users/测试/a & <b> \"c\" 'd'/KeyFlux")).unwrap(),
            "/Users/测试/a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos;/KeyFlux");
        assert!(xml_path(Path::new("/tmp/line\nbreak/KeyFlux")).is_err());
        assert_eq!(app_bundle(Path::new("/Applications/KeyFlux.app/Contents/MacOS/KeyFlux")),
            Some(Path::new("/Applications/KeyFlux.app")));
        assert!(app_bundle(Path::new("/project/target/release/KeyFlux")).is_none());
    }

    #[test]
    fn macos_accepts_generated_login_plist_without_writing_system_settings() {
        use std::process::{Command, Stdio};
        let configuration = Configuration { file: PathBuf::new(),
            arguments: format!("<key>ProgramArguments</key>\n    <array><string>{}</string></array>",
                xml_path(Path::new("/Users/测试/a & <b>/KeyFlux")).unwrap()),
            log: PathBuf::from("/Users/测试/Library/Logs/a & b/KeyFlux.log") };
        let mut command = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", "-"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        command.stdin.take().unwrap().write_all(configuration.render().unwrap().as_bytes()).unwrap();
        let result = command.wait_with_output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let json: String = String::from_utf8(result.stdout).unwrap().chars().filter(|c| !c.is_whitespace()).collect();
        assert!(json.contains("\"Disabled\":false"));
        assert!(json.contains("\"RunAtLoad\":true"));
        assert!(json.contains("\"KeepAlive\":false"));
    }
}
