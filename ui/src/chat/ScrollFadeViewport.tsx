import type { ComponentProps } from "react";
import { useEffect, useImperativeHandle, useRef, useState } from "react";

/** ZCode ScrollFadeViewport 的移植：滚动容器在顶/底边缘 24px 内做渐隐，
 *  滚离边缘即恢复不透明，让「下面还有内容」可被感知而不占常驻空间。 */

type ScrollMaskState = "none" | "top" | "bottom" | "both";

const SCROLL_MASK_CLASS_BY_STATE: Record<ScrollMaskState, string> = {
  none: "",
  top: "[mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_100%)] [-webkit-mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_100%)]",
  // Tailwind arbitrary value 里用下划线表示 calc 空格，避免生成无效的 calc(100%-24px)。
  bottom:
    "[mask-image:linear-gradient(to_bottom,black_0,black_calc(100%_-_24px),transparent_100%)] [-webkit-mask-image:linear-gradient(to_bottom,black_0,black_calc(100%_-_24px),transparent_100%)]",
  both: "[mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_calc(100%_-_24px),transparent_100%)] [-webkit-mask-image:linear-gradient(to_bottom,transparent_0,black_24px,black_calc(100%_-_24px),transparent_100%)]",
};

export function ScrollFadeViewport({ className, children, ref, ...props }: ComponentProps<"div">) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [maskState, setMaskState] = useState<ScrollMaskState>("none");

  // 渐隐组件与调用方共享同一个滚动节点，调用方原有的滚动引用仍然有效。
  useImperativeHandle(ref, () => viewportRef.current as HTMLDivElement, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 以 children 为依赖是刻意的——列表内容变化时需重量测滚动边缘（ZCode 同款口径）
  useEffect(() => {
    const viewport = viewportRef.current;
    const content = contentRef.current;
    if (!viewport || !content) {
      setMaskState("none");
      return;
    }

    const updateMaskState = () => {
      const maxScrollTop = viewport.scrollHeight - viewport.clientHeight;
      if (maxScrollTop <= 1) {
        setMaskState("none");
        return;
      }

      const hasHiddenTop = viewport.scrollTop > 1;
      const hasHiddenBottom = viewport.scrollTop < maxScrollTop - 1;
      setMaskState(
        hasHiddenTop && hasHiddenBottom
          ? "both"
          : hasHiddenTop
            ? "top"
            : hasHiddenBottom
              ? "bottom"
              : "none",
      );
    };

    let rafId: number | null = null;
    const scheduleUpdate = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        updateMaskState();
      });
    };

    updateMaskState();
    viewport.addEventListener("scroll", updateMaskState, { passive: true });

    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", scheduleUpdate);
      return () => {
        viewport.removeEventListener("scroll", updateMaskState);
        window.removeEventListener("resize", scheduleUpdate);
        if (rafId !== null) cancelAnimationFrame(rafId);
      };
    }

    const resizeObserver = new ResizeObserver(scheduleUpdate);
    resizeObserver.observe(viewport);
    resizeObserver.observe(content);
    window.addEventListener("resize", scheduleUpdate);

    return () => {
      viewport.removeEventListener("scroll", updateMaskState);
      resizeObserver.disconnect();
      window.removeEventListener("resize", scheduleUpdate);
      if (rafId !== null) cancelAnimationFrame(rafId);
    };
  }, [children]);

  return (
    <div
      ref={viewportRef}
      data-scroll-mask={maskState}
      className={[
        "min-h-0 flex-1 overflow-y-auto",
        SCROLL_MASK_CLASS_BY_STATE[maskState],
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      {...props}
    >
      <div ref={contentRef}>{children}</div>
    </div>
  );
}
