/**
 * 【S1 spike 原型，随时可弃】镜像工具清单：defineToolkit 声明的「后端工具渲染副本」。
 *
 * 与设计文档 §10.3 的对照（0.15.31 实测差异，详见 spike 报告）：
 * - defineToolkit 真实存在（@assistant-ui/react 0.15.31 导出），条目形状
 *   { type, description, parameters, render } 与设计假设一致；
 * - 但 **execute 桥接在 local runtime 不成立**：0.15.31 里负责调 execute 的
 *   ToolInvocationTracker 只接在 external-store runtime 上，useLocalRuntime 路径
 *   永远不会触发 execute。故镜像工具不带 execute，结果由 adapter 把 Rust
 *   tool-result 帧直接写进 tool-call part 的 result 字段驱动状态机；
 * - makeAssistantToolUI / useAssistantToolUI 已标 @deprecated（官方迁移指引：
 *   render 放 toolkit 条目上），设计文档若引用旧 API 需要更新。
 * - parameters 用 JSONSchema7 字面量（与 Rust ipc-manifest.json 同源校验属 §12.3）。
 */
import { defineToolkit } from "@assistant-ui/react";
import { ReadWorkspaceFileCard, RunCommandCard } from "./MirrorCards";

/** 镜像工具注册表：read_workspace_file 与 run_command 两个镜像声明条目
 *  （前端只渲染，不带 execute，见文件头说明）。 */
export const mirrorToolkit = defineToolkit({
  read_workspace_file: {
    type: "frontend",
    description: "（镜像声明）读取工作区文件——真实执行在 Rust 后端，前端只渲染。",
    parameters: {
      type: "object",
      properties: {
        path: { type: "string", description: "工作区相对路径" },
        startLine: { type: "number" },
      },
      required: ["path"],
    },
    // 不写 execute：见文件头注释。类型上 frontend 允许 execute 缺省。
    render: ReadWorkspaceFileCard,
  },
  run_command: {
    type: "frontend",
    description: "（镜像声明）在工作区执行命令——真实执行在 Rust 后端，前端只渲染。",
    parameters: {
      type: "object",
      properties: {
        command: { type: "string" },
      },
      required: ["command"],
    },
    render: RunCommandCard,
  },
});
