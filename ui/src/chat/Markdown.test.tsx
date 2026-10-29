// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { Markdown } from "./Markdown";

afterEach(cleanup);

it("中文标点旁的强调正常渲染，列表、任务、表格和引用保留语义", () => {
  const { container } = render(
    <Markdown>
      {
        "# 一级\n\n## 二级\n\n**中文（带标点）。**接着正文\n\n- 项目\n  - 子项\n- [x] 完成\n\n> 引用\n\n| 名称 | 值 |\n| --- | ---: |\n| 配置 | 14 |"
      }
    </Markdown>,
  );
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("一级");
  expect(container.querySelector('[data-streamdown="strong"]')?.textContent).toBe(
    "中文（带标点）。",
  );
  expect(container.querySelector("li ul li")?.textContent).toBe("子项");
  expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(true);
  expect(container.querySelector("blockquote")?.textContent?.trim()).toBe("引用");
  expect(screen.getAllByRole("columnheader")).toHaveLength(2);
  expect(screen.getAllByRole("cell").at(-1)?.getAttribute("style")).toContain("right");
});

it("半截代码围栏可流式显示，完成后保留代码与中文复制控件", () => {
  const { container, rerender } = render(<Markdown streaming>{"```ts\nconst value ="}</Markdown>);
  expect(container.querySelector("pre")?.textContent).toContain("const value =");
  rerender(<Markdown>{"```ts\nconst value = 1;\n```"}</Markdown>);
  expect(container.querySelector("pre")?.textContent).toContain("const value = 1;");
  expect(screen.getByRole("button", { name: "复制代码" })).toBeTruthy();
});

it("保留链接安全确认并阻止危险协议", () => {
  const { container } = render(
    <Markdown>{"[安全](https://example.com) [危险](javascript:alert%281%29)"}</Markdown>,
  );
  expect(screen.getByRole("button", { name: "安全" })).toBeTruthy();
  expect(container.querySelector('[href^="javascript:"]')).toBeNull();
});
