/**
 * HTML 沙箱预览（14 号票，FR-12-04/§8.3 render_html 行）：把模型产出的
 * 自包含 HTML 渲染进隔离 iframe。
 *
 * 加载通道（票内定案，§9.5 回写）：srcdoc/blob: 帧会继承主窗 CSP
 * （`script-src 'self'` 拦预览页内联脚本），因此走后端自定义协议
 * `preview://localhost/<token>`（Windows `http://preview.localhost/`）——
 * 协议响应自带预览专用 CSP（放内联脚本/样式、禁一切外联），不继承主窗
 * CSP。iframe 叠加 `sandbox="allow-scripts"`（**无 allow-same-origin**）：
 * opaque origin 双重隔离——预览页脚本无法访问 parent/top DOM、无法读
 * cookie/localStorage、无法顶层导航（沙箱规范语义）。
 *
 * 内容注册：HTML 全文经 preview_register 命令进后端进程级注册表（不落盘，
 * token = sha256 前 16 hex 幂等）。应用重启后注册表清空，历史卡片重新
 * 展开时按参数原文重注册（本组件每次挂载都注册，天然覆盖该路径）。
 */

import { useEffect, useState } from "react";
import { registerPreview } from "@/ipc/client";

/** 沙箱 token 清单（逃逸防线的契约面，测试逐字断言）：
 *  - allow-scripts：允许预览页脚本执行（预览的意义）；
 *  - 其余一律不放行——allow-same-origin 不给（否则 opaque origin 失效，
 *    预览页可读主窗存储）、allow-top-navigation 不给（防劫持主窗）、
 *    allow-forms 不给（防表单导航探测）。 */
export const PREVIEW_SANDBOX_TOKENS = ["allow-scripts"] as const;

/** 渲染沙箱预览 iframe：挂载即注册内容并加载；注册失败（如内容超限）
 *  显示错误行，不打扰会话流。`html` 为完整 HTML 全文（≤50,000 字符）。 */
export function HtmlPreview({
  html,
  height = 240,
  testId = "html-preview",
}: {
  /** 完整 HTML 全文。 */
  html: string;
  /** iframe 高度（px），缺省 240。 */
  height?: number;
  /** 测试锚点。 */
  testId?: string;
}) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setError(null);
    registerPreview(html)
      .then((result) => {
        if (!cancelled) setSrc(result.url);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [html]);

  return (
    <div data-testid={testId} className="min-w-0 space-y-1">
      {error !== null ? (
        <p className="text-ui-sm text-[var(--destructive)]">预览注册失败：{error}</p>
      ) : src === null ? (
        <p className="text-ui-sm text-[var(--muted-foreground)]">正在准备沙箱预览…</p>
      ) : (
        /* 沙箱属性即逃逸防线：只有 allow-scripts 一个 token（见
           PREVIEW_SANDBOX_TOKENS 注释）；referrerPolicy 收紧不泄露来源。 */
        <iframe
          data-testid={`${testId}-frame`}
          title="HTML 沙箱预览"
          sandbox={PREVIEW_SANDBOX_TOKENS.join(" ")}
          src={src}
          referrerPolicy="no-referrer"
          style={{ height }}
          className="w-full rounded border border-[var(--border)] bg-white"
        />
      )}
    </div>
  );
}
