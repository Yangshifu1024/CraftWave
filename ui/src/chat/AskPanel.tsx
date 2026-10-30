/**
 * AskPanel（§3.2 AskPayload 五形态，14 号票起 question/approval，08 号票
 * 增 mcpTrust，09 号票增 projectTrust，24 号票增 externalDir）：
 * - question：逐题选项点选 + 自由文本输入 → resolveAsk(kind=question)；
 * - approval：diff 预览 / 命令行 + 围栏原因 → 允许/拒绝 → resolveAsk(kind=approval，
 *   optionId = allow_once/always_session/deny；23 号票起命令类审批增
 *   「始终允许（本会话）」选项与「加入白名单」勾选——选项由后端按
 *   approval.sessionMemoryEnabled 下发，勾选经 save_config 持久白名单)；
 * - mcpTrust：MCP 启动信任（TOFU）摘要 + 三选项 → resolveAsk(kind=approval，
 *   optionId = allow_once/remember/deny)。后端以 sessionId=""（应用级询问）
 *   发 ask:opened——不属任何会话桶，任意会话上下文都渲染（设置页连接触发）；
 * - projectTrust：项目资源信任（技能/项目指令首用，§9.8）摘要 + 三选项 →
 *   应答同 Approval 协议；sessionId="" 应用级桶（run 开始装配时触发）；
 * - externalDir：外部目录授权（§9.9，24 号票）——AI 访问会话 roots 外
 *   路径前的「始终允许」确认：路径 + 粒度说明 + 允许/拒绝 → 应答同
 *   Approval 协议（optionId = allow/deny），允许后后端把该目录并入会话
 *   roots 快照（授权仅本会话有效）。
 *
 * 事件驱动：ask:opened 置 store.current、ask:closed 清除（App.tsx 事件桥）。
 * 后端默认无限等待，autoConfirm 300s 由 Rust 侧兜底（前端无需计时）；
 * mcpTrust/projectTrust/externalDir 无推荐项，不参与自动批准（高危外部
 * 资源永远过人）。
 *
 * 失效面（05 号票，§5.6 待审批恢复）：重连后恢复的审批可能在重连期间已被
 * 解决/取消——resolveAsk 返回 E_NOT_FOUND 时按 askId 关闭弹窗（不残留可
 * 操作面板）；其余错误原位显示可重试。
 */

