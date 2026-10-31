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

/// 断言权限矩阵判定表：各档位下给出正确判定（放行/挂起/硬拦逐例核对）、
/// 含写信号的命令挂起且携带围栏原因（界面卡片展示用）、未知工具放行给
/// 执行器（由执行器回喂 E_INVALID_ARG，不在门这里拦截）。
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
        // 写信号命令挂起且带围栏原因（§8.7 退化语义：有写目标即 Confirm）
        (
            "command",
            command_args("echo hi > out.txt"),
            PermissionTier::ConfirmEach,
            "suspend+fence",
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
        // plan：FileWrite/Network 过滤硬拦；plan/ask 放行；未知工具透传
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
        ("nope", "{}".into(), PermissionTier::Plan, "allow"),
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

/// 断言围栏硬面：黑名单命令在任何权限档位都被 E_FENCE_BLOCKED 硬拦（红线：
/// 任何档位不豁免）；装配工作区上下文后对写目标做 §8.7 四象限判定——
/// roots 内写 fence Allow（落矩阵合成，挂起无围栏原因）、symlink 逃逸与
/// 变量写路径挂起并带具体原因、界外删除硬拦 E_FENCE_BLOCKED、解析失败
/// fail-closed 保守挂起（不 fail-open）。22 号票。
#[cfg(unix)]
#[test]
fn fence_blacklist_and_workspace_quadrants() {
    // 黑名单全档硬拦
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

    // 四象限判定（22 号票）
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

/// 断言计划门顺序（红线）：已批准计划全部完成后再写文件被 E_PLAN_STALE
/// 拦截且提示更新计划、只读工具不受计划门约束（M1 零回归）；E_PLAN_STALE
/// 优先于范围门——计划全部完成后范围外写是过期硬拦而非范围卡挂起（25 号
/// 票 FR-06-02）。
#[test]
fn plan_stale_gate_order() {
    // M1：全部完成后写被 E_PLAN_STALE 拦、只读不受约束
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
    assert!(
        matches!(gate.judge("read", r#"{"path":"a"}"#), GateVerdict::Allow),
        "只读不受计划门约束"
    );

    // 过期优先于范围门：范围外写 → 过期硬拦（非范围卡挂起）
    let dir = tempfile::tempdir().expect("临时目录");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let project = std::fs::canonicalize(&project_root).expect("canonicalize");
    let mut state = PlanState {
        items: vec![PlanItem {
            id: "p1".to_owned(),
            text: "任务".to_owned(),
            status: PlanItemStatus::Completed,
        }],
        paths: vec!["src/a.rs".to_owned()],
        ..PlanState::default()
    };
    state.approve(1000);
    let gate = ApprovalGate::new(
        PermissionTier::FullAccess,
        Arc::new(Mutex::new(Some(state))),
    )
    .with_workspace(crate::backend::gate::GateWorkspace {
        roots: vec![project.clone()],
        base: project.clone(),
    });
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/outside.rs", "content": "x"}).to_string(),
    );
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("过期优先：应为 E_PLAN_STALE 硬拦：{verdict:?}")
    };
    assert_eq!(*code, "E_PLAN_STALE");
}

/// 断言审批签名与会话记忆：已批准签名不再重复挂起、不同签名不连带放行；
/// always_session 记忆跨 run 免批（新 ApprovalGate 挂同一会话桶）、其他
/// 会话不延续、forget 与 clear_all 生效（Q41：删除/恢复后须重新确认）；
/// 记忆句柄与登记写入口同源（bucket 取出的句柄在 remember 之后即时可见）。
#[test]
fn approved_signature_and_session_memory_cases() {
    use crate::backend::gate::ApprovalMemory;

    // 已批准签名不再挂起；不同签名不连带
    let (gate, _) = gate_with_plan(PermissionTier::ConfirmEach);
    let args = r#"{"path":"a.txt","content":"x"}"#;
    assert!(matches!(
        gate.judge("write", args),
        GateVerdict::Suspend { .. }
    ));
    gate.approve("write", args);
    assert!(matches!(gate.judge("write", args), GateVerdict::Allow));
    assert!(
        matches!(
            gate.judge("write", r#"{"path":"b.txt","content":"x"}"#),
            GateVerdict::Suspend { .. }
        ),
        "不同签名不连带放行"
    );

    // always_session 跨 run 免批 + 边界
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
    // bucket 句柄同源：登记后即时可见
    let bucket = memory.bucket(session);
    assert!(bucket.contains("command", &args));
    assert!(!bucket.contains("command", &command_args("npm test")));
    // 第二个 run（新门实例）：同签名免批
    let gate2 = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
        .with_session_memory(memory.bucket(session));
    assert!(
        matches!(gate2.judge("command", &args), GateVerdict::Allow),
        "always_session 后同命令跨 run 免批"
    );
    // 不同签名不连带放行
    assert!(
        matches!(
            gate2.judge("command", &command_args("npm test")),
            GateVerdict::Suspend { .. }
        ),
        "不同签名不连带放行"
    );
    // 新会话不延续
    assert!(
        matches!(
            ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
                .with_session_memory(memory.bucket("sess-2"))
                .judge("command", &args),
            GateVerdict::Suspend { .. }
        ),
        "其他会话不延续"
    );
    // 会话删除随清（forget）
    memory.forget(session);
    assert_eq!(memory.len_of(session), 0, "forget 后桶空");
    assert!(
        matches!(
            ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)))
                .with_session_memory(memory.bucket(session))
                .judge("command", &args),
            GateVerdict::Suspend { .. }
        ),
        "forget 后须重新确认"
    );
    // Q41 接线面：恢复备份清全部（33 号票恢复流程调用 clear_all）
    memory.remember(session, "command", &args);
    memory.clear_all();
    assert_eq!(memory.len_of(session), 0, "clear_all 清全部");
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

use crate::backend::gate::{ApprovalPolicy, allowlist_hit};
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

/// 断言禁区篡改向量全硬拦（红线，票面验收①）：write 工具直写数据侧
/// config.json、命令重定向覆盖全局 mcp.json、改 .segmeta.json（段元数据）、
/// 改项目侧 .craftwave/project.json、写 sessions/ 会话存储——全部
/// E_INTERNAL_STATE，且任何权限档都不豁免（含 full_access）；工作区内
/// symlink 词法在 roots 内、canonical 落点在禁区同样硬拒（fence 原判定
/// Confirm，禁区硬拒优先于挂起）。
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

    // symlink 指入禁区（unix）：词法在 roots 内、canonical 落点在禁区
    #[cfg(unix)]
    {
        let (gate, data, project, _keep) = guarded_gate(PermissionTier::FullAccess);
        std::fs::write(data.join("config.json"), "{}").expect("写 config.json");
        std::os::unix::fs::symlink(data.join("config.json"), project.join("out.json"))
            .expect("建 symlink");
        let verdict = gate.judge("command", &command_args("echo x > out.json"));
        assert_internal_blocked(&verdict, "symlink 指入禁区");
    }
}

