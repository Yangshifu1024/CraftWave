// @vitest-environment happy-dom
/**
 * SkillsPage 交互测试（技能组）：清单渲染（名称/来源徽标/
 * 启停开关/描述）、启停切换（乐观翻转 + 失败回滚）、重新扫描、行点击正文
 * 预览、信任撤销。IPC 全部经 vi.mock 替换（McpPage.test 同款桩面）。
 * 精简版：每功能面留代表 1 例，相关段合并。
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

describe("SkillsPage", () => {
  it("清单渲染（三来源徽标/启停开关态）；空清单显示安装引导", async () => {
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
    const view = render(<SkillsPage />);
    await waitFor(() => expect(screen.getAllByTestId("skill-row")).toHaveLength(3));
    expect(screen.getByTestId("skill-source-hello").textContent).toBe("内置");
    expect(screen.getByTestId("skill-source-pdf").textContent).toBe("全局");
    expect(screen.getByTestId("skill-source-proj").textContent).toBe("项目");
    // Switch = button[role=switch]，开关态经 aria-checked 表达
    expect(screen.getByTestId("skill-toggle-hello").getAttribute("aria-checked")).toBe("true");
    expect(screen.getByTestId("skill-toggle-pdf").getAttribute("aria-checked")).toBe("false");
    expect(screen.getByTestId("skill-list").textContent).toContain("全局 PDF 技能");
    view.unmount();

    // 空清单
    seedSkills([]);
    const empty = render(<SkillsPage />);
    await waitFor(() => expect(screen.queryByTestId("skill-list")).toBeNull());
    expect(screen.getByText(/还没有安装任何技能/)).toBeDefined();
    empty.unmount();
  });

  it("启停：开关切换调 skill_toggle；失败回滚开关态（后端为权威）", async () => {
    const entry: SkillListEntry = {
      name: "demo",
      source: "global",
      enabled: true,
      description: "演示",
      trustStatus: "pending",
    };
    // 成功路径
    seedSkills([entry]);
    const view = render(<SkillsPage />);
    const sw = await waitFor(() => screen.getByTestId("skill-toggle-demo"));
    fireEvent.click(sw);
    await waitFor(() => expect(toggleMock).toHaveBeenCalledWith("demo", false));
    view.unmount();

    // 失败路径：回滚为 true（初始态）
    toggleMock.mockRejectedValue(new Error("E_IO 落盘失败"));
    seedSkills([entry]);
    const failed = render(<SkillsPage />);
    const sw2 = await waitFor(() => screen.getByTestId("skill-toggle-demo"));
    fireEvent.click(sw2);
    await waitFor(() => expect(toggleMock).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByTestId("skill-toggle-demo").getAttribute("aria-checked")).toBe("true"),
    );
    failed.unmount();
  });

  it("重扫重拉清单；行点击预览正文；信任撤销（trusted 有入口，其余无）", async () => {
    // 重扫后清单多出 added（reload 感知新增）
    seedSkills([
      {
        name: "first",
        source: "global",
        enabled: true,
        description: "最初",
        trustStatus: "pending",
      },
    ]);
    const view = render(<SkillsPage />);
    await waitFor(() => expect(screen.getByTestId("skill-name-first")).toBeDefined());
    listMock.mockResolvedValue([
      { name: "first", source: "global", enabled: true, description: "最初" },
      { name: "added", source: "project", enabled: true, description: "后来新增" },
    ]);
    fireEvent.click(screen.getByTestId("skill-reload"));
    await waitFor(() => expect(reloadMock).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId("skill-name-added")).toBeDefined());
    view.unmount();

    // 行点击名称调 skill_inspect 弹正文预览（Markdown 只读）
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
    const preview = render(<SkillsPage />);
    fireEvent.click(await waitFor(() => screen.getByTestId("skill-name-demo")));
    const body = await waitFor(() => screen.getByTestId("skill-preview"));
    expect(body.textContent).toContain("技能正文");
    expect(screen.getByText(/技能 demo · SKILL\.md/)).toBeDefined();
    preview.unmount();

    // 已信任（trusted）技能行显示撤销按钮；点击 → skill_revoke_trust + 刷新清单
    listMock.mockClear();
    seedSkills([
      {
        name: "guard",
        source: "project",
        enabled: true,
        description: "已记住技能",
        trustStatus: "trusted",
      },
    ]);
    const trust = render(<SkillsPage />);
    const btn = await waitFor(() => screen.getByTestId("skill-revoke-trust-guard"));
    fireEvent.click(btn);
    await waitFor(() => expect(revokeMock).toHaveBeenCalledWith({ name: "guard" }));
    // 撤销后刷新清单（listMock 第二次调用 = revoke 成功后的 refresh）
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));
    trust.unmount();

    // 待决策/已撤销/内置技能不显示撤销按钮（内置豁免信任，撤销无入口）
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
    const noEntry = render(<SkillsPage />);
    await waitFor(() => expect(screen.getAllByTestId("skill-row")).toHaveLength(3));
    expect(screen.queryByTestId("skill-revoke-trust-pending-skill")).toBeNull();
    expect(screen.queryByTestId("skill-revoke-trust-revoked-skill")).toBeNull();
    expect(screen.queryByTestId("skill-revoke-trust-builtin-skill")).toBeNull();
    noEntry.unmount();
  });
});