import { CircleAlert, Eye, FolderOpen, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { DiffView } from "@/chat/DiffView";
import { HtmlPreview } from "@/chat/HtmlPreview";
import { useSessionScope } from "@/chat/session-scope";
import { resolveAsk, toIpcError } from "@/ipc/client";
import type { AskPayload } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { useSettingsStore } from "@/stores/settings";

/**
 * 从审批命令文本提取白名单前缀（23 号票，§3.2 approval 注记的词序列前缀
 * 语义）：按空白切词取前两词（单词命令取一词）——与后端 allowlist_hit 的
 * 「首词+子命令」匹配口径一致（`git status --short` → `git status`）。
 * 空文本返回 null（无法构成条目）。
 */
export function approvalCommandPrefix(command: string): string | null {
  const words = command.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  return words.slice(0, 2).join(" ");
}

/** 询问面板：本会话有未决 ask 时浮层渲染对应表单；会话桶为空时看应用级
 *  桶（sessionId=""，MCP 启动信任询问专用）。 */
export function AskPanel() {
  const sessionId = useSessionScope();
  const current = useAskStore((s) => s.bySession[sessionId]);
  // 应用级 MCP 信任询问（后端 sessionId=""）：不属任何 run/会话，任意
  // 会话 Tab 都要能看到并应答（多 Tab 下先到先答，后端 oneshot 槽保证
  // 第二次应答自然失效）
  const appLevel = useAskStore((s) => s.bySession[""]);
  if (!current && appLevel?.payload.kind === "mcpTrust") {
    return (
      <div
        data-testid="ask-panel"
        data-kind="mcpTrust"
        className="fixed inset-x-4 bottom-4 z-50 mx-auto max-w-xl rounded-xl border border-[var(--border)] bg-[var(--card)] p-4 shadow-2xl"
      >
        <McpTrustForm sessionId="" askId={appLevel.askId} payload={appLevel.payload} />
      </div>
    );
  }
  // 应用级项目资源信任询问（09 号票，§9.8）：run 开始装配触发，不属任何 run。
  if (!current && appLevel?.payload.kind === "projectTrust") {
    return (
      <div
        data-testid="ask-panel"
        data-kind="projectTrust"
        className="fixed inset-x-4 bottom-4 z-50 mx-auto max-w-xl rounded-xl border border-[var(--border)] bg-[var(--card)] p-4 shadow-2xl"
      >
        <ProjectTrustForm sessionId="" askId={appLevel.askId} payload={appLevel.payload} />
      </div>
    );
  }
  if (!current) return null;
  return (
    <div
      data-testid="ask-panel"
      data-kind={current.payload.kind}
      className="fixed inset-x-4 bottom-4 z-50 mx-auto max-w-xl rounded-xl border border-[var(--border)] bg-[var(--card)] p-4 shadow-2xl"
    >
      {current.payload.kind === "question" ? (
        <QuestionForm sessionId={sessionId} askId={current.askId} payload={current.payload} />
      ) : current.payload.kind === "mcpTrust" ? (
        <McpTrustForm sessionId={sessionId} askId={current.askId} payload={current.payload} />
      ) : current.payload.kind === "projectTrust" ? (
        <ProjectTrustForm sessionId={sessionId} askId={current.askId} payload={current.payload} />
      ) : current.payload.kind === "externalDir" ? (
        <ExternalDirForm sessionId={sessionId} askId={current.askId} payload={current.payload} />
      ) : (
        <ApprovalForm sessionId={sessionId} askId={current.askId} payload={current.payload} />
      )}
    </div>
  );
}

/** 应答提交的共享错误面：resolveAsk 失败（槽失效/IPC 异常）时在卡片内
 *  原位显示错误——点击无反馈会让审批永远卡死，静默失败不可接受。
 *  E_NOT_FOUND（询问已失效——重连期间已被解决/取消）例外：按 askId 关闭
 *  弹窗，不残留可操作面板（§5.6，05 号票）。 */
function useSubmitError(sessionId: string): [string | null, (err: unknown) => void, () => void] {
  const [error, setError] = useState<string | null>(null);
  const report = (err: unknown) => {
    const ipcErr = toIpcError(err);
    if (ipcErr.code === "E_NOT_FOUND") {
      const askId = useAskStore.getState().bySession[sessionId]?.askId;
      if (askId !== undefined) useAskStore.getState().close(sessionId, askId);
      return;
    }
    const message = err instanceof Error ? err.message : String(err);
    console.error("[ask] 应答提交失败", err);
    setError(message || "应答提交失败");
  };
  return [error, report, () => setError(null)];
}

/** 问卷表单：逐题点选选项 / 填自由文本，全部作答后可提交（resolveAsk）。 */
function QuestionForm({
  sessionId,
  askId,
  payload,
}: {
  sessionId: string;
  askId: string;
  payload: Extract<AskPayload, { kind: "question" }>;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [error, reportError, clearError] = useSubmitError(sessionId);
  const complete = payload.questions.every(
    (q) => answers[q.id] !== undefined && answers[q.id] !== "",
  );

  const submit = () => {
    clearError();
    resolveAsk(askId, { kind: "question", answers }).catch(reportError);
  };

  return (
    <div className="space-y-3">
      <h3 className="text-ui-base font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.questions.map((q) => (
        <div key={q.id} data-testid={`ask-question-${q.id}`} className="space-y-1">
          <p className="text-ui-sm text-[var(--foreground)]">{q.text}</p>
          <div className="flex flex-wrap gap-1.5">
            {q.options?.map((opt) => (
              <button
                key={opt.id}
                type="button"
                data-selected={answers[q.id] === opt.id}
                onClick={() => setAnswers((prev) => ({ ...prev, [q.id]: opt.id }))}
                className={`rounded-full border px-2.5 py-1 text-ui-sm ${
                  answers[q.id] === opt.id
                    ? "border-[var(--ring)] bg-[var(--accent)] text-[var(--foreground)]"
                    : "border-[var(--border)] text-[var(--foreground)] hover:border-[var(--border)]"
                }`}
              >
                {opt.label}
              </button>
            ))}
            {q.allowFreeText && (
              <input
                type="text"
                placeholder="自由输入…"
                value={answers[q.id] ?? ""}
                onChange={(e) => setAnswers((prev) => ({ ...prev, [q.id]: e.target.value }))}
                className="min-w-32 flex-1 rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-ui-sm text-[var(--foreground)]"
              />
            )}
          </div>
        </div>
      ))}
      <button
        type="button"
        disabled={!complete}
        onClick={submit}
        className="rounded-full bg-[var(--primary)] px-4 py-1.5 text-ui-sm text-[var(--primary-foreground)] disabled:opacity-40"
      >
        提交回答
      </button>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-2 py-1 text-ui-sm text-[var(--destructive)]"
        >
          应答提交失败：{error}（可重试）
        </p>
      )}
    </div>
  );
}

