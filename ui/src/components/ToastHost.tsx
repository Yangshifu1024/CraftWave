/**
 * 顶部 Toast 容器：反馈统一为顶部轻提示的
 * 渲染面，挂在应用根（App SkeletonApp 内）。
 *
 * 布局与行为口径：
 * - 标题栏下方顶部居中（fixed top-10，避开自绘标题栏的交通灯/拖拽区）；
 * - 容器骨架 pointer-events-none，只有卡片本体 pointer-events-auto——
 *   不抢键盘焦点、不挡下方点击（不打断输入）；
 * - 成功/信息 3s、错误 6s 自动消失（时长由 toast store 排定）；
 * - 错误卡点击 → 既有 Dialog 组件展开完整 detail（App 组件，
 *   错误负载的字符串面，pre-wrap 保留换行）。
 */

import { CheckCircle2, CircleAlert, Info } from "lucide-react";
import { useState } from "react";
import { Dialog } from "@/components/ui/dialog";
import { useT } from "@/i18n/core";
import { type ToastItem, type ToastKind, useToastStore } from "@/stores/toast";

/** 单类提示的图标与配色（不喧宾夺主：图标 + 左缘色条，正文正常字号）。 */
const KIND_STYLE: Record<ToastKind, { icon: typeof Info; border: string; text: string }> = {
  success: {
    icon: CheckCircle2,
    border: "border-l-[var(--success)]",
    text: "text-[var(--success)]",
  },
  error: {
    icon: CircleAlert,
    border: "border-l-[var(--danger)]",
    text: "text-[var(--danger)]",
  },
  info: {
    icon: Info,
    border: "border-l-[var(--muted-foreground)]",
    text: "text-[var(--muted-foreground)]",
  },
};

/** 应用根挂载的 Toast 渲染面（单例；MirrorDemo 演示页不挂）。 */
export function ToastHost() {
  const t = useT();
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);
  // 点击展开的错误详情（Dialog 承载；关闭即清）
  const [detailToast, setDetailToast] = useState<ToastItem | null>(null);
  return (
    <>
      {/* z 序高于设置页浮层（settings-shell）与各菜单浮层（z-20）：反馈是
          全局最上层提示，但不模态、不聚焦 */}
      <div
        data-testid="toast-host"
        className="pointer-events-none fixed inset-x-0 top-10 z-[80] flex flex-col items-center gap-2 px-4"
      >
        {toasts.map((toast) => {
          const style = KIND_STYLE[toast.kind];
          const Icon = style.icon;
          const expandable = toast.kind === "error" && !!toast.detail;
          return (
            <div
              key={toast.id}
              data-testid={`toast-${toast.kind}`}
              className={`cw-pop-in pointer-events-auto flex max-w-xl items-start gap-2 rounded-xl border border-[var(--border)] ${style.border} border-l-4 bg-[var(--card)] px-3 py-2 text-ui-sm shadow-lg`}
              role={toast.kind === "error" ? "alert" : "status"}
            >
              <Icon size={14} aria-hidden className={`mt-0.5 shrink-0 ${style.text}`} />
              <div className="min-w-0 flex-1">
                <p className="text-[var(--foreground)]">{toast.title}</p>
                {/* 非错误的 detail 直接带一行（成功/信息详情通常短）；错误
                    详情收进「查看详情」，避免长错误刷屏 */}
                {toast.detail && toast.kind !== "error" && (
                  <p className="mt-0.5 break-all text-[var(--muted-foreground)]">{toast.detail}</p>
                )}
                {expandable && (
                  <button
                    type="button"
                    data-testid="toast-detail-button"
                    onClick={() => setDetailToast(toast)}
                    className="mt-0.5 shrink-0 rounded text-[var(--muted-foreground)] underline underline-offset-2 hover:text-[var(--foreground)]"
                  >
                    {t("workspace.viewDetail")}
                  </button>
                )}
              </div>
              <button
                type="button"
                aria-label={t("workspace.dismissToast")}
                onClick={() => dismiss(toast.id)}
                className="shrink-0 rounded px-1 text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
              >
                ×
              </button>
            </div>
          );
        })}
      </div>
      {/* 错误详情弹框（既有 Dialog 组件；关闭即清，不影响 store 出队节奏） */}
      <Dialog
        open={detailToast !== null}
        title={t("workspace.errorDetailTitle")}
        onClose={() => setDetailToast(null)}
        actions={[{ label: t("workspace.gotIt"), onClick: () => setDetailToast(null) }]}
      >
        <p className="mb-2 text-[var(--foreground)]">{detailToast?.title}</p>
        <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-all font-mono text-ui-sm">
          {detailToast?.detail}
        </pre>
      </Dialog>
    </>
  );
}
