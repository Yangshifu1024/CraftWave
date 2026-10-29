import {
  type ComponentPropsWithoutRef,
  type ElementType,
  useEffect,
  useRef,
  useState,
} from "react";

/** ZCode TaskTitleOverflowText 的移植：标题溢出时 hover 停留 1 秒启动走马灯。
 *  与纯 CSS 方案的关键差异（保留 ZCode 的实现口径）：
 *  - 溢出判定以单份标题的真实排版宽度为准（scrollWidth 取自原始副本），
 *    避免走马灯副本撑大 scrollWidth 后无法退出溢出状态；
 *  - Web Animations 用动态 offset 分离「移动」与「对齐后停留 2 秒」，
 *    纯 CSS infinite 动画只能按百分比停顿，标题宽度变化时无法保证停留时长；
 *  - mask 与位移动画共用时长：主体尾部到达左边缘前用双侧渐隐，
 *    到达后切回仅右侧渐隐，让对齐停留画面看得清标题开头。 */

const overflowFadeClassName =
  "[mask-image:linear-gradient(to_right,black_calc(100%_-_1.5rem),transparent)] [-webkit-mask-image:linear-gradient(to_right,black_calc(100%_-_1.5rem),transparent)]";
const marqueeGapPx = 24;
const marqueePixelsPerSecond = 40;
const minimumMarqueeDurationSeconds = 6;
const marqueePauseSeconds = 2;
const marqueeHoverDelayMs = 1_000;
const marqueeMaskFadeInSeconds = 0.15;
const rightFadeMask =
  "linear-gradient(to right, black 0, black calc(100% - 1.5rem), transparent 100%)";
const bothEdgesFadeMask =
  "linear-gradient(to right, transparent 0, black 1.5rem, black calc(100% - 1.5rem), transparent 100%)";

function createMaskKeyframe(maskImage: string, offset: number): Keyframe {
  return { maskImage, offset, webkitMaskImage: maskImage };
}

type TaskTitleOverflowTextProps = ComponentPropsWithoutRef<"p"> & {
  as?: "p" | "span";
};