/** 审批表单：命令行与 diff 预览 + 允许/拒绝按钮（点击即提交所选选项）。
 *  命令类审批（载荷含 always_session 选项 = 后端 ③ 开）增「加入白名单」
 *  勾选（23 号票，④ 开才展示）：勾选后点允许类选项，先把命令前缀经
 *  save_config 追加进 commandAllowlist 持久化，再提交应答。 */
function ApprovalForm({
  sessionId,
  askId,
  payload,
}: {
  sessionId: string;
  askId: string;
  payload: Extract<AskPayload, { kind: "approval" }>;
}) {
  const [error, reportError, clearError] = useSubmitError(sessionId);
  // 「查看预览」展开态（FR-12-04，14 号票）：默认收起——展开才挂载沙箱
  // iframe（防闪烁、防未经查看的脚本执行，与 render_html 卡同口径）。
  const [previewOpen, setPreviewOpen] = useState(false);
  // 「加入白名单」勾选态（23 号票）
  const [addToList, setAddToList] = useState(false);
  // 白名单配置面（④ 整体开关）：settings store 全局状态
  const allowlistEnabled = useSettingsStore((s) => s.config?.approval.allowlistEnabled ?? false);
  const appendToAllowlist = useSettingsStore((s) => s.appendToAllowlist);
  // 载荷含 always_session 选项 = 后端判定为命令类审批（③ 开才下发）
  const isCommandApproval = payload.options.some((o) => o.mode === "always_session");
  const showAddToList = isCommandApproval && allowlistEnabled;
  const submit = (optionId: string) => {
    clearError();
    // 勾选「加入白名单」且选项是放行类：先持久化白名单再应答（写入失败
    // 不阻断审批——错误仅记控制台，应答照常提交）
    if (addToList && showAddToList && optionId !== "deny" && payload.command) {
      const prefix = approvalCommandPrefix(payload.command);
      if (prefix) {
        appendToAllowlist(prefix).catch((err) => console.error("[ask] 白名单写入失败", err));
      }
    }
    resolveAsk(askId, { kind: "approval", optionId }).catch(reportError);
  };
  return (
    <div className="space-y-3">
      <h3 className="text-ui-base font-medium text-[var(--foreground)]">{payload.title}</h3>
      {payload.command && (
        <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-[var(--background)] px-2 py-1 font-mono text-ui-sm text-[var(--foreground)]">
          {payload.command}
        </pre>
      )}
      {payload.fenceReason && (
        <p className="flex items-center gap-1.5 text-ui-sm text-[var(--warning)]">
          <CircleAlert size={12} aria-hidden className="shrink-0" />
          {payload.fenceReason}
        </p>
      )}
      {/* 计划范围门（25 号票，§8.6）：载荷带 planScope = 范围卡——明示
          计划外写路径；三选项由后端下发（允许一次 / 本会话始终允许（写入
          所在目录）/ 拒绝），选中 always_session 后后端把目标父目录并入
          会话基线扩展。 */}
      {payload.planScope && (
        <p
          data-testid="ask-plan-scope"
          className="rounded bg-[var(--warning)]/10 px-2 py-1 font-mono text-ui-sm text-[var(--warning)]"
        >
          计划外路径：{payload.planScope}
        </p>
      )}
      {payload.diff && <DiffView diff={payload.diff} testId="ask-diff" />}
      {/* HTML 产出预览入口（14 号票）：载荷附带 previewHtml（write/edit 目标
          为 .html/.htm 时由宿主构造）才显示，render_html 同款沙箱渲染 */}
      {payload.previewHtml && (
        <div data-testid="ask-preview">
          <button
            type="button"
            data-testid="ask-preview-toggle"
            onClick={() => setPreviewOpen((v) => !v)}
            className="inline-flex cursor-pointer items-center gap-1 text-ui-sm text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
            aria-expanded={previewOpen}
          >
            <Eye size={14} aria-hidden className="shrink-0" />
            {previewOpen ? "收起预览" : "查看预览"}
          </button>
          {previewOpen && (
            <div className="mt-2">
              <HtmlPreview html={payload.previewHtml} testId="ask-preview-frame-host" />
            </div>
          )}
        </div>
      )}
      {showAddToList && (
        <label
          data-testid="ask-add-allowlist"
          className="flex cursor-pointer items-center gap-1.5 text-ui-sm text-[var(--muted-foreground)]"
        >
          <input
            type="checkbox"
            checked={addToList}
            onChange={(e) => setAddToList(e.target.checked)}
            className="accent-[var(--primary)]"
          />
          把命令前缀加入白名单（此后同前缀命令全局免审批）
        </label>
      )}
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-ui-sm ${
              opt.mode === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-[var(--destructive)]"
                : "bg-[var(--primary)] text-[var(--primary-foreground)]"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-2 py-1 text-ui-sm text-[var(--destructive)]"
        >
          应答提交失败：{error}（可重试；若反复失败请停止该任务）
        </p>
      )}
    </div>
  );
}

