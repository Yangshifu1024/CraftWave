/**
 * 正式镜像工具清单（§10.3/§12.3，14 号票）：defineToolkit 声明 M1 注册表
 * 8 个工具的「后端工具渲染副本」——type:"frontend"、不带 execute（S1 钉死：
 * local runtime 无 execute 管线，结果由 adapter 把 toolResult 帧直写进
 * tool-call part 的 result/isError 驱动状态机）。
 *
 * name/parameters 与 Rust ToolSpec 同源：参数 schema 逐字镜像
 * src-tauri/src/tools/*.rs 的 params_schema，由 contract.test.ts 对照
 * cargo test 产出的 ipc-manifest.json toolSpecs 双向锁定（§12.3）——
 * 原始表 [`CW_TOOL_PARAMETERS`] 单独导出供契约测试比对。
 * suggest 仅 run 尾调用（非模型工具）不声明；ask 落地为极简状态卡
 * （交互在 AskPanel，票面记录）。
 */

import { defineToolkit } from "@assistant-ui/react";
import type { JSONSchema7 } from "json-schema";
import {
  AskCard,
  BatchReadCard,
  CalculateCard,
  CommandCard,
  EditCard,
  GlobCard,
  GrepCard,
  HttpRequestCard,
  ListFilesCard,
  PlanCard,
  ReadCard,
  RenderHtmlCard,
  SkillCard,
  SubagentCard,
  WebFetchCard,
  WebSearchCard,
  WriteCard,
} from "./ToolCards";

