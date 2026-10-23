//! 菜单栏行为：启动提醒后检查权限，权限齐全时开始监听。

use crate::application::app::KeyFlux;
use crate::error::Result;
use crate::infrastructure::macos::control_window::{self, ControlAction, ControlState};
use crate::infrastructure::macos::permissions::PermissionStatus;
use crate::infrastructure::macos::diagnostics::record;
use std::time::{Duration, Instant};

pub(crate) fn run(mut engine: KeyFlux) -> Result<()> {
    let mut message = "尚未开始监听。请先关闭系统的中/英键切换输入法选项。".to_owned();
    let mut recovery = RecoveryPlan::default();
    let mut caps_target = None;
    control_window::run(move |action| {
        match action {
            ControlAction::Refresh => {}
            ControlAction::Start | ControlAction::Permissions => {
                recovery.request_start();
                caps_target = None;
                message = match PermissionStatus::check().and_then(|permissions| {
                    if permissions.is_granted() { engine.start() }
                    else { permissions.request_missing()?; Err(crate::error::Error::MissingPermissions) }
                }) {
                    Ok(()) => {
                        recovery.started(Instant::now());
                        "正在后台监听。可从菜单栏停止监听或退出 KeyFlux。".into()
                    }
                    Err(error) => {
                        recovery.stop();
                        record(format_args!("KeyFlux：{error}"));
                        format!("未能开始监听：{error}")
                    }
                };
            }
            ControlAction::Stop => {
                recovery.stop();
                caps_target = None;
                message = match engine.stop() {
                    Ok(()) => "监听已停止，键盘已交回系统。可以从菜单栏重新开始或退出。".into(),
                    Err(error) => {
                        record(format_args!("KeyFlux：{error}"));
                        format!("监听已停止：{error}")
                    }
                };
            }
            ControlAction::MonitorFailed => {
                // 延迟到原生回调返回后，完整注销旧 tap / HID / timer，再安排重建。
                // 手动停止后到达的旧故障信号不允许重新开启监听。
                if recovery.wanted && engine.is_running() {
                    caps_target = engine.recovery_caps_target();
                    let error = engine.stop().err();
                    if let Some(error) = &error { record(format_args!("KeyFlux：监听故障：{error}")); }
                    if let Some(error) = error.as_ref().filter(|error| !can_recover(error)) {
                        recovery.stop();
                        message = format!("监听已停止，需要手动处理：{error}");
                    } else {
                        let delay = recovery.failed(Instant::now());
                        message = recovery_message(delay);
                        record(format_args!("KeyFlux：{message}"));
                    }
                }
            }
            ControlAction::Recover => {
                if recovery.is_due(Instant::now()) && !engine.is_running() {
                    match engine.resume(caps_target) {
                        Ok(()) => {
                            recovery.started(Instant::now());
                            caps_target = None;
                            message = "已自动重建监听，正在后台运行。".into();
                            record(format_args!("KeyFlux：{message}"));
                        }
                        Err(error) => {
                            // start 可能只完成部分注册；停止会释放已有资源及原生连接。
                            let _ = engine.stop();
                            record(format_args!("KeyFlux：自动重建未完成：{error}"));
                            if can_recover(&error) {
                                let delay = recovery.failed(Instant::now());
                                message = recovery_message(delay);
                            } else {
                                recovery.stop();
                                caps_target = None;
                                message = format!("自动恢复已暂停，需要手动处理：{error}");
                            }
                        }
                    }
                }
            }
        }
        let (permissions_granted, permissions) = match PermissionStatus::check() {
            Ok(status) => (status.is_granted(), format!("设备控制 / 辅助功能：{}\n事件发送：{}\n输入监控：{}",
                allowed(status.accessibility), allowed(status.post_event), allowed(status.input_monitoring))),
            Err(error) => (false, format!("无法查询权限：{error}")),
        };
        ControlState { running: engine.is_running(), message: message.clone(), permissions, permissions_granted,
            recovery_delay: recovery.delay(Instant::now()) }
    })
}

