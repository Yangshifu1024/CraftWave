//! 审批门（ApprovalGate）单元测试：权限矩阵各档判定、围栏黑名单与写信号、
//! 计划门拦截、已批准签名放行、未知工具透传。

use std::sync::{Arc, Mutex};

use super::gate::ApprovalGate;
use super::run_loop::{GateVerdict, ToolGate};
use crate::safety::tier::PermissionTier;
use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::tools::plan::PlanState;

/// 造一个计划条目（给定 id 与状态）。
fn item(id: &str, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        id: id.to_owned(),
        text: format!("任务 {id}"),
        status,
    }
}

/// 造审批门与其共享计划状态（返回同一把锁的句柄，便于测试中修改计划）。
fn gate_with_plan(tier: PermissionTier) -> (ApprovalGate, Arc<Mutex<Option<PlanState>>>) {
    let plan: Arc<Mutex<Option<PlanState>>> = Arc::new(Mutex::new(None));
    (ApprovalGate::new(tier, plan.clone()), plan)
}

/// 造 command 工具的参数 JSON。
fn command_args(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

/// 断言权限矩阵在各档位下给出正确判定（放行/挂起/硬拦逐例核对）。
#[test]
fn matrix_verdicts_by_tier() {
    // (name, args, tier) → 期望判定形态
    let cases: Vec<(&str, String, PermissionTier, &str)> = vec![
        // confirm_each：FileWrite 挂起
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::ConfirmEach,
            "suspend",
        ),
        (
            "edit",
            r#"{"path":"a.txt","oldText":"x","newText":"y"}"#.into(),
            PermissionTier::ConfirmEach,
            "suspend",
        ),
        // auto_edit：FileWrite 直写、Network（无写信号命令）挂起
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::AutoEdit,
            "allow",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::AutoEdit,
            "suspend",
        ),
        // full_access：FileWrite/无信号 Network 放行
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::FullAccess,
            "allow",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::FullAccess,
            "allow",
        ),
        // plan：FileWrite/Network 过滤硬拦；plan/ask 放行
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::Plan,
            "blocked:E_TOOL_BLOCKED",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::Plan,
            "blocked:E_TOOL_BLOCKED",
        ),
        (
            "plan",
            r#"{"action":"set"}"#.into(),
            PermissionTier::Plan,
            "allow",
        ),
        (
            "ask",
            r#"{"title":"t"}"#.into(),
            PermissionTier::Plan,
            "allow",
        ),
    ];
    for (name, args, tier, expect) in cases {
        let (gate, _) = gate_with_plan(tier);
        let verdict = gate.judge(name, &args);
        let actual = match &verdict {
            GateVerdict::Allow => "allow".to_owned(),
            GateVerdict::Suspend { fence_reason, .. } => {
                if fence_reason.is_some() {
                    "suspend+fence".to_owned()
                } else {
                    "suspend".to_owned()
                }
            }
            GateVerdict::Blocked { code, .. } => format!("blocked:{code}"),
        };
        assert_eq!(actual, expect, "{name} @ {tier:?}");
    }
}

/// 断言围栏黑名单命令在任何权限档位都被 E_FENCE_BLOCKED 硬拦。
#[test]
fn fence_blacklist_blocks_all_tiers() {
    for tier in [
        PermissionTier::Plan,
        PermissionTier::ConfirmEach,
        PermissionTier::AutoEdit,
        PermissionTier::FullAccess,
    ] {
        let (gate, _) = gate_with_plan(tier);
        let verdict = gate.judge("command", &command_args("rm -rf /tmp/whatever"));
        let GateVerdict::Blocked { code, .. } = &verdict else {
            panic!("黑名单应硬拦（{tier:?}）：{verdict:?}")
        };
        assert_eq!(*code, "E_FENCE_BLOCKED");
    }
}

