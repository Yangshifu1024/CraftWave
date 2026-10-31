//! MCP 工具清单动态同步与注册表。
//!
//! 三个职责：
//! 1. **工具名归一**：MCP 工具名不保证满足本仓工具名合法集
//!    `[a-z_][a-z0-9_]{0,63}`——归一化（小写化 + 非法字符替换
//!    `_` + 首字符数字加前缀 + 截断 64），跨 server 与静态段冲突时加
//!    `__2` 序号后缀，并维护「归一名 → (连接键, 原始名)」反向映射供调用
//!    还原（调用仍发原始名给 server）；
//! 2. **注册表**：连接池拉到 tools/list 后经 [`McpToolRegistry::sync`]
//!    热更新（`tools/list_changed` 通知 → 重拉清单 → 再 sync），模型工具
//!    表 = 静态段 + 本注册表动态段快照（[`crate::tools::lookup_spec`]）；
//! 3. **许可身份**：免审批判定所需的身份三元组（服务器启动指纹 +
//!    原始工具名 + 输入 schema 哈希）从这里取——展示名不参与许可绑定
//!    （碰撞后缀会漂移，展示名不作身份）。

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::config::{McpScope, McpServerConfig, McpToolFilter};
use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOrigin, ToolSpec};

/// 连接键（连接池 key 逐字段）：`(scope, projectId, name,
/// canonicalPath)`。放 config 域（纯数据，tools/pool/commands 共用）。
/// `project_id` 全局作用域为 None；`canonical_path` 为该作用域 mcp.json
/// 的 canonical 路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConnKey {
    /// 作用域。
    pub scope: McpScope,
    /// 项目 id（全局 = None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub project_id: Option<String>,
    /// 配置里的服务器名。
    pub name: String,
    /// 该作用域 mcp.json 的 canonical 路径。
    pub canonical_path: String,
}

impl ConnKey {
    /// 事件载荷与日志里的短标识（scope:name）。
    pub fn display(&self) -> String {
        format!("{}:{}", self.scope.as_str(), self.name)
    }
}

/// 审批门与调用还原所需的工具身份（三元组 + 门控输入）。
#[derive(Debug, Clone)]
pub struct McpToolIdentity {
    /// 归属连接键（调用还原面）。
    pub conn_key: ConnKey,
    /// 配置里的服务器名（审批标题展示）。
    pub server_name: String,
    /// 服务器启动指纹（许可三元组第一元）。
    pub server_fingerprint: String,
    /// 原始工具名（server 侧名单名；调用还原与许可三元组第二元）。
    pub original_name: String,
    /// 输入 schema 哈希（许可三元组第三元）。
    pub schema_hash: String,
    /// 用户配置的只读声明（true → 能力按 ReadOnly 进矩阵）。
    pub read_only: bool,
    /// 单次调用超时（server 配置的 timeout，缺省 60s）。
    pub call_timeout: std::time::Duration,
}

/// 一个 server 的动态工具段（同步快照）。
#[derive(Debug, Clone)]
struct ServerEntry {
    /// 连接键。
    key: ConnKey,
    /// 服务器启动指纹。
    fingerprint: String,
    /// 用户配置快照（read_only/always_allow/timeout 判定用）。
    config: McpServerConfig,
    /// 工具清单（归一名 → 身份 + 规格）。
    tools: Vec<(String, McpToolIdentity, ToolSpec)>,
}

/// MCP 动态工具注册表（进程级单例 + Arc 共享）。
///
/// 单例化理由（「MCP 动态工具并入 ToolServer 模式」）：主循环策略门
/// （is_interactive）、看门狗只读判定（make_record）、审批门（judge）与
/// 执行器（dispatch）都查「静态段 + 动态段」全表，而这些调用点分散在
/// 不同模块且部分无 AppState 入参（watchdog 纯函数）——进程级单例让
/// [`crate::tools::lookup_spec`] 一个函数覆盖全表。测试经
/// [`McpToolRegistry::install_global`] 注入独立实例（测试进程内一次）。
#[derive(Default)]
pub struct McpToolRegistry {
    inner: Mutex<Vec<ServerEntry>>,
}

// McpToolRegistry 的查询与同步方法。
impl McpToolRegistry {
    /// 进程级单例（惰性建空表）。
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<McpToolRegistry> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// 替换进程级单例内容（测试注入面：把 Arc 里的条目搬进 global）。
    /// 生产装配不需要——global 本就是生产实例。
    pub fn install_from(other: &Self) {
        let snapshot = other
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        *Self::global()
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
    }