export function TaskTitleOverflowText({
  as = "p",
  children,
  className,
  // 原生 title 提示框会在 hover 走马灯时遮挡正在滚动的标题，统一丢弃。
  title: _nativeTitle,
  ...props
}: TaskTitleOverflowTextProps) {
  const Component = as as ElementType;
  const textRef = useRef<HTMLElement | null>(null);
  const originalTextRef = useRef<HTMLSpanElement | null>(null);
  const marqueeTrackRef = useRef<HTMLSpanElement | null>(null);
  const [overflowState, setOverflowState] = useState({
    distance: 0,
    duration: minimumMarqueeDurationSeconds,
    isOverflowing: false,
  });
  const { distance, duration, isOverflowing } = overflowState;

  // biome-ignore lint/correctness/useExhaustiveDependencies: 以 children 为依赖是刻意的——标题文本变化时需重新量测溢出（ZCode 同款口径）
  useEffect(() => {
    const textElement = textRef.current;
    const originalTextElement = originalTextRef.current;
    if (!textElement || !originalTextElement) return;

    const updateOverflow = () => {
      const contentWidth = originalTextElement.scrollWidth;
      const nextIsOverflowing = contentWidth > textElement.clientWidth;
      const nextDistance = nextIsOverflowing ? contentWidth + marqueeGapPx : 0;
      const nextDuration = nextIsOverflowing
        ? Math.max(minimumMarqueeDurationSeconds, nextDistance / marqueePixelsPerSecond)
        : minimumMarqueeDurationSeconds;
      setOverflowState((current) =>
        current.isOverflowing === nextIsOverflowing &&
        current.distance === nextDistance &&
        current.duration === nextDuration
          ? current
          : {
              distance: nextDistance,
              duration: nextDuration,
              isOverflowing: nextIsOverflowing,
            },
      );
    };

    updateOverflow();

    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", updateOverflow);
      return () => window.removeEventListener("resize", updateOverflow);
    }

    const resizeObserver = new ResizeObserver(updateOverflow);
    resizeObserver.observe(textElement);
    resizeObserver.observe(originalTextElement);
    return () => resizeObserver.disconnect();
  }, [children]);

  useEffect(() => {
    const textElement = textRef.current;
    const marqueeTrack = marqueeTrackRef.current;
    if (
      !isOverflowing ||
      !textElement ||
      !marqueeTrack ||
      typeof marqueeTrack.animate !== "function"
    ) {
      return;
    }

    const reducedMotionQuery =
      typeof window.matchMedia === "function"
        ? window.matchMedia("(prefers-reduced-motion: reduce)")
        : null;
    // 事件绑到最近的行容器：鼠标位于时间或操作按钮区域时同样触发，
    // 与 ZCode 绑 [data-task-item-key] 行边界的口径一致。
    const hoverTarget = textElement.closest<HTMLElement>("[data-task-item-key]") ?? textElement;
    let animations: Animation[] = [];
    let startTimer: number | null = null;

    const stopMarquee = () => {
      if (startTimer !== null) {
        window.clearTimeout(startTimer);
        startTimer = null;
      }
      animations.forEach((animation) => {
        animation.cancel();
      });
      animations = [];
    };
    const startMarquee = () => {
      if (reducedMotionQuery?.matches) return;

      stopMarquee();
      const totalDuration = duration + marqueePauseSeconds;
      const movementEndOffset = duration / totalDuration;
      const maskFadeInEndOffset = marqueeMaskFadeInSeconds / totalDuration;
      // 主体尾部到达左边缘的时刻：位移里含 24px 间距，尾部先于副本头部到达。
      const originalTailArrivalTime = duration * ((distance - marqueeGapPx) / distance);
      const originalTailArrivalOffset = originalTailArrivalTime / totalDuration;
      const animationOptions: KeyframeAnimationOptions = {
        duration: totalDuration * 1_000,
        easing: "linear",
        iterations: Infinity,
      };
      const trackAnimation = marqueeTrack.animate(
        [
          { offset: 0, transform: "translate3d(0, 0, 0)" },
          {
            offset: movementEndOffset,
            transform: `translate3d(-${distance}px, 0, 0)`,
          },
          {
            offset: 1,
            transform: `translate3d(-${distance}px, 0, 0)`,
          },
        ],
        animationOptions,
      );
      const maskAnimation = textElement.animate(
        [
          createMaskKeyframe(rightFadeMask, 0),
          createMaskKeyframe(bothEdgesFadeMask, maskFadeInEndOffset),
          createMaskKeyframe(bothEdgesFadeMask, originalTailArrivalOffset),
          createMaskKeyframe(rightFadeMask, originalTailArrivalOffset),
          createMaskKeyframe(rightFadeMask, 1),
        ],
        animationOptions,
      );
      animations = [trackAnimation, maskAnimation];
    };
    const scheduleMarquee = () => {
      if (reducedMotionQuery?.matches) return;
      stopMarquee();
      // 短暂划过不应立即触发运动；停留满 1 秒才开始。
      startTimer = window.setTimeout(() => {
        startTimer = null;
        startMarquee();
      }, marqueeHoverDelayMs);
    };
    const handleMotionPreferenceChange = (event: MediaQueryListEvent) => {
      if (event.matches) stopMarquee();
    };

    hoverTarget.addEventListener("mouseenter", scheduleMarquee);
    hoverTarget.addEventListener("mouseleave", stopMarquee);
    reducedMotionQuery?.addEventListener("change", handleMotionPreferenceChange);

    return () => {
      hoverTarget.removeEventListener("mouseenter", scheduleMarquee);
      hoverTarget.removeEventListener("mouseleave", stopMarquee);
      reducedMotionQuery?.removeEventListener("change", handleMotionPreferenceChange);
      stopMarquee();
    };
  }, [distance, duration, isOverflowing]);

  return (
    <Component
      ref={textRef}
      className={[
        "min-w-0 flex-1 overflow-hidden whitespace-nowrap",
        isOverflowing && overflowFadeClassName,
        isOverflowing && "task-title-marquee",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      {...props}
    >
      <span
        ref={marqueeTrackRef}
        data-task-title-marquee-track="true"
        className="task-title-marquee-track inline-flex w-max min-w-max items-center gap-6"
      >
        <span ref={originalTextRef} data-task-title-copy="original" className="shrink-0">
          {children}
        </span>
        {isOverflowing ? (
          <span aria-hidden="true" data-task-title-copy="duplicate" className="shrink-0">
            {children}
          </span>
        ) : null}
      </span>
    </Component>
  );
}
