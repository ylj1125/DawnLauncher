// Dawn Launcher - Rust + Slint 重构版入口
// 替代原 Electron 主进程 electron/main/index.ts

#![windows_subsystem = "windows"]

mod app;
mod db;
mod models;
mod native;

// 导入 Slint 编译生成的 UI 组件 (MainWindow, ClassificationInfo, ItemInfo)
// 由 build.rs 中的 slint_build::compile 产出
slint::include_modules!();

use app::App;

fn main() {
    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .try_init()
        .ok();

    log::info!("Dawn Launcher 启动中...");

    // 初始化应用
    let app = match App::new() {
        Ok(a) => a,
        Err(e) => {
            log::error!("应用初始化失败: {}", e);
            // MVP 阶段: 弹出错误对话框 (Windows MessageBox)
            #[cfg(target_os = "windows")]
            {
                use windows::Win32::UI::WindowsAndMessaging::{
                    MessageBoxW, MB_ICONERROR, MB_OK,
                };
                use windows::core::PCWSTR;
                use windows::core::HSTRING;
                let msg = format!("应用初始化失败:\n{}\n\n请检查日志或联系开发者。", e);
                let title = "Dawn Launcher 启动错误";
                let msg_h = HSTRING::from(&msg);
                let title_h = HSTRING::from(title);
                unsafe {
                    MessageBoxW(
                        None,
                        PCWSTR(msg_h.as_ptr()),
                        PCWSTR(title_h.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            std::process::exit(1);
        }
    };

    log::info!("应用初始化完成，进入事件循环");

    // 运行事件循环
    if let Err(e) = app.run() {
        log::error!("事件循环错误: {}", e);
        std::process::exit(1);
    }
}