/// 22 号票：装配工作区上下文后，围栏对写目标做 §8.7 四象限判定——
/// roots 内写 fence Allow（落矩阵合成，挂起无围栏原因）；symlink 逃逸、
/// 变量写路径挂起并带具体原因；界外删除硬拦 E_FENCE_BLOCKED。
#[cfg(unix)]
#[test]
fn fence_workspace_quadrant_gate() {
    use crate::backend::gate::GateWorkspace;

    let dir = tempfile::tempdir().expect("临时目录");
    // root 与界外目录做兄弟（outside 必须在 root 之外），root 用 canonical
    // 后路径（macOS /var → /private/var symlink 语义，与生产 PathGuard
    // 装配同口径），root 内建逃逸 symlink。
    let root_dir = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root_dir).expect("建 root");
    std::fs::create_dir_all(&outside).expect("建界外目录");
    let root = std::fs::canonicalize(&root_dir).expect("root canonicalize");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("建 symlink");
    let gate = ApprovalGate::new(PermissionTier::AutoEdit, Arc::new(Mutex::new(None)))
        .with_workspace(GateWorkspace {
            roots: vec![root.clone()],
            base: root.clone(),
        });

    // roots 内写：fence Allow → 落矩阵合成（Network@auto_edit 审批，无围栏原因）。
    match gate.judge("command", &command_args("echo done > build.log")) {
        GateVerdict::Suspend { fence_reason, .. } => {
            assert!(
                fence_reason.is_none(),
                "roots 内写 fence Allow，原因应为 None（纯矩阵审批）"
            )
        }
        other => panic!("roots 内写应走矩阵挂起：{other:?}"),
    }

    // symlink 穿越界外：挂起并带具体原因。
    match gate.judge("command", &command_args("echo x > link/evil.txt")) {
        GateVerdict::Suspend { fence_reason, .. } => {
            let reason = fence_reason.expect("symlink 逃逸应带原因");
            assert!(reason.contains("symlink 逃逸"), "原因应指明逃逸：{reason}");
        }
        other => panic!("symlink 逃逸应挂起：{other:?}"),
    }

    // 变量拼接写路径：挂起并带「未解析变量」原因（字面量掩码）。
    match gate.judge("command", &command_args("echo x > $HOME/evil.txt")) {
        GateVerdict::Suspend { fence_reason, .. } => {
            let reason = fence_reason.expect("变量写路径应带原因");
            assert!(reason.contains("未解析变量"), "原因应指明变量：{reason}");
        }
        other => panic!("变量写路径应挂起：{other:?}"),
    }

    // 界外删除：E_FENCE_BLOCKED 硬拦（删除类越界直接 Block）。
    let outside_file = outside.join("keep.txt");
    match gate.judge(
        "command",
        &command_args(&format!("rm {}", outside_file.display())),
    ) {
        GateVerdict::Blocked { code, message } => {
            assert_eq!(code, "E_FENCE_BLOCKED");
            assert!(
                message.contains("越出工作区"),
                "硬拦信息应指明越界：{message}"
            );
        }
        other => panic!("界外删除应硬拦：{other:?}"),
    }

    // 解析失败 fail-closed：畸形命令挂起并带保守原因（不 fail-open）。
    match gate.judge("command", &command_args("echo \"unterminated")) {
        GateVerdict::Suspend { fence_reason, .. } => {
            let reason = fence_reason.expect("解析失败应带原因");
            assert!(reason.contains("解析失败"), "原因应指明解析失败：{reason}");
        }
        other => panic!("解析失败应保守挂起：{other:?}"),
    }
}

/// 断言含写信号的命令被挂起，且携带围栏原因（界面卡片展示用）。
#[test]
fn fence_write_signal_suspend_with_reason() {
    let (gate, _) = gate_with_plan(PermissionTier::ConfirmEach);
    let verdict = gate.judge("command", &command_args("echo hi > out.txt"));
    let GateVerdict::Suspend { fence_reason, .. } = &verdict else {
        panic!("写信号应挂起：{verdict:?}")
    };
    assert!(
        fence_reason.is_some(),
        "挂起带 fenceReason（AskPayload 展示）"
    );
}

/// 断言 plan 档下白名单命令虽通过围栏，仍被权限矩阵以 E_TOOL_BLOCKED 拦下
/// （围栏放行不等于权限档放行）。
#[test]
fn fence_plan_whitelist_still_filtered_by_matrix() {
    // plan 档：ls 命中白名单（fence Allow），但 Network 能力被矩阵 Filtered 硬拦
    let (gate, _) = gate_with_plan(PermissionTier::Plan);
    let verdict = gate.judge("command", &command_args("ls"));
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("plan 档 command 应被矩阵拦截：{verdict:?}")
    };
    assert_eq!(*code, "E_TOOL_BLOCKED");
}

