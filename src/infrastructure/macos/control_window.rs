//! AppKit 菜单栏适配；状态、权限和使用说明均在菜单中展示，不创建弹窗。

use crate::error::{Error, Result};

#[derive(Clone, Copy)]
pub(crate) enum ControlAction { Refresh, Start, Stop, Permissions, MonitorFailed, Recover }

pub(crate) struct ControlState {
    pub running: bool,
    pub message: String,
    pub permissions: String,
    pub permissions_granted: bool,
    pub recovery_delay: Option<std::time::Duration>,
}

pub(crate) fn run(handler: impl FnMut(ControlAction) -> ControlState + 'static) -> Result<()> {
    #[cfg(target_os = "macos")]
    { native::run(Box::new(handler)) }
    #[cfg(not(target_os = "macos"))]
    { let _ = handler; Err(Error::UnsupportedPlatform) }
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::{Cell, OnceCell, RefCell};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
        NSBaselineOffsetAttributeName, NSColor, NSFont, NSFontAttributeName,
        NSControlStateValueMixed, NSControlStateValueOff, NSControlStateValueOn,
        NSForegroundColorAttributeName, NSMenu, NSMenuDelegate, NSMenuItem,
        NSStatusBar, NSStatusItem, NSWorkspace,
    };
    use objc2_foundation::{ns_string, MainThreadMarker, NSAttributedString, NSDictionary, NSNotification, NSNumber, NSObject, NSObjectProtocol, NSString, NSURL};
    use super::*;

    const KEYBOARD_SETTINGS: &str = "x-apple.systempreferences:com.apple.Keyboard-Settings.extension";
    const ACCESSIBILITY_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
    const INPUT_MONITORING_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

    type Handler = Box<dyn FnMut(ControlAction) -> ControlState>;

    struct Controls {
        item: Retained<NSStatusItem>,
        status: Retained<NSMenuItem>,
        start: Retained<NSMenuItem>,
        stop: Retained<NSMenuItem>,
        message: Retained<NSMenuItem>,
        permissions: [Retained<NSMenuItem>; 3],
        login: Retained<NSMenuItem>,
        login_status: Retained<NSMenuItem>,
    }

    struct DelegateIvars {
        handler: RefCell<Handler>,
        controls: OnceCell<Controls>,
        recovery: OnceCell<crate::infrastructure::macos::timer::Wakeup>,
        reported_state: Cell<Option<&'static str>>,
        login_error: RefCell<Option<String>>,
    }

    define_class!(
        // SAFETY：NSObject 没有额外子类要求；对象和全部 UI 仅由主线程访问。
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = DelegateIvars]
        struct KeyFluxControlDelegate;

        unsafe impl NSObjectProtocol for KeyFluxControlDelegate {}

        unsafe impl NSMenuDelegate for KeyFluxControlDelegate {
            #[unsafe(method(menuNeedsUpdate:))]
            fn menu_needs_update(&self, _menu: &NSMenu) {
                let _ = self.dispatch(ControlAction::Refresh);
                self.refresh_login_item();
            }
        }

        unsafe impl NSApplicationDelegate for KeyFluxControlDelegate {
            #[unsafe(method(applicationDidFinishLaunching:))]
            fn did_finish_launching(&self, _notification: &NSNotification) {
                self.create_status_item();
                let Some(state) = self.dispatch(ControlAction::Refresh) else { return; };
                crate::infrastructure::macos::diagnostics::record(format_args!(
                    "使用前请关闭系统中/英键切换输入法选项；菜单栏 ⇧ 的“使用说明”可查看设置方法。"));
                if !std::env::args_os().skip(1).any(|argument| argument == "--paused") && state.permissions_granted {
                    let _ = self.dispatch(ControlAction::Start);
                }
            }

            #[unsafe(method(applicationWillTerminate:))]
            fn will_terminate(&self, _notification: &NSNotification) {
                let _ = self.dispatch(ControlAction::Stop);
            }
        }

        impl KeyFluxControlDelegate {
            #[unsafe(method(startListening:))]
            fn start_listening(&self, _sender: Option<&AnyObject>) {
                self.start_or_check(ControlAction::Start);
            }

            #[unsafe(method(stopListening:))]
            fn stop_listening(&self, _sender: Option<&AnyObject>) { let _ = self.dispatch(ControlAction::Stop); }

            #[unsafe(method(checkPermissions:))]
            fn check_permissions(&self, _sender: Option<&AnyObject>) {
                self.start_or_check(ControlAction::Permissions);
            }

            #[unsafe(method(toggleLoginItem:))]
            fn toggle_login_item(&self, _sender: Option<&AnyObject>) {
                let result = super::super::login_item::status().and_then(|status| {
                    // 项目移动后再次点击更新启动路径，不继续保留失效路径。
                    let enabled = !status.enabled || !status.current_location;
                    super::super::login_item::set_enabled(enabled)
                });
                match result {
                    Ok(_) => { self.ivars().login_error.borrow_mut().take(); }
                    Err(error) => {
                        crate::infrastructure::macos::diagnostics::record(format_args!("KeyFlux：登录自启设置失败：{error}"));
                        *self.ivars().login_error.borrow_mut() = Some(error);
                    }
                }
                self.refresh_login_item();
            }

            #[unsafe(method(openKeyboardSettings:))]
            fn open_keyboard_settings(&self, _sender: Option<&AnyObject>) { self.open_settings(KEYBOARD_SETTINGS); }

            #[unsafe(method(openAccessibilitySettings:))]
            fn open_accessibility_settings(&self, _sender: Option<&AnyObject>) { self.open_settings(ACCESSIBILITY_SETTINGS); }

            #[unsafe(method(openInputMonitoringSettings:))]
            fn open_input_monitoring_settings(&self, _sender: Option<&AnyObject>) { self.open_settings(INPUT_MONITORING_SETTINGS); }

            #[unsafe(method(quitKeyFlux:))]
            fn quit(&self, _sender: Option<&AnyObject>) {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
        }
    );

    impl KeyFluxControlDelegate {
        fn new(mtm: MainThreadMarker, handler: Handler) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(DelegateIvars {
                handler: RefCell::new(handler), controls: OnceCell::new(), recovery: OnceCell::new(),
                reported_state: Cell::new(None),
                login_error: RefCell::new(None),
            });
            // SAFETY：NSObject init 的签名与返回所有权正确。
            unsafe { msg_send![super(this), init] }
        }

        fn dispatch(&self, action: ControlAction) -> Option<ControlState> {
            // 先结束 handler 的可变借用，再更新菜单，不嵌套模态 RunLoop。
            let result = catch_unwind(AssertUnwindSafe(|| (self.ivars().handler.borrow_mut())(action)));
            let controls = self.ivars().controls.get()?;
            match result {
                Ok(state) => {
                    if let Some(wakeup) = self.ivars().recovery.get() {
                        match state.recovery_delay { Some(delay) => wakeup.arm(delay), None => wakeup.pause() }
                    }
                    let status = if state.running { "KeyFlux · 已启用" }
                        else if state.recovery_delay.is_some() { "KeyFlux · 正在恢复" }
                        else if !state.permissions_granted { "KeyFlux · 等待授权" }
                        else { "KeyFlux · 已暂停" };
                    let reported_state = if state.running { "listening" }
                        else if state.recovery_delay.is_some() { "recovering" }
                        else if !state.permissions_granted { "permission-required" }
                        else { "stopped" };
                    if self.ivars().reported_state.replace(Some(reported_state)) != Some(reported_state) {
                        crate::infrastructure::macos::diagnostics::record(format_args!("KeyFlux state={reported_state}"));
                    }
                    controls.status.setTitle(&NSString::from_str(status));
                    controls.status.setToolTip(Some(&NSString::from_str(&state.message)));
                    let mut characters = state.message.chars();
                    let mut summary: String = characters.by_ref().take(42).collect();
                    if characters.next().is_some() { summary.push('…'); }
                    controls.message.setTitle(&NSString::from_str(&summary));
                    controls.message.setToolTip(Some(&NSString::from_str(&state.message)));
                    let mut permissions = state.permissions.lines();
                    for item in &controls.permissions {
                        item.setTitle(&NSString::from_str(permissions.next().unwrap_or("权限查询失败，请重新检查")));
                    }
                    controls.start.setEnabled(!state.running);
                    controls.stop.setEnabled(state.running || state.recovery_delay.is_some());
                    if let Some(button) = controls.item.button(self.mtm()) {
                        button.setToolTip(Some(&NSString::from_str(&format!("{status}\n{}", state.message))));
                    }
                    Some(state)
                }
                Err(_) => {
                    crate::infrastructure::macos::diagnostics::record(format_args!("KeyFlux：菜单操作异常，正在退出。"));
                    // 不让已失去控制的监听进程继续隐藏运行；run 返回后仍会执行停止清理。
                    NSApplication::sharedApplication(self.mtm()).terminate(None);
                    None
                }
            }
        }

        fn create_status_item(&self) {
            let mtm = self.mtm();
            // 固定本应用的占位宽度；其他应用之间的间距由系统管理。
            let item = NSStatusBar::systemStatusBar().statusItemWithLength(22.0);
            if let Some(button) = item.button(mtm) {
                // 仅创建一次轻量文字标题，不加载 NSImage / SF Symbols 图片。
                // 缩小字形并下移 1pt；动态系统文字色适配深浅菜单栏背景。
                let font = NSFont::systemFontOfSize(15.0);
                let baseline = NSNumber::numberWithDouble(-1.0);
                let color = NSColor::labelColor();
                // SAFETY：系统属性键为有效常量；对应值分别是 NSFont、NSNumber、NSColor。
                let title = unsafe {
                    let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
                        &[NSFontAttributeName, NSBaselineOffsetAttributeName, NSForegroundColorAttributeName],
                        &[&*font, &*baseline, &*color]);
                    NSAttributedString::initWithString_attributes(
                        NSAttributedString::alloc(), ns_string!("⇧"), Some(&attributes))
                };
                button.setAttributedTitle(&title);
            }
            let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("KeyFlux"));
            menu.setAutoenablesItems(false);
            menu.setDelegate(Some(ProtocolObject::from_ref(self)));
            let status = self.menu_item("KeyFlux · 未启用", None);
            status.setEnabled(false);
            menu.addItem(&status);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let start = self.menu_item("启用键盘切换", Some(sel!(startListening:)));
            let stop = self.menu_item("暂停键盘切换", Some(sel!(stopListening:)));
            menu.addItem(&start);
            menu.addItem(&stop);
            menu.addItem(&self.menu_item("检查权限并启用…", Some(sel!(checkPermissions:))));
            menu.addItem(&self.menu_item("打开键盘设置…", Some(sel!(openKeyboardSettings:))));
            let login = self.menu_item("登录时自动启动", Some(sel!(toggleLoginItem:)));
            login.setToolTip(Some(ns_string!("只影响下次登录，不会停止或重启当前 KeyFlux。若系统限制后台项目，需在登录项设置中允许。")));
            menu.addItem(&login);
            let details = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("状态与权限"));
            details.setAutoenablesItems(false);
            let message = self.info_item("尚未启用键盘切换");
            details.addItem(&message);
            details.addItem(&NSMenuItem::separatorItem(mtm));
            let permissions = [self.info_item("设备控制 / 辅助功能：查询中"),
                self.info_item("事件发送：查询中"), self.info_item("输入监控：查询中")];
            for item in &permissions { details.addItem(item); }
            details.addItem(&NSMenuItem::separatorItem(mtm));
            let login_status = self.info_item("登录自启：打开菜单时检查");
            details.addItem(&login_status);
            details.addItem(&NSMenuItem::separatorItem(mtm));
            details.addItem(&self.menu_item("打开设备控制 / 辅助功能设置…", Some(sel!(openAccessibilitySettings:))));
            details.addItem(&self.menu_item("打开输入监控设置…", Some(sel!(openInputMonitoringSettings:))));
            let details_item = self.menu_item("状态与权限", None);
            details_item.setSubmenu(Some(&details));
            menu.addItem(&details_item);

            let usage = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("使用说明"));
            usage.setAutoenablesItems(false);
            for line in ["单按 Shift：拼音 ↔ 小写英文", "Caps：英文大写 ↔ 小写", "大写下单按 Shift：回到小写英文",
                "请先关闭系统中/英键切换输入法选项", "系统设置 → 键盘 → 文字输入 → 编辑", "→ 所有输入法"] {
                usage.addItem(&self.info_item(line));
            }
            usage.addItem(&NSMenuItem::separatorItem(mtm));
            usage.addItem(&self.menu_item("打开键盘设置…", Some(sel!(openKeyboardSettings:))));
            let usage_item = self.menu_item("使用说明", None);
            usage_item.setSubmenu(Some(&usage));
            menu.addItem(&usage_item);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&self.menu_item("退出 KeyFlux", Some(sel!(quitKeyFlux:))));
            // AppKit 的标准状态栏菜单同时支持左击与右击。
            item.setMenu(Some(&menu));
            let _ = self.ivars().controls.set(Controls { item, status, start, stop, message, permissions, login, login_status });
        }

        fn refresh_login_item(&self) {
            let Some(controls) = self.ivars().controls.get() else { return; };
            // 菜单点击后会收起；错误保留到下次成功设置，确保再次打开时仍可查看。
            let last_error = self.ivars().login_error.borrow();
            let error = last_error.as_deref();
            match super::super::login_item::status() {
                Ok(status) => {
                    controls.login.setState(if status.enabled && !status.current_location { NSControlStateValueMixed }
                        else if status.enabled { NSControlStateValueOn } else { NSControlStateValueOff });
                    controls.login.setTitle(if status.enabled && !status.current_location {
                        ns_string!("登录时自动启动（点击更新路径）")
                    } else { ns_string!("登录时自动启动") });
                    let text = if status.enabled && !status.current_location { "登录自启：路径已变化，请点击开关更新" }
                        else if status.enabled { "登录自启配置：已开启，下次登录生效（以系统允许为准）" }
                        else { "登录自启配置：已关闭" };
                    controls.login_status.setTitle(&NSString::from_str(if error.is_some() {
                        "登录自启：设置失败，请停留查看原因"
                    } else { text }));
                    controls.login_status.setToolTip(Some(&NSString::from_str(error.unwrap_or(text))));
                }
                Err(reason) => {
                    controls.login.setState(NSControlStateValueMixed);
                    controls.login_status.setTitle(ns_string!("登录自启：无法读取配置，请停留查看原因"));
                    controls.login_status.setToolTip(Some(&NSString::from_str(&reason)));
                }
            }
        }

        fn menu_item(&self, title: &str, action: Option<Sel>) -> Retained<NSMenuItem> {
            // SAFETY：所有 selector 都是本类声明的单参数方法，delegate 由 run 保持存活。
            unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()), &NSString::from_str(title), action, ns_string!(""));
                item.setTarget(Some(self));
                item
            }
        }

        fn info_item(&self, text: &str) -> Retained<NSMenuItem> {
            let item = self.menu_item(text, None);
            item.setEnabled(false);
            item
        }

        fn start_or_check(&self, action: ControlAction) {
            if let Some(state) = self.dispatch(action)
                && !state.permissions_granted { self.open_permission_settings(); }
        }

        fn open_permission_settings(&self) {
            let url = match super::super::permissions::PermissionStatus::check() {
                Ok(status) if status.accessibility && status.post_event && !status.input_monitoring => INPUT_MONITORING_SETTINGS,
                _ => ACCESSIBILITY_SETTINGS,
            };
            self.open_settings(url);
        }

        fn open_settings(&self, address: &str) {
            let opened = NSURL::URLWithString(&NSString::from_str(address))
                .is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
            if !opened {
                crate::infrastructure::macos::diagnostics::record(format_args!("KeyFlux：无法打开系统设置：{address}"));
            }
        }
    }

    pub(super) fn run(handler: Handler) -> Result<()> {
        let mtm = MainThreadMarker::new().ok_or(Error::NativeApi("菜单栏必须在主线程启动"))?;
        let app = NSApplication::sharedApplication(mtm);
        // 直接执行 target/release/KeyFlux 也隐藏 Dock 图标，不依赖 App 打包信息。
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let delegate = KeyFluxControlDelegate::new(mtm, handler);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let recovery_target = delegate.clone();
        let recovery_timer = crate::infrastructure::macos::timer::Timer::new(move || {
            let _ = recovery_target.dispatch(ControlAction::Recover);
        })?;
        let _ = delegate.ivars().recovery.set(recovery_timer.handle());
        let failure_target = delegate.clone();
        let _stop_signal = crate::infrastructure::macos::run_loop::StopSignal::new(move || {
            // 不用模态对话框阻断自动恢复；状态、原因与恢复进度保留在菜单和日志中。
            let _ = failure_target.dispatch(ControlAction::MonitorFailed);
        })?;
        // AppKit 驱动主线程 RunLoop；菜单操作只注册监听来源。
        app.run();
        let _ = delegate.dispatch(ControlAction::Stop);
        if let Some(controls) = delegate.ivars().controls.get() {
            controls.item.setMenu(None);
            NSStatusBar::systemStatusBar().removeStatusItem(&controls.item);
        }
        app.setDelegate(None);
        Ok(())
    }
}
