import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

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

// StrictMode 是 React 的开发期检查模式：会故意重复执行一次渲染与副作用，
// 用来暴露缺清理的订阅、定时器等隐患；生产构建下不会带来额外开销
createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