/// 断言禁区判定不误拦的边界：读不禁（C-8 已声明风险）——读命令即使目标
/// 是禁区文件也不因内部状态硬拒（24 号票适配：roots 外路径落外部目录授权
/// 挂起 §9.9，「读不禁」语义保持为非 Blocked）；既有 workspace 写不受禁区
/// 判定影响（confirm_each 挂起、full_access 放行）；未装配禁区判定面
///（with_internal_guard 缺省）不拦——headless 测试与旧装配面零漂移；目标
/// 含未解析变量（Opaque）不做硬拒——值未知不猜，交围栏字面量掩码保守
/// Confirm + 36 号票审计（挂起而非 E_INTERNAL_STATE，票面边界）。
#[test]
fn internal_guard_no_overblock_cases() {
    // ① 读不禁：confirm_each 的 cat 与 full_access 的 read 均非 Blocked
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

    // ② 既有 workspace 写不受禁区判定影响
    let (gate_confirm, _data, _project, _keep) = guarded_gate(PermissionTier::ConfirmEach);
    assert!(matches!(
        gate_confirm.judge("command", &command_args("echo hi > out.txt")),
        GateVerdict::Suspend { .. }
    ));
    let (gate_full, _d, _p, _k) = guarded_gate(PermissionTier::FullAccess);
    assert!(
        matches!(
            gate_full.judge("command", &command_args("echo hi > out.txt")),
            GateVerdict::Allow
        ),
        "full_access 工作区写放行，不被误拦"
    );

    // ③ 未装配禁区判定面不拦
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    assert!(matches!(
        gate.judge(
            "write",
            r#"{"path":"/Users/x/.craftwave/config.json","content":"{}"}"#
        ),
        GateVerdict::Allow
    ));

    // ④ Opaque 目标不硬拒：围栏字面量掩码保守挂起
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

/// 断言命令白名单与审批开关面（23 号票四开关，票面验收③④）：
/// 词序列前缀匹配表（23 号票字段级语义）；白名单命中免除围栏 Confirm 与
/// 矩阵 NeedsApproval（前缀命中、未命中仍挂起）、黑名单 Block 永不豁免、
/// ④ 关闭不豁免、plan 档不豁免（档位只读铁律）；审批总开关关闭后
/// confirm_each 档不再弹审批（矩阵 NeedsApproval 与围栏 Confirm 直接放行）、
/// 黑名单/内部状态/plan 档不变。
#[test]
fn allowlist_and_switch_cases() {
    // ---- 词序列前缀匹配表 ----
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

    // ---- 白名单消费：命中免除围栏 Confirm 与矩阵 NeedsApproval ----
    let policy_hit = ApprovalPolicy {
        enabled: true,
        allowlist_enabled: true,
        command_allowlist: vec!["npm install".to_owned()],
        require_plan: false,
        git_allow_stage_commit: false,
    };
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
            require_plan: false,
            git_allow_stage_commit: false,
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
    assert!(
        matches!(
            gate_disabled.judge("command", &command_args("npm install")),
            GateVerdict::Suspend { .. }
        ),
        "白名单开关关闭不豁免"
    );
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

    // ---- 审批总开关（票面验收④①）：关闭后 confirm_each 档不再弹审批 ----
    let policy_off = ApprovalPolicy {
        enabled: false,
        allowlist_enabled: false,
        command_allowlist: Vec::new(),
        require_plan: false,
        git_allow_stage_commit: false,
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
        require_plan: false,
        git_allow_stage_commit: false,
    });
    let args = serde_json::json!({
        "command": format!("echo x > {}", data.join("config.json").display())
    })
    .to_string();
    assert_internal_blocked(&gate_zone.judge("command", &args), "禁区命中@总开关关");
}

