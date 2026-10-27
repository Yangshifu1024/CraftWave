/**
 * 应用根组件：负责三件事——
 * 1. 启动期的两个副作用：自绘标题栏激活（useTitlebarActivation，防闪启动）与
 *    主题应用（useThemeBoot，按已保存配置换肤）；
 * 2. 顶层页面装配：多会话工作区（SessionWorkspace）+ 设置页全屏浮层（SettingsPage）；
 * 3. 调试后门：URL 带 ?mirror=1 时进入镜像工具演示页（MirrorDemo）。
 */

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { MirrorDemo } from "@/chat/mirror/MirrorDemo";
import { SessionWorkspace } from "@/chat/SessionWorkspace";
import { SettingsPage } from "@/settings/SettingsPage";
import { applyTheme } from "@/theme";

/** decoration 插件激活成功后在 <html> 上打的标志属性名（出现即表示自绘标题栏就绪）。 */
const ACTIVATION_FLAG = "data-tauri-plugin-decoration-active";
/** 看门狗超时：超过 5 秒自绘标题栏仍未激活，就退回系统标题栏并强制亮窗。 */
const ACTIVATION_TIMEOUT_MS = 5000;

/**
 * 启动主题应用（FR-13-03）：通过 IPC 读配置里的 ui.theme 并应用。
 * invoke 是 Tauri 的 IPC 调用函数（前端请求 Rust 后端执行命令）；IPC 失败
 * （例如纯浏览器环境）时保持默认 system 主题，不阻断启动。
 */
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
      // 组件卸载后不再应用结果（避免竞态写主题）
      cancelled = true;
    };
  }, []);
}

/**
 * 标题栏激活协议（decoration 插件要求；兼作防闪启动 + 亮窗看门狗）：
 *
 * 背景：应用使用自绘标题栏（窗口顶栏由网页绘制而非系统绘制）。窗口必须以
 * 隐藏状态启动，等自绘标题栏就绪后再显示，否则会先闪一下系统默认标题栏
 * （"防闪启动"即指这个）。看门狗（watchdog）指超时兜底逻辑：主路径没在
 * 期限内完成就走备用路径。
 *
 * 流程：
 * 1. 窗口以 visible:false（隐藏）启动；
 * 2. 调用后端 activate_custom_titlebar 激活自绘标题栏；
 * 3. 用 MutationObserver 盯 <html>：插件激活成功会打上标志属性，属性出现
 *    即就绪 → 显示窗口（showOnce 保证只亮一次）；
 * 4. 看门狗：5 秒（ACTIVATION_TIMEOUT_MS）仍未就绪 → 调 restore_native_titlebar
 *    退回系统标题栏，并无条件亮窗——宁可标题栏朴素也不能永远黑屏。
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

/** 应用根组件：先挂两个启动期 hook，再按 URL 选择镜像演示页或主界面。 */
export default function App() {
  useTitlebarActivation();
  useThemeBoot();
  // 【S1 spike 原型入口】spike 指验证技术可行性的临时原型：URL 带 ?mirror=1
  // 时切到镜像工具桥接演示页，验证完随时可移除
  if (new URLSearchParams(window.location.search).has("mirror")) {
    return <MirrorDemo />;
  }
  return <SkeletonApp />;
}

/**
 * 主界面骨架（M1 多会话工作区挂载点，16 号票）：左栏 / Tab 条 / 每个会话
 * 独立的聊天运行时都在 SessionWorkspace 内部装配；设置页以全屏浮层形式挂在
 * 本层，由右上角"设置"按钮唤起。
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
