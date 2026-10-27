use tauri::{Manager, WebviewWindow};
use tauri_plugin_decoration::WebviewWindowExt;

pub mod host;
pub mod providers;
pub mod spike;
pub mod storage;
pub mod util;

// 【S1 spike 原型】dev-only mock 帧源（release 不编译、不注册）
#[cfg(debug_assertions)]
mod mock_chat;
#[cfg(test)]
mod mock_chat_test;

/// 激活覆盖式自绘标题栏；macOS 上按 40px 顶栏摆放交通灯（y=顶栏中线）。
#[tauri::command]
async fn activate_custom_titlebar(window: WebviewWindow) -> Result<(), String> {
    window
        .activate_decoration()
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    window
        .set_traffic_lights_inset(14.0, 20.0)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 看门狗超时后的退化路径：恢复系统标题栏。
#[tauri::command]
async fn restore_native_titlebar(window: WebviewWindow) -> Result<(), String> {
    window.restore_decoration().await.map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_decoration::init())
        .setup(|app| {
            // §7.1：数据目录 ~/.craftwave（config.json + intent.json）
            let home = app.path().home_dir()?;
            let data_dir = home.join(".craftwave");
            std::fs::create_dir_all(&data_dir)?;
            app.manage(host::AppState::new_in(
                data_dir,
                std::sync::Arc::new(providers::keyring::OsKeyring),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            activate_custom_titlebar,
            restore_native_titlebar,
            #[cfg(debug_assertions)]
            mock_chat::mock_chat_run,
            host::commands::config::get_config,
            host::commands::config::save_config,
            host::commands::provider::save_provider,
            host::commands::provider::delete_provider,
        ])
        .run(tauri::generate_context!())
        .expect("CraftWave 启动失败");
}