// ---- 外部目录授权判定（24 号票，§9.9）----

/// 断言外部目录授权判定全流程：文件类工具访问 roots 外路径 → Suspend 且
/// external_dir = 目标父目录（read/write/edit）、目录型工具（list_files/
/// glob/grep）授权目标 = 路径本身、非文件工具不判；授权后 grant_external_dir
/// → 重放放行（「目录及子目录」粒度），其他目录仍逐个询问；禁授权目标
///（§9.9 整体放行禁区）——托管目录本身与家目录 → E_INVALID_ARG 硬拦；
/// batch_read 多路径——首个越界路径的父目录作为授权目标，授权后其余越界
/// 目录在重放时继续逐个询问（签名放行不吞并未授权路径——外部判定在签名前）。
#[test]
fn external_dir_grant_flow_cases() {
    let dir = tempfile::tempdir().expect("临时目录");
    let data_root = dir.path().join("data");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(data_root.join("workspace")).expect("建 workspace");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    for sub in ["docs", "other", "d1", "d2"] {
        std::fs::create_dir_all(dir.path().join("outside").join(sub)).expect("建外部目录");
    }
    let data = std::fs::canonicalize(&data_root).unwrap();
    let project = std::fs::canonicalize(&project_root).unwrap();
    let outside = std::fs::canonicalize(dir.path().join("outside")).unwrap();
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
    assert_eq!(
        std::path::Path::new(&external_dir.expect("带授权目标")),
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

    // 禁授权目标：数据目录本身（托管目录）——workspace 是核心根内不触发
    // 外部判定；托管目录下非 workspace 的路径 = 禁区（读路径不触及内部
    // 状态硬拒（23 号票只拦写），但授权目标 = 托管目录 → E_INVALID_ARG）
    let verdict = gate.judge(
        "read",
        &serde_json::json!({"path": data.join("config.json").to_string_lossy()}).to_string(),
    );
    let GateVerdict::Blocked { code, .. } = verdict else {
        panic!("托管目录路径应硬拦（授权禁区）");
    };
    assert_eq!(code, "E_INVALID_ARG", "托管目录 = 授权禁区");
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

    // batch_read 多路径：首个越界路径的父目录作为授权目标
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

// ---- 计划范围门与硬门（25 号票，FR-06-02/03/§8.6）----

use crate::backend::gate::{PlanScopeMemory, SessionPlanScope};
use crate::storage::entries::PlanItem as PlanItemEntry;

/// 造一个共享计划状态并批准（基线 = paths 参数；条目一个 in_progress，
/// 不过期——E_PLAN_STALE 不触发的共同前提）。
fn approved_plan(baseline: &[&str]) -> Arc<Mutex<Option<PlanState>>> {
    let mut state = PlanState {
        items: vec![PlanItemEntry {
            id: "p1".to_owned(),
            text: "任务".to_owned(),
            status: PlanItemStatus::InProgress,
        }],
        paths: baseline.iter().map(|p| p.to_string()).collect(),
        ..PlanState::default()
    };
    state.approve(1000);
    Arc::new(Mutex::new(Some(state)))
}

/// 范围门装配：full_access + workspace（临时项目目录 canonical）+ 已批准
/// 计划（基线由用例给）。返回门与项目根绝对路径。
fn scoped_gate(baseline: &[&str]) -> (ApprovalGate, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("临时目录");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let project = std::fs::canonicalize(&project_root).expect("canonicalize");
    let gate = ApprovalGate::new(PermissionTier::FullAccess, approved_plan(baseline))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone()],
            base: project.clone(),
        });
    (gate, project, dir)
}

