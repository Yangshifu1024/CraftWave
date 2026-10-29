//! MCP 双作用域配置（FR-08-02，§9.7/§3.3 #40/#41 字段级；08 号票）。
//!
//! 两份配置文件：
//! - 全局：`~/.craftwave/mcp.json`（数据目录根）；
//! - 项目：`<项目主目录>/.craftwave/mcp.json`（§9.8：项目作用域 mcp.json
//!   属项目信任资源——24 号票落地通用信任门前，本票先按最小版处理：加载
//!   接 TOFU 指纹核验（配置内容进指纹），保存不设额外门；收敛点已记录在
//!   08 号票）。
//!
//! 合并规则（FR-08-02）：按服务器名「同名覆盖」——项目条目整体替换全局
//! 同名条目，其余并存；合并结果即工具发现面。
//!
//! 未知字段 round-trip（票面明文）：服务器条目用 `#[serde(flatten)]` 收留
//! 未知字段到保序 map（serde_json preserve_order 底座 = IndexMap），保存
//! 时原样写回——向前兼容用户手写的注释性/前瞻性字段，读取-保存往返不丢。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 作用域（§3.3 #40–47 逐字口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum McpScope {
    /// 全局：`~/.craftwave/mcp.json`。
    Global,
    /// 项目：`<项目主目录>/.craftwave/mcp.json`。
    Project,
}

impl McpScope {
    /// 事件载荷与指纹元组的字符串面（"global"/"project"）。
    pub fn as_str(self) -> &'static str {
        match self {
            McpScope::Global => "global",
            McpScope::Project => "project",
        }
    }
}

/// 服务器条目字段（§9.7 逐字：command/args/env/cwd/url/headers/enabled/
/// timeout/read_only/always_allow/tools{allow,deny glob}）。
///
/// 字段语义定案（08 号票，设计文档 §9.7 已回写）：
/// - `command/args/env/cwd`：stdio 传输的四要素（command 有值即 stdio）；
/// - `url/headers`：streamable HTTP 传输的两要素（url 有值即 HTTP）；
///   两者同时给出以 stdio 为准（配置校验报错，不允许二义）；
/// - `enabled`：缺省 true——false 的服务器不出现在发现面，连接命令报
///   E_INVALID_ARG（用户显式停用）；
/// - `timeout`：单次工具调用超时秒数，缺省 60，合法区间 1–600（对齐
///   §8.8 命令超时上界）；
/// - `read_only`/`always_allow`：**只取用户确认的配置**（§9.7 Q44）——
///   read_only=true 把该 server 全部工具标为 ReadOnly 能力；always_allow
///   =true 时未声明 read_only 的工具调用仍须通过 Q44 许可表核验（绑定
///   服务器指纹 + 原始工具名 + 输入 schema 哈希），不是无脑放行；
/// - `tools.allow/deny`：注册时过滤的 glob 工具名清单（对原始工具名匹配；
///   deny 优先于 allow——命中 deny 一律不注册）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// stdio 传输：可执行命令（有值即 stdio 服务器）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub command: Option<String>,
    /// stdio 传输：命令参数（空缺省不序列化）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// stdio 传输：子进程附加环境变量。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// stdio 传输：子进程工作目录（缺省继承应用进程）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub cwd: Option<String>,
    /// streamable HTTP 传输：服务器地址（http/https）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub url: Option<String>,
    /// streamable HTTP 传输：附加请求头。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// 是否启用（缺省 true；serde alias 兼容手写 false）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 单次工具调用超时秒数（缺省 60，1–600）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub timeout: Option<u64>,
    /// 用户确认的只读声明（true = 全部工具按 ReadOnly 能力参与门控矩阵；
    /// 远端 annotations 不参与判定，§9.7）。字段名按票面逐字 snake_case。
    #[serde(
        rename = "read_only",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub read_only: bool,
    /// 用户确认的服务器级免审批声明（配合 Q44 许可表逐工具核验，见
    /// [`Self`] 文档）。字段名按票面逐字 snake_case。
    #[serde(
        rename = "always_allow",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub always_allow: bool,
    /// 工具注册过滤（allow/deny glob 清单）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub tools: Option<McpToolFilter>,
    /// 未知字段收留（保序 round-trip，票面明文；ts-rs 对 flatten 字段
    /// 生成 index signature，无需 type 覆写）。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// tools.allow/deny 的 glob 过滤清单（对原始工具名匹配）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpToolFilter {
    /// 白名单 glob（空/缺省 = 不过滤，全部注册）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// 黑名单 glob（命中即不注册，优先于 allow）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// 缺省 enabled=true（serde default 函数）。
fn default_true() -> bool {
    true
}

