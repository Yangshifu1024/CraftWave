/**
 * 轻量右键菜单（ZCode 列表级单例 ContextMenu 同款）：fixed 定位 + 透明
 * 遮罩（左键/右键点击、Esc 均关闭）。项为定义式数组，支持分隔线与
 * 危险项；位置自动向视口内收拢，避免边缘溢出。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";

/** 菜单项定义：separator 为通边分隔线；其余为文字项（danger 红字）。 */
export type ContextMenuEntry =
  | { separator: true }
  | { separator?: false; label: string; danger?: boolean; onSelect: () => void };

/** 右键菜单的 props：x/y 为触发坐标（菜单向视口内收拢）；entries 见上；
 *  onClose 由遮罩/Esc 触发。 */
export interface ContextMenuProps {
  x: number;
  y: number;
  entries: ContextMenuEntry[];
  onClose: () => void;
}

/** 渲染右键菜单。open 由调用方以 entries 是否为空表达（空数组不渲染）。 */
export function ContextMenu({ x, y, entries, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  // Esc 关闭（capture 捕获行内输入框的 Esc 优先级与弹框一致）
  useEffect(() => {
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing) {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeydown, true);
    return () => window.removeEventListener("keydown", onKeydown, true);
  }, [onClose]);

  // 挂载后按实际尺寸向视口内收拢（默认尺寸预估，测量后校正）
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { innerWidth, innerHeight } = window;
    const rect = el.getBoundingClientRect();
    setPos({
      left: Math.max(4, Math.min(x, innerWidth - rect.width - 4)),
      top: Math.max(4, Math.min(y, innerHeight - rect.height - 4)),
    });
  }, [x, y]);

  return (
    <>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: 透明遮罩关闭是菜单标准交互，键盘路径由 Esc 承担 */}
      <div
        className="fixed inset-0 z-40"
        role="presentation"
        onMouseDown={onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          onClose();
        }}
      />
      <div
        ref={ref}
        role="menu"
        className="cw-pop-in fixed z-50 min-w-40 rounded-lg border border-[var(--border)] bg-[var(--card)] p-1 shadow-md"
        style={{ left: pos.left, top: pos.top }}
      >
        {entries.map((entry, index) =>
          entry.separator ? (
            // 静态定义数组，分隔线无业务 id，index key 无重排风险
            // biome-ignore lint/suspicious/noArrayIndexKey: 见上
            <div key={`sep-${index}`} className="-mx-1 my-1 h-px bg-[var(--border)]" />
          ) : (
            <button
              key={entry.label}
              type="button"
              role="menuitem"
              className={`flex min-h-7 w-full cursor-default items-center rounded-md px-2 text-left text-ui-sm text-[var(--foreground)] hover:bg-[var(--accent)] ${
                entry.danger ? "text-[var(--destructive)]" : ""
              }`}
              onClick={() => {
                onClose();
                entry.onSelect();
              }}
            >
              {entry.label}
            </button>
          ),
        )}
      </div>
    </>
  );
}