/// 断言计划范围门判定表（基线匹配语义 + command 写目标同判，22/25 号票）：
/// 文件基线——范围外写挂范围卡（plan_scope 带计划外路径、与外部目录授权
/// 互斥、纯范围卡无围栏原因）、基线内写不触发、文件范围不自动扩成顶层
/// 目录（子路径仍询问）；目录基线——含其子路径（任意深度免问）、基线外
/// 兄弟路径仍询问；空基线——首次写入也须确认（§8.6 空范围不豁免）；无
/// 已批准计划不守门（FR-06-02 默认"有计划即守门"）；command 的写目标走
/// AST WriteTarget 同判——基线外重定向纯范围卡、词法越界的基线外重定向
/// （围栏 Confirm 待决）被范围卡一并吸收围栏原因（一次交互覆盖本次调用）、
/// 基线内重定向落矩阵放行、Opaque 不判范围（值未知不猜）。
#[test]
fn plan_scope_baseline_judgement_cases() {
    // ---- 文件基线：范围外挂范围卡、范围内放行、子路径不因文件基线免问 ----
    let (gate, project, _keep) = scoped_gate(&["src/a.rs"]);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/b.rs", "content": "x"}).to_string(),
    );
    let GateVerdict::Suspend {
        fence_reason,
        external_dir,
        plan_scope,
    } = &verdict
    else {
        panic!("范围外写应挂范围卡：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("src/b.rs"), "载荷带计划外路径");
    assert!(external_dir.is_none(), "与外部目录授权互斥");
    assert!(fence_reason.is_none(), "纯范围卡无围栏原因");
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/a.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "基线内写放行（full_access）：{verdict:?}"
    );
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/sub/c.rs", "content": "x"}).to_string(),
    );
    let GateVerdict::Suspend { plan_scope, .. } = &verdict else {
        panic!("文件基线不含子路径授权：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("src/sub/c.rs"));
    let _ = project;

    // ---- 目录基线：子路径免问、基线外兄弟路径仍询问 ----
    let (gate, _project, _keep) = scoped_gate(&["src"]);
    for path in ["src/main.rs", "src/deep/nested/x.rs"] {
        let verdict = gate.judge(
            "write",
            &serde_json::json!({"path": path, "content": "x"}).to_string(),
        );
        assert!(
            matches!(verdict, GateVerdict::Allow),
            "目录基线内写 {path} 应放行：{verdict:?}"
        );
    }
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "docs/readme.md", "content": "x"}).to_string(),
    );
    let GateVerdict::Suspend { plan_scope, .. } = &verdict else {
        panic!("目录基线外写应询问：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("docs/readme.md"));

    // ---- 空基线：首写也须确认 ----
    let (gate, _project, _keep) = scoped_gate(&[]);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "notes.md", "content": "x"}).to_string(),
    );
    let GateVerdict::Suspend { plan_scope, .. } = &verdict else {
        panic!("空基线首写应询问：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("notes.md"));

    // ---- 无已批准计划：不守门 ----
    let dir = tempfile::tempdir().expect("临时目录");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let project = std::fs::canonicalize(&project_root).expect("canonicalize");
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone()],
            base: project.clone(),
        });
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "anywhere.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "无批准计划不触发范围门：{verdict:?}"
    );

    // ---- command 写目标走 AST WriteTarget 同判（22 号票接缝）----
    let (gate, project, _keep) = scoped_gate(&["src/a.rs"]);
    // 基线外重定向（roots 内）：范围门命中、围栏四象限 Allow → 纯范围卡
    let verdict = gate.judge("command", &command_args("echo x > src/b.rs"));
    let GateVerdict::Suspend {
        fence_reason,
        plan_scope,
        ..
    } = &verdict
    else {
        panic!("范围外命令写应挂范围卡：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("src/b.rs"));
    assert!(
        fence_reason.is_none(),
        "roots 内写围栏 Allow，范围卡无围栏原因：{fence_reason:?}"
    );
    // 词法越界的基线外重定向：围栏 Confirm 待决被范围卡吸收——plan_scope
    // 与 fence_reason 同时带出（一次交互覆盖本次调用的全部审批）
    let verdict = gate.judge("command", &command_args("echo x > ../outside/x.rs"));
    let GateVerdict::Suspend {
        fence_reason,
        plan_scope,
        ..
    } = &verdict
    else {
        panic!("越界基线外命令写应挂范围卡：{verdict:?}")
    };
    assert_eq!(
        plan_scope.as_deref(),
        Some("../outside/x.rs"),
        "范围门取 AST 字面量目标"
    );
    assert!(
        fence_reason
            .as_ref()
            .is_some_and(|r| r.contains("越出工作区")),
        "范围卡吸收围栏 Confirm 原因：{fence_reason:?}"
    );
    let _ = project;
    // 基线内重定向（roots 内）：范围门不命中、围栏四象限 Allow →
    // full_access 落矩阵放行（范围门不产生额外审批）
    let verdict = gate.judge("command", &command_args("echo x > src/a.rs"));
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "基线内命令写放行：{verdict:?}"
    );
    // Opaque 写目标（$VAR）不做范围判定（值未知不猜）——落围栏掩码挂起
    let verdict = gate.judge("command", &command_args("echo x > $OUT/target.rs"));
    let GateVerdict::Suspend { plan_scope, .. } = &verdict else {
        panic!("Opaque 应保守挂起：{verdict:?}")
    };
    assert!(plan_scope.is_none(), "Opaque 不判范围");
}