/// 断言计划门：已批准计划全部完成后再写文件被 E_PLAN_STALE 拦截且提示
/// 更新计划；只读工具不受计划门约束。
#[test]
fn plan_gate_approved_all_completed_blocks_writes() {
    let (gate, plan) = gate_with_plan(PermissionTier::AutoEdit);
    {
        let mut state = plan.lock().unwrap();
        *state = Some(PlanState::default());
        let s = state.as_mut().unwrap();
        s.items = vec![item("a", PlanItemStatus::Completed)];
        s.approve(1);
    }
    let verdict = gate.judge("write", r#"{"path":"a.txt","content":"x"}"#);
    let GateVerdict::Blocked { code, message } = &verdict else {
        panic!("计划门应拦写：{verdict:?}")
    };
    assert_eq!(*code, "E_PLAN_STALE");
    assert!(message.contains("plan"), "提示模型先更新计划：{message}");
    // 只读不受计划门约束
    assert!(matches!(
        gate.judge("read", r#"{"path":"a"}"#),
        GateVerdict::Allow
    ));
}

/// 断言已批准签名不再重复挂起；不同签名的调用不连带放行。
#[test]
fn approved_signature_no_resuspend() {
    let (gate, _) = gate_with_plan(PermissionTier::ConfirmEach);
    let args = r#"{"path":"a.txt","content":"x"}"#;
    assert!(matches!(
        gate.judge("write", args),
        GateVerdict::Suspend { .. }
    ));
    gate.approve("write", args);
    assert!(matches!(gate.judge("write", args), GateVerdict::Allow));
    // 不同签名不连带放行
    let other = r#"{"path":"b.txt","content":"x"}"#;
    assert!(matches!(
        gate.judge("write", other),
        GateVerdict::Suspend { .. }
    ));
}

/// 断言未知工具放行给执行器（由执行器回喂 E_INVALID_ARG，不在门这里拦截）。
#[test]
fn unknown_tool_passes_to_executor() {
    let (gate, _) = gate_with_plan(PermissionTier::Plan);
    assert!(matches!(gate.judge("nope", "{}"), GateVerdict::Allow));
}

/// MCP 动态工具判定表（08 号票，Q44/§9.7）：read_only × 许可三元组 × tier
/// ——未声明 read_only 即便 full_access 也挂起（FR-08-04）；审批「始终允许」
/// 授予的许可三元组命中即放行（热生效，与服务器级 always_allow 开关解耦）；
/// 输入 schema 变更 → 三元组不命中 → 重新挂起（新版本首次调用重确认）；
/// read_only 声明的工具按 ReadOnly 能力全档放行。真实许可存储驱动（与生产
/// McpGateAdapter 同构：view 反查进程级注册表，license_hit 查 trust 文件）。
#[test]
fn mcp_license_gate_matrix() {
    use crate::backend::gate::{McpGateView, McpToolView};
    use crate::mcp::config::{McpScope, McpServerConfig};
    use crate::mcp::tools::ConnKey;
    use crate::mcp::trust::ToolLicenseStore;

    let dir = tempfile::tempdir().expect("临时目录");
    struct LicGate(ToolLicenseStore);
    impl McpGateView for LicGate {
        fn view(&self, name: &str) -> Option<McpToolView> {
            crate::mcp::tools::McpToolRegistry::global()
                .identity(name)
                .map(|i| McpToolView {
                    server_name: i.server_name,
                    server_fingerprint: i.server_fingerprint,
                    original_name: i.original_name,
                    schema_hash: i.schema_hash,
                    read_only: i.read_only,
                })
        }
        fn license_hit(&self, view: &McpToolView) -> bool {
            self.0.is_licensed(
                &view.server_fingerprint,
                &view.original_name,
                &view.schema_hash,
            )
        }
    }
    let mk_gate = || {
        ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
            .with_mcp(Arc::new(LicGate(ToolLicenseStore::new(dir.path()))))
    };

    // 动态段注册 gateprobe 工具（进程级单例；用例尾摘除防污染）
    let key = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "gate-server".into(),
        canonical_path: String::new(),
    };
    let fingerprint = "gate-fp".to_owned();
    let mk_list = |schema: serde_json::Value| {
        vec![(
            "gateprobe".to_owned(),
            "判定探针".to_owned(),
            schema,
            crate::tools::ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        )]
    };
    let cfg = McpServerConfig {
        read_only: false,
        ..McpServerConfig::default()
    };
    let registered = crate::mcp::tools::McpToolRegistry::global().sync(
        key.clone(),
        fingerprint.clone(),
        &cfg,
        mk_list(
            serde_json::json!({ "type": "object", "properties": { "q": { "type": "string" } } }),
        ),
    );
    assert_eq!(registered, vec!["gateprobe".to_owned()]);

    // ① full_access × 未声明 read_only × 无许可 → 挂起（高危外部调用永远过人）
    assert!(matches!(
        mk_gate().judge("gateprobe", "{}"),
        GateVerdict::Suspend { .. }
    ));
    // confirm_each 档同样挂起（MCP 审批与档位无关）
    assert!(matches!(
        ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
            .with_mcp(Arc::new(LicGate(ToolLicenseStore::new(dir.path()))))
            .judge("gateprobe", "{}"),
        GateVerdict::Suspend { .. }
    ));

    // ② 审批「始终允许」授予许可（当前 schema 版本）→ 热生效放行
    let identity = crate::mcp::tools::McpToolRegistry::global()
        .identity("gateprobe")
        .expect("身份在场");
    let licenses = ToolLicenseStore::new(dir.path());
    licenses
        .grant(&fingerprint, "gateprobe", &identity.schema_hash, 1)
        .expect("授予许可");
    assert!(matches!(
        mk_gate().judge("gateprobe", "{}"),
        GateVerdict::Allow
    ));

    // ③ 输入 schema 变更 → 三元组第三元漂移 → 重新挂起（新版本首次重确认；
    // 旧版本许可记录留存但不命中）
    crate::mcp::tools::McpToolRegistry::global().sync(
        key.clone(),
        fingerprint.clone(),
        &cfg,
        mk_list(
            serde_json::json!({ "type": "object", "properties": { "q": { "type": "number" } } }),
        ),
    );
    assert!(matches!(
        mk_gate().judge("gateprobe", "{}"),
        GateVerdict::Suspend { .. }
    ));

    // ④ read_only 用户声明 → 能力 ReadOnly → 矩阵全档放行（无许可也放行）
    let ro_cfg = McpServerConfig {
        read_only: true,
        ..McpServerConfig::default()
    };
    crate::mcp::tools::McpToolRegistry::global().sync(
        key.clone(),
        fingerprint.clone(),
        &ro_cfg,
        mk_list(serde_json::json!({ "type": "object" })),
    );
    assert!(matches!(
        mk_gate().judge("gateprobe", "{}"),
        GateVerdict::Allow
    ));
    assert!(matches!(
        ApprovalGate::new(PermissionTier::Plan, Arc::new(Mutex::new(None)))
            .with_mcp(Arc::new(LicGate(ToolLicenseStore::new(dir.path()))))
            .judge("gateprobe", "{}"),
        GateVerdict::Allow
    ));

    // 用例尾：摘除动态段（进程级单例复原）
    crate::mcp::tools::McpToolRegistry::global().remove(&key);
    assert!(
        crate::mcp::tools::McpToolRegistry::global()
            .identity("gateprobe")
            .is_none(),
        "动态段已清理"
    );
}

// ---- 23 号票：内部状态保护 + 审批记忆 + 命令白名单 + 审批四开关 ----

use crate::backend::gate::{ApprovalMemory, ApprovalPolicy, allowlist_hit};
use crate::safety::internal_state::InternalStateGuard;

/// 造装配了禁区判定面 + 工作区上下文的审批门（生产同构：roots = 项目主
/// 目录 + 数据根下 workspace/ 子目录，均 canonical 后传入；相对路径基准
/// = 项目主目录）。返回门与两根的绝对路径供用例拼参数。
fn guarded_gate(
    tier: PermissionTier,
) -> (
    ApprovalGate,
    std::path::PathBuf,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("临时目录");
    let data_root = dir.path().join("data");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(data_root.join("workspace")).expect("建 workspace");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let data = std::fs::canonicalize(&data_root).expect("data canonicalize");
    let project = std::fs::canonicalize(&project_root).expect("project canonicalize");
    let gate = ApprovalGate::new(tier, Arc::new(Mutex::new(None)))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone(), data.join("workspace")],
            base: project.clone(),
        })
        .with_internal_guard(InternalStateGuard::new(&data, &project));
    (gate, data, project, dir)
}

