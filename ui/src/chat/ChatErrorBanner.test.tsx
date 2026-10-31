// @vitest-environment happy-dom
/**
 * ChatErrorBanner 测试。精简版：复制链路/模型错误入口/长错误折叠合并为代表例。
 */

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ChatErrorBanner } from "./ChatErrorBanner";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("错误横幅：复制链路（成功/失败反馈）、模型错误设置入口、长错误折叠", async () => {
  // 复制完整错误码及文案，并给模型错误提供设置入口
  const copy = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  const settings = vi.fn();
  const message = "E_MODEL_NOT_FOUND: 模型不存在\n完整上下文";
  const first = render(<ChatErrorBanner message={message} onOpenSettings={settings} />);
  fireEvent.click(first.getByRole("button", { name: "复制错误" }));
  await waitFor(() => expect(first.getByRole("status").textContent).toBe("已复制"));
  expect(copy).toHaveBeenCalledWith(message);
  fireEvent.click(first.getByRole("button", { name: "打开模型设置" }));
  expect(settings).toHaveBeenCalledOnce();
  first.unmount();

  // 复制失败给出反馈；网络错误不引导修改模型
  vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
  const second = render(
    <ChatErrorBanner message="E_NETWORK: 连接超时" onOpenSettings={() => {}} />,
  );
  expect(second.queryByRole("button", { name: "打开模型设置" })).toBeNull();
  fireEvent.click(second.getByRole("button", { name: "复制错误" }));
  await waitFor(() => expect(second.getByRole("status").textContent).toContain("复制失败"));
  second.unmount();

  // 长错误默认收成摘要，完整上下文仍可展开读取
  const long = `E_NETWORK: 连接超时\n${"完整错误上下文".repeat(100)}`;
  const third = render(<ChatErrorBanner message={long} />);
  expect(third.container.querySelector("details")?.open).toBe(false);
  expect(third.container.querySelector("summary")?.textContent).toBe("E_NETWORK: 连接超时");
  expect(third.container.querySelector("pre")?.textContent).toBe(long);
  expect(third.getByLabelText("查看错误详情")).toBeTruthy();
  third.unmount();
  copy.mockRestore();
});