/// 断言计划范围授权路径（恢复循环入口，always_session）：grant_plan_scope
/// 把目标父目录以目录范围并入后——目录内其他文件免问（本 run 热追加面）、
/// 未装配会话桶时热追加仍生效（headless 面）；会话级授权桶跨 run——新
/// ApprovalGate 挂同一 SessionPlanScope 授权生效、其他会话的新桶不受影响、
/// forget 即清（表内映射移除，判定面从表内重新取桶即空）。
#[test]
fn plan_scope_grant_and_session_bucket_cases() {
    // ---- 本 run 热追加：授权目录内免问、目录外仍询问 ----
    let (gate, project, _keep) = scoped_gate(&["src/a.rs"]);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/b.rs", "content": "x"}).to_string(),
    );
    assert!(matches!(
        &verdict,
        GateVerdict::Suspend {
            plan_scope: Some(_),
            ..
        }
    ));
    // 用户「本会话始终允许」→ 宿主把目标父目录（src/）入基线扩展
    //（恢复循环同款：目标词法路径按项目主目录展开后归一，取父目录）
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("建 src");
    gate.grant_plan_scope(std::fs::canonicalize(&src).expect("canonicalize"));
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/new.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "授权目录内免问：{verdict:?}"
    );
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "docs/x.md", "content": "x"}).to_string(),
    );
    assert!(
        matches!(
            &verdict,
            GateVerdict::Suspend {
                plan_scope: Some(_),
                ..
            }
        ),
        "授权目录外仍询问：{verdict:?}"
    );

    // ---- 会话级授权桶跨 run + forget ----
    let memory = PlanScopeMemory::new();
    let bucket: SessionPlanScope = memory.bucket("sess-1");
    let (gate, project, _keep) = scoped_gate(&["src/a.rs"]);
    let gate = gate.with_plan_scope_memory(bucket.clone());
    // run 1：授权 src/
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("建 src");
    gate.grant_plan_scope(std::fs::canonicalize(&src).expect("canonicalize"));
    // bucket 句柄同源：授权即时可见
    assert_eq!(bucket.paths().len(), 1, "会话桶登记一条");
    // run 2：新 gate 挂同桶 → 目录内写免问（跨 run 生效）
    let gate2 = ApprovalGate::new(PermissionTier::FullAccess, approved_plan(&["src/a.rs"]))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone()],
            base: project.clone(),
        })
        .with_plan_scope_memory(memory.bucket("sess-1"));
    let verdict = gate2.judge(
        "write",
        &serde_json::json!({"path": "src/another.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "跨 run 同桶生效：{verdict:?}"
    );
    // 其他会话的新桶为空：不延续
    let gate3 = ApprovalGate::new(PermissionTier::FullAccess, approved_plan(&["src/a.rs"]))
        .with_workspace(crate::backend::gate::GateWorkspace {
            roots: vec![project.clone()],
            base: project.clone(),
        })
        .with_plan_scope_memory(memory.bucket("sess-2"));
    let verdict = gate3.judge(
        "write",
        &serde_json::json!({"path": "src/another.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(
            &verdict,
            GateVerdict::Suspend {
                plan_scope: Some(_),
                ..
            }
        ),
        "其他会话不延续授权：{verdict:?}"
    );
    // forget 即清（表内映射移除——已取出的旧句柄不清空，与 ApprovalMemory
    // 同语义：判定面从表内重新取桶即空）
    memory.forget("sess-1");
    assert!(
        memory.bucket("sess-1").paths().is_empty(),
        "会话删除随清（表内新取为空）"
    );
}

/// 严格硬门（FR-06-02）：默认关（require_plan=false 无计划写放行）；开启
/// 后无批准计划的写 E_PLAN_REQUIRED 硬拦；已批准计划不拦；只读工具不受
/// 硬门影响。
#[test]
fn plan_hard_gate_default_off_and_on() {
    let dir = tempfile::tempdir().expect("临时目录");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let project = std::fs::canonicalize(&project_root).expect("canonicalize");
    let make = |plan: Arc<Mutex<Option<PlanState>>>, require: bool| {
        ApprovalGate::new(PermissionTier::FullAccess, plan)
            .with_workspace(crate::backend::gate::GateWorkspace {
                roots: vec![project.clone()],
                base: project.clone(),
            })
            .with_policy(ApprovalPolicy {
                require_plan: require,
                ..ApprovalPolicy::default()
            })
    };
    // 默认关：无计划写放行（FR-06-02 默认策略锚定）
    let gate = make(Arc::new(Mutex::new(None)), false);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "a.rs", "content": "x"}).to_string(),
    );
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "默认关不拦写：{verdict:?}"
    );
    // 开启 + 无已批准计划 → E_PLAN_REQUIRED 硬拦
    let gate = make(Arc::new(Mutex::new(None)), true);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "a.rs", "content": "x"}).to_string(),
    );
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("硬门开启无计划应硬拦：{verdict:?}")
    };
    assert_eq!(*code, "E_PLAN_REQUIRED");
    // 开启 + 已批准计划（有过期门接管）→ 不落硬门
    let gate = make(approved_plan(&[]), true);
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "a.rs", "content": "x"}).to_string(),
    );
    assert!(
        !matches!(&verdict, GateVerdict::Blocked { code, .. } if *code == "E_PLAN_REQUIRED"),
        "已批准计划不触发硬门：{verdict:?}"
    );
    // 只读工具不受硬门影响
    let gate = make(Arc::new(Mutex::new(None)), true);
    let verdict = gate.judge("read", r#"{"path":"a.txt"}"#);
    assert!(
        matches!(verdict, GateVerdict::Allow),
        "只读不受硬门：{verdict:?}"
    );
}

