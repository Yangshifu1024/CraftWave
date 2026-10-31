// @vitest-environment happy-dom
/**
 * Markdown 渲染测试。精简版：语义结构/流式围栏/链接安全合并为代表例。
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { Markdown } from "./Markdown";

afterEach(cleanup);

it("Markdown 渲染面：语义结构、流式代码围栏与中文复制控件、链接安全", () => {
  // 标题/强调/嵌套列表/任务/引用/表格（含右对齐列）保留语义
  const first = render(
    <Markdown>
      {
        "# 一级\n\n## 二级\n\n**中文（带标点）。**接着正文\n\n- 项目\n  - 子项\n- [x] 完成\n\n> 引用\n\n| 名称 | 值 |\n| --- | ---: |\n| 配置 | 14 |"
      }
    </Markdown>,
  );
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("一级");
  expect(first.container.querySelector('[data-streamdown="strong"]')?.textContent).toBe(
    "中文（带标点）。",
  );
  expect(first.container.querySelector("li ul li")?.textContent).toBe("子项");
  expect(first.container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(
    true,
  );
  expect(first.container.querySelector("blockquote")?.textContent?.trim()).toBe("引用");
  expect(screen.getAllByRole("columnheader")).toHaveLength(2);
  expect(screen.getAllByRole("cell").at(-1)?.getAttribute("style")).toContain("right");
  first.unmount();

  // 半截代码围栏可流式显示，完成后保留代码与复制控件
  const streamed = render(<Markdown streaming>{"```ts\nconst value ="}</Markdown>);
  expect(streamed.container.querySelector("pre")?.textContent).toContain("const value =");
  streamed.rerender(<Markdown>{"```ts\nconst value = 1;\n```"}</Markdown>);
  expect(streamed.container.querySelector("pre")?.textContent).toContain("const value = 1;");
  expect(screen.getByRole("button", { name: "复制代码" })).toBeTruthy();
  streamed.unmount();

  // 链接转安全确认，危险协议被阻止
  const links = render(
    <Markdown>{"[安全](https://example.com) [危险](javascript:alert%281%29)"}</Markdown>,
  );
  expect(screen.getByRole("button", { name: "安全" })).toBeTruthy();
  expect(links.container.querySelector('[href^="javascript:"]')).toBeNull();
  links.unmount();
});
