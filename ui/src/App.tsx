import { AssistantRuntimeProvider, AuiConfig, Tools, useLocalRuntime } from "@assistant-ui/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { AskPanel } from "@/chat/AskPanel";
import { useBackendEvents } from "@/chat/event-bridge";
import { MirrorDemo } from "@/chat/mirror/MirrorDemo";
import { LOCAL_SESSION_ID, tauriChatModel } from "@/chat/runtime";
import { ChatThread } from "@/chat/Thread";
import { cwToolkit } from "@/chat/toolkit";
import { loadedTreeToExported } from "@/chat/tree-persistence";
import { SettingsPage } from "@/settings/SettingsPage";
import { loadHistory } from "@/stores/session";

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

// 正式工具卡装配（S1 实测钉死：Tools 是 resource 工厂，经 AuiConfig 注入；
// config 为纯数据可提模块级）。
const cwAuiConfig = AuiConfig({ tools: Tools({ toolkit: cwToolkit }) });

/**
 * M1 核心链路聊天（tauriChatModel：start_chat → Channel 帧 → assistant-ui）。
 * 14 号票：注入正式镜像工具卡（§10.3 Tools resource）+ AskPanel + 事件桥。
 * 15 号票：启动收养 canonical 历史（load → import；新会话无史跳过）。
 */
function SkeletonApp() {
  const runtime = useLocalRuntime(tauriChatModel);
  const [settingsOpen, setSettingsOpen] = useState(false);
  useBackendEvents();
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const tree = await loadHistory(LOCAL_SESSION_ID);
        if (cancelled || tree === null) return;
        runtime.thread.import(loadedTreeToExported(tree));
      } catch (err) {
        console.warn("[session] 历史收养失败（以空会话继续）", err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [runtime]);
  return (
    <AssistantRuntimeProvider runtime={runtime} config={cwAuiConfig}>
      <ChatThread />
      {!settingsOpen && (
        <button
          type="button"
          className="fixed top-3 right-3 z-40 rounded border border-neutral-300 bg-white px-3 py-1 text-sm"
          onClick={() => setSettingsOpen(true)}
        >
          设置
        </button>
      )}
      {settingsOpen && <SettingsPage onClose={() => setSettingsOpen(false)} />}
      <AskPanel />
    </AssistantRuntimeProvider>
  );
}
