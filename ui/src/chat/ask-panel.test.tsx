// @vitest-environment happy-dom
/**
 * AskPanel 交互测试（§3.2 两形态，14 号票）：问卷作答 / 审批允许拒绝 →
 * resolveAsk 载荷形状。client.resolveAsk 注入 fake。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AskPayload } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { DEFAULT_CONFIG, useSettingsStore } from "@/stores/settings";
import { AskPanel, approvalCommandPrefix } from "./AskPanel";
import { SessionScopeContext } from "./session-scope";

/** resolveAsk / saveConfig 的 mock：记录面板提交的载荷与白名单写入
 *  （真实 IPC 被 vi.mock 替换；appendToAllowlist 内部走 saveConfig）。 */
const resolveAskMock = vi.fn<(askId: string, answer: unknown) => Promise<void>>();
const saveConfigMock = vi.fn<(config: unknown) => Promise<unknown>>();
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    resolveAsk: (askId: string, answer: unknown) => resolveAskMock(askId, answer),
    saveConfig: (config: unknown) => saveConfigMock(config),
  };
});

beforeEach(() => {
  resolveAskMock.mockReset();
  resolveAskMock.mockResolvedValue(undefined);
  saveConfigMock.mockReset();
  saveConfigMock.mockImplementation(async (config: unknown) => config);
  useAskStore.setState({ bySession: {}, hints: {} });
  // 设置 store 复位：默认带审批四开关（白名单开启），载荷为空表
  useSettingsStore.setState({
    config: {
      ...DEFAULT_CONFIG,
      approval: { ...DEFAULT_CONFIG.approval, commandAllowlist: [] },
    },
    baseline: null,
    loading: false,
    saving: false,
    error: null,
  });
});
afterEach(cleanup);

/** AskPanel 经 useSessionScope 取所属会话桶：统一以 "s1" 作用域渲染。 */
function renderPanel() {
  return render(
    <SessionScopeContext.Provider value="s1">
      <AskPanel />
    </SessionScopeContext.Provider>,
  );
}

/** 问卷载荷样例：一题选项单选 + 一题自由文本。 */
const questionPayload: AskPayload = {
  kind: "question",
  title: "选方案",
  questions: [
    {
      id: "q1",
      text: "用哪个方案？",
      options: [
        { id: "a", label: "方案A" },
        { id: "b", label: "方案B" },
      ],
      allowFreeText: false,
    },
    {
      id: "q2",
      text: "补充说明？",
      options: undefined,
      allowFreeText: true,
    },
  ],
};

/** 审批载荷样例：命令行 + 围栏原因 + 允许/拒绝两选项。 */
const approvalPayload: AskPayload = {
  kind: "approval",
  title: "执行命令",
  command: "echo hi > out.txt",
  fenceReason: "命令含写信号，需确认",
  options: [
    { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" },
    { id: "deny", label: "拒绝", mode: "deny" },
  ],
};

/** 向 ask store 放入一个未决 ask（模拟后端 ask:opened 事件的效果）。 */
function openPanel(payload: AskPayload) {
  useAskStore.getState().open("s1", "ask-1", payload);
}

describe("AskPanel：question 形态", () => {
  it("无未决 ask 时不渲染", () => {
    const { container } = renderPanel();
    expect(container.childElementCount).toBe(0);
    expect(screen.queryByTestId("ask-panel")).toBeNull();
  });

  it("选项点选 + 自由文本 → 提交 answers", () => {
    openPanel(questionPayload);
    renderPanel();
    expect(screen.getByTestId("ask-panel").getAttribute("data-kind")).toBe("question");

    const submit = screen.getByText("提交回答");
    expect((submit as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByText("方案A"));
    fireEvent.change(screen.getByPlaceholderText("自由输入…"), {
      target: { value: "注意并发" },
    });
    expect((screen.getByText("提交回答") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByText("提交回答"));

    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "question",
      answers: { q1: "a", q2: "注意并发" },
    });
  });
});

describe("AskPanel：approval 形态", () => {
  it("命令 + 围栏原因 + diff 预览；允许/拒绝载荷", () => {
    openPanel({
      ...approvalPayload,
      diff: {
        path: "src/a.rs",
        hunks: [
          {
            header: "新文件",
            lines: [
              { kind: "add", text: "fn main() {}" },
              { kind: "del", text: "旧实现" },
            ],
          },
        ],
      },
    });
    renderPanel();
    expect(screen.getByTestId("ask-panel").getAttribute("data-kind")).toBe("approval");
    expect(screen.getByTestId("ask-panel").textContent).toContain("echo hi > out.txt");
    expect(screen.getByTestId("ask-panel").textContent).toContain("写信号");
    expect(screen.getByTestId("ask-diff").textContent).toContain("src/a.rs");
    expect(screen.getByTestId("ask-diff").textContent).toContain("+ fn main() {}");
    expect(screen.getByTestId("ask-diff").textContent).toContain("− 旧实现");

    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "approval",
      optionId: "allow_once",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "approval",
      optionId: "deny",
    });
  });
});

