/**
 * 【S1 spike 原型，随时可弃】镜像工具桥接 demo 页。
 *
 * `?mirror=1` 时替换 S6 骨架聊天窗。发送任意消息即触发 mock 剧本：
 * 文本 → 两个镜像工具（参数准备中 → 运行中 → 完成/错误）→ 收尾文本。
 * `<Tools toolkit>` 负责把渲染器注册进消息渲染管线（Thread.tsx 的
 * MessagePrimitive.Parts children 对 tool-call 返回 null 时自动 fallback 到注册渲染器）。
 */
import { AssistantRuntimeProvider, AuiConfig, Tools, useLocalRuntime } from "@assistant-ui/react";
import { ChatThread } from "../Thread";
import { resetMountLog } from "./MirrorCards";
import { mirrorAdapter } from "./mirrorRuntime";
import { mirrorToolkit } from "./toolkit";

// 进入原型页时清一次挂载日志（页面级，不在渲染体内做副作用）
resetMountLog();

// 0.15.31 实测：Tools 是 tap resource 工厂，不能当 JSX 元素用；
// 官方姿势是 AuiConfig({ tools: Tools({ toolkit }) }) 经 config 传入 Provider。
// config 是纯数据，可提模块级（文档明示 provider 不依赖 config 身份）。
const mirrorAuiConfig = AuiConfig({ tools: Tools({ toolkit: mirrorToolkit }) });

/** 镜像 demo 页：镜像 adapter + 镜像工具注册表包住正式 ChatThread，
 *  发任意消息即播放 mock 剧本。 */
export function MirrorDemo() {
  const runtime = useLocalRuntime(mirrorAdapter);
  return (
    <AssistantRuntimeProvider runtime={runtime} config={mirrorAuiConfig}>
      <ChatThread />
    </AssistantRuntimeProvider>
  );
}
