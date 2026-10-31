/**
 * 应用根组件：负责四件事——
 * 1. 启动期的两个副作用：自绘标题栏激活（useTitlebarActivation，防闪启动）与
 *    主题应用（useThemeBoot，按已保存配置换肤）；
 * 2. 顶层页面装配：多会话工作区（SessionWorkspace）；
 * 3. 全局反馈层：ToastHost（29 号票，顶部轻提示统一出口）；
 * 4. 全局热键：按配置注册系统级唤起/隐藏（29 号票；WebView 重载后重放
 *    挂载逻辑，applyHotkey 幂等）。
 * 调试后门：URL 带 ?mirror=1 时进入镜像工具演示页（MirrorDemo）。
 */

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import { MirrorDemo } from "@/chat/mirror/MirrorDemo";
import { SessionWorkspace } from "@/chat/SessionWorkspace";
import { ToastHost } from "@/components/ToastHost";
import { applyHotkey } from "@/hotkey";
import { applyTheme } from "@/theme";
import { applyUiFontSize } from "@/ui-font-size";

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
    void invoke<{ ui?: { theme?: "light" | "dark" | "system"; fontSize?: number } }>("get_config")
      .then((config) => {
        if (!cancelled) {
          applyTheme(config.ui?.theme ?? "system");
          applyUiFontSize(config.ui?.fontSize);
        }
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
 * 全局热键启动注册（29 号票，FR-13-11）：读配置 ui.hotkey → applyHotkey
 * （enabled=开才注册；冲突错误经 applyHotkey 内部 Toast + store 呈现，
 * 启动路径不重试不阻塞）。与主题同一个 get_config 拉取即可，但热键注册
 * 失败面（Toast/store）已内聚，独立 hook 保持两副作用互不拖累。
 */
function useHotkeyBoot() {
  useEffect(() => {
    let cancelled = false;
    void invoke<{ ui?: { hotkey?: { enabled: boolean; accelerator: string } } }>("get_config")
      .then((config) => {
        if (cancelled) return;
        const hotkey = config.ui?.hotkey;
        if (hotkey) {
          void applyHotkey(hotkey);
        }
      })
      .catch(() => {
        // 纯浏览器/IPC 异常：不注册，设置页打开保存后再试
      });
    return () => {
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

/** 应用根组件：先挂启动期 hook，再按 URL 选择镜像演示页或主界面。 */
export default function App() {
  useTitlebarActivation();
  useThemeBoot();
  useHotkeyBoot();
  // 【S1 spike 原型入口】spike 指验证技术可行性的临时原型：URL 带 ?mirror=1
  // 时切到镜像工具桥接演示页，验证完随时可移除
  if (new URLSearchParams(window.location.search).has("mirror")) {
    return <MirrorDemo />;
  }
  return (
    <>
      <SkeletonApp />
      {/* 顶部 Toast 反馈层（29 号票）：主界面单例（镜像演示页不挂） */}
      <ToastHost />
    </>
  );
}

/**
 * 主界面骨架（M1 多会话工作区挂载点，16 号票）：左栏 / 会话大圆角卡 /
 * 每个会话独立的聊天运行时都在 SessionWorkspace 内部装配；设置页浮层也
 * 由 SessionWorkspace 挂载（入口在左栏底部，17 号票后调整）。
 */
function SkeletonApp() {
  return <SessionWorkspace />;
}