// ---- git 写策略矩阵（26 号票，FR-11-03 / §8.7 git 分类表细化）----
//
// 票面验收口径逐项钉住：默认关 commit Block / 开后 stage+commit Confirm /
// push 任何配置恒 Confirm / push --force 恒 Block / merge-rebase 恒禁 /
// 只读子命令任何配置不触发策略 / 复合命令逐段取最严 / 策略 Block 不被
// 会话级签名放行复活。

/// 造带 git 策略开关的审批门（full_access 档：矩阵面不拦，隔离出策略面）。
fn git_gate(git_allow: bool, enabled: bool) -> ApprovalGate {
    ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None))).with_policy(
        crate::backend::gate::ApprovalPolicy {
            enabled,
            allowlist_enabled: false,
            command_allowlist: Vec::new(),
            require_plan: false,
            git_allow_stage_commit: git_allow,
        },
    )
}

/// 断言判定为 E_FENCE_BLOCKED 硬拦。
fn assert_blocked(verdict: &GateVerdict, why: &str) {
    let GateVerdict::Blocked { code, .. } = verdict else {
        panic!("{why}：应硬拦，实际 {verdict:?}")
    };
    assert_eq!(*code, "E_FENCE_BLOCKED", "{why}：错误码应为围栏 Block");
}

