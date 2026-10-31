/**
 * 压测帧采样器（dev-only）：requestAnimationFrame 采样渲染
 * 帧间隔，统计超过阈值（100ms）的「渲染长帧」数。
 *
 * 量化口径（评审可调的阈值线）：**压测期间 >100ms
 * 的帧间隔计数 = 0 为通过线**。统计核心（summarizeGaps）为纯函数，单元
 * 测试直接覆盖；浏览器采样壳（FrameSampler）只在 dev 压测页运行。
 */

/** 一次压测采样的统计报告。 */
export interface FrameSampleReport {
  /** 采样到的帧数（含首帧）。 */
  totalFrames: number;
  /** 相邻帧间隔超过 thresholdMs 的次数（渲染长帧 = 卡顿的直接证据）。 */
  longFrames: number;
  /** 最大帧间隔（毫秒）。 */
  maxGapMs: number;
  /** 平均帧间隔（毫秒；无样本时为 0）。 */
  avgGapMs: number;
  /** 长帧判定阈值（毫秒）。 */
  thresholdMs: number;
  /** 采样总时长（毫秒；首帧到最后帧）。 */
  durationMs: number;
  /** 是否达到通过线（长帧数 = 0）。 */
  pass: boolean;
  /** 原始间隔序列（诊断用：定位长帧发生在哪次交互）。 */
  gaps: number[];
}

/**
 * 纯统计核心：由相邻帧间隔序列算报告。
 * 独立导出以便在 vitest（无 rAF 环境）直接测试。
 */
export function summarizeGaps(
  gaps: readonly number[],
  thresholdMs: number,
  durationMs: number,
): FrameSampleReport {
  const totalFrames = gaps.length + 1;
  const maxGapMs = gaps.length > 0 ? Math.max(...gaps) : 0;
  const sum = gaps.reduce((acc, g) => acc + g, 0);
  return {
    totalFrames,
    longFrames: gaps.filter((g) => g > thresholdMs).length,
    maxGapMs,
    avgGapMs: gaps.length > 0 ? sum / gaps.length : 0,
    thresholdMs,
    durationMs,
    pass: gaps.every((g) => g <= thresholdMs),
    gaps: [...gaps],
  };
}

/** rAF 采样壳：start 到 stop 之间持续记录相邻帧间隔。 */
export class FrameSampler {
  private gaps: number[] = [];
  private rafId: number | null = null;
  private lastMs = 0;
  private startedAtMs = 0;

  constructor(private readonly thresholdMs = 100) {}

  /** 开始采样（重复调用无副作用——以首次为准）。 */
  start(): void {
    if (this.rafId !== null) return;
    this.gaps = [];
    this.startedAtMs = performance.now();
    this.lastMs = this.startedAtMs;
    const loop = (now: number) => {
      this.gaps.push(now - this.lastMs);
      this.lastMs = now;
      this.rafId = requestAnimationFrame(loop);
    };
    this.rafId = requestAnimationFrame(loop);
  }

  /** 停止采样并产出报告；未开始时返回 null。 */
  stop(): FrameSampleReport | null {
    if (this.rafId === null) return null;
    cancelAnimationFrame(this.rafId);
    this.rafId = null;
    return summarizeGaps(this.gaps, this.thresholdMs, this.lastMs - this.startedAtMs);
  }
}