    /// 动态段规格快照（模型工具表的动态段；顺序 = server 同步序）。
    pub fn snapshot(&self) -> Vec<ToolSpec> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flat_map(|e| e.tools.iter().map(|(_, _, spec)| spec.clone()))
            .collect()
    }

    /// 归一名 → 工具身份（审批门判定 + 调用还原入口）。
    pub fn identity(&self, normalized_name: &str) -> Option<McpToolIdentity> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find_map(|e| {
                e.tools
                    .iter()
                    .find(|(n, _, _)| n == normalized_name)
                    .map(|(_, id, _)| id.clone())
            })
    }

    /// 移除一个 server 的动态段（断开/配置移除时调用——工具表即时收窄，
    /// 下一轮模型请求生效）。
    pub fn remove(&self, key: &ConnKey) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|e| &e.key != key);
    }

    /// 同步一个 server 的工具清单（tools/list 结果 → 归一 + 碰撞 + 过滤
    /// → 热更新动态段）。返回注册的归一名列表（事件/日志面）。
    ///
    /// `list` 元组 = (原始工具名, 描述, 输入 schema, annotations 只读提示
    /// 等四 hints)。：schema 变化不删除旧许可记录——免审批判定按三元组
    /// 现算（schema_hash 变了自然不命中），记录留存无副作用。
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &self,
        key: ConnKey,
        fingerprint: String,
        config: &McpServerConfig,
        list: Vec<(String, String, serde_json::Value, ToolAnnotations)>,
    ) -> Vec<String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 查重集：静态段名 + 其他 server 已占用的归一名（本 server 重同步
        // 时保留自己已占用的名，避免热更新抖动改名）。
        let mut taken: std::collections::HashSet<String> = crate::tools::STATIC_TOOL_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();
        for e in inner.iter() {
            if e.key != key {
                for (n, _, _) in &e.tools {
                    taken.insert(n.clone());
                }
            }
        }
        let filter = config.tools.clone().unwrap_or_default();
        let entry = ServerEntry {
            key: key.clone(),
            fingerprint: fingerprint.clone(),
            config: config.clone(),
            tools: Vec::new(),
        };
        let slot = match inner.iter_mut().find(|e| e.key == key) {
            Some(existing) => {
                // 重同步：旧名先还给查重集之外（本 server 自己的名可复用）
                for (n, _, _) in &existing.tools {
                    taken.remove(n);
                }
                existing.fingerprint = fingerprint;
                existing.config = config.clone();
                existing.tools.clear();
                existing
            }
            None => {
                inner.push(entry);
                inner.last_mut().expect("刚 push 的条目")
            }
        };
        let mut registered = Vec::new();
        for (raw_name, description, schema, annotations) in list {
            if !filter_admits(&filter, &raw_name) {
                continue;
            }
            let normalized = allocate_name(&mut taken, &raw_name);
            let identity = McpToolIdentity {
                conn_key: key.clone(),
                server_name: key.name.clone(),
                server_fingerprint: slot.fingerprint.clone(),
                original_name: raw_name.clone(),
                schema_hash: super::trust::schema_fingerprint(&schema),
                read_only: config.read_only,
                call_timeout: std::time::Duration::from_secs(
                    config.timeout.unwrap_or(DEFAULT_CALL_TIMEOUT_SECS),
                ),
            };
            let spec = ToolSpec::build(
                normalized.clone(),
                clamp_description(&description, &raw_name),
                if config.read_only {
                    Capability::ReadOnly
                } else {
                    Capability::Network
                },
                schema,
                annotations,
                ToolOrigin::Mcp,
            );
            slot.tools.push((normalized, identity, spec));
            registered.push(raw_name);
        }
        registered
    }

    /// server 条目当前清单的同步签名（(原始名, schema 哈希) 按序拼接的
    /// 哈希）：清单热同步的「无变化不刷」比对面（轮询兜底路径用）。
    pub fn signature_of(&self, key: &ConnKey) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = inner.iter().find(|e| &e.key == key)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (normalized, identity, _) in &entry.tools {
            normalized.hash(&mut hasher);
            identity.schema_hash.hash(&mut hasher);
        }
        Some(hasher.finish())
    }

    /// 池快照（mcp_snapshot 的 toolCount 面）。
    pub fn tool_count(&self, key: &ConnKey) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|e| &e.key == key)
            .map(|e| e.tools.len())
            .unwrap_or(0)
    }

    /// 某连接下全部工具的（原始名, schema 哈希）列表（always_allow
    /// 自动授权面：清单同步后由连接池批量授予/刷新许可）。
    pub fn tool_identities_of(&self, key: &ConnKey) -> Vec<(String, String)> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|e| &e.key == key)
            .map(|e| {
                e.tools
                    .iter()
                    .map(|(_, id, _)| (id.original_name.clone(), id.schema_hash.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 单次工具调用超时缺省（秒； 字段级定案，设计文档已回写）。
pub const DEFAULT_CALL_TIMEOUT_SECS: u64 = 60;

/// glob 过滤判定：deny 优先（命中即拒）；allow 非空时须命中其一；其余放行。
fn filter_admits(filter: &McpToolFilter, raw_name: &str) -> bool {
    if filter
        .deny
        .iter()
        .any(|pat| glob::Pattern::new(pat).is_ok_and(|p| p.matches(raw_name)))
    {
        return false;
    }
    filter.allow.is_empty()
        || filter
            .allow
            .iter()
            .any(|pat| glob::Pattern::new(pat).is_ok_and(|p| p.matches(raw_name)))
}

/// 工具名归一：合法集 `[a-z_][a-z0-9_]{0,63}`。
/// 非 ASCII/大写字符小写化后仍非法的替换 `_`；首字符数字加 `_` 前缀；
/// 超 64 截断；空结果用 `_`。
pub fn normalize_tool_name(raw: &str) -> String {
    let lowered = raw.to_lowercase();
    let mut out: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !out.starts_with(|c: char| c == '_' || c.is_ascii_lowercase()) {
        out.insert(0, '_');
    }
    out.truncate(64);
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// 归一名分配：先按 [`normalize_tool_name`]，被占用则追加 `__2`、`__3`…
/// （追加后仍合法：后缀全为小写与数字/下划线，且总长按截断后基础名计）。
fn allocate_name(taken: &mut std::collections::HashSet<String>, raw: &str) -> String {
    let base = normalize_tool_name(raw);
    if taken.insert(base.clone()) {
        return base;
    }
    for suffix in 2u32.. {
        let candidate = format!("{base}__{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("u32 后缀空间耗尽不可达");
}

/// 描述截断（≤800 字符；缺省描述 = "MCP 工具 <原名>"）。
fn clamp_description(description: &str, raw_name: &str) -> String {
    let text = if description.trim().is_empty() {
        format!("MCP 工具 {raw_name}")
    } else {
        description.to_owned()
    };
    if text.chars().count() <= 800 {
        text
    } else {
        text.chars().take(800).collect()
    }
}