/// 断言 git 写策略分类表（26 号票，FR-11-03/§8.7）：
/// 默认关——add/stage/commit 全部硬拦（缺省构造同样默认禁，安全缺省口径）；
/// 开关开后——add/stage/commit 降为挂起审批（fenceReason 带原因），且受
/// 审批总开关豁免（总开关关 → 放行，用户自担）；push（非 force）任何配置
/// 恒挂起确认（总开关关也不豁免），已批准签名放行（否则恢复死循环）；
/// merge/rebase 恒禁（spec 定案「仍禁」，总开关关闭也不豁免，策略铁律位阶
/// 同黑名单）；push --force 恒 Block（黑名单正则先行 + 策略层双保险，开关
/// 状态无关）；只读子命令任何配置不触发策略（走既有围栏/矩阵面）；复合
/// 命令逐段取最严（默认关 add+commit 硬拦、开后挂起一次审批覆盖、只读段
/// +禁段混合仍拦）；策略 Block 先于会话级签名放行（开关切换后残留的
/// 「始终允许（本会话）」签名不放行禁面，Block 类判定与黑名单同位阶）。
#[test]
fn git_policy_classification_cases() {
    // ---- 默认关：add/stage/commit 硬拦（缺省构造同口径）----
    for cmd in ["git add .", "git stage -A", "git commit -m msg"] {
        let gate = git_gate(false, true);
        assert_blocked(&gate.judge("command", &command_args(cmd)), cmd);
    }
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    assert_blocked(
        &gate.judge("command", &command_args("git commit")),
        "缺省构造",
    );

    // ---- 开关开后：add/commit 降为挂起审批（带原因）；总开关关 → 放行 ----
    for cmd in ["git add .", "git commit -m msg"] {
        let gate = git_gate(true, true);
        let GateVerdict::Suspend { fence_reason, .. } = gate.judge("command", &command_args(cmd))
        else {
            panic!("{cmd}：开关开后应挂起审批");
        };
        assert!(fence_reason.is_some(), "{cmd}：挂起应带原因");
    }
    let gate = git_gate(true, false);
    assert!(
        matches!(
            gate.judge("command", &command_args("git commit")),
            GateVerdict::Allow
        ),
        "开关开 + 总开关关（用户自担）→ 放行"
    );

    // ---- push 恒确认（开关关/开/总开关关都确认）+ 已批准签名放行 ----
    for allow in [false, true] {
        let gate = git_gate(allow, true);
        let GateVerdict::Suspend { fence_reason, .. } =
            gate.judge("command", &command_args("git push origin main"))
        else {
            panic!("push 应恒确认（开关 {allow}）");
        };
        assert!(fence_reason.is_some(), "push 挂起带原因");
    }
    let gate = git_gate(true, false);
    assert!(
        matches!(
            gate.judge("command", &command_args("git push")),
            GateVerdict::Suspend { .. }
        ),
        "总开关关不豁免 push 确认"
    );
    // 已批准签名放行（挂起恢复重放语义）：批准过的 push 同签名重放不再挂起
    let gate = git_gate(true, true);
    let args = command_args("git push");
    gate.approve("command", &args);
    assert!(
        matches!(gate.judge("command", &args), GateVerdict::Allow),
        "批准过的 push 重放放行（否则恢复死循环）"
    );

    // ---- merge/rebase 恒禁 + push --force 恒 Block（开关状态无关）----
    for allow in [false, true] {
        for cmd in ["git merge main", "git rebase main"] {
            let gate = git_gate(allow, true);
            assert_blocked(&gate.judge("command", &command_args(cmd)), cmd);
        }
        for cmd in [
            "git push --force origin main",
            "git push -f",
            "git push --force-with-lease",
        ] {
            let gate = git_gate(allow, true);
            assert_blocked(&gate.judge("command", &command_args(cmd)), cmd);
        }
    }
    // 总开关关闭也不豁免恒禁面（策略铁律位阶同黑名单）
    let gate = git_gate(true, false);
    assert_blocked(
        &gate.judge("command", &command_args("git merge")),
        "总开关关不豁免",
    );

    // ---- 只读子命令任何配置不触发策略 ----
    for allow in [false, true] {
        for cmd in [
            "git status",
            "git diff",
            "git log --oneline",
            "git show HEAD",
        ] {
            let gate = git_gate(allow, true);
            assert!(
                matches!(
                    gate.judge("command", &command_args(cmd)),
                    GateVerdict::Allow
                ),
                "{cmd}：只读应放行（开关 {allow}）"
            );
        }
    }
    let gate = git_gate(false, true);
    assert!(
        matches!(
            gate.judge("command", &command_args("echo hello")),
            GateVerdict::Allow
        ),
        "非 git 命令不触发策略"
    );

    // ---- 复合命令逐段取最严 ----
    let gate = git_gate(false, true);
    assert_blocked(
        &gate.judge("command", &command_args("git add . && git commit -m x")),
        "复合默认关",
    );
    let gate = git_gate(true, true);
    assert!(
        matches!(
            gate.judge("command", &command_args("git add . && git commit -m x")),
            GateVerdict::Suspend { .. }
        ),
        "复合开关开后挂起"
    );
    // 只读段 + 禁段混合 → 仍拦（最严优先）
    let gate = git_gate(false, true);
    assert_blocked(
        &gate.judge("command", &command_args("git status && git push --force")),
        "只读+禁段混合",
    );

    // ---- 策略 Block 不被会话级签名放行复活 ----
    // 策略 Block 先于会话级签名放行：开关切换后残留的「始终允许（本会话）」
    // 签名不放行禁面（Block 类判定在 (0.6) 先于 (1) 签名放行——与黑名单
    // 同位阶，会话桶命中不豁免）。
    let memory = crate::backend::gate::ApprovalMemory::new();
    let bucket = memory.bucket("s1");
    memory.remember("s1", "command", &command_args("git commit -m x"));
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_session_memory(bucket)
        .with_policy(crate::backend::gate::ApprovalPolicy {
            enabled: true,
            allowlist_enabled: false,
            command_allowlist: Vec::new(),
            require_plan: false,
            git_allow_stage_commit: false,
        });
    assert_blocked(
        &gate.judge("command", &command_args("git commit -m x")),
        "会话签名不复活策略禁面",
    );
    // 对照：策略开关开时同签名正常放行（行为随配置切换）
    let bucket = memory.bucket("s2");
    memory.remember("s2", "command", &command_args("git commit -m x"));
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)))
        .with_session_memory(bucket)
        .with_policy(crate::backend::gate::ApprovalPolicy {
            enabled: true,
            allowlist_enabled: false,
            command_allowlist: Vec::new(),
            require_plan: false,
            git_allow_stage_commit: true,
        });
    assert!(
        matches!(
            gate.judge("command", &command_args("git commit -m x")),
            GateVerdict::Allow
        ),
        "开关开时会话签名放行"
    );
}

// ---- 灰区命令审计（36 号票，§8.7）：资格判定 + 审计放行单槽 ----

