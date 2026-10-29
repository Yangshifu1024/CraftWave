import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
// Streamdown（流式 Markdown 渲染组件）的结构样式：动画与列表记号等
// 基础规则；配色由 styles.css 里的 .cw-md 段按设计令牌提供
import "streamdown/styles.css";

/**
 * 前端入口：把 React 应用挂载到页面的 #root 节点。
 *
 * 运行环境说明：本项目是 Tauri 桌面应用（Rust 后端 + 网页前端），这份代码
 * 跑在应用内嵌的 WebView（网页视图）里，通过 IPC（Inter-Process
 * Communication，进程间通信，即前端与 Rust 后端之间的调用通道，统一收口
 * 在 src/ipc/client.ts）与后端交互。
 */
const container = document.getElementById("root");
if (!container) {
  // #root 是 index.html 预留的挂载点；找不到说明页面结构被破坏，直接抛错终止
  throw new Error("缺少 #root 挂载点");
}

// 平台 class（React 挂载前执行，styles.css 的 platform-* custom-variant 消费）：
// macOS 交通灯让位 / 窗控排布等平台差异按此区分（对照 ZCode renderer main.tsx:104）
const userAgent = navigator.userAgent;
document.documentElement.classList.toggle("platform-mac-desktop", userAgent.includes("Mac"));
document.documentElement.classList.toggle(
  "platform-windows-desktop",
  userAgent.includes("Windows"),
);
document.documentElement.classList.toggle(
  "platform-linux-desktop",
  !userAgent.includes("Mac") && !userAgent.includes("Windows"),
);

// StrictMode 是 React 的开发期检查模式：会故意重复执行一次渲染与副作用，
// 用来暴露缺清理的订阅、定时器等隐患；生产构建下不会带来额外开销
createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
