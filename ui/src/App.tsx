import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { MirrorDemo } from "@/chat/mirror/MirrorDemo";
import { SessionWorkspace } from "@/chat/SessionWorkspace";
import { SettingsPage } from "@/settings/SettingsPage";
import { applyTheme } from "@/theme";

const ACTIVATION_FLAG = "data-tauri-plugin-decoration-active";
const ACTIVATION_TIMEOUT_MS = 5000;

/** 启动主题应用（FR-13-03）：读 config.ui.theme（IPC 失败时保持 system）。 */
function useThemeBoot() {
  useEffect(() => {
    let cancelled = false;
    void invoke<{ ui?: { theme?: "light" | "dark" | "system" } }>("get_config")
      .then((config) => {
        if (!cancelled) applyTheme(config.ui?.theme ?? "system");
      })
      .catch(() => {
        // 纯浏览器/IPC 异常：维持默认 system
      });
    return () => {
      cancelled = true;
    };
  }, []);
}

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
  useThemeBoot();
  // 【S1 spike 原型入口】?mirror=1 切到镜像工具桥接 demo，随时可弃
  if (new URLSearchParams(window.location.search).has("mirror")) {
    return <MirrorDemo />;
  }
  return <SkeletonApp />;
}

/**
 * M1 多会话工作区挂载点（16 号票）：左栏/Tab/每会话独立 runtime 在
 * SessionWorkspace 内装配；设置页浮层保留在此层。
 */
function SkeletonApp() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  return (
    <>
      <SessionWorkspace />
      {!settingsOpen && (
        <button
          type="button"
          className="fixed top-3 right-3 z-40 rounded border border-[var(--border)] bg-[var(--card)] px-3 py-1 text-sm text-[var(--foreground)]"
          onClick={() => setSettingsOpen(true)}
        >
          设置
        </button>
      )}
      {settingsOpen && <SettingsPage onClose={() => setSettingsOpen(false)} />}
    </>
  );
}