/// 断言灰区审计各面（36 号票，§8.7）：
/// 资格判定表——围栏 Confirm 灰区命中（词法越界写目标、安装类信号、Opaque
/// 字面量掩码）；git 策略确认/plan 档/矩阵人批/非 command 工具不命中；硬拒
/// 不进挂起面天然不判。灰区命令样例用重定向写目标（touch 不在 AST 写目标
/// 清单——词法越界语义用 echo > 承载）。
/// 审计放行单槽——未放行围栏灰区挂起、置位重放命中放行、消费即清（同
/// 签名新调用不继承旧结论）、单槽覆盖（新签名置位后旧签名不再命中）、
/// 签名不同不命中（结论绑定实际输入）。
/// 审计放行不豁免硬拒面——git 策略 Block 与黑名单灾难级先于审计放行检查点。
#[test]
fn audit_gate_cases() {
    // ---- 资格判定表 ----
    // full_access 档：写目标越界（词法不在 roots）→ 围栏 Confirm = 灰区。
    // 未装配 workspace 时退化语义「有写目标即 Confirm」，同属灰区。
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    assert!(
        gate.audit_class("command", &command_args("echo hi > /tmp/outside-audit.txt")),
        "围栏 Confirm（词法越界写目标）= 灰区"
    );
    assert!(
        gate.audit_class("command", &command_args("npm install left-pad")),
        "安装类信号（词法信号保留）= 灰区"
    );
    assert!(
        gate.audit_class("command", &command_args("echo hi > \"$OUT/file\"")),
        "Opaque 写目标（字面量掩码）= 灰区"
    );
    assert!(
        !gate.audit_class("command", &command_args("echo hello")),
        "无写信号命令（围栏 Allow）→ 非灰区（full_access 直接执行）"
    );
    assert!(
        !gate.audit_class(
            "write",
            r#"{"path":"/tmp/x","content":"y"}"#.to_owned().as_str(),
        ),
        "非 command 工具不在审计范围（M2 仅覆盖 command）"
    );
    // git 策略确认类（明确人批）不进审计。
    let gate_git = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    assert!(
        !gate_git.audit_class("command", &command_args("git push origin main")),
        "git push 恒确认 = 明确人批，不审计"
    );
    assert!(
        !gate_git.audit_class("command", &command_args("git commit -m x")),
        "git add/commit 策略确认 = 明确人批，不审计"
    );
    // plan 档：围栏 Confirm 属档位语义（白名单外只读命令），审计不豁免。
    let gate_plan = ApprovalGate::new(PermissionTier::Plan, Arc::new(Mutex::new(None)));
    assert!(
        !gate_plan.audit_class("command", &command_args("node --eval '1'")),
        "plan 档围栏 Confirm = 档位语义，不审计"
    );
    // 矩阵 NeedsApproval（confirm_each 档无写信号命令）= 权限档人批，不审计。
    let gate_confirm = ApprovalGate::new(PermissionTier::ConfirmEach, Arc::new(Mutex::new(None)));
    assert!(
        !gate_confirm.audit_class("command", &command_args("npm test")),
        "矩阵人批挂起（无围栏原因）= 权限档，不审计"
    );

    // ---- 审计放行单槽：判定序位置与消费语义 ----
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    let gray = command_args("echo hi > /tmp/outside-audit.txt");
    // 未放行：围栏灰区挂起。
    assert!(matches!(
        gate.judge("command", &gray),
        GateVerdict::Suspend {
            fence_reason: Some(_),
            ..
        }
    ));
    // 审计放行（单槽置位）→ 重放判定命中放行。
    gate.grant_audit("command", &gray);
    assert!(
        matches!(gate.judge("command", &gray), GateVerdict::Allow),
        "审计放行槽命中应放行"
    );
    // 消费即清：同签名新调用不继承旧结论，重新走灰区挂起。
    assert!(
        matches!(
            gate.judge("command", &gray),
            GateVerdict::Suspend {
                fence_reason: Some(_),
                ..
            }
        ),
        "槽已消费，同签名新调用应重新挂起（不继承旧结论）"
    );
    // 单槽覆盖：新签名置位后旧签名不再命中。
    let gray2 = command_args("echo hi > /tmp/outside-audit-2.txt");
    gate.grant_audit("command", &gray);
    gate.grant_audit("command", &gray2);
    assert!(
        !matches!(gate.judge("command", &gray), GateVerdict::Allow),
        "单槽被覆盖后旧签名不命中"
    );
    assert!(
        matches!(gate.judge("command", &gray2), GateVerdict::Allow),
        "新签名命中"
    );
    // 签名不同（参数变化）不命中：结论绑定实际输入。
    gate.grant_audit("command", &gray);
    assert!(
        !matches!(
            gate.judge(
                "command",
                &command_args("echo hi > /tmp/outside-audit-3.txt")
            ),
            GateVerdict::Allow
        ),
        "参数不同不命中（签名全等比较）"
    );

    // ---- 审计放行不豁免硬拒面（红线）----
    let gate = ApprovalGate::new(PermissionTier::FullAccess, Arc::new(Mutex::new(None)));
    // git merge 恒禁（策略 Block）。
    let banned = command_args("git merge main");
    gate.grant_audit("command", &banned);
    assert!(
        matches!(
            gate.judge("command", &banned),
            GateVerdict::Blocked {
                code: "E_FENCE_BLOCKED",
                ..
            }
        ),
        "策略硬拒先于审计放行检查点，不可豁免"
    );
    // 黑名单灾难级（rm -rf）同样先于检查点。
    let disaster = command_args("rm -rf /tmp/x");
    gate.grant_audit("command", &disaster);
    assert!(
        matches!(
            gate.judge("command", &disaster),
            GateVerdict::Blocked {
                code: "E_FENCE_BLOCKED",
                ..
            }
        ),
        "黑名单 Block 先于审计放行检查点"
    );
}
