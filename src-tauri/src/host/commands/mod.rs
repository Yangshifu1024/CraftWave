//! 命令层（§2.2 host/commands，按域分文件）+ IPC 命令注册表（§12.3 manifest 数据源）。

pub mod config;
pub mod provider;

/// 一条已注册命令的契约面（§12.3：`{name, argsType, retType}`）。
pub struct CommandInfo {
    /// invoke 名（snake_case，与 Rust fn 名一致，§3.1）。
    pub name: &'static str,
    /// 参数类型（TS 面，信息性；payload 类型面 ts-rs 接管前手写维护）。
    pub args_type: &'static str,
    /// 返回类型（TS 面，信息性）。
    pub ret_type: &'static str,
}

/// ChatFrame P0 全集 9 帧（§3.4；type 值与 TS 判别值一致）。
pub const FRAMES: &[&str] = &[
    "runStarted",
    "stepStarted",
    "textDelta",
    "reasoningDelta",
    "toolStarted",
    "toolArgs",
    "toolResult",
    "usage",
    "runEnded",
];

/// 事件键（§3.5 P0 18 键）。事件汇属后续票；本切片不发射事件，先占位空表。
pub const EVENTS: &[&str] = &[];

/// 镜像工具清单（§12.3 manifest.tools）。工具系统属后续票，先占位空表。
pub const TOOLS: &[&str] = &[];

/// 已注册命令清单（与 lib.rs `generate_handler!` 一一对应——增删命令时两处同步）。
pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "activate_custom_titlebar",
        args_type: "{}",
        ret_type: "void",
    },
    CommandInfo {
        name: "restore_native_titlebar",
        args_type: "{}",
        ret_type: "void",
    },
    // 【S1 spike 原型】仅 debug 构建注册（test profile 亦存在）
    #[cfg(debug_assertions)]
    CommandInfo {
        name: "mock_chat_run",
        args_type: "{ script: MockScript[], channel: Channel }",
        ret_type: "void",
    },
    // ---- config 域（§3.3 #6/#7）----
    CommandInfo {
        name: "get_config",
        args_type: "{}",
        ret_type: "AppConfig",
    },
    CommandInfo {
        name: "save_config",
        args_type: "{ config: AppConfig }",
        ret_type: "AppConfig",
    },
    // ---- provider 域（§3.3 #8/#9）----
    CommandInfo {
        name: "save_provider",
        args_type: "{ provider: ProviderConfig, newKey?: string }",
        ret_type: "ProviderConfig",
    },
    CommandInfo {
        name: "delete_provider",
        args_type: "{ providerId: string }",
        ret_type: "void",
    },
];