describe("AskPanel：mcpTrust 形态（08 号票）", () => {
  /** MCP 启动信任载荷样例（后端 ask:opened 的 sessionId="" 形态）。 */
  const trustPayload: AskPayload = {
    kind: "mcpTrust",
    server: "filesystem",
    transport: "stdio",
    fingerprint: "a1b2c3d4e5f60718",
    commandOrUrl: "npx -y @modelcontextprotocol/server-filesystem /tmp",
    options: [
      { id: "allow_once", label: "仅本次连接" },
      { id: "remember", label: "记住此版本" },
      { id: "deny", label: "拒绝" },
    ],
  };

  it('应用级桶（sessionId=""）的 mcpTrust 询问在任意会话上下文渲染', () => {
    // 信任询问不属任何会话：进 "" 桶；AskPanel 在 s1 作用域下也要能看到
    useAskStore.getState().open("", "trust-1", trustPayload);
    renderPanel();
    const panel = screen.getByTestId("ask-panel");
    expect(panel.getAttribute("data-kind")).toBe("mcpTrust");
    expect(panel.textContent).toContain("filesystem");
    expect(panel.textContent).toContain("npx -y @modelcontextprotocol/server-filesystem /tmp");
    expect(screen.getByTestId("mcp-trust-fingerprint").textContent).toBe("a1b2c3d4e5f60718");
  });

  it("三选项 → resolveAsk(approval) 携 allow_once/remember/deny", () => {
    useAskStore.getState().open("", "trust-1", trustPayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-remember"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "remember",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "allow_once",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "deny",
    });
  });

  it("会话桶为空且无应用级询问时不渲染", () => {
    const { container } = renderPanel();
    expect(container.childElementCount).toBe(0);
  });

  it("应答失效（E_NOT_FOUND）时按 askId 关闭弹窗，不残留面板", async () => {
    useAskStore.getState().open("", "trust-1", trustPayload);
    resolveAskMock.mockRejectedValueOnce({ code: "E_NOT_FOUND", message: "槽不存在" });
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    await waitFor(() => expect(useAskStore.getState().bySession[""]).toBeUndefined());
    expect(screen.queryByTestId("ask-panel")).toBeNull();
  });
});

