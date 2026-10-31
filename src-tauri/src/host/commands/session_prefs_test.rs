//! 会话偏好命令测试（18 号票，§3.3 #24/#25）：存在性校验、merge/清除语义、
//! 与 prefs 存储面的联通、RunConfig 旧载荷兼容。

use std::sync::Arc;

use super::{
    GetSessionPrefsResult, SetSessionPrefsPayload, get_session_prefs_core, set_session_prefs_core,
};
use crate::host::AppState;
use crate::providers::reasoning::ThinkingLevel;
use crate::storage::prefs::{SessionPrefs, load_prefs, save_prefs};
use crate::util::errors::E_NOT_FOUND;
use crate::util::events::RecordingEventSink;

/// 临时数据根 + 已注册一个会话的 AppState（index 条目，无历史文件——
/// 偏好独立于正文存在）。
fn app_with_session(tag: &str) -> (tempfile::TempDir, AppState, String) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("cw-session-prefs-{tag}-"))
        .tempdir()
        .expect("tempdir");
    let app = AppState::new_in(
        dir.path().to_path_buf(),
        Arc::new(RecordingEventSink::default()),
    );
    let sid = format!("sess-{tag}-1");
    let ts = 1_760_000_000_000;
    app.store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: sid.clone(),
            project_id: None,
            title: "测试会话".to_owned(),
            created_at: ts,
            updated_at: ts,
            revision: 0,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .expect("建索引条目失败");
    (dir, app, sid)
}

/// 断言 #24/#25 偏好读写落盘语义全链：未设置 → 空偏好；预置存储读回一致；
/// merge（只传的字段被改，未传字段保持）；null 清除（modelId/thinking 清回
/// 默认，前端「跟随默认」的落盘形态）；空白 modelId 归一为清除（宽容前端
/// 空串，与 null 等价）；会话不存在 → E_NOT_FOUND（读与写同拒绝）；
/// RunConfig 旧载荷兼容（§3.2 18 号票回写）：无 thinking 字段的旧前端
/// start_chat 载荷照常解析（serde default → None）；新载荷带 thinking 同样解析。
#[test]
fn prefs_read_merge_clear_and_run_config_compat() {
    let (_dir, app, sid) = app_with_session("main");
    // 未设置：空结果（无 modelId/thinking 键）
    let empty = get_session_prefs_core(&app, &sid).expect("读偏好失败");
    assert_eq!(empty, GetSessionPrefsResult::default());
    // 预置存储后读回
    save_prefs(
        app.store.root(),
        &sid,
        &SessionPrefs {
            model_id: Some("m-1".to_owned()),
            thinking: Some(ThinkingLevel::Medium),
        },
    )
    .expect("预置偏好失败");
    assert_eq!(
        get_session_prefs_core(&app, &sid).expect("读偏好失败"),
        GetSessionPrefsResult {
            model_id: Some("m-1".to_owned()),
            thinking: Some(ThinkingLevel::Medium),
        }
    );

    // merge：先设两项，只改 thinking → modelId 保持。
    set_session_prefs_core(
        &app,
        &sid,
        SetSessionPrefsPayload {
            model_id: Some(Some("m-2".to_owned())),
            thinking: Some(Some(ThinkingLevel::High)),
        },
    )
    .expect("首次设置失败");
    set_session_prefs_core(
        &app,
        &sid,
        SetSessionPrefsPayload {
            thinking: Some(Some(ThinkingLevel::Off)),
            ..Default::default()
        },
    )
    .expect("二次设置失败");
    let prefs = load_prefs(app.store.root(), &sid);
    assert_eq!(prefs.model_id.as_deref(), Some("m-2"), "未传字段不动");
    assert_eq!(prefs.thinking, Some(ThinkingLevel::Off));

    // null 清除：modelId=null 清除、thinking 不传不动；thinking=null 同样清除。
    set_session_prefs_core(
        &app,
        &sid,
        SetSessionPrefsPayload {
            model_id: Some(None),
            ..Default::default()
        },
    )
    .expect("清除失败");
    let prefs = load_prefs(app.store.root(), &sid);
    assert_eq!(prefs.model_id, None, "null 清除模型覆盖");
    assert_eq!(prefs.thinking, Some(ThinkingLevel::Off), "未传字段不动");
    set_session_prefs_core(
        &app,
        &sid,
        SetSessionPrefsPayload {
            thinking: Some(None),
            ..Default::default()
        },
    )
    .expect("清除 thinking 失败");
    assert_eq!(load_prefs(app.store.root(), &sid), SessionPrefs::default());

    // 空白 modelId 归一为清除。
    set_session_prefs_core(
        &app,
        &sid,
        SetSessionPrefsPayload {
            model_id: Some(Some("  ".to_owned())),
            thinking: Some(Some(ThinkingLevel::Minimal)),
        },
    )
    .expect("设置失败");
    let prefs = load_prefs(app.store.root(), &sid);
    assert_eq!(prefs.model_id, None, "空白归一为清除");
    assert_eq!(prefs.thinking, Some(ThinkingLevel::Minimal));

    // 会话不存在 → E_NOT_FOUND（读与写同拒绝）。
    let (_dir, app, _sid) = app_with_session("missing");
    let err = get_session_prefs_core(&app, "no-such-session").unwrap_err();
    assert_eq!(err.code, E_NOT_FOUND);
    let err = set_session_prefs_core(
        &app,
        "no-such-session",
        SetSessionPrefsPayload {
            thinking: Some(Some(ThinkingLevel::Low)),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, E_NOT_FOUND);

    // RunConfig 旧载荷兼容（§3.2 18 号票回写）。
    let config: super::super::chat::RunConfig =
        serde_json::from_str(r#"{"tier":"autoEdit","maxTurns":24,"steer":false}"#)
            .expect("旧载荷应可解析");
    assert_eq!(config.thinking, None);
    assert_eq!(config.model_id, None);
    let config: super::super::chat::RunConfig = serde_json::from_str(
        r#"{"tier":"autoEdit","maxTurns":24,"steer":false,"thinking":"high"}"#,
    )
    .expect("新载荷应可解析");
    assert_eq!(config.thinking, Some(ThinkingLevel::High));
}