/// 断言判定为 E_INTERNAL_STATE 硬拦（禁区写入）。
fn assert_internal_blocked(verdict: &GateVerdict, context: &str) {
    let GateVerdict::Blocked { code, message } = verdict else {
        panic!("{context} 应为 E_INTERNAL_STATE 硬拦：{verdict:?}")
    };
    assert_eq!(*code, "E_INTERNAL_STATE", "{context}");
    assert!(
        message.contains("内部状态"),
        "硬拦信息应指明内部状态：{message}"
    );
}

/// 篡改向量（票面验收①）：write 工具直写数据侧 config.json、命令重定向
/// 覆盖全局 mcp.json、改 .segmeta.json（段元数据）、改项目侧
/// .craftwave/project.json、写 sessions/ 会话存储——全部 E_INTERNAL_STATE，
/// 且任何权限档都不豁免（含 full_access）。
#[test]
fn tamper_vectors_all_hard_blocked() {
    for tier in [
        PermissionTier::ConfirmEach,
        PermissionTier::AutoEdit,
        PermissionTier::FullAccess,
    ] {
        let (gate, data, project, _keep) = guarded_gate(tier);
        // (工具名, 参数) 向量表：数据侧用绝对路径，项目侧混一条相对路径
        //（相对基准 = 项目主目录，验证 cwd 基准展开面）
        let vectors: Vec<(String, String)> = vec![
            (
                "write".into(),
                serde_json::json!({
                    "path": data.join("config.json").to_string_lossy(),
                    "content": "{}"
                })
                .to_string(),
            ),
            (
                "command".into(),
                serde_json::json!({
                    "command": format!("echo '{{}}' > {}", data.join("mcp.json").display())
                })
                .to_string(),
            ),
            (
                "command".into(),
                serde_json::json!({
                    "command": format!(
                        "echo x > {}",
                        data.join("histories/s1/.segmeta.json").display()
                    )
                })
                .to_string(),
            ),
            (
                "command".into(),
                r#"{"command":"echo x > .craftwave/project.json"}"#.to_owned(),
            ),
            (
                "command".into(),
                serde_json::json!({
                    "command": format!("echo x > {}", data.join("sessions/s.json").display())
                })
                .to_string(),
            ),
        ];
        for (name, args) in &vectors {
            let verdict = gate.judge(name, args);
            let ctx = format!("{name}@{tier:?} args={args}");
            assert_internal_blocked(&verdict, &ctx);
        }
        let _ = project;
    }
}