/** 参数 schema 原始表（与 Rust params_schema 逐字镜像；契约测试比对面）。 */
export const CW_TOOL_PARAMETERS: Record<string, JSONSchema7> = {
  read: {
    type: "object",
    properties: {
      path: { type: "string", description: "工作区内相对路径（或绝对路径，须在 roots 内）" },
      offset: { type: "integer", description: "起始行号（1-based）" },
      limit: { type: "integer", description: "返回行数，默认 2000" },
      kind: { type: "string", enum: ["text", "image"] },
    },
    required: ["path"],
    additionalProperties: false,
  },
  write: {
    type: "object",
    properties: {
      path: { type: "string", description: "工作区内目标路径" },
      content: { type: "string", description: "完整文件内容（UTF-8）" },
    },
    required: ["path", "content"],
    additionalProperties: false,
  },
  edit: {
    type: "object",
    properties: {
      path: { type: "string", description: "工作区内目标路径" },
      oldText: { type: "string", description: "待替换的原文（须唯一命中）" },
      newText: { type: "string", description: "替换后的文本（空串即删除）" },
      version: {
        type: "string",
        description: "sha256 前 16 hex 乐观锁；不匹配时报版本过期（可唯一匹配则降级警告执行）",
      },
    },
    required: ["path", "oldText", "newText"],
    additionalProperties: false,
  },
  list_files: {
    type: "object",
    properties: {
      path: { type: "string", description: "目标目录，缺省为项目根" },
      depth: { type: "integer", description: "最大深度" },
      respectGitignore: { type: "boolean", description: "是否尊重 .gitignore，默认 true" },
    },
    additionalProperties: false,
  },
  grep: {
    type: "object",
    properties: {
      pattern: { type: "string", description: "Rust regex 正则" },
      path: { type: "string", description: "搜索起点，缺省项目根" },
      glob: { type: "string", description: '文件名过滤，如 "*.rs"' },
      maxResults: { type: "integer", description: "命中上限，默认 100，最大 1000" },
    },
    required: ["pattern"],
    additionalProperties: false,
  },
  command: {
    type: "object",
    properties: {
      command: { type: "string" },
      timeoutSec: { type: "integer", minimum: 1, maximum: 600 },
      cwd: { type: "string" },
    },
    required: ["command"],
    additionalProperties: false,
  },
  plan: {
    type: "object",
    properties: {
      action: { type: "string", enum: ["set", "update"] },
      items: {
        type: "array",
        items: {
          type: "object",
          properties: {
            id: { type: "string" },
            text: { type: "string" },
            status: { type: "string", enum: ["pending", "in_progress", "completed"] },
          },
          required: ["id", "text", "status"],
          additionalProperties: false,
        },
      },
    },
    required: ["action"],
    additionalProperties: false,
  },
  ask: {
    type: "object",
    properties: {
      title: { type: "string" },
      questions: {
        type: "array",
        items: {
          type: "object",
          properties: {
            id: { type: "string" },
            text: { type: "string" },
            options: {
              type: "array",
              items: {
                type: "object",
                properties: {
                  id: { type: "string" },
                  label: { type: "string" },
                },
                required: ["id", "label"],
                additionalProperties: false,
              },
            },
            allowFreeText: { type: "boolean" },
          },
          required: ["id", "text", "allowFreeText"],
          additionalProperties: false,
        },
      },
    },
    required: ["title", "questions"],
    additionalProperties: false,
  },
  // skill 工具（§8.3 M2 表 Meta 行，09 号票；与 knowledge/skills.rs 的
  // skill_spec params_schema 逐字镜像，contract.test 双向锁定）
  skill: {
    type: "object",
    properties: {
      name: { type: "string", description: "技能名（系统提示清单中的名称）" },
      args: { type: "string", description: "可选的自由参数，透传给技能语境" },
    },
    required: ["name"],
    additionalProperties: false,
  },
  // subagent 工具（§8.3 M2 表 Meta 行，10 号票；与 agent/roles.rs 的
  // subagent_spec params_schema 逐字镜像，contract.test 双向锁定）
  subagent: {
    type: "object",
    properties: {
      role: {
        type: "string",
        enum: [
          "explore",
          "planner",
          "implementer",
          "reviewer",
          "tester",
          "frontend-dev",
          "backend-dev",
        ],
        description: "角色名（见 description 清单）",
      },
      task: {
        type: "string",
        description: "完整自足的任务描述（子代理无本对话历史）",
      },
      budgetSteps: {
        type: "integer",
        minimum: 1,
        maximum: 1000,
        description: "步数预算（模型请求轮次上限），缺省按角色默认",
      },
    },
    required: ["role", "task"],
    additionalProperties: false,
  },
  // web_fetch 工具（§8.3 M2 表 Network 行，12 号票；与 tools/web_fetch.rs
  // 的 params_schema 逐字镜像，contract.test 双向锁定）
  web_fetch: {
    type: "object",
    properties: {
      url: { type: "string", description: "目标网页地址（http/https）" },
      format: {
        type: "string",
        enum: ["readability", "raw"],
        description: "输出模式：readability=正文 Markdown（默认）；raw=清洗后的原始 HTML",
      },
    },
    required: ["url"],
    additionalProperties: false,
  },
  // http_request 工具（§8.3 M2 表 Network 行，12 号票；与 tools/http_request.rs
  // 的 params_schema 逐字镜像，contract.test 双向锁定）
  http_request: {
    type: "object",
    properties: {
      method: {
        type: "string",
        enum: ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
        description: "HTTP 方法",
      },
      url: { type: "string", description: "目标地址（http/https）" },
      headers: {
        type: "object",
        additionalProperties: { type: "string" },
        description: "自定义请求头（跨源重定向时 Authorization/Cookie 等敏感头自动剥除）",
      },
      body: { type: "string", description: "请求体文本（GET/HEAD 忽略）" },
    },
    required: ["method", "url"],
    additionalProperties: false,
  },
  // web_search 工具（§8.3 M2 表 Network 行，13 号票；与 tools/web_search.rs
  // 的 params_schema 逐字镜像，contract.test 双向锁定）
  web_search: {
    type: "object",
    properties: {
      query: { type: "string", description: "搜索关键词" },
      limit: {
        type: "integer",
        minimum: 1,
        maximum: 30,
        description: "返回条数上限（默认 10）",
      },
    },
    required: ["query"],
    additionalProperties: false,
  },
  // batch_read 工具（§8.3 M2 表 ReadOnly 行，14 号票；与 tools/batch_read.rs
  // 的 params_schema 逐字镜像，contract.test 双向锁定）
  batch_read: {
    type: "object",
    properties: {
      paths: {
        type: "array",
        items: { type: "string" },
        description: "要读取的文件路径列表（工作区内相对路径或绝对路径），按源序读取",
      },
    },
    required: ["paths"],
    additionalProperties: false,
  },
  // glob 工具（§8.3 M2 表 ReadOnly 行，14 号票；与 tools/glob.rs 逐字镜像）
  glob: {
    type: "object",
    properties: {
      pattern: { type: "string", description: '文件名模式，如 "*.rs" 或 "**/*.rs"' },
      path: { type: "string", description: "遍历起点，缺省项目根" },
    },
    required: ["pattern"],
    additionalProperties: false,
  },
  // calculate 工具（§8.3 M2 表 ReadOnly 行，14 号票；与 tools/calculate.rs
  // 逐字镜像）
  calculate: {
    type: "object",
    properties: {
      expression: {
        type: "string",
        description: '算术表达式，如 "(1 + 2) * 3.5"、"2^10"、"pi * 2"',
      },
    },
    required: ["expression"],
    additionalProperties: false,
  },
  // render_html 工具（§8.3 M2 表 ReadOnly* 行，14 号票；与 tools/render_html.rs
  // 逐字镜像）
  render_html: {
    type: "object",
    properties: {
      html: {
        type: "string",
        description: "完整的自包含 HTML 文档（含 <!DOCTYPE html> 与内联样式/脚本）",
      },
    },
    required: ["html"],
    additionalProperties: false,
  },
};

