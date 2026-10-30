//! provider 域命令（§3.3 #8/#9/#32）：save_provider / delete_provider /
//! test_provider。
//!
//! 薄壳：save/delete 转调 providers::service；test_provider 转调
//! providers::probe（连接探测领域面，19 号票，FR-02-09）。

use serde::Deserialize;
use serde::Serialize;
use tauri::State;

use crate::host::AppState;
use crate::providers::probe;
use crate::providers::service;
use crate::providers::types::ProviderConfig;
use crate::util::errors::{E_NOT_FOUND, IpcError};

/// `save_provider` 参数契约（§3.3 #8；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderArgs {
    pub provider: ProviderConfig,
    #[cfg_attr(test, ts(optional))]
    pub new_key: Option<String>,
}

/// `delete_provider` 参数契约（§3.3 #9；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderArgs {
    pub provider_id: String,
}

/// §3.3 #8：保存供应商；newKey 有值时写入 apiKey（明文随 config.json 0600
/// 落盘，§9.4 改写）。返回归一化后的供应商。
#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: ProviderConfig,
    new_key: Option<String>,
) -> Result<ProviderConfig, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SaveProviderArgs { provider, new_key };
    service::save_provider(&state.config, args.provider, args.new_key)
}

/// §3.3 #9：删除供应商（Key 随配置一并消失）。
#[tauri::command]
pub async fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = DeleteProviderArgs { provider_id };
    service::delete_provider(&state.config, &args.provider_id)
}

/// `test_provider` 参数契约（§3.3 #32，19 号票）：目标供应商 + 可选地址覆写
/// （草稿试连语义——表单里改了还没保存的 baseUrl 直接探测，不落库）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TestProviderArgs {
    pub provider_id: String,
    /// 草稿基地址覆写（缺省用已保存的 baseUrl / 官方缺省地址）。
    #[cfg_attr(test, ts(optional))]
    pub base_url_override: Option<String>,
}

/// `test_provider` 返回契约（§3.3 #32）：`{ ok, latencyMs?, models?, error? }`
/// （error = `{ code, message }` 脱敏，code 为供应商七类之一）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TestProviderResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<probe::ProbeError>,
}

/// §3.3 #32：保存供应商前的连通性测试（FR-02-09）。探测请求走代理工厂
/// 当前客户端（与模型请求同一条代理语义）；按方言两段探测（38 号票接通
/// 三协议：openai_chat 与 openai_responses 共用 /models + Bearer，回退段
/// 各自打 chat/completions 或 responses；anthropic_messages 用 x-api-key +
/// anthropic-version 鉴权，路径 /v1/models → /v1/messages）——连接测试与
/// 实际对话走同一 `ApiFormat` 选择。探测结果不落库。
#[tauri::command]
pub async fn test_provider(
    state: State<'_, AppState>,
    provider_id: String,
    base_url_override: Option<String>,
) -> Result<TestProviderResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = TestProviderArgs {
        provider_id,
        base_url_override,
    };
    let config = state.config.load()?.as_ref().clone();
    let provider = config
        .providers
        .iter()
        .find(|p| p.id == args.provider_id)
        .ok_or_else(|| IpcError::new(E_NOT_FOUND, format!("供应商不存在：{}", args.provider_id)))?;
    test_provider_core(&state, provider, args.base_url_override.as_deref()).await
}

/// 探测核心（命令壳分离：不依赖 Tauri State 的可测面）。
pub(crate) async fn test_provider_core(
    state: &AppState,
    provider: &ProviderConfig,
    base_url_override: Option<&str>,
) -> Result<TestProviderResult, IpcError> {
    // 地址优先级：草稿覆写 > 已存 baseUrl > 官方缺省（§3.2 注记）
    let base_url = base_url_override
        .map(str::to_string)
        .or_else(|| provider.base_url.clone())
        .unwrap_or_else(|| provider.api_format.official_base_url().to_owned());
    // Key：已存的 Key 参与探测（本地供应商按 local 标记免 Key——不携带）；
    // 草稿里改了但没保存的 Key 无法参与（契约面只有 baseUrlOverride，
    // 票面口径：草稿测试只覆写地址，Key 仍按已存值）。
    let api_key = if provider.local {
        None
    } else {
        provider.api_key.as_deref()
    };
    let client = state.http.client();
    let result = probe::probe(&client, provider.api_format, &base_url, api_key).await;
    Ok(TestProviderResult {
        ok: result.ok,
        latency_ms: result.latency_ms,
        models: result.models,
        error: result.error,
    })
}