/** MCP 启动信任表单（08 号票，§9.7 TOFU 第三形态）：server/transport/
 *  commandOrUrl/fingerprint 摘要 + 三选项按钮 → resolveAsk(approval)。
 *  应答与普通审批同协议（optionId：allow_once / remember / deny），
 *  后端在 resolve_ask 的 run 槽未命中时落到 MCP 信任槽。 */
function McpTrustForm({
  sessionId,
  askId,
  payload,
}: {
  sessionId: string;
  askId: string;
  payload: Extract<AskPayload, { kind: "mcpTrust" }>;
}) {
  const [error, reportError, clearError] = useSubmitError(sessionId);
  const submit = (optionId: string) => {
    clearError();
    resolveAsk(askId, { kind: "approval", optionId }).catch(reportError);
  };
  return (
    <div className="space-y-3">
      <h3 className="flex items-center gap-2 text-ui-base font-medium text-[var(--foreground)]">
        <ShieldCheck size={16} aria-hidden className="shrink-0 text-[var(--warning)]" />
        信任并连接 MCP 服务器 {payload.server}？
      </h3>
      <div className="space-y-1 rounded border border-[var(--border)] bg-[var(--background)] px-3 py-2">
        <p className="text-ui-sm text-[var(--foreground)]">
          传输方式：
          {payload.transport === "stdio" ? "stdio 子进程" : "streamable HTTP"}
        </p>
        <p className="min-w-0 font-mono text-ui-sm break-all text-[var(--foreground)]">
          {payload.commandOrUrl}
        </p>
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          配置指纹：<code data-testid="mcp-trust-fingerprint">{payload.fingerprint}</code>
        </p>
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          首次连接需要确认服务器身份；「记住此版本」后同指纹不再询问，配置被改动会重新确认。
        </p>
      </div>
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-ui-sm ${
              opt.id === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-[var(--destructive)]"
                : "bg-[var(--primary)] text-[var(--primary-foreground)]"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-2 py-1 text-ui-sm text-[var(--destructive)]"
        >
          应答提交失败：{error}（可重试）
        </p>
      )}
    </div>
  );
}

/** 项目资源信任表单（09 号票，§9.8 第四形态）：技能名/作用域/指纹摘要 +
 *  三选项按钮 → resolveAsk(approval)。技能首用未信任时后端在 run 装配时
 *  弹出（挂应用级槽），「记住此版本」后同指纹免问；拒绝后该技能本次运行
 *  不可用，模型侧调用回 E_PROJECT_UNTRUSTED。 */
