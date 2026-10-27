/**
 * shadcn/ui 风格 Card：供应商卡片（标题区 + 内容区 + actions 底栏）。
 * 复刻 CodeWave 供应商卡结构：antd Card size=small + title + actions。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

export interface CardProps {
  title?: ReactNode;
  /** 底部动作区（一行分隔线 + 等宽按钮槽） */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Card({ title, actions, children, className }: CardProps) {
  return (
    <article
      className={cn(
        "flex min-w-0 flex-col rounded-[10px] border border-[var(--border)] bg-[var(--card)] text-[var(--card-foreground)]",
        className,
      )}
    >
      {title != null && (
        <header className="flex min-w-0 items-center gap-2 px-4 py-3">{title}</header>
      )}
      <div className="min-w-0 flex-1 p-4">{children}</div>
      {actions != null && <footer className="settings-card-actions">{actions}</footer>}
    </article>
  );
}
