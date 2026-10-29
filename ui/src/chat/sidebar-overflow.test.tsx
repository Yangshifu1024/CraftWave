// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { MarqueeTitle, ProjectRow, useScrollFade } from "./SessionWorkspace";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("项目行悬停钮：项目名右侧常驻「新建会话」入口，点击触发回调（回归防护）", () => {
  const onAddSession = vi.fn();
  render(
    <ProjectRow
      project={{ id: "p1", name: "CraftWave", homeDir: "/code/craftwave", createdAt: 0 }}
      editing={false}
      collapsed={false}
      onToggle={() => {}}
      onRenameEnd={() => {}}
      onAddSession={onAddSession}
      onContextMenu={() => {}}
    />,
  );
  const button = screen.getByRole("button", { name: "在项目 CraftWave 下新建会话" });
  // 悬停露出态（opacity-0 → group-hover:opacity-100）：类名在，DOM 常驻可点
  expect(button.className).toContain("group-hover:opacity-100");
  // + 钮不冒泡给行（点击只新建，不切换折叠）
  fireEvent.click(button);
  expect(onAddSession).toHaveBeenCalledOnce();
});

it("项目行点击切换折叠：整行可点展开/收起，+ 钮点击不触发折叠", () => {
  const onToggle = vi.fn();
  const { container, rerender } = render(
    <ProjectRow
      project={{ id: "p1", name: "rig", homeDir: "/code/rig", createdAt: 0 }}
      editing={false}
      collapsed={false}
      onToggle={onToggle}
      onRenameEnd={() => {}}
      onAddSession={() => {}}
      onContextMenu={() => {}}
    />,
  );
  // 折叠状态用名称前的文件夹图标表达：展开=打开态（lucide-folder-open）
  expect(container.querySelector(".lucide-folder-open")).not.toBeNull();
  fireEvent.click(screen.getByText("rig"));
  expect(onToggle).toHaveBeenCalledOnce();
  rerender(
    <ProjectRow
      project={{ id: "p1", name: "rig", homeDir: "/code/rig", createdAt: 0 }}
      editing={false}
      collapsed
      onToggle={onToggle}
      onRenameEnd={() => {}}
      onAddSession={() => {}}
      onContextMenu={() => {}}
    />,
  );
  // 收起=闭合态文件夹（无打开态图标）；lucide 类 token 整词匹配，互不误中
  expect(container.querySelector(".lucide-folder-open")).toBeNull();
  expect(container.querySelector(".lucide-folder")).not.toBeNull();
});

it("标题只测量原文，重复测量保持稳定，变宽后撤掉副本", () => {
  let viewportWidth = 100;
  const observers: ResizeObserverCallback[] = [];
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: ResizeObserverCallback) {
        observers.push(callback);
      }
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => viewportWidth);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    return new DOMRect(0, 0, this.textContent === "很长的会话标题" ? 400 : 50, 20);
  });
  const { container } = render(<MarqueeTitle text="很长的会话标题" />);
  const title = container.firstElementChild!;
  expect(title.getAttribute("data-overflow")).toBe("true");
  const track = container.querySelector<HTMLElement>(".cw-marquee-track")!;
  expect(track.style.getPropertyValue("--marquee-shift")).toBe("-424px");
  act(() =>
    observers.forEach((callback) => {
      callback([], {} as ResizeObserver);
    }),
  );
  expect(track.style.getPropertyValue("--marquee-shift")).toBe("-424px");
  viewportWidth = 500;
  act(() =>
    observers.forEach((callback) => {
      callback([], {} as ResizeObserver);
    }),
  );
  expect(title.getAttribute("data-overflow")).toBe("false");
  expect(container.querySelector("[aria-hidden]")).toBeNull();
});

function FadeHarness({ text }: { text: string }) {
  const { ref, fade } = useScrollFade();
  return (
    <div ref={ref} data-testid="viewport" data-fade={fade}>
      <p>{text}</p>
    </div>
  );
}

it("列表内容追加即更新渐隐，无需等待用户滚动或窗口 resize", async () => {
  let height = 80;
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const { rerender } = render(<FadeHarness text="一条会话" />);
  expect(screen.getByTestId("viewport").getAttribute("data-fade")).toBe("none");
  height = 200;
  rerender(<FadeHarness text="增加了多条会话" />);
  await waitFor(() =>
    expect(screen.getByTestId("viewport").getAttribute("data-fade")).toBe("bottom"),
  );
});
