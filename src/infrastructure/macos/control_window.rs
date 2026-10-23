//! AppKit 菜单栏适配；没有主窗口，提醒与权限说明仅在需要时弹出。

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
    use std::cell::{OnceCell, RefCell};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn,
        NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
        NSFont, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength, NSWorkspace,
    };
    use objc2_foundation::{ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL};
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
    }

    struct DelegateIvars {
        handler: RefCell<Handler>,
        controls: OnceCell<Controls>,
        recovery: OnceCell<crate::infrastructure::macos::timer::Wakeup>,
    }

    define_class!(
        // SAFETY：NSObject 没有额外子类要求；对象和全部 UI 仅由主线程访问。
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = DelegateIvars]
        struct KeyFluxControlDelegate;

        unsafe impl NSObjectProtocol for KeyFluxControlDelegate {}

        unsafe impl NSApplicationDelegate for KeyFluxControlDelegate {
            #[unsafe(method(applicationDidFinishLaunching:))]
            fn did_finish_launching(&self, _notification: &NSNotification) {
                self.create_status_item();
                self.show_startup_reminder();
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

            #[unsafe(method(openKeyboardSettings:))]
            fn open_keyboard_settings(&self, _sender: Option<&AnyObject>) { self.open_settings(KEYBOARD_SETTINGS); }

            #[unsafe(method(showStatus:))]
            fn show_status(&self, _sender: Option<&AnyObject>) {
                if let Some(state) = self.dispatch(ControlAction::Refresh) {
                    self.show_details(&state);
                }
            }

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
            });
            // SAFETY：NSObject init 的签名与返回所有权正确。
            unsafe { msg_send![super(this), init] }
        }

        fn dispatch(&self, action: ControlAction) -> Option<ControlState> {
            // 先结束 handler 的可变借用，再更新 UI 或显示模态提示。
            let result = catch_unwind(AssertUnwindSafe(|| (self.ivars().handler.borrow_mut())(action)));
            let controls = self.ivars().controls.get()?;
            match result {
                Ok(state) => {
                    if let Some(wakeup) = self.ivars().recovery.get() {
                        match state.recovery_delay { Some(delay) => wakeup.arm(delay), None => wakeup.pause() }
                    }
                    let status = if state.running { "KeyFlux · 正在监听" }
                        else if state.recovery_delay.is_some() { "KeyFlux · 正在恢复监听" }
                        else if !state.permissions_granted { "KeyFlux · 等待授权" }
                        else { "KeyFlux · 已停止" };
                    controls.status.setTitle(&NSString::from_str(status));
                    controls.status.setToolTip(Some(&NSString::from_str(&state.message)));
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
            let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
            if let Some(button) = item.button(mtm) {
                // 系统字体的单色 Shift 字形自动适配菜单栏深浅背景，无需外部图片。
                button.setTitle(ns_string!("⇧"));
                button.setFont(Some(&NSFont::boldSystemFontOfSize(18.0)));
            }
            let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("KeyFlux"));
            menu.setAutoenablesItems(false);
            let status = self.menu_item("KeyFlux · 尚未开始监听", None);
            status.setEnabled(false);
            menu.addItem(&status);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let start = self.menu_item("开始监听", Some(sel!(startListening:)));
            let stop = self.menu_item("停止监听", Some(sel!(stopListening:)));
            menu.addItem(&start);
            menu.addItem(&stop);
            menu.addItem(&self.menu_item("检查权限并开始监听…", Some(sel!(checkPermissions:))));
            menu.addItem(&self.menu_item("打开键盘设置…", Some(sel!(openKeyboardSettings:))));
            menu.addItem(&self.menu_item("状态与权限…", Some(sel!(showStatus:))));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&self.menu_item("退出 KeyFlux", Some(sel!(quitKeyFlux:))));
            // AppKit 的标准状态栏菜单同时支持左击与右击。
            item.setMenu(Some(&menu));
            let _ = self.ivars().controls.set(Controls { item, status, start, stop });
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

        fn show_startup_reminder(&self) {
            let Some(state) = self.dispatch(ControlAction::Refresh) else { return; };
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(ns_string!("KeyFlux · 使用前提醒"));
            alert.setInformativeText(&NSString::from_str(&format!(
                "请关闭系统自带的“使用中/英键（大写锁定键）切换输入法”选项，避免与 KeyFlux 冲突。\n\n系统设置 → 键盘 → 文字输入 → 编辑 → 所有输入法。\n\n当前权限：\n{}\n\n权限齐全后开始后台监听，菜单栏 ⇧ 可停止或退出。若先打开键盘设置，请设置完成后从 ⇧ 菜单选择“开始监听”。",
                state.permissions)));
            alert.addButtonWithTitle(&NSString::from_str(if state.permissions_granted { "知道了，开始监听" } else { "打开权限设置" }));
            alert.addButtonWithTitle(ns_string!("打开键盘设置"));
            alert.addButtonWithTitle(ns_string!("暂不监听"));
            NSApplication::sharedApplication(self.mtm()).activate();
            let response = alert.runModal();
            if response == NSAlertFirstButtonReturn {
                self.start_or_check(ControlAction::Start);
            } else if response == NSAlertSecondButtonReturn {
                self.open_settings(KEYBOARD_SETTINGS);
            }
        }

        fn start_or_check(&self, action: ControlAction) {
            if let Some(state) = self.dispatch(action) {
                if !state.running { self.show_details(&state); }
            }
        }

        fn show_details(&self, state: &ControlState) {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(&NSString::from_str(if state.running { "KeyFlux · 正在监听" } else { "KeyFlux · 尚未监听" }));
            alert.setInformativeText(&NSString::from_str(&format!(
                "{}\n\n{}\n\n缺少权限时，请在系统设置中自行开启。授权后从菜单栏 ⇧ 选择“检查权限并开始监听”；系统要求重启时，退出 KeyFlux 后重新打开。\n\n从终端启动时，权限可能显示在 Warp 或实际启动应用名下。",
                state.message, state.permissions)));
            alert.addButtonWithTitle(&NSString::from_str(if state.permissions_granted { "知道了" } else { "打开权限设置" }));
            if !state.permissions_granted { alert.addButtonWithTitle(ns_string!("稍后")); }
            NSApplication::sharedApplication(self.mtm()).activate();
            if alert.runModal() == NSAlertFirstButtonReturn && !state.permissions_granted { self.open_permission_settings(); }
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
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(ns_string!("请手动打开系统设置"));
                alert.setInformativeText(ns_string!("权限：系统设置 → 隐私与安全性 → 辅助功能 / 输入监控。\n中/英键：系统设置 → 键盘 → 文字输入 → 编辑 → 所有输入法。"));
                alert.addButtonWithTitle(ns_string!("知道了"));
                NSApplication::sharedApplication(self.mtm()).activate();
                alert.runModal();
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
