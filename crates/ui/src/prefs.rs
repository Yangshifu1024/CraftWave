//! 壳自管小存档（gpui-rewrite 阶段 2.B）。
//!
//! 数据目录与引擎同源：`~/.craftwave/`（engine `AppState::new_in` 的同一
//! 约定，engine 的 config/ui-state 等也住这里）。壳侧自己的两个文件：
//!
//! - `window-state.json`：主窗几何记忆（见 [`crate::window_state`]，
//!   字段语义对齐旧壳 `src-tauri/src/host/window_state.rs`）；
//! - `ui-prefs.json`：界面偏好（本阶段只有界面缩放档）。
//!
//! 为什么不走 engine 的 `get/set_ui_state` API：那对函数需要整套
//! `AppState`（storage/config 等装配），而旧栈里 ui-state 本就是前端自管域
//! （window_state.rs 模块文档：「不进 ui-state.json——那是前端自管域」）；
//! GPUI 壳同样自管，阶段 3 接入引擎后若需要会话级偏好再走 engine API。
//!
//! 存档是可丢数据：任何读写失败都只 eprintln 不惊动用户，调用方一律
//! 回默认值（与旧壳 window_state.rs「几何损坏 → 整体丢弃回默认」同语义）。

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

use craftwave_engine::util::atomic;

/// 应用数据目录：`~/.craftwave`。目录不存在则惰性创建（读路径不依赖目录
/// 存在；写盘前必须先建）。
pub fn data_dir() -> PathBuf {
    // std::env::home_dir：Rust 1.85 起解除废弃并修正了行为（本仓 1.97），
    // 单条 home 路径不值得为此引 dirs 依赖
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let dir = home.join(".craftwave");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("[craftwave] 数据目录创建失败（忽略，落盘时会再报）：{err}");
    }
    dir
}

/// 壳侧存档文件路径（`data_dir/<name>`）。
pub fn file(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// 原子写一份 JSON 存档（temp + rename，engine 的 util::atomic）。
/// 失败只记警告——存档可丢，不值得惊动用户。
pub fn write_json<T: Serialize>(path: &std::path::Path, value: &T) {
    let json = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    if let Err(err) = atomic::write_json(path, &json) {
        eprintln!("[craftwave] 存档落盘失败 {}（忽略）：{err}", path.display());
    }
}

/// 读一份 JSON 存档：不存在/损坏（非法 JSON、缺字段、类型不符）一律
/// `None`，调用方回默认值，不区分错误种类。
pub fn read_json<T: DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}
