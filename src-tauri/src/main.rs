//! 程序入口文件：只做一件事——转调 crate 根的 `run()` 完成 Tauri 应用装配
//! 与启动。逻辑都放在 craftwave_lib 库 crate 里，二进制入口保持最薄，
//! 集成测试可以直接整 crate 引用。

// release 构建在 Windows 上不额外弹出控制台黑窗（GUI 程序的标准做法）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 进程入口：转交 `craftwave_lib::run` 启动应用。
fn main() {
    craftwave_lib::run()
}