function ProjectTrustForm({
  sessionId,
  askId,
  payload,
}: {
  sessionId: string;
  askId: string;
  payload: Extract<AskPayload, { kind: "projectTrust" }>;
}) {
  const [error, reportError, clearError] = useSubmitError(sessionId);
  const submit = (optionId: string) => {
    clearError();
    resolveAsk(askId, { kind: "approval", optionId }).catch(reportError);
  };
  return (
    <div className="space-y-3">
      <h3 className="flex items-center gap-2 text-ui-base font-medium text-[var(--foreground)]">
        <ShieldCheck size={16} aria-hidden className="shrink-0 text-[var(--warning)]" />
        信任技能 {payload.resourceName}？
      </h3>
      <div className="space-y-1 rounded border border-[var(--border)] bg-[var(--background)] px-3 py-2">
        <p className="text-ui-sm text-[var(--foreground)]">
          来源：{payload.scope === "global" ? "全局技能目录" : "项目技能目录"}
        </p>
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          版本指纹：<code data-testid="skill-trust-fingerprint">{payload.fingerprint}</code>
        </p>
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          技能包含说明与随附脚本、模板等全部文件；「记住此版本」后同版本不再询问，内容变更会重新确认。拒绝后本次运行不加载该技能。
        </p>
      </div>
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-ui-sm ${
              opt.id === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-[var(--destructive)]"
                : "bg-[var(--primary)] text-[var(--primary-foreground)]"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-2 py-1 text-ui-sm text-[var(--destructive)]"
        >
          应答提交失败：{error}（可重试）
        </p>
      )}
    </div>
  );
}

/** 外部目录授权表单（24 号票，§9.9 第五形态）：授权目标路径 + 粒度说明 +
 *  允许/拒绝按钮 → resolveAsk(approval，optionId = allow/deny)。工具访问
 *  会话 roots 外路径时后端在审批门挂起弹出；允许后后端把该目录并入会话
 *  roots 快照（「该目录及子目录的读/写/列」粒度，授权仅本会话有效，
 *  Inspector「授权」页可移除）。无推荐项，不参与自动批准。 */
function ExternalDirForm({
  sessionId,
  askId,
  payload,
}: {
  sessionId: string;
  askId: string;
  payload: Extract<AskPayload, { kind: "externalDir" }>;
}) {
  const [error, reportError, clearError] = useSubmitError(sessionId);
  const submit = (optionId: string) => {
    clearError();
    resolveAsk(askId, { kind: "approval", optionId }).catch(reportError);
  };
  return (
    <div className="space-y-3">
      <h3 className="flex items-center gap-2 text-ui-base font-medium text-[var(--foreground)]">
        <FolderOpen size={16} aria-hidden className="shrink-0 text-[var(--warning)]" />
        允许访问项目外的目录？
      </h3>
      <div className="space-y-1 rounded border border-[var(--border)] bg-[var(--background)] px-3 py-2">
        <p className="min-w-0 font-mono text-ui-sm break-all text-[var(--foreground)]">
          <code data-testid="external-dir-path">{payload.path}</code>
        </p>
        <p className="text-ui-sm text-[var(--muted-foreground)]">
          授权粒度：该目录及子目录的读/写/列。授权仅本会话有效（可在
          Inspector「授权」页移除）；项目内的目录不需要授权。
        </p>
      </div>
      <div className="flex justify-end gap-2">
        {payload.options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            data-testid={`ask-option-${opt.id}`}
            onClick={() => submit(opt.id)}
            className={`rounded-full px-4 py-1.5 text-ui-sm ${
              opt.id === "deny"
                ? "border border-[var(--border)] text-[var(--foreground)] hover:border-[var(--destructive)]"
                : "bg-[var(--primary)] text-[var(--primary-foreground)]"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
      {error !== null && (
        <p
          data-testid="ask-error"
          className="rounded border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-2 py-1 text-ui-sm text-[var(--destructive)]"
        >
          应答提交失败：{error}（可重试）
        </p>
      )}
    </div>
  );
}

/** diff 行配色与预览渲染已抽为公共组件 DiffView（26 号票，FR-11-02）：
 *  审批卡与变更面板共用同一套渲染（见 DiffView.tsx）。 */
