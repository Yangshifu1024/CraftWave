//! 审批 AskPayload 构造与 suggest 上下文提取的纯逻辑单测（14 号票）。

use crate::backend::ask::{AskMode, AskPayload, DiffLineKind};
use crate::backend::run_loop::PendingCall;
use crate::storage::entries::{MessageEntry, MessageRole, WirePart};

use super::chat::{build_approval_payload, context_tail};

/// 构造一个挂起中的工具调用（PendingCall），call_id 固定、参数按用例传入。
fn pending(name: &str, args_json: &str) -> PendingCall {
    PendingCall {
        model_call_id: "call_1".to_owned(),
        name: name.to_owned(),
        args_json: args_json.to_owned(),
        fence_reason: None,
        external_dir: None,
    }
}

/// write 工具的审批载荷：标题含路径、diff 全为新增行（新文件整体是增量）、
/// 选项固定「允许（推荐）/ 拒绝」两项（write 非命令类，③ 开也不出
/// always_session）。
#[test]
fn write_payload_new_file_all_add() {
    let payload = build_approval_payload(
        &pending(
            "write",
            r#"{"path":"src/new.rs","content":"fn main() {}\n"}"#,
        ),
        true,
    );
    let AskPayload::Approval {
        title,
        diff,
        command,
        options,
        ..
    } = payload
    else {
        panic!("应为 Approval 形态")
    };
    assert!(title.contains("src/new.rs"));
    assert!(command.is_none());
    let diff = diff.expect("write 带 diff");
    assert_eq!(diff.path, "src/new.rs");
    let lines = &diff.hunks[0].lines;
    assert!(
        lines.iter().all(|l| l.kind == DiffLineKind::Add),
        "新文件全 add"
    );
    assert_eq!(lines[0].text, "fn main() {}");
    assert_eq!(options.len(), 2, "允许/拒绝两项");
    assert_eq!(options[0].id, "allow_once");
    assert_eq!(options[0].recommended, Some(true));
    assert_eq!(options[1].id, "deny");
}

/// edit 工具的审批载荷：旧文本逐行标删除、新文本逐行标新增，拼成一个替换块。
#[test]
fn edit_payload_replacement_hunk_del_add() {
    let payload = build_approval_payload(
        &pending(
            "edit",
            r#"{"path":"a.txt","oldText":"旧一\n旧二","newText":"新一"}"#,
        ),
        true,
    );
    let AskPayload::Approval { diff, .. } = payload else {
        panic!("应为 Approval")
    };
    let diff = diff.expect("edit 带 diff");
    let kinds: Vec<DiffLineKind> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![DiffLineKind::Del, DiffLineKind::Del, DiffLineKind::Add]
    );
    assert_eq!(diff.hunks[0].lines[2].text, "新一");
}

/// command 工具的审批载荷：命令行原文随载荷展示、围栏确认原因透传、不带 diff；
/// ③ 会话内记忆开启时选项含 always_session（23 号票），关闭时不出。
#[test]
fn command_payload_with_fence_reason() {
    let p = PendingCall {
        model_call_id: "c".to_owned(),
        name: "command".to_owned(),
        args_json: r#"{"command":"echo hi > out.txt"}"#.to_owned(),
        fence_reason: Some("命令含写信号，需确认".to_owned()),
        external_dir: None,
    };
    let AskPayload::Approval {
        title,
        diff,
        command,
        fence_reason,
        options,
        ..
    } = build_approval_payload(&p, true)
    else {
        panic!("应为 Approval")
    };
    assert_eq!(command.as_deref(), Some("echo hi > out.txt"));
    assert!(fence_reason.is_some(), "围栏 Confirm 原因随载荷展示");
    assert!(diff.is_none(), "命令不走 diff");
    assert!(title.contains("命令"));
    // ③ 开：允许 / 始终允许（本会话）/ 拒绝 三项，always_session 非推荐项
    assert_eq!(options.len(), 3, "命令类审批三选项（23 号票）");
    assert_eq!(options[1].id, "always_session");
    assert_eq!(options[1].mode, Some(AskMode::AlwaysSession));
    assert_eq!(options[1].recommended, None, "非推荐项不参与自动确认");
    // ③ 关：不出该选项（M1 两项口径不变）
    let AskPayload::Approval { options, .. } = build_approval_payload(&p, false) else {
        panic!("应为 Approval")
    };
    assert_eq!(options.len(), 2, "③ 关闭时保持允许/拒绝两项");
    assert!(!options.iter().any(|o| o.id == "always_session"));
}

/// suggest 上下文提取优先取最后一条 assistant 的正文；没有 assistant 时
/// 退而取最后一条 user；完全为空则返回空串（suggest 静默放弃）。
#[test]
fn suggest_context_prefers_last_assistant_text() {
    let entry = |role: MessageRole, text: &str| MessageEntry {
        id: format!("m-{text:?}"),
        parent_id: None,
        role,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let messages = vec![
        entry(MessageRole::User, "读一下"),
        entry(MessageRole::Assistant, "第一轮回答"),
        entry(MessageRole::User, "再总结"),
        entry(MessageRole::Assistant, "最终结论：OK"),
    ];
    assert_eq!(context_tail(&messages), "最终结论：OK");

    // 无 assistant：取最后一条（user）文本
    let only_user = vec![entry(MessageRole::User, "只有问题")];
    assert_eq!(context_tail(&only_user), "只有问题");

    // 全空：空串（suggest 静默不发）
    assert_eq!(context_tail(&[]), "");
}
