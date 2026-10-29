import type { ReasoningMessagePart } from "@assistant-ui/react";
import { ChevronRight, Lightbulb } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";

/** 消费帧已有的状态和计时；历史记录缺少计时时只显示“思考过程”。 */
export function ReasoningPart({ text, status, timing }: Omit<ReasoningMessagePart, "type">) {
  const streaming = status?.type === "running";
  const [open, setOpen] = useState(false);
  const [, tick] = useState(0);
  const summary = useRef<HTMLSpanElement>(null);
  const content = useRef<HTMLParagraphElement>(null);
  const follow = useRef(true);
  const [overflow, setOverflow] = useState(false);
  const lastLine = text.trimEnd().split(/\r?\n/).at(-1)?.trim() ?? "";
  useEffect(() => {
    if (!streaming || timing === undefined) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [streaming, timing]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: 文本和折叠状态变化后重新测量实际溢出
  useLayoutEffect(() => {
    const el = summary.current;
    if (!el) return;
    const update = () => {
      setOverflow(el.scrollWidth > el.clientWidth + 1);
      el.scrollLeft = el.scrollWidth;
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, [lastLine, open, streaming]);
  // 用户上翻时暂停吸底，重新展开或滚回底部后继续跟随。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 文本追加时需要重新吸底
  useLayoutEffect(() => {
    if (open && follow.current && content.current) {
      content.current.scrollTop = content.current.scrollHeight;
    }
  }, [text, open]);
  const seconds =
    timing === undefined
      ? undefined
      : Math.max(
          0,
          Math.round(((timing.completedAt ?? performance.now()) - timing.startedAt) / 1000),
        );
  return (
    <details
      open={open}
      onToggle={(e) => {
        follow.current = true;
        setOpen(e.currentTarget.open);
      }}
      className="group/reasoning min-w-0 text-ui-sm text-[var(--muted-foreground)]"
    >
      <summary className="flex min-w-0 cursor-pointer select-none items-center gap-2 rounded px-1 py-1 hover:bg-[var(--accent)]/60">
        <Lightbulb size={14} aria-hidden className="shrink-0" />
        <span className={`shrink-0 ${streaming ? "cw-shimmer-text" : ""}`}>
          {streaming ? "思考中" : "思考过程"}
          {seconds !== undefined ? ` · ${seconds}s` : ""}
        </span>
        {streaming && !open && lastLine && (
          <span
            ref={summary}
            data-testid="reasoning-preview"
            className={`min-w-0 flex-1 overflow-hidden whitespace-nowrap ${overflow ? "cw-fade-inline" : ""}`}
          >
            {lastLine}
          </span>
        )}
        <ChevronRight
          size={12}
          aria-hidden
          className="shrink-0 transition-transform group-open/reasoning:rotate-90"
        />
      </summary>
      <p
        ref={content}
        onScroll={(e) => {
          const el = e.currentTarget;
          follow.current = el.scrollHeight - el.clientHeight - el.scrollTop < 24;
        }}
        className="my-1 ml-2 max-h-60 overflow-y-auto border-l border-[var(--border)] pl-3.5 whitespace-pre-wrap break-words"
      >
        {text}
      </p>
    </details>
  );
}