/// 篡改向量（symlink）：工作区内 symlink 指入禁区 → E_INTERNAL_STATE 硬拦
///（词法在 roots 内、canonical 落点在禁区；fence 原判定 Confirm，禁区硬拒
/// 优先于挂起）。
#[cfg(unix)]
#[test]
fn symlink_into_internal_state_hard_blocked() {
    let (gate, data, project, _keep) = guarded_gate(PermissionTier::FullAccess);
    std::fs::write(data.join("config.json"), "{}").expect("写 config.json");
    std::os::unix::fs::symlink(data.join("config.json"), project.join("out.json"))
        .expect("建 symlink");
    let verdict = gate.judge("command", &command_args("echo x > out.json"));
    assert_internal_blocked(&verdict, "symlink 指入禁区");
}

/// 读不禁（C-8 已声明风险）：读命令即使目标是禁区文件，也不因内部状态
/// 硬拒（落矩阵原有判定——confirm_each 挂起、full_access 放行）。
/// 24 号票适配：config.json 在会话 roots 外，full_access 的 read 现在落
/// 外部目录授权挂起（§9.9）——「读不禁内部状态」的语义保持为非 Blocked
///（禁区硬拒不触及读路径），授权挂起是外部目录判定层的独立行为。
#[test]
fn read_paths_not_blocked_by_internal_state() {
    let (gate_confirm, data, _project, _keep) = guarded_gate(PermissionTier::ConfirmEach);
    let args = serde_json::json!({
        "command": format!("cat {}", data.join("config.json").display())
    })
    .to_string();
    let verdict = gate_confirm.judge("command", &args);
    assert!(
        !matches!(verdict, GateVerdict::Blocked { .. }),
        "读不禁：{verdict:?}"
    );
    let (gate_full, data2, _project2, _keep2) = guarded_gate(PermissionTier::FullAccess);
    let verdict = gate_full.judge(
        "read",
        &serde_json::json!({"path": data2.join("config.json").to_string_lossy()}).to_string(),
    );
    assert!(
        !matches!(verdict, GateVerdict::Blocked { .. }),
        "读不禁内部状态硬拒：{verdict:?}"
    );
    // roots 外路径挂外部目录授权（24 号票判定层接管，非权限矩阵 Allow）
    assert!(
        matches!(
            &verdict,
            GateVerdict::Suspend {
                external_dir: Some(_),
                ..
            }
        ),
        "外部路径挂授权询问：{verdict:?}"
    );
}

/// 既有 workspace 写不受禁区判定影响：工作区内写走原有判定
///（confirm_each 矩阵挂起、full_access 放行），不被误拦。
#[test]
fn workspace_writes_unaffected() {
    let (gate_confirm, _data, _project, _keep) = guarded_gate(PermissionTier::ConfirmEach);
    assert!(matches!(
        gate_confirm.judge("command", &command_args("echo hi > out.txt")),
        GateVerdict::Suspend { .. }
    ));
    let (gate_full, _d, _p, _k) = guarded_gate(PermissionTier::FullAccess);
    assert!(matches!(
        gate_full.judge("command", &command_args("echo hi > out.txt")),
        GateVerdict::Allow
    ));
}

/// 未装配禁区判定面（with_internal_guard 缺省）不拦——headless 测试与
/// 旧装配面零漂移（防线完整性由宿主装配保证，测试装配面显式覆盖）。
#[test]
fn internal_guard_not_assembled_no_block() {
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    assert!(matches!(
        gate.judge(
            "write",
            r#"{"path":"/Users/x/.craftwave/config.json","content":"{}"}"#
        ),
        GateVerdict::Allow
    ));
}

/// 目标含未解析变量（Opaque）不做硬拒：值未知不猜，交围栏字面量掩码
/// 保守 Confirm + 36 号票审计（挂起而非 E_INTERNAL_STATE，票面边界）。
#[test]
fn opaque_target_falls_back_to_fence_confirm() {
    let (gate, _d, _p, _k) = guarded_gate(PermissionTier::FullAccess);
    let verdict = gate.judge(
        "command",
        &command_args("echo x > $HOME/.craftwave/config.json"),
    );
    match &verdict {
        GateVerdict::Suspend { fence_reason, .. } => {
            let reason = fence_reason.as_ref().expect("Opaque 目标应有围栏原因");
            assert!(reason.contains("未解析变量"), "字面量掩码原因：{reason}");
        }
        other => panic!("Opaque 目标应保守挂起：{other:?}"),
    }
}

