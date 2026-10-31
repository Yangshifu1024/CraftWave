/**
 * 取消传播前端挂接：abortSignal → invoke("cancel_run")。
 *
 * ChatModelAdapter.run 入口把 assistant-ui 传入的 abortSignal
 * 桥到 Rust 侧 cancel_run 命令——fire-and-forget（不 await），取消的「权威终态」
 * 由 Rust 回发的 runEnded(cancelled) 帧经 FrameBuffer 快照流出，前端不自行收尾。
 */

/** invoke 的最小面（便于测试注入；生产 = @tauri-apps/api/core 的 invoke）。 */
export type InvokeFn = (cmd: string, args?: unknown) => Promise<unknown>;

/**
 * 把 abort 事件桥到 cancel_run。返回解绑函数（adapter 的 finally 里调用）。
 * signal 已 aborted 时立即触发（异步 run 中途 abort 的窗口）。
 */
export function bindCancelToRun(
  signal: AbortSignal,
  sessionId: string,
  invoke: InvokeFn,
): () => void {
  const abort = () => {
    // cancel_run 置位即返回，无需等待；失败（run 已终态等）
    // 不影响 UI——终态以 runEnded 帧为准。
    void invoke("cancel_run", { sessionId }).catch(() => {});
  };
  if (signal.aborted) {
    abort();
  } else {
    signal.addEventListener("abort", abort, { once: true });
  }
  return () => signal.removeEventListener("abort", abort);
}
