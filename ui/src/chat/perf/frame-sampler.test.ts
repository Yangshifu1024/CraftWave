import { describe, expect, it } from "vitest";
import { summarizeGaps } from "./frame-sampler";

/**
 * 帧采样统计核心的单元测试（34 号票 NFR-03）：量化口径「>100ms 渲染长帧
 * 计数 = 0 为通过线」的判定逻辑在此钉住——阈值边界、统计口径与 pass 判定
 * 漂移即红。rAF 采样壳依赖浏览器环境，不在单测面。
 */

describe("summarizeGaps", () => {
  it("空样本：无帧时报告零值且通过", () => {
    const report = summarizeGaps([], 100, 0);
    expect(report.totalFrames).toBe(1);
    expect(report.longFrames).toBe(0);
    expect(report.maxGapMs).toBe(0);
    expect(report.avgGapMs).toBe(0);
    expect(report.pass).toBe(true);
  });

  it("全部短帧：通过线判定为通过", () => {
    const report = summarizeGaps([16.7, 16.9, 8.3, 33.2], 100, 75.1);
    expect(report.totalFrames).toBe(5);
    expect(report.longFrames).toBe(0);
    expect(report.maxGapMs).toBeCloseTo(33.2);
    expect(report.avgGapMs).toBeCloseTo((16.7 + 16.9 + 8.3 + 33.2) / 4);
    expect(report.pass).toBe(true);
  });

  it("阈值边界：恰好 100ms 不算长帧，超过才算（>100ms 口径）", () => {
    const atThreshold = summarizeGaps([100], 100, 100);
    expect(atThreshold.longFrames).toBe(0);
    expect(atThreshold.pass).toBe(true);
    const justOver = summarizeGaps([100.1], 100, 100.1);
    expect(justOver.longFrames).toBe(1);
    expect(justOver.pass).toBe(false);
  });

  it("混合样本：只统计超过阈值的部分，最大间隔如实带出", () => {
    const report = summarizeGaps([16, 120.5, 16, 200, 16], 100, 348.5);
    expect(report.totalFrames).toBe(6);
    expect(report.longFrames).toBe(2);
    expect(report.maxGapMs).toBe(200);
    expect(report.pass).toBe(false);
  });

  it("报告回带阈值与采样时长（评审可调口径的审计面）", () => {
    const report = summarizeGaps([50], 150, 50);
    expect(report.thresholdMs).toBe(150);
    expect(report.durationMs).toBe(50);
  });
});