/// 审批记忆（票面验收②）：always_session 登记后同签名跨 run 免批
///（新 ApprovalGate 挂同一会话桶）；其他会话不延续；forget 与 clear_all
/// 生效（Q41：删除/恢复后须重新确认）。
#[test]
fn always_session_memory_cross_run() {
    let memory = ApprovalMemory::new();
    let session = "sess-1";
    let args = command_args("npm install");
    // 第一个 run：confirm_each 档挂起 → 用户选 always_session → 登记
    let gate1 = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_session_memory(memory.bucket(session));
    assert!(matches!(
        gate1.judge("command", &args),
        GateVerdict::Suspend { .. }
    ));
    memory.remember(session, "command", &args);
    // 第二个 run（新门实例）：同签名免批
    let gate2 = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_session_memory(memory.bucket(session));
    assert!(
        matches!(gate2.judge("command", &args), GateVerdict::Allow),
        "always_session 后同命令跨 run 免批"
    );
    // 不同签名不连带放行
    let other = command_args("npm test");
    assert!(matches!(
        gate2.judge("command", &other),
        GateVerdict::Suspend { .. }
    ));
    // 新会话不延续
    assert!(matches!(
        ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
            .with_session_memory(memory.bucket("sess-2"))
            .judge("command", &args),
        GateVerdict::Suspend { .. }
    ));
    // 会话删除随清（forget）
    memory.forget(session);
    assert_eq!(memory.len_of(session), 0, "forget 后桶空");
    assert!(matches!(
        ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
            .with_session_memory(memory.bucket(session))
            .judge("command", &args),
        GateVerdict::Suspend { .. }
    ));
    // Q41 接线面：恢复备份清全部（33 号票恢复流程调用 clear_all）
    memory.remember(session, "command", &args);
    memory.clear_all();
    assert_eq!(memory.len_of(session), 0);
}

