//! §12.3 契约测试（NFR-12）Rust 侧：export_ipc_manifest。
//!
//! 遍历注册命令清单生成 `{commands, frames, events, tools}` → 写
//! `target/ipc-manifest.json`，供前端 vitest 消费（CI 顺序：cargo test 产出
//! artifact → 前端测试消费，不齐即红）。

use std::fs;

use serde_json::json;

use super::commands::{COMMANDS, EVENTS, FRAMES, TOOLS};

#[test]
fn export_ipc_manifest_命令清单合法并落盘() {
    // 键面不变式：invoke 名唯一且 snake_case（§3.1/D14）
    let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "命令名必须唯一");
    for name in &names {
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "命令名必须 snake_case：{name}"
        );
    }
    // 本票命令在册
    for expected in [
        "get_config",
        "save_config",
        "save_provider",
        "delete_provider",
    ] {
        assert!(
            COMMANDS.iter().any(|c| c.name == expected),
            "缺命令 {expected}"
        );
    }

    let manifest = json!({
        "commands": COMMANDS.iter()
            .map(|c| json!({ "name": c.name, "argsType": c.args_type, "retType": c.ret_type }))
            .collect::<Vec<_>>(),
        "frames": FRAMES,
        "events": EVENTS,
        "tools": TOOLS,
    });

    let out_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("ipc-manifest.json");
    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    fs::write(&out_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

    // 落盘内容可回读
    let readback: serde_json::Value =
        serde_json::from_slice(&fs::read(&out_path).unwrap()).unwrap();
    assert!(readback["commands"].as_array().unwrap().len() >= 6);
}