fn allowed(value: bool) -> &'static str { if value { "已允许" } else { "未允许" } }

fn can_recover(error: &crate::error::Error) -> bool {
    use crate::error::Error;
    !matches!(error, Error::MissingPermissions | Error::UnsupportedPlatform | Error::InputSourcesUnavailable(_))
        && !matches!(error, Error::MonitorStopped(reason) if reason.contains("panic"))
}

fn recovery_message(delay: Duration) -> String {
    format!("键盘暂交系统，{}ms 后自动重建监听；可从菜单栏停止或退出。", delay.as_millis())
}

/// 正常运行时没有定时检查；只有故障才安排一次唤醒，持续故障最多每 30 秒重建一次。
#[derive(Default)]
struct RecoveryPlan {
    wanted: bool,
    failures: u8,
    running_since: Option<Instant>,
    next_at: Option<Instant>,
}

impl RecoveryPlan {
    fn request_start(&mut self) { *self = Self { wanted: true, ..Self::default() }; }

    fn started(&mut self, now: Instant) { self.running_since = Some(now); self.next_at = None; }

    fn stop(&mut self) { *self = Self::default(); }

    fn failed(&mut self, now: Instant) -> Duration {
        if self.running_since.take().is_some_and(|since| now.saturating_duration_since(since) >= Duration::from_secs(60)) {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        let delay = match self.failures {
            1 => Duration::from_millis(250),
            2 => Duration::from_secs(1),
            3 => Duration::from_secs(3),
            4 => Duration::from_secs(10),
            _ => Duration::from_secs(30),
        };
        self.next_at = Some(now + delay);
        delay
    }

    fn delay(&self, now: Instant) -> Option<Duration> {
        self.next_at.filter(|_| self.wanted).map(|next| next.saturating_duration_since(now))
    }

    fn is_due(&self, now: Instant) -> bool { self.delay(now) == Some(Duration::ZERO) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_stop_cancels_recovery_and_stale_wakeups() {
        let mut plan = RecoveryPlan::default();
        let now = Instant::now();
        plan.request_start();
        plan.started(now);
        let delay = plan.failed(now);
        assert!(!plan.is_due(now));
        assert!(plan.is_due(now + delay));
        plan.stop();
        assert!(!plan.wanted);
        assert!(!plan.is_due(now + Duration::from_secs(60)));
        assert_eq!(plan.delay(now), None);
    }

    #[test]
    fn repeated_faults_keep_retrying_with_a_capped_interval() {
        let mut plan = RecoveryPlan::default();
        let mut now = Instant::now();
        plan.request_start();
        for millis in [250, 1000, 3000, 10000, 30000, 30000, 30000] {
            let delay = plan.failed(now);
            assert_eq!(delay, Duration::from_millis(millis));
            now += delay;
            assert!(plan.is_due(now));
            plan.started(now);
            assert_eq!(plan.delay(now), None);
            now += Duration::from_secs(1);
        }
        assert!(plan.wanted);
    }

    #[test]
    fn healthy_minute_resets_backoff_and_refresh_does_not_extend_deadline() {
        let mut plan = RecoveryPlan::default();
        let now = Instant::now();
        plan.request_start();
        plan.failed(now);
        assert_eq!(plan.delay(now + Duration::from_millis(200)), Some(Duration::from_millis(50)));
        plan.started(now + Duration::from_millis(250));
        assert_eq!(plan.failed(now + Duration::from_millis(60250)), Duration::from_millis(250));
    }

    #[test]
    fn permission_loss_and_panics_do_not_start_a_background_prompt_loop() {
        use crate::error::Error;
        assert!(!can_recover(&Error::MissingPermissions));
        assert!(!can_recover(&Error::MonitorStopped("Caps HID 回调发生 panic")));
        assert!(can_recover(&Error::MonitorStopped("一分钟内多次回调超时，需要重建监听")));
        assert!(can_recover(&Error::HidOperation { operation: "HID 输入回调", status: -1 }));
    }
}