/**
 * 正式镜像工具注册表：8 个 M1 工具各配一张前端渲染卡（镜像工具卡——
 * 只负责把后端工具调用过程渲染出来，不在前端执行任何工具逻辑）。
 * 经 SessionWorkspace 的 AuiConfig(tools) 装配进各会话的聊天渲染管线。
 */
export const cwToolkit = defineToolkit({
  read: {
    type: "frontend",
    description: "读取工作区文件",
    parameters: CW_TOOL_PARAMETERS.read,
    render: ReadCard,
  },
  write: {
    type: "frontend",
    description: "新建文件",
    parameters: CW_TOOL_PARAMETERS.write,
    render: WriteCard,
  },
  edit: {
    type: "frontend",
    description: "编辑文件",
    parameters: CW_TOOL_PARAMETERS.edit,
    render: EditCard,
  },
  list_files: {
    type: "frontend",
    description: "列出目录树",
    parameters: CW_TOOL_PARAMETERS.list_files,
    render: ListFilesCard,
  },
  grep: {
    type: "frontend",
    description: "正则搜索",
    parameters: CW_TOOL_PARAMETERS.grep,
    render: GrepCard,
  },
  command: {
    type: "frontend",
    description: "执行 shell 命令",
    parameters: CW_TOOL_PARAMETERS.command,
    render: CommandCard,
  },
  plan: {
    type: "frontend",
    description: "维护任务计划",
    parameters: CW_TOOL_PARAMETERS.plan,
    render: PlanCard,
  },
  ask: {
    type: "frontend",
    description: "向用户提问（交互在 AskPanel）",
    parameters: CW_TOOL_PARAMETERS.ask,
    render: AskCard,
  },
  skill: {
    type: "frontend",
    description: "加载技能的完整操作指引（正文按需进上下文）",
    parameters: CW_TOOL_PARAMETERS.skill,
    render: SkillCard,
  },
  subagent: {
    type: "frontend",
    description: "委派子代理执行独立任务（过程实时可见，汇报回父上下文）",
    parameters: CW_TOOL_PARAMETERS.subagent,
    render: SubagentCard,
  },
  web_fetch: {
    type: "frontend",
    description: "抓取网页并提取正文（Readability，Markdown）",
    parameters: CW_TOOL_PARAMETERS.web_fetch,
    render: WebFetchCard,
  },
  http_request: {
    type: "frontend",
    description: "发送通用 HTTP 请求并返回响应",
    parameters: CW_TOOL_PARAMETERS.http_request,
    render: HttpRequestCard,
  },
  web_search: {
    type: "frontend",
    description: "联网搜索（本地 SearXNG / 内置抓取 / Tavily 三通道自动选）",
    parameters: CW_TOOL_PARAMETERS.web_search,
    render: WebSearchCard,
  },
  batch_read: {
    type: "frontend",
    description: "一次读取多个文本文件的开头窗口（共享 512KB 响应预算）",
    parameters: CW_TOOL_PARAMETERS.batch_read,
    render: BatchReadCard,
  },
  glob: {
    type: "frontend",
    description: "按文件名模式检索工作区文件（尊重 .gitignore）",
    parameters: CW_TOOL_PARAMETERS.glob,
    render: GlobCard,
  },
  calculate: {
    type: "frontend",
    description: "精确算术求值（防心算出错）",
    parameters: CW_TOOL_PARAMETERS.calculate,
    render: CalculateCard,
  },
  render_html: {
    type: "frontend",
    description: "产出自包含 HTML 供沙箱预览卡查看效果",
    parameters: CW_TOOL_PARAMETERS.render_html,
    render: RenderHtmlCard,
  },
});
