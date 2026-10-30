// @vitest-environment happy-dom
/**
 * SkillsPage 交互测试（09 号票，§10.7 技能组）：清单渲染（名称/来源徽标/
 * 启停开关/描述）、启停切换（乐观翻转 + 失败回滚）、重新扫描（reload 后
 * 重拉清单）、行点击正文预览（skill_inspect 弹窗）；24 号票增信任撤销
 * （已记住技能的「撤销信任」按钮 → skill_revoke_trust）。IPC 全部经
 * vi.mock 替换（McpPage.test 同款桩面）。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SkillListEntry } from "@/ipc/types";
import { SkillsPage } from "./SkillsPage";

// ---- IPC 测试桩（vi.hoisted：vi.mock 工厂随 import 提前执行，桩须同升）----

const mocks = vi.hoisted(() => ({
  listMock: vi.fn<() => Promise<unknown>>(),
  toggleMock: vi.fn<(name: string, enabled: boolean) => Promise<void>>(),
  reloadMock: vi.fn<() => Promise<void>>(),
  inspectMock: vi.fn<(name: string) => Promise<{ markdown: string }>>(),
  revokeMock: vi.fn<(args: { name: string }) => Promise<void>>(),
}));

const { listMock, toggleMock, reloadMock, inspectMock, revokeMock } = mocks;

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    skillList: () => mocks.listMock(),
    skillToggle: (name: string, enabled: boolean) => mocks.toggleMock(name, enabled),
    skillReload: () => mocks.reloadMock(),
    skillInspect: (args: { name: string }) => mocks.inspectMock(args.name),
    skillRevokeTrust: (args: { name: string }) => mocks.revokeMock(args),
  };
});

beforeEach(() => {
  listMock.mockReset().mockResolvedValue([]);
  toggleMock.mockReset().mockResolvedValue(undefined);
  reloadMock.mockReset().mockResolvedValue(undefined);
  inspectMock.mockReset().mockResolvedValue({ markdown: "" });
  revokeMock.mockReset().mockResolvedValue(undefined);
});
afterEach(cleanup);

/** 基线清单：三来源各一（同名覆盖后的合并视图形状）。 */
function seedSkills(entries: SkillListEntry[]) {
  listMock.mockResolvedValue(entries);
}