describe("23 号票：审批记忆与白名单（ApprovalForm 增量）", () => {
  it("approvalCommandPrefix：词序列前缀提取表（与后端 allowlist_hit 同口径）", () => {
    const cases: [string, string | null][] = [
      ["git status --short", "git status"],
      ["git status", "git status"],
      ["  npm   run build  ", "npm run"],
      ["ls", "ls"],
      ["   ", null],
    ];
    for (const [command, expect_] of cases) {
      expect(approvalCommandPrefix(command), `command=${command}`).toBe(expect_);
    }
  });

  /** 命令类审批载荷样例（③ 开：后端下发 always_session 选项）。 */
  const commandApprovalPayload: AskPayload = {
    kind: "approval",
    title: "执行命令",
    command: "git status --short",
    options: [
      { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" },
      { id: "always_session", label: "始终允许（本会话）", mode: "always_session" },
      { id: "deny", label: "拒绝", mode: "deny" },
    ],
  };

  it("命令类审批（③ 开）渲染「加入白名单」勾选；未勾选直接允许不写白名单", async () => {
    openPanel(commandApprovalPayload);
    renderPanel();
    expect(screen.getByTestId("ask-add-allowlist")).toBeTruthy();
    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    await waitFor(() => expect(resolveAskMock).toHaveBeenCalled());
    expect(saveConfigMock).not.toHaveBeenCalled();
  });

  it("勾选「加入白名单」后点允许 → 白名单持久（前缀条目）+ 应答照常", async () => {
    openPanel(commandApprovalPayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-add-allowlist").querySelector("input")!);
    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    await waitFor(() => expect(saveConfigMock).toHaveBeenCalled());
    const saved = saveConfigMock.mock.calls[0][0] as { approval: { commandAllowlist: string[] } };
    expect(saved.approval.commandAllowlist).toEqual(["git status"]);
    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "approval",
      optionId: "allow_once",
    });
  });

  it("③ 关（载荷无 always_session 选项）不出勾选框", () => {
    openPanel(approvalPayload);
    renderPanel();
    expect(screen.queryByTestId("ask-add-allowlist")).toBeNull();
  });

  it("④ 关（allowlistEnabled=false）命令类审批也不出勾选框", () => {
    useSettingsStore.setState((s) => ({
      config: s.config
        ? { ...s.config, approval: { ...s.config.approval, allowlistEnabled: false } }
        : s.config,
    }));
    openPanel({
      kind: "approval",
      title: "执行命令",
      command: "git status",
      options: [
        { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" },
        { id: "always_session", label: "始终允许（本会话）", mode: "always_session" },
        { id: "deny", label: "拒绝", mode: "deny" },
      ],
    });
    renderPanel();
    expect(screen.queryByTestId("ask-add-allowlist")).toBeNull();
  });

  it("拒绝时勾选不写白名单", async () => {
    openPanel({
      kind: "approval",
      title: "执行命令",
      command: "git status",
      options: [
        { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" },
        { id: "always_session", label: "始终允许（本会话）", mode: "always_session" },
        { id: "deny", label: "拒绝", mode: "deny" },
      ],
    });
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-add-allowlist").querySelector("input")!);
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    await waitFor(() => expect(resolveAskMock).toHaveBeenCalled());
    expect(saveConfigMock).not.toHaveBeenCalled();
  });
});

/** 外部目录授权载荷样例（24 号票，§9.9）：路径 + 粒度说明 + 允许/拒绝。 */
const externalDirPayload: AskPayload = {
  kind: "externalDir",
  path: "/Users/demo/外部资料",
  options: [
    { id: "allow", label: "始终允许（本会话）" },
    { id: "deny", label: "拒绝" },
  ],
};

describe("externalDir 外部目录授权卡（24 号票，§9.9）", () => {
  it("渲染授权路径与粒度说明（data-kind=externalDir）", () => {
    useAskStore.getState().open("s1", "ask-ext", externalDirPayload);
    const { container } = renderPanel();
    const panel = container.querySelector('[data-testid="ask-panel"]');
    expect(panel?.getAttribute("data-kind")).toBe("externalDir");
    expect(screen.getByTestId("external-dir-path").textContent).toBe("/Users/demo/外部资料");
    expect(container.textContent).toContain("该目录及子目录的读/写/列");
  });

  it("点允许 → resolveAsk(approval) 载荷 optionId=allow", async () => {
    useAskStore.getState().open("s1", "ask-ext", externalDirPayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-allow"));
    await waitFor(() =>
      expect(resolveAskMock).toHaveBeenCalledWith("ask-ext", {
        kind: "approval",
        optionId: "allow",
      }),
    );
  });

  it("点拒绝 → resolveAsk(approval) 载荷 optionId=deny", async () => {
    useAskStore.getState().open("s1", "ask-ext2", externalDirPayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    await waitFor(() =>
      expect(resolveAskMock).toHaveBeenCalledWith("ask-ext2", {
        kind: "approval",
        optionId: "deny",
      }),
    );
  });
});

/** 计划范围卡载荷样例（25 号票，§8.6）：planScope + 三选项无推荐项。 */
const planScopePayload: AskPayload = {
  kind: "approval",
  title: "计划范围确认",
  command: undefined,
  fenceReason: "写入路径「src/b.rs」不在已批准计划的范围内（批准范围：src/a.rs）",
  planScope: "src/b.rs",
  options: [
    { id: "allow_once", label: "允许（仅本次）", mode: "allow_once" },
    { id: "always_session", label: "本会话始终允许（写入所在目录）", mode: "always_session" },
    { id: "deny", label: "拒绝", mode: "deny" },
  ],
};

describe("计划范围卡（25 号票，§8.6）", () => {
  it("范围卡展示计划外路径与三选项，点「本会话始终允许」回传 always_session", async () => {
    useAskStore.getState().open("s1", "ask-scope", planScopePayload);
    renderPanel();
    // 计划外路径明示行
    expect(screen.getByTestId("ask-plan-scope").textContent).toContain("src/b.rs");
    expect(screen.getByText("计划范围确认")).toBeTruthy();
    // 三选项渲染
    expect(screen.getByTestId("ask-option-allow_once")).toBeTruthy();
    expect(screen.getByTestId("ask-option-always_session")).toBeTruthy();
    expect(screen.getByTestId("ask-option-deny")).toBeTruthy();
    // 无推荐项不参与 autoConfirm 的 UI 面：载荷断言在契约测试锚点（后端侧
    // recommended 全 None）；这里验证点击 always_session 的应答形状
    fireEvent.click(screen.getByTestId("ask-option-always_session"));
    await waitFor(() => expect(resolveAskMock).toHaveBeenCalled());
    expect(resolveAskMock).toHaveBeenCalledWith("ask-scope", {
      kind: "approval",
      optionId: "always_session",
    });
  });

  it("范围卡点拒绝回传 deny", async () => {
    useAskStore.getState().open("s1", "ask-scope-2", planScopePayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    await waitFor(() => expect(resolveAskMock).toHaveBeenCalled());
    expect(resolveAskMock).toHaveBeenCalledWith("ask-scope-2", {
      kind: "approval",
      optionId: "deny",
    });
  });
});

describe("灰区命令审计注记（36 号票，approval.audit）", () => {
  it("audit.state=ask 显示模型审计建议行（含理由）", () => {
    useAskStore.getState().open("s1", "ask-audit-1", {
      ...approvalPayload,
      audit: { state: "ask", reason: "写入位置在授权目录外" },
    });
    renderPanel();
    const note = screen.getByTestId("ask-audit-note");
    expect(note.textContent).toContain("模型审计建议人工确认");
    expect(note.textContent).toContain("写入位置在授权目录外");
  });

  it("audit.state=manual 显示转人工行（含失败原因）", () => {
    useAskStore.getState().open("s1", "ask-audit-2", {
      ...approvalPayload,
      audit: { state: "manual", reason: "审计超时：15 秒总预算已到" },
    });
    renderPanel();
    const note = screen.getByTestId("ask-audit-note");
    expect(note.textContent).toContain("审计不可用");
    expect(note.textContent).toContain("审计超时");
  });

  it("载荷无 audit 注记时不渲染审计行", () => {
    useAskStore.getState().open("s1", "ask-audit-3", approvalPayload);
    const { container } = renderPanel();
    expect(container.querySelector('[data-testid="ask-audit-note"]')).toBeNull();
  });
});
