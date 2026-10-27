use tauri::WebviewWindow;
use tauri_plugin_decoration::WebviewWindowExt;

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
        .invoke_handler(tauri::generate_handler![
            activate_custom_titlebar,
            restore_native_titlebar
        ])
        .run(tauri::generate_context!())
        .expect("CraftWave 启动失败");
}