/// 记忆句柄与登记写入口同源：bucket 取出的句柄在 remember 之后即时可见。
#[test]
fn session_bucket_shares_with_memory() {
    let memory = ApprovalMemory::new();
    let bucket = memory.bucket("s");
    memory.remember("s", "write", r#"{"path":"a"}"#);
    assert!(bucket.contains("write", r#"{"path":"a"}"#));
    assert!(!bucket.contains("write", r#"{"path":"b"}"#));
}

/// 命令白名单词序列前缀匹配表（23 号票字段级语义）。
#[test]
fn allowlist_word_prefix_table() {
    let cases: Vec<(&str, &str, bool)> = vec![
        ("git status --short", "git status", true),  // 前缀命中
        ("git status", "git status", true),          // 全等命中
        ("git statusx", "git status", false),        // 词边界：statusx 不匹配
        ("git st", "git status", false),             // 子命令前缀不算命中
        ("cd a && git status", "git status", false), // 复合命令整串切词不命中
        ("git status", "", false),                   // 空条目不命中
        ("npm run build", "npm run", true),          // 首词+子命令
        ("NPM run build", "npm run", false),         // 大小写敏感（保守）
        ("cargo test --lib", "cargo test", true),
    ];
    for (command, entry, expect) in cases {
        assert_eq!(
            allowlist_hit(command, entry),
            expect,
            "command={command:?} entry={entry:?}"
        );
    }
}

/// 白名单消费（票面验收③）：命中免除围栏 Confirm 与矩阵 NeedsApproval
///（confirm_each 档直放）；黑名单 Block 永不豁免；④ 关闭不豁免；plan 档
/// 不豁免（档位只读铁律）。
#[test]
fn command_allowlist_consumption() {
    let policy_hit = ApprovalPolicy {
        enabled: true,
        allowlist_enabled: true,
        command_allowlist: vec!["npm install".to_owned()],
    };
    // 命中：围栏写信号 Confirm 免除 + 矩阵 NeedsApproval 免除 → 放行
    let gate = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_policy(policy_hit.clone());
    assert!(matches!(
        gate.judge("command", &command_args("npm install")),
        GateVerdict::Allow
    ));
    assert!(
        matches!(
            gate.judge("command", &command_args("npm install --save-dev")),
            GateVerdict::Allow
        ),
        "前缀命中（多出的参数不改命中）"
    );
    assert!(
        matches!(
            gate.judge("command", &command_args("npm ci")),
            GateVerdict::Suspend { .. }
        ),
        "未命中条目的命令仍挂起"
    );
    // 黑名单永不豁免：条目命中但命令是灾难级
    let gate_evil = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_policy(ApprovalPolicy {
            enabled: true,
            allowlist_enabled: true,
            command_allowlist: vec!["rm -rf /tmp/x".to_owned()],
        });
    let verdict = gate_evil.judge("command", &command_args("rm -rf /tmp/x"));
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("黑名单应硬拦（白名单不豁免）：{verdict:?}")
    };
    assert_eq!(*code, "E_FENCE_BLOCKED");
    // ④ 关闭：条目在但不参与判定
    let gate_disabled = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_policy(ApprovalPolicy {
            allowlist_enabled: false,
            ..policy_hit.clone()
        });
    assert!(matches!(
        gate_disabled.judge("command", &command_args("npm install")),
        GateVerdict::Suspend { .. }
    ));
    // plan 档不豁免：档位只读铁律——白名单命中也保持围栏规则 2 的
    // Confirm 挂起（「不在 plan 档只读白名单内」），不放行
    let gate_plan =
        ApprovalGate::new(PermissionTier::Plan, Arc::new(Mutex::new(None))).with_policy(policy_hit);
    let verdict = gate_plan.judge("command", &command_args("npm install"));
    let GateVerdict::Suspend { fence_reason, .. } = &verdict else {
        panic!("plan 档应保持围栏挂起（白名单不豁免）：{verdict:?}")
    };
    let reason = fence_reason.as_ref().expect("plan 档挂起带原因");
    assert!(reason.contains("plan 档只读白名单"), "原因：{reason}");
}

/// 审批总开关（票面验收④①）：关闭后 confirm_each 档不再弹审批（矩阵
/// NeedsApproval 与围栏 Confirm 直接放行）；黑名单/内部状态/plan 档不变。
#[test]
fn approval_switch_enabled_false_auto_allows() {
    let policy_off = ApprovalPolicy {
        enabled: false,
        allowlist_enabled: false,
        command_allowlist: Vec::new(),
    };
    let gate = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_policy(policy_off.clone());
    // 矩阵挂起面（write@confirm_each）→ 放行
    assert!(matches!(
        gate.judge("write", r#"{"path":"a.txt","content":"x"}"#),
        GateVerdict::Allow
    ));
    // 围栏 Confirm 面（写信号命令）→ 放行
    assert!(matches!(
        gate.judge("command", &command_args("echo hi > out.txt")),
        GateVerdict::Allow
    ));
    // 黑名单仍硬拦
    let verdict = gate.judge("command", &command_args("sudo rm x"));
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("黑名单应硬拦（总开关不豁免）：{verdict:?}")
    };
    assert_eq!(*code, "E_FENCE_BLOCKED");
    // plan 档判定不变（总开关不解锁计划档：围栏规则 2 的 Confirm 保留）
    let gate_plan =
        ApprovalGate::new(PermissionTier::Plan, Arc::new(Mutex::new(None))).with_policy(policy_off);
    assert!(
        matches!(
            gate_plan.judge("command", &command_args("echo hi > out.txt")),
            GateVerdict::Suspend { .. }
        ),
        "plan 档围栏 Confirm 保留（档位语义，不因总开关放行）"
    );
    // 内部状态硬拒不受总开关影响
    let (gate_zone, data, _project, _keep) = guarded_gate(PermissionTier::ConfirmEach);
    let gate_zone = gate_zone.with_policy(ApprovalPolicy {
        enabled: false,
        allowlist_enabled: true,
        command_allowlist: vec![],
    });
    let args = serde_json::json!({
        "command": format!("echo x > {}", data.join("config.json").display())
    })
    .to_string();
    assert_internal_blocked(&gate_zone.judge("command", &args), "禁区命中@总开关关");
}

// ---- 外部目录授权判定（24 号票，§9.9）----

/// 文件类工具访问 roots 外路径 → Suspend 且 external_dir = 目标父目录
///（read/write/edit）；授权后 grant_external_dir → 重放放行；目录型工具
///（list_files/glob/grep）授权目标 = 路径本身；非文件工具不判。
#[test]
fn external_dir_hit_and_grant_flow() {
    let dir = tempfile::tempdir().expect("临时目录");
    let data_root = dir.path().join("data");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(data_root.join("workspace")).expect("建 workspace");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let external = dir.path().join("outside");
    std::fs::create_dir_all(&external).expect("建外部目录");
    let data = std::fs::canonicalize(&data_root).unwrap();
    let project = std::fs::canonicalize(&project_root).unwrap();
    let outside = std::fs::canonicalize(&external).unwrap();
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone(), data.join("workspace")],
            base: project.clone(),
        })
        .with_external_dirs(Vec::new(), data.clone());
    let args_of =
        |p: &std::path::Path| serde_json::json!({"path": p.to_string_lossy()}).to_string();

    // roots 内：不命中（Allow，矩阵放行）
    assert!(matches!(
        gate.judge("read", &args_of(&project.join("a.txt"))),
        GateVerdict::Allow
    ));
    // roots 外文件：Suspend 且 external_dir = 父目录（文件类工具粒度）
    let GateVerdict::Suspend { external_dir, .. } =
        gate.judge("read", &args_of(&outside.join("docs").join("a.md")))
    else {
        panic!("外部路径应挂授权");
    };
    let granted_dir = external_dir.expect("带授权目标");
    assert_eq!(
        std::path::Path::new(&granted_dir),
        outside.join("docs"),
        "文件类工具授权目标 = 父目录"
    );
    // 目录型工具：授权目标 = 路径本身（「该目录及子目录」的列目录粒度）
    let GateVerdict::Suspend { external_dir, .. } =
        gate.judge("list_files", &args_of(&outside.join("docs")))
    else {
        panic!("目录型工具外部路径应挂授权");
    };
    assert_eq!(
        std::path::Path::new(&external_dir.expect("带授权目标")),
        outside.join("docs"),
    );
    // 非 roots 外关注的工具（command）：外部判定不触及（围栏/矩阵原判定）
    assert!(matches!(
        gate.judge("command", &command_args("echo hi")),
        GateVerdict::Suspend { .. } | GateVerdict::Allow
    ));

    // 授权（恢复循环热追加）→ 重放放行
    gate.grant_external_dir(outside.join("docs"));
    assert!(matches!(
        gate.judge("read", &args_of(&outside.join("docs").join("a.md"))),
        GateVerdict::Allow
    ));
    // 同目录子路径也放行（「目录及子目录」粒度）
    assert!(matches!(
        gate.judge(
            "read",
            &args_of(&outside.join("docs").join("sub").join("b.md"))
        ),
        GateVerdict::Allow
    ));
    // 其他外部目录仍挂起
    let GateVerdict::Suspend { external_dir, .. } =
        gate.judge("read", &args_of(&outside.join("other").join("c.md")))
    else {
        panic!("其他目录仍应挂授权");
    };
    assert_eq!(
        std::path::Path::new(&external_dir.expect("带授权目标")),
        outside.join("other"),
    );
}

/// 禁授权目标（§9.9 整体放行禁区）：家目录本身 → E_INVALID_ARG 硬拦
///（home 由环境变量注入，tempdir 数据目录做托管目录基准）。
#[test]
fn external_grant_protected_targets_blocked() {
    let dir = tempfile::tempdir().expect("临时目录");
    let data_root = dir.path().join("data");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(data_root.join("workspace")).expect("建 workspace");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let data = std::fs::canonicalize(&data_root).unwrap();
    let project = std::fs::canonicalize(&project_root).unwrap();
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone(), data.join("workspace")],
            base: project_root.canonicalize().unwrap(),
        })
        .with_external_dirs(Vec::new(), data.clone());
    // 数据目录本身（托管目录）及其后代（workspace 是核心根内不触发外部判定；
    // 托管目录下非 workspace 的路径 = 禁区）
    let verdict = gate.judge(
        "read",
        &serde_json::json!({"path": data.join("config.json").to_string_lossy()}).to_string(),
    );
    let GateVerdict::Blocked { code, .. } = verdict else {
        panic!("托管目录路径应硬拦（授权禁区）");
    };
    assert_eq!(
        code, "E_INVALID_ARG",
        "读路径不触及内部状态硬拒（23 号票只拦写），但授权目标 = 数据目录本身（托管目录禁区）→ E_INVALID_ARG"
    );
    // 家目录（若环境有 HOME）：读 <home>/xxx → 授权目标 <home> = 禁区
    if let Ok(home) = std::fs::canonicalize(std::path::Path::new(
        &std::env::var("HOME").unwrap_or_default(),
    )) && home != project
        && !data.starts_with(&home)
    {
        let verdict = gate.judge(
            "read",
            &serde_json::json!({"path": home.join("外文件.txt").to_string_lossy()}).to_string(),
        );
        let GateVerdict::Blocked { code, .. } = verdict else {
            panic!("家目录授权目标应硬拦");
        };
        assert_eq!(code, "E_INVALID_ARG");
    }
}

