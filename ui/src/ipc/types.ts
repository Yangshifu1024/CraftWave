/**
 * IPC 契约类型（§3.2 共享类型的 TS 侧手工镜像，D14 camelCase）。
 *
 * 本切片只镜像 provider/settings 域（CW schema v2）；会话/消息/运行配置等
 * 类型随各自实现票增补。键面校验由 ui/src/ipc/contract.test.ts 消费
 * src-tauri/target/ipc-manifest.json 完成（§12.3/NFR-12）。
 */

// ---- 供应商（CW schema v2，实现 FR-02-01/06/07）----

export type ApiFormat = "openai_chat" | "anthropic_messages" | "openai_responses";

export type ReasoningEffort = "default" | "low" | "medium" | "high" | "max";

export interface ProviderModel {
  /** uuid，activeModelId 指向它 */
  id: string;
  /** wire id，即显示名 */
  model: string;
  /** default 32768 */
  maxTokens: number;
  /** default 128000 */
  contextWindow: number;
  reasoningEffort?: ReasoningEffort;
  vision?: boolean;
}

export interface ProviderConfig {
  /** uuid */
  id: string;
  name: string;
  apiFormat: ApiFormat;
  /** 缺省按 apiFormat 用官方缺省地址；自定义 OpenAI 兼容端点在此覆写 */
  baseUrl?: string;
  /** keyring 账户引用；config 永不存明文 Key */
  credentialRef?: string;
  /** 展示用 Key 数量 */
  credentialCount: number;
  models: ProviderModel[];
  headers?: Record<string, string>;
}

// ---- 应用配置（实现 FR-14）----

export type ProxyMode = "none" | "system" | "manual";

export interface ProxyConfig {
  mode: ProxyMode;
  url?: string;
}

export interface ApprovalConfig {
  enabled: boolean;
  autoConfirm: boolean;
  commandAllowlist: string[];
}

export type ThemeName = "light" | "dark" | "system";

export type Language = "zh-CN" | "en-US";

export interface UiConfig {
  language: Language;
  theme: ThemeName;
  aiLanguage?: string;
}

export type LogLevel = "debug" | "info" | "warn" | "error";

export interface LogConfig {
  level: LogLevel;
}

export interface AppConfig {
  schemaVersion: 2;
  providers: ProviderConfig[];
  activeModelId?: string;
  proxy?: ProxyConfig;
  approval: ApprovalConfig;
  ui: UiConfig;
  log: LogConfig;
}

// ---- 错误（§3.1：命令失败统一 reject {code, message, detail?}）----

export interface IpcErrorShape {
  code: string;
  message: string;
  detail?: unknown;
}
