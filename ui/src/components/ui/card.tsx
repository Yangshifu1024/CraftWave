/**
 * shadcn/ui 风格 Card：卡片容器（供应商卡片：标题区 + 内容区 + 底部动作区）。
 * 复刻 CodeWave 供应商卡结构：antd Card size=small + title + actions。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

/** Card 的 props：title 标题区内容；actions 底部动作区；children 主体内容；className 追加类名。 */
export interface CardProps {
  title?: ReactNode;
  /** 底部动作区（一行分隔线 + 等宽按钮槽） */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}

/** 渲染一张卡片：可选标题头 / 自适应主体 / 可选动作底栏。 */
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