/// batch_read 多路径：首个越界路径的父目录作为授权目标；授权后其余越界
/// 目录在重放时继续逐个询问（签名放行不吞并未授权路径——外部判定在签名前）。
#[test]
fn batch_read_multiple_external_paths_ask_one_at_a_time() {
    let dir = tempfile::tempdir().expect("临时目录");
    let data_root = dir.path().join("data");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(data_root.join("workspace")).expect("建 workspace");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let external = dir.path().join("outside");
    std::fs::create_dir_all(external.join("d1")).unwrap();
    std::fs::create_dir_all(external.join("d2")).unwrap();
    let data = std::fs::canonicalize(&data_root).unwrap();
    let project = std::fs::canonicalize(&project_root).unwrap();
    let outside = std::fs::canonicalize(&external).unwrap();
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project, data.join("workspace")],
            base: project_root.canonicalize().unwrap(),
        })
        .with_external_dirs(Vec::new(), data);
    let args = serde_json::json!({
        "paths": [
            outside.join("d1").join("a.md").to_string_lossy(),
            outside.join("d2").join("b.md").to_string_lossy(),
        ]
    })
    .to_string();
    let GateVerdict::Suspend { external_dir, .. } = gate.judge("batch_read", &args) else {
        panic!("batch_read 外部路径应挂授权");
    };
    assert_eq!(
        std::path::Path::new(&external_dir.expect("带授权目标")),
        outside.join("d1"),
        "首个越界路径的父目录"
    );
    // 授权 d1 后重放：d2 仍未授权 → 再次挂起（签名放行不吞并未授权路径）
    gate.grant_external_dir(outside.join("d1"));
    let GateVerdict::Suspend { external_dir, .. } = gate.judge("batch_read", &args) else {
        panic!("d2 未授权应继续挂起");
    };
    assert_eq!(
        std::path::Path::new(&external_dir.expect("带授权目标")),
        outside.join("d2"),
    );
    // d2 也授权 → 整批放行
    gate.grant_external_dir(outside.join("d2"));
    assert!(matches!(
        gate.judge("batch_read", &args),
        GateVerdict::Allow
    ));
}
