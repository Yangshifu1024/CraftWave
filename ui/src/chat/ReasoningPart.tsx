import type { ReasoningMessagePart } from "@assistant-ui/react";
import { Brain, ChevronRight } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { t, useT } from "@/i18n/core";

/** 思考时长（ZCode Math.ceil 口径）：最少 1 秒，跨分钟带分。 */
function formatThoughtDuration(seconds: number): string {
  if (seconds < 60) return t("workspace.thoughtSeconds", { n: seconds });
  const minutes = Math.floor(seconds / 60);
  return t("workspace.thoughtMinSec", { m: minutes, s: seconds - minutes * 60 });
}

/**
 * 思考块（ZCode ReasoningTrigger 同款复刻）：
 * - 完成态单行「思考 · 持续了 N 秒」——动作词与时长全部最淡档（subtlest），
 *   加粗只给动作词；图标静态大脑（旋转 loader 会长期占用渲染资源）。
 * - 流式态「正在思考」文案流光 + 最后一个非空行预览（双侧 16px 渐隐、吸尾滚动）。
 * - 收起态内容 300ms 延迟卸载（Radix 高度动画需要 closed 阶段读到真实高度）。
 * - 展开内容：左导线 + 缩进 + 限高滚动 + 纯文本保留换行，吸底跟随。
 */
export function ReasoningPart({ text, status, timing }: Omit<ReasoningMessagePart, "type">) {
  const t = useT();
  const streaming = status?.type === "running";
  const [open, setOpen] = useState(false);
  const [, tick] = useState(0);
  const summary = useRef<HTMLSpanElement>(null);
  const content = useRef<HTMLParagraphElement>(null);
  const follow = useRef(true);
  const [overflow, setOverflow] = useState(false);
  const lastLine = text.trimEnd().split(/\r?\n/).at(-1)?.trim() ?? "";
  // 收起态展示流式摘要，不为隐藏的耗时每秒触发重渲染；展开时再持续计时。
  useEffect(() => {
    if (!streaming || !open || timing === undefined) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [streaming, open, timing]);
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
          1,
          Math.ceil(((timing.completedAt ?? performance.now()) - timing.startedAt) / 1000),
        );
  const streamingSummary = streaming && !open && lastLine ? lastLine : null;
  // ZCode 分支语义：收起 + 流式 → 「正在思考」流光 + 单行摘要；
  // 展开或完成 → 「思考 · 持续了 N 秒」（展开中的流式按秒持续计时）。
  const thinkingLabel =
    streaming && !open ? (
      <span className="cw-shimmer-text shrink-0 font-medium whitespace-nowrap">
        {t("workspace.thinkingNow")}
      </span>
    ) : (
      <span className="inline-flex shrink-0 items-center gap-2 whitespace-nowrap">
        <span className="font-medium text-[var(--foreground-subtlest)]">
          {t("workspace.thoughtLabel")}
        </span>
        <span className="font-normal text-[var(--foreground-subtlest)]">·</span>
        <span className="font-normal text-[var(--foreground-subtlest)]">
          {seconds !== undefined
            ? formatThoughtDuration(seconds)
            : t("workspace.thoughtFewSeconds")}
        </span>
      </span>
    );
  return (
    <details
      open={open}
      onToggle={(e) => {
        follow.current = true;
        setOpen(e.currentTarget.open);
      }}
      className="group/reasoning flex min-w-0 max-w-full flex-col text-ui-base"
    >
      <summary className="inline-flex max-w-full min-w-0 cursor-pointer select-none items-center gap-2 self-start text-left transition-colors [&_svg]:text-[var(--foreground-subtlest)]">
        <Brain size={16} aria-hidden className="shrink-0" />
        {thinkingLabel}
        {streamingSummary !== null && (
          <>
            <span aria-hidden className="shrink-0 text-[var(--foreground-subtlest)]">
              ·
            </span>
            <span
              ref={summary}
              data-testid="reasoning-preview"
              className={`min-w-0 flex-1 overflow-hidden text-[var(--foreground-subtle)] whitespace-nowrap ${overflow ? "cw-fade-inline" : ""}`}
            >
              {streamingSummary}
            </span>
          </>
        )}
        <ChevronRight
          size={16}
          aria-hidden
          className={`shrink-0 text-[var(--foreground-subtlest)] opacity-0 transition-[transform,opacity] duration-200 ease-out group-open/reasoning:rotate-90 group-open/reasoning:opacity-100 group-hover/reasoning:opacity-100 ${open ? "rotate-90 opacity-100" : ""}`}
        />
      </summary>
      {open && (
        <div className="pt-3">
          <p
            ref={content}
            onScroll={(e) => {
              const el = e.currentTarget;
              follow.current = el.scrollHeight - el.clientHeight - el.scrollTop < 2;
            }}
            className="ml-2 max-h-60 overflow-y-auto border-l border-[var(--border)] pl-3.5 text-[var(--foreground-subtlest)] whitespace-pre-wrap break-words [overflow-wrap:anywhere]"
          >
            {text}
          </p>
        </div>
      )}
    </details>
  );
}
