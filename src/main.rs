mod application;
mod bootstrap;
mod domain;
mod error;
mod infrastructure;
mod presentation;

use std::process::ExitCode;

/// 进程入口只处理退出码和错误输出，依赖组装交给 bootstrap。
fn main() -> ExitCode {
    match bootstrap::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("KeyFlux: {error}");
            ExitCode::FAILURE
        }
    }
}