describe("SkillsPage：清单渲染", () => {
  it("渲染来源徽标 / 描述 / 启停开关（三来源合并视图）", async () => {
    seedSkills([
      {
        name: "hello",
        source: "builtin",
        enabled: true,
        description: "内置示例技能",
        trustStatus: "pending",
      },
      {
        name: "pdf",
        source: "global",
        enabled: false,
        description: "全局 PDF 技能",
        trustStatus: "pending",
      },
      {
        name: "proj",
        source: "project",
        enabled: true,
        description: "项目独有技能",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    await waitFor(() => expect(screen.getAllByTestId("skill-row")).toHaveLength(3));
    expect(screen.getByTestId("skill-source-hello").textContent).toBe("内置");
    expect(screen.getByTestId("skill-source-pdf").textContent).toBe("全局");
    expect(screen.getByTestId("skill-source-proj").textContent).toBe("项目");
    // Switch = button[role=switch]，开关态经 aria-checked 表达
    expect(screen.getByTestId("skill-toggle-hello").getAttribute("aria-checked")).toBe("true");
    expect(screen.getByTestId("skill-toggle-pdf").getAttribute("aria-checked")).toBe("false");
    expect(screen.getByTestId("skill-list").textContent).toContain("全局 PDF 技能");
  });

  it("空清单显示安装引导文案", async () => {
    seedSkills([]);
    render(<SkillsPage />);
    await waitFor(() => expect(screen.queryByTestId("skill-list")).toBeNull());
    expect(screen.getByText(/还没有安装任何技能/)).toBeDefined();
  });
});

describe("SkillsPage：启停 / 重扫 / 预览", () => {
  it("开关切换调 skill_toggle（FR-14-04）", async () => {
    seedSkills([
      {
        name: "demo",
        source: "global",
        enabled: true,
        description: "演示",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    const sw = await waitFor(() => screen.getByTestId("skill-toggle-demo"));
    fireEvent.click(sw);
    await waitFor(() => expect(toggleMock).toHaveBeenCalledWith("demo", false));
  });

  it("toggle 失败回滚开关态（后端为权威）", async () => {
    toggleMock.mockRejectedValue(new Error("E_IO 落盘失败"));
    seedSkills([
      {
        name: "demo",
        source: "global",
        enabled: true,
        description: "演示",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    const sw = await waitFor(() => screen.getByTestId("skill-toggle-demo"));
    fireEvent.click(sw);
    await waitFor(() => expect(toggleMock).toHaveBeenCalled());
    // 失败后回滚为 true（初始态）
    await waitFor(() =>
      expect(screen.getByTestId("skill-toggle-demo").getAttribute("aria-checked")).toBe("true"),
    );
  });

  it("重新扫描调 skill_reload 并重拉清单（reload 感知新增）", async () => {
    seedSkills([
      {
        name: "first",
        source: "global",
        enabled: true,
        description: "最初",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    await waitFor(() => expect(screen.getByTestId("skill-name-first")).toBeDefined());
    // 重扫后清单多出 added
    listMock.mockResolvedValue([
      { name: "first", source: "global", enabled: true, description: "最初" },
      { name: "added", source: "project", enabled: true, description: "后来新增" },
    ]);
    fireEvent.click(screen.getByTestId("skill-reload"));
    await waitFor(() => expect(reloadMock).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId("skill-name-added")).toBeDefined());
  });

  it("行点击名称调 skill_inspect 弹正文预览（Markdown 只读）", async () => {
    inspectMock.mockResolvedValue({ markdown: "# 技能正文\n\n按需加载的指引。" });
    seedSkills([
      {
        name: "demo",
        source: "project",
        enabled: true,
        description: "演示",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    fireEvent.click(await waitFor(() => screen.getByTestId("skill-name-demo")));
    const preview = await waitFor(() => screen.getByTestId("skill-preview"));
    expect(preview.textContent).toContain("技能正文");
    expect(screen.getByText(/技能 demo · SKILL\.md/)).toBeDefined();
  });

  it("inspect 失败显示错误提示（不留空弹窗）", async () => {
    inspectMock.mockRejectedValue(new Error("E_NOT_FOUND 技能不存在"));
    seedSkills([
      {
        name: "demo",
        source: "project",
        enabled: true,
        description: "演示",
        trustStatus: "pending",
      },
    ]);
    render(<SkillsPage />);
    fireEvent.click(await waitFor(() => screen.getByTestId("skill-name-demo")));
    await waitFor(() =>
      expect(screen.getByText(/技能 demo 预览失败/).textContent).toContain("E_NOT_FOUND"),
    );
  });
});

describe("信任撤销（24 号票，§9.8）", () => {
  it("已信任（trusted）技能行显示撤销按钮；点击 → skill_revoke_trust + 刷新清单", async () => {
    seedSkills([
      {
        name: "guard",
        source: "project",
        enabled: true,
        description: "已记住技能",
        trustStatus: "trusted",
      },
    ]);
    render(<SkillsPage />);
    const btn = await waitFor(() => screen.getByTestId("skill-revoke-trust-guard"));
    fireEvent.click(btn);
    await waitFor(() => expect(revokeMock).toHaveBeenCalledWith({ name: "guard" }));
    // 撤销后刷新清单（listMock 第二次调用 = revoke 成功后的 refresh）
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));
  });

  it("待决策（pending）/已撤销（revoked）/内置技能不显示撤销按钮（内置豁免信任，撤销无入口）", async () => {
    seedSkills([
      {
        name: "pending-skill",
        source: "global",
        enabled: true,
        description: "待决策",
        trustStatus: "pending",
      },
      {
        name: "revoked-skill",
        source: "project",
        enabled: true,
        description: "已撤销",
        trustStatus: "revoked",
      },
      {
        name: "builtin-skill",
        source: "builtin",
        enabled: true,
        description: "内置",
        trustStatus: "trusted",
      },
    ]);
    render(<SkillsPage />);
    await waitFor(() => expect(screen.getAllByTestId("skill-row")).toHaveLength(3));
    expect(screen.queryByTestId("skill-revoke-trust-pending-skill")).toBeNull();
    expect(screen.queryByTestId("skill-revoke-trust-revoked-skill")).toBeNull();
    expect(screen.queryByTestId("skill-revoke-trust-builtin-skill")).toBeNull();
  });

  it("撤销失败 → 显示错误提示（不静默，避免用户误以为已脱险）", async () => {
    seedSkills([
      {
        name: "guard",
        source: "project",
        enabled: true,
        description: "已记住技能",
        trustStatus: "trusted",
      },
    ]);
    revokeMock.mockRejectedValue(new Error("E_INTERNAL 写信任文件失败"));
    render(<SkillsPage />);
    fireEvent.click(await waitFor(() => screen.getByTestId("skill-revoke-trust-guard")));
    await waitFor(() =>
      expect(screen.getByText(/信任撤销失败/).textContent).toContain("E_INTERNAL"),
    );
  });
});
