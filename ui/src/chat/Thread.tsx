import {
  ActionBarPrimitive,
  AuiIf,
  BranchPickerPrimitive,
  ComposerPrimitive,
  ErrorPrimitive,
  MessagePartPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { matchGhost, useAskStore } from "@/stores/ask";

/** S6 最小聊天窗：primitives 手搭（kit copy-paste 属后续 UX 票的增量）。 */
export function ChatThread() {
  return (
    <div className="flex h-dvh flex-col bg-neutral-950 text-neutral-100">
      <header
        data-tauri-drag-region
        className="flex h-10 shrink-0 cursor-default select-none items-center justify-center bg-neutral-900 text-xs text-neutral-400"
        style={{
          paddingLeft: "var(--tauri-plugin-decoration-left-clearance, 76px)",
          paddingRight: "var(--tauri-plugin-decoration-right-clearance, 8px)",
        }}
      >
        CraftWave · S6 骨架（顶栏即拖拽区）
      </header>
      <ThreadPrimitive.Root className="flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport className="flex flex-1 flex-col overflow-y-auto p-4">
          <AuiIf condition={(s) => s.thread.isEmpty}>
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-neutral-500">
              <p className="text-lg text-neutral-300">CraftWave</p>
              <p>空聊天窗已就绪 —— rig 流式接入见 S2/S3 票</p>
            </div>
          </AuiIf>
          <ThreadPrimitive.Messages>
            {({ message }) =>
              // system = runMarker 等存储标记节点（custom 载荷保真，§6.2 跳过内容）——不渲染
              message.role === "system" ? null : message.role === "user" ? (
                <UserMessage />
              ) : (
                <AssistantMessage />
              )
            }
          </ThreadPrimitive.Messages>
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 pt-2">
            <ComposerPrimitive.Root className="flex items-end gap-2 rounded-2xl border border-neutral-800 bg-neutral-900 p-2">
              <ComposerPrimitive.Input
                rows={1}
                placeholder="发一条消息试试骨架回声…"
                className="max-h-32 flex-1 resize-none bg-transparent px-2 py-1.5 text-sm focus:outline-none"
                onKeyDown={useGhostTabFill()}
              />
              <AuiIf condition={(s) => !s.thread.isRunning}>
                <ComposerPrimitive.Send className="rounded-full bg-sky-600 px-3 py-1.5 text-sm text-white" />
              </AuiIf>
              <AuiIf condition={(s) => s.thread.isRunning}>
                <ComposerPrimitive.Cancel className="rounded-full border border-neutral-700 px-3 py-1.5 text-sm">
                  停止
                </ComposerPrimitive.Cancel>
              </AuiIf>
            </ComposerPrimitive.Root>
            <ComposerGhost />
            <RunMetricsBar />
          </ThreadPrimitive.ViewportFooter>
        </ThreadPrimitive.Viewport>
      </ThreadPrimitive.Root>
    </div>
  );
}

function UserMessage() {
  return (
    <MessagePrimitive.Root className="flex flex-col items-end py-1">
      <div className="max-w-[80%] rounded-2xl bg-sky-700/70 px-4 py-2 text-sm">
        <MessagePrimitive.Parts>
          {({ part }) => (part.type === "text" ? <MessagePartPrimitive.Text /> : null)}
        </MessagePrimitive.Parts>
      </div>
      <BranchBar editOnly />
    </MessagePrimitive.Root>
  );
}

function AssistantMessage() {
  return (
    <MessagePrimitive.Root className="flex flex-col items-start py-1">
      <div className="max-w-[80%] rounded-2xl bg-neutral-800 px-4 py-2 text-sm">
        {/* 内联 render props（勿抽组件）：S1 实证 Parts 对 tool-call part 渲染
            null 时才 fallback 到注册的镜像工具卡；抽组件会让 fallback 判定失效。 */}
        <MessagePrimitive.Parts>
          {({ part }) =>
            part.type === "text" ? (
              <p className="whitespace-pre-wrap">
                <MessagePartPrimitive.Text />
              </p>
            ) : part.type === "reasoning" ? (
              <details className="my-1 rounded border border-neutral-700 bg-neutral-900/60 px-2 py-1 text-xs text-neutral-400">
                <summary className="cursor-default select-none">思考过程</summary>
                <p className="mt-1 whitespace-pre-wrap">{part.text}</p>
              </details>
            ) : null
          }
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ErrorPrimitive.Root className="text-xs text-red-400">
            <ErrorPrimitive.Message />
          </ErrorPrimitive.Root>
        </MessagePrimitive.Error>
      </div>
      <BranchBar editOnly={false} />
    </MessagePrimitive.Root>
  );
}

/** 分支操作条（§6.3 树操作 API 的 UI 接地，15 号票）：上一/下一分支 +
 * 重生成（assistant）/ 编辑重发（user）/ 复制。官方原语自带禁用态。 */
function BranchBar({ editOnly }: { editOnly: boolean }) {
  const btn =
    "rounded border border-transparent px-1.5 py-0.5 text-[11px] text-neutral-500 hover:border-neutral-700 hover:text-neutral-300 disabled:opacity-30";
  return (
    <div className="mt-0.5 flex items-center gap-0.5" data-testid="branch-bar">
      <BranchPickerPrimitive.Root className="flex items-center gap-0.5">
        <BranchPickerPrimitive.Previous className={btn} aria-label="上一个分支">
          ←
        </BranchPickerPrimitive.Previous>
        <span className="text-[11px] text-neutral-600">
          <BranchPickerPrimitive.Count />
        </span>
        <BranchPickerPrimitive.Next className={btn} aria-label="下一个分支">
          →
        </BranchPickerPrimitive.Next>
      </BranchPickerPrimitive.Root>
      <ActionBarPrimitive.Root className="flex items-center gap-0.5">
        {editOnly ? (
          <ActionBarPrimitive.Edit className={btn} aria-label="编辑重发">
            编辑
          </ActionBarPrimitive.Edit>
        ) : (
          <>
            <ActionBarPrimitive.Reload className={btn} aria-label="重新生成">
              重新生成
            </ActionBarPrimitive.Reload>
            <ActionBarPrimitive.Copy className={btn} aria-label="复制">
              复制
            </ActionBarPrimitive.Copy>
          </>
        )}
      </ActionBarPrimitive.Root>
    </div>
  );
}

/** 计数条三数（§10.5：rate / turnUsage / sessionUsage 常驻 Composer 下方）。 */
function RunMetricsBar() {
  const { rate, turnUsage, sessionUsage, lastRun } = useRunMetricsStore();
  const tokens = (u: { input: number; output: number }) => `${u.input + u.output}`;
  return (
    <div className="mt-1 flex items-center gap-3 px-2 text-[11px] text-neutral-500">
      <span data-testid="metric-rate">{rate === null ? "—" : `${rate.toFixed(0)} tok/s`}</span>
      <span data-testid="metric-turn">本轮 {tokens(turnUsage)} tok</span>
      <span data-testid="metric-session">会话 {tokens(sessionUsage)} tok</span>
      {lastRun && (
        <span data-testid="metric-run">
          {lastRun.reason === "done"
            ? `完成 · ${(lastRun.durationMs / 1000).toFixed(1)}s · ${lastRun.toolCount} 次工具`
            : lastRun.reason === "cancelled"
              ? "已取消"
              : "出错"}
        </span>
      )}
    </div>
  );
}

/** ghost 填入动作（Tab 与按钮共用）：填入整句建议并清除 hint（§10.4）。 */
function useGhostFill(): (() => void) | null {
  const hint = useAskStore((s) => s.hint);
  const clearHint = useAskStore((s) => s.clearHint);
  const input = useAuiState((s) => s.composer.text);
  const aui = useAui();
  if (matchGhost(hint, input) === null) return null;
  return () => {
    aui.composer.setText(hint ?? "", { origin: "user" });
    clearHint();
  };
}

/** Composer 输入框的 Tab 接受（hint 激活时 Tab = 填入建议，不移动焦点）。 */
function useGhostTabFill(): React.KeyboardEventHandler<HTMLTextAreaElement> {
  const fill = useGhostFill();
  return (event) => {
    if (event.key === "Tab" && fill) {
      event.preventDefault();
      fill();
    }
  };
}

/** 建议提示词 ghost（§10.4：空输入整句 / 前缀匹配补尾；接受 = 填入不发送）。 */
function ComposerGhost() {
  const hint = useAskStore((s) => s.hint);
  const input = useAuiState((s) => s.composer.text);
  const ghost = matchGhost(hint, input);
  const fill = useGhostFill();
  if (ghost === null || !fill) return null;
  return (
    <div
      data-testid="composer-ghost"
      className="mt-1 flex items-center gap-2 px-2 text-[11px] text-neutral-500"
    >
      <span>
        <span className="text-neutral-300">{input}</span>
        {ghost}
      </span>
      <button
        type="button"
        onClick={fill}
        className="rounded border border-neutral-700 px-1.5 py-0.5 text-neutral-400 hover:border-neutral-500"
      >
        Tab 填入
      </button>
    </div>
  );
}
