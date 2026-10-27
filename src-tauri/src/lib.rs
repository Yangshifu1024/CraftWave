use tauri::{Manager, WebviewWindow};
use tauri_plugin_decoration::WebviewWindowExt;

pub mod agent;
pub mod backend;
pub mod host;
pub mod providers;
pub mod safety;
pub mod spike;
pub mod storage;
pub mod tools;
pub mod util;

// 契约类型仅在 `ipc_commands!` 清单中经 stringify! 引用（manifest 的 args_type/ret_type 名），
// 实际类型面由命令形参承载；这些导入为清单可读性服务，编译器视为未用。
#[allow(unused_imports)]
use host::commands::config::{SaveConfigArgs, get_config, save_config};
#[allow(unused_imports)]
use host::commands::provider::{
    DeleteProviderArgs, SaveProviderArgs, delete_provider, save_provider,
};
#[allow(unused_imports)]
use host::commands::session_tree::{
    LoadSessionTreeArgs, SaveSessionTreeArgs, SaveSessionTreeResult,
};
use host::commands::session_tree::{load_session_tree, save_session_tree};
#[allow(unused_imports)]
use providers::types::{AppConfig, ProviderConfig};
use storage::SessionStore;
#[allow(unused_imports)]
use storage::SessionTreeData;

#[cfg(test)]
#[path = "ipc_manifest_test.rs"]
mod ipc_manifest_test;
#[cfg(debug_assertions)]
mod mock_chat;
#[cfg(test)]
#[path = "mock_chat_test.rs"]
mod mock_chat_test;

/// IPC 命令规格（§12.3 manifest 的 commands 元素；字段名沿用文档口径）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct IpcCommandSpec {
    pub name: String,
    pub args_type: String,
    pub ret_type: String,
}

/// IPC 命令注册单一事实源（§12.3/NFR-12 防漂移）：`invoke_handler` 与 `ipc_manifest`
/// 由同一份清单宏展开，命令名与契约类型名不可能漂移；dev-only 命令带 cfg 同源注册。
macro_rules! ipc_commands {
    ($($(#[$cfg:meta])* $path:path => args: $args:ty, ret: $ret:ty;)*) => {
        /// 注册全部 IPC 命令（与 manifest 同源）。
        pub fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool {
            tauri::generate_handler![ $( $(#[$cfg])* $path , )* ]
        }

        /// IPC 契约清单：cargo test 落盘 target/ipc-manifest.json（§12.3），前端双向扫描。
        /// frames/events 键随对应模块实装时由各自清单填充，当前 P0 未实装先占位空集。
        pub fn ipc_manifest() -> Vec<IpcCommandSpec> {
            vec![ $(
                $(#[$cfg])*
                IpcCommandSpec {
                    name: command_name(stringify!($path)),
                    args_type: stringify!($args).to_string(),
                    ret_type: stringify!($ret).to_string(),
                },
            )* ]
        }
    };
}

/// 命令路径 → 命令名（取末段；D14：命令名 snake_case）。
fn command_name(path: &str) -> String {
    path.rsplit("::").next().unwrap_or(path).to_string()
}

ipc_commands! {
    activate_custom_titlebar => args: (), ret: ();
    restore_native_titlebar => args: (), ret: ();
    save_session_tree => args: SaveSessionTreeArgs, ret: SaveSessionTreeResult;
    load_session_tree => args: LoadSessionTreeArgs, ret: SessionTreeData;
    host::commands::config::get_config => args: (), ret: AppConfig;
    host::commands::config::save_config => args: SaveConfigArgs, ret: AppConfig;
    host::commands::provider::save_provider => args: SaveProviderArgs, ret: ProviderConfig;
    host::commands::provider::delete_provider => args: DeleteProviderArgs, ret: ();
    #[cfg(debug_assertions)]
    mock_chat::mock_chat_run => args: (), ret: ();
}

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
        .manage(SessionStore::with_root(SessionStore::default_root()))
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
        .invoke_handler(invoke_handler())
        .run(tauri::generate_context!())
        .expect("CraftWave 启动失败");
}
