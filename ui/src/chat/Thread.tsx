import {
  AuiIf,
  ComposerPrimitive,
  ErrorPrimitive,
  MessagePartPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
} from "@assistant-ui/react";

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
            {({ message }) => (message.role === "user" ? <UserMessage /> : <AssistantMessage />)}
          </ThreadPrimitive.Messages>
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 pt-2">
            <ComposerPrimitive.Root className="flex items-end gap-2 rounded-2xl border border-neutral-800 bg-neutral-900 p-2">
              <ComposerPrimitive.Input
                rows={1}
                placeholder="发一条消息试试骨架回声…"
                className="max-h-32 flex-1 resize-none bg-transparent px-2 py-1.5 text-sm focus:outline-none"
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
          </ThreadPrimitive.ViewportFooter>
        </ThreadPrimitive.Viewport>
      </ThreadPrimitive.Root>
    </div>
  );
}

function UserMessage() {
  return (
    <MessagePrimitive.Root className="flex justify-end py-1">
      <div className="max-w-[80%] rounded-2xl bg-sky-700/70 px-4 py-2 text-sm">
        <MessagePrimitive.Parts>
          {({ part }) => (part.type === "text" ? <MessagePartPrimitive.Text /> : null)}
        </MessagePrimitive.Parts>
      </div>
    </MessagePrimitive.Root>
  );
}

function AssistantMessage() {
  return (
    <MessagePrimitive.Root className="flex justify-start py-1">
      <div className="max-w-[80%] rounded-2xl bg-neutral-800 px-4 py-2 text-sm">
        <MessagePrimitive.Parts>
          {({ part }) =>
            part.type === "text" ? (
              <p className="whitespace-pre-wrap">
                <MessagePartPrimitive.Text />
              </p>
            ) : null
          }
        </MessagePrimitive.Parts>
        <MessagePrimitive.Error>
          <ErrorPrimitive.Root className="text-xs text-red-400">
            <ErrorPrimitive.Message />
          </ErrorPrimitive.Root>
        </MessagePrimitive.Error>
      </div>
    </MessagePrimitive.Root>
  );
}
