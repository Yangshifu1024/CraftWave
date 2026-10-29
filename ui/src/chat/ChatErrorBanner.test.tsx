// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ChatErrorBanner } from "./ChatErrorBanner";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("复制完整错误码及文案，并给模型错误提供设置入口", async () => {
  const copy = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  const settings = vi.fn();
  const message = "E_MODEL_NOT_FOUND: 模型不存在\n完整上下文";
  render(<ChatErrorBanner message={message} onOpenSettings={settings} />);
  fireEvent.click(screen.getByRole("button", { name: "复制错误" }));
  await waitFor(() => expect(screen.getByRole("status").textContent).toBe("已复制"));
  expect(copy).toHaveBeenCalledWith(message);
  fireEvent.click(screen.getByRole("button", { name: "打开模型设置" }));
  expect(settings).toHaveBeenCalledOnce();
});

it("复制失败给出反馈；网络错误不引导修改模型", async () => {
  vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
  render(<ChatErrorBanner message="E_NETWORK: 连接超时" onOpenSettings={() => {}} />);
  expect(screen.queryByRole("button", { name: "打开模型设置" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "复制错误" }));
  await waitFor(() => expect(screen.getByRole("status").textContent).toContain("复制失败"));
});

it("长错误默认收成摘要，完整上下文仍可展开读取", () => {
  const message = `E_NETWORK: 连接超时\n${"完整错误上下文".repeat(100)}`;
  const { container } = render(<ChatErrorBanner message={message} />);
  expect(container.querySelector("details")?.open).toBe(false);
  expect(container.querySelector("summary")?.textContent).toBe("E_NETWORK: 连接超时");
  expect(container.querySelector("pre")?.textContent).toBe(message);
  expect(screen.getByLabelText("查看错误详情")).toBeTruthy();
});
