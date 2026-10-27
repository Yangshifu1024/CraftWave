import { AssistantRuntimeProvider, useLocalRuntime } from "@assistant-ui/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import { MirrorDemo } from "@/chat/mirror/MirrorDemo";
import { echoAdapter } from "@/chat/runtime";
import { ChatThread } from "@/chat/Thread";

const ACTIVATION_FLAG = "data-tauri-plugin-decoration-active";
const ACTIVATION_TIMEOUT_MS = 5000;

/**
 * 标题栏激活协议（decoration 插件要求，兼作 §2.1 防闪启动 + 亮窗看门狗）：
 * 窗口以 visible:false 启动 → 调 activate_custom_titlebar →
 * 等插件在 documentElement 打上激活标志 → 亮窗；
 * 5s 超时 → 退化系统标题栏 → 无条件亮窗。
 */
function useTitlebarActivation() {
  useEffect(() => {
    const win = getCurrentWindow();
    let shown = false;
    const showOnce = () => {
      if (shown) return;
      shown = true;
      void win.show();
      void win.setFocus();
    };
    const activate = async () => {
      try {
        await invoke("activate_custom_titlebar");
      } catch {
        // 激活失败：走看门狗退系统标题栏
      }
      const root = document.documentElement;
      if (root.hasAttribute(ACTIVATION_FLAG)) {
        showOnce();
        return;
      }
      const observer = new MutationObserver(() => {
        if (root.hasAttribute(ACTIVATION_FLAG)) {
          observer.disconnect();
          showOnce();
        }
      });
      observer.observe(root, { attributes: true, attributeFilter: [ACTIVATION_FLAG] });
      window.setTimeout(async () => {
        if (shown) return;
        observer.disconnect();
        try {
          await invoke("restore_native_titlebar");
        } catch {
          // 恢复失败也照样亮窗
        }
        showOnce();
      }, ACTIVATION_TIMEOUT_MS);
    };
    void activate();
  }, []);
}

export default function App() {
  useTitlebarActivation();
  // 【S1 spike 原型入口】?mirror=1 切到镜像工具桥接 demo，随时可弃
  if (new URLSearchParams(window.location.search).has("mirror")) {
    return <MirrorDemo />;
  }
  return <SkeletonApp />;
}

/** S6 骨架回声聊天（原型期间原样保留）。 */
function SkeletonApp() {
  const runtime = useLocalRuntime(echoAdapter);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatThread />
    </AssistantRuntimeProvider>
  );
}
