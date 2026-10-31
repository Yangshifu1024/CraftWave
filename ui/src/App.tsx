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
import { initLangFromConfig, t } from "@/i18n/core";
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
 * 31 号票同通道追加：界面语言初始化（config.ui.language → i18n 内存态，
 * zh-CN 兜底）。
 */
function useThemeBoot() {
  useEffect(() => {
    let cancelled = false;
    void invoke<{
      ui?: {
        theme?: "light" | "dark" | "system";
        fontSize?: number;
        language?: "zh-CN" | "en-US";
      };
    }>("get_config")
      .then((config) => {
        if (!cancelled) {
          applyTheme(config.ui?.theme ?? "system");
          applyUiFontSize(config.ui?.fontSize);
          if (config.ui?.language !== undefined) {
            initLangFromConfig(config.ui.language);
          }
        }
      })
      .catch(() => {
        // 纯浏览器/IPC 异常：维持默认 system 主题与 zh-CN 语言
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

/**
 * 启动静默更新检查（33 号票，FR-16-03「检查可开关可调频率」）：
 * App 挂载后按配置判定——`update.enabled=false` 不发起任何请求（NFR-11）；
 * updater pubkey 未实配（HITL 密钥未配，占位空串）同样不发起（入口兜底门
 * 同源）；距上次静默检查不足 intervalHours 小时跳过（记账在 ui-state 的
 * lastUpdateCheckAt，本轮检查完成后立即记账）。检查到新版仅 Toast 轻提示
 * 引导到设置→关于（静默语义：不打断启动流程，不自动下载）；失败静默。
 */
function useSilentUpdateCheck() {
  useEffect(() => {
    let cancelled = false;
    void invoke<{
      update?: { enabled?: boolean; intervalHours?: number };
    }>("get_config")
      .then(async (config) => {
        if (cancelled) return;
        const update = config.update;
        if (!update?.enabled) return;
        // pubkey 门：与设置页入口同一兜底（update_status 命令，33 号票 #59m）
        const status = await invoke<{
          pubkeyConfigured: boolean;
          endpoints: string[];
        }>("update_status");
        if (cancelled || !status.pubkeyConfigured) return;
        const intervalHours = update.intervalHours ?? 24;
        const { shouldSilentCheck } = await import("@/ipc/updater");
        const sessions = await import("@/stores/sessions");
        const store = sessions.useSessionsStore.getState();
        if (!shouldSilentCheck(Date.now(), store.lastUpdateCheckAt, intervalHours, true)) {
          return;
        }
        store.setLastUpdateCheckAt(Date.now());
        const { checkForUpdate } = await import("@/ipc/updater");
        const state = await checkForUpdate();
        if (cancelled) return;
        if (state.kind === "available") {
          const { pushToast } = await import("@/stores/toast");
          pushToast(
            "info",
            t("settings.updateSilentFound", { version: state.version }),
            t("settings.updateSilentHint"),
          );
        }
      })
      .catch(() => {
        // 静默检查任何失败（纯浏览器/网络/配置）都不打扰用户
      });
    return () => {
      cancelled = true;
    };
  }, []);
}

/** 应用根组件：先挂启动期 hook，再按 URL 选择镜像演示页或主界面。 */
export default function App() {
  useTitlebarActivation();
  useThemeBoot();
  useHotkeyBoot();
  useSilentUpdateCheck();
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