/// 单作用域配置文件形状：`{ "mcpServers": { "<名称>": { … } } }`（对齐
/// 社区 mcp.json 惯例顶层键）。未知顶层字段同样 round-trip。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpConfigFile {
    /// 服务器名 → 条目（保序 map：文件书写序）。
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub mcp_servers: serde_json::Map<String, serde_json::Value>,
    /// 未知顶层字段收留（flatten → index signature）。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// McpConfigFile 的读写方法。
impl McpConfigFile {
    /// 解析服务器条目（值非法 → 跳过并返回名称入错误表，不炸整份配置——
    /// 单条目写坏不应拖垮其余服务器）。
    pub fn parse_servers(&self) -> (Vec<(String, McpServerConfig)>, Vec<String>) {
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        for (name, value) in &self.mcp_servers {
            match serde_json::from_value::<McpServerConfig>(value.clone()) {
                Ok(cfg) => ok.push((name.clone(), cfg)),
                Err(e) => bad.push(format!("{name}: {e}")),
            }
        }
        (ok, bad)
    }

    /// 按 JSON 路径写回服务器条目（保留其余顶层未知字段）。
    pub fn set_server(&mut self, name: &str, config: &McpServerConfig) {
        let value = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
        self.mcp_servers.insert(name.to_owned(), value);
    }
}

/// 双作用域配置的加载与保存（08 号票命令面 #40/#41 的存储层）。
#[derive(Debug, Clone)]
pub struct McpConfigStore {
    /// 全局 mcp.json 路径（canonical 前已为绝对路径）。
    pub global_path: PathBuf,
    /// 项目 mcp.json 路径（项目未注册时为 None——project 作用域不可用）。
    pub project_path: Option<PathBuf>,
}

// McpConfigStore 的读写方法。
impl McpConfigStore {
    /// 构造：全局路径取数据目录根，项目路径取项目主目录（None = 无项目
    /// 上下文，project 作用域命令报参数错误）。
    pub fn new(data_dir: &Path, project_home: Option<&Path>) -> Self {
        Self {
            global_path: data_dir.join("mcp.json"),
            project_path: project_home.map(|h| h.join(".craftwave").join("mcp.json")),
        }
    }

    /// 作用域 → 文件路径（project 未注册 → E_INVALID_ARG 语义的 None）。
    pub fn path_of(&self, scope: McpScope) -> Option<&Path> {
        match scope {
            McpScope::Global => Some(self.global_path.as_path()),
            McpScope::Project => self.project_path.as_deref(),
        }
    }

    /// 读取一份作用域配置：文件缺失 = 空配置（首次使用形态，不报错）。
    pub fn load(&self, scope: McpScope) -> Result<McpConfigFile, String> {
        let path = self
            .path_of(scope)
            .ok_or_else(|| "project 作用域不可用：当前无项目上下文".to_owned())?;
        load_file(path)
    }

    /// 原子保存一份作用域配置（§7.4 util/atomic；目录不存在先建）。
    pub fn save(&self, scope: McpScope, config: &McpConfigFile) -> Result<(), String> {
        let path = self
            .path_of(scope)
            .ok_or_else(|| "project 作用域不可用：当前无项目上下文".to_owned())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
        crate::util::atomic::write_json(path, &serde_json::to_value(config).unwrap_or_default())
            .map_err(|e| format!("保存 mcp.json 失败：{e}"))
    }

    /// 合并视图（FR-08-02 同名覆盖）：项目条目整体替换全局同名条目。
    /// 返回（服务器名 → (作用域, 条目)），同名时作用域为 project。
    pub fn merged(&self) -> Result<Vec<(String, McpScope, McpServerConfig)>, String> {
        let (global, g_bad) = self.load(McpScope::Global)?.parse_servers();
        let mut all: Vec<(String, McpScope, McpServerConfig)> = global
            .into_iter()
            .map(|(name, cfg)| (name, McpScope::Global, cfg))
            .collect();
        if self.project_path.is_some() {
            let (project, p_bad) = self.load(McpScope::Project)?.parse_servers();
            let _ = (g_bad, p_bad); // 解析错误表由命令面上抛（snapshot 里带出）
            for (name, cfg) in project {
                match all.iter_mut().find(|(n, _, _)| *n == name) {
                    Some(slot) => {
                        slot.1 = McpScope::Project;
                        slot.2 = cfg;
                    }
                    None => all.push((name, McpScope::Project, cfg)),
                }
            }
        }
        Ok(all)
    }
}

/// 读一份 mcp.json（缺失 = 空；解析失败报错字符串——让用户看到文件坏了）。
fn load_file(path: &Path) -> Result<McpConfigFile, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(McpConfigFile::default()),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("解析 {} 失败：{e}", path.display()))
}
