import { describe, expect, it } from "vitest";
import { summarizeGaps } from "./frame-sampler";

/**
 * 帧采样统计核心的单元测试：量化口径「>100ms 渲染长帧
 * 计数 = 0 为通过线」的判定逻辑在此钉住——阈值边界、统计口径与 pass 判定
 * 漂移即红。rAF 采样壳依赖浏览器环境，不在单测面。
 */

describe("summarizeGaps（帧采样口径）", () => {
  it("阈值边界（>100ms 才算长帧）、空样本零值通过、混合样本如实统计、报告回带审计字段", () => {
    // 空样本：无帧时报告零值且通过
    const empty = summarizeGaps([], 100, 0);
    expect(empty.totalFrames).toBe(1);
    expect(empty.longFrames).toBe(0);
    expect(empty.maxGapMs).toBe(0);
    expect(empty.avgGapMs).toBe(0);
    expect(empty.pass).toBe(true);

    // 全部短帧：通过线判定为通过
    const short = summarizeGaps([16.7, 16.9, 8.3, 33.2], 100, 75.1);
    expect(short.totalFrames).toBe(5);
    expect(short.longFrames).toBe(0);
    expect(short.maxGapMs).toBeCloseTo(33.2);
    expect(short.avgGapMs).toBeCloseTo((16.7 + 16.9 + 8.3 + 33.2) / 4);
    expect(short.pass).toBe(true);

    // 阈值边界：恰好 100ms 不算长帧，超过才算（>100ms 口径）
    expect(summarizeGaps([100], 100, 100).longFrames).toBe(0);
    expect(summarizeGaps([100], 100, 100).pass).toBe(true);
    expect(summarizeGaps([100.1], 100, 100.1).longFrames).toBe(1);
    expect(summarizeGaps([100.1], 100, 100.1).pass).toBe(false);

    // 混合样本：只统计超过阈值的部分，最大间隔如实带出
    const mixed = summarizeGaps([16, 120.5, 16, 200, 16], 100, 348.5);
    expect(mixed.totalFrames).toBe(6);
    expect(mixed.longFrames).toBe(2);
    expect(mixed.maxGapMs).toBe(200);
    expect(mixed.pass).toBe(false);

    // 报告回带阈值与采样时长（评审可调口径的审计面）
    const audit = summarizeGaps([50], 150, 50);
    expect(audit.thresholdMs).toBe(150);
    expect(audit.durationMs).toBe(50);
  });
});
