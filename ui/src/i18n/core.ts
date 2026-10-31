/**
 * 轻量 i18n 核心（31 号票，FR-13-09 + FR-14-04，NFR-19 门禁的数据面）。
 *
 * 方案选型说明（票面点名 i18next，本票收口为轻量自研，理由见票内决议
 * 记录与设计文档 §10.8 回写）：本项目只有 zh-CN / en-US 两个静态打包
 * 语言，字典全量进 bundle，不需要 i18next 的异步加载/语言检测/插件生态；
 * 票面边界也已排除复数/ICU 高级语法——`{{var}}` 插值够用。自研面收敛
 * 为三件套：字典（按域拆分、zh/en 同构）+ 语言状态（模块级单例 +
 * useSyncExternalStore 订阅）+ 查词函数 t()（回退链 + 插值）。接口形状
 * 对齐 i18next 惯例（t(key, vars)、changeLanguage、`{{var}}` 语法），
 * 后续若需要升级到 i18next，调用面迁移成本最小。
 *
 * 生效链（票面定案）：App 启动读 `config.ui.language` 初始化（useLangBoot，
 * 默认 zh-CN 兜底防裸 key）→ 设置页语言选择即时 setLang + save_config →
 * 首启向导语言步写同字段自动联动。语言切换只改内存状态并通知订阅者，
 * React 组件经 useT() 订阅自动重渲染，即时生效无刷新。
 */

import { useSyncExternalStore } from "react";

// 各域字典（每域一个文件，zh 基准 + en 全量翻译相邻同构：
// `Record<keyof typeof zh, string>` 让键集合一致性在编译期强制）
import { dialogsDomain } from "./domains/dialogs";
import { errorsDomain } from "./domains/errors";
import { inspectorDomain } from "./domains/inspector";
import { onboardingDomain } from "./domains/onboarding";
import { settingsDomain } from "./domains/settings";
import { toastsDomain } from "./domains/toasts";
import { toolcardsDomain } from "./domains/toolcards";
import { workspaceDomain } from "./domains/workspace";

/** 界面语言（与后端 UiConfig.language 的序列化值一致）。 */
export type Lang = "zh-CN" | "en-US";

/** 基准语言：en 包缺键时回退到这里（zh 是抽词源头，永不应缺）。 */
const BASE_LANG: Lang = "zh-CN";

/** 单域字典：域内键 → 文案，按语言各一份。 */
interface DomainDict {
  /** 域名（键前缀，如 "workspace"）。 */
  name: string;
  zh: Record<string, string>;
  en: Record<string, string>;
}

/** 全部域（新增域在此注册——抽词新组件时追加）。 */
const DOMAINS: DomainDict[] = [
  workspaceDomain,
  settingsDomain,
  inspectorDomain,
  toolcardsDomain,
  dialogsDomain,
  toastsDomain,
  onboardingDomain,
  errorsDomain,
];

/** 嵌套域结构 → 扁平键表（"域.键" → 文案），模块加载时各展开一次。 */
function flattenDomain(domain: DomainDict, lang: Lang): Map<string, string> {
  const out = new Map<string, string>();
  for (const [key, text] of Object.entries(lang === "zh-CN" ? domain.zh : domain.en)) {
    out.set(`${domain.name}.${key}`, text);
  }
  return out;
}

/** 各语言扁平键表（查词数据面）：逐域展开合并（后域同键覆盖先域，键带
 *  域前缀本就不应冲突——覆盖仅作防御）。 */
const FLAT: Record<Lang, Map<string, string>> = {
  "zh-CN": new Map<string, string>(),
  "en-US": new Map<string, string>(),
};
for (const domain of DOMAINS) {
  for (const [key, text] of flattenDomain(domain, "zh-CN")) FLAT["zh-CN"].set(key, text);
  for (const [key, text] of flattenDomain(domain, "en-US")) FLAT["en-US"].set(key, text);
}

// ---- 语言状态（模块级单例 + 订阅）----

let currentLang: Lang = BASE_LANG;
const listeners = new Set<() => void>();

/** 当前语言（非组件场景与测试断言用）。 */
export function getLang(): Lang {
  return currentLang;
}

/** 切换语言并通知订阅者（即时生效：组件订阅后自动重渲染）。 */
export function setLang(lang: Lang): void {
  if (lang === currentLang) return;
  currentLang = lang;
  for (const notify of listeners) notify();
}

/** 订阅语言变化（useSyncExternalStore 的 subscribe 面）。 */
export function subscribeLang(notify: () => void): () => void {
  listeners.add(notify);
  return () => listeners.delete(notify);
}

// ---- 查词 ----

/** 插值变量表：文案中的 `{{name}}` 占位按此替换。 */
export type TVars = Record<string, string | number>;

/** 查词函数形态（t / useT 共用）。 */
export type TFn = (key: string, vars?: TVars) => string;

/**
 * 查词：当前语言 → 基准语言 zh-CN 兜底 → key 本身（防裸空白）。
 * 组件内用 useT()（语言切换触发重渲染）；非组件场景（store、纯函数、
 * 事件回调）直接 import 本函数——每次调用查当前语言，切换后的新文案
 * 即新语言（已渲染/已入队的旧文案不追溯，属可接受语义）。
 */
export const t: TFn = (key, vars) => {
  let text = FLAT[currentLang].get(key) ?? FLAT[BASE_LANG].get(key) ?? key;
  if (vars !== undefined) {
    for (const [name, value] of Object.entries(vars)) {
      text = text.replaceAll(`{{${name}}}`, String(value));
    }
  }
  return text;
};

/**
 * 错误码本地化（§3.6 错误面口径，31 号票定案）：errors 域有该码的映射
 * → 本地化文案；无映射 → 回退后端原始 message（不空白）。
 */
export function tError(code: string, fallbackMessage: string, vars?: TVars): string {
  const key = `errors.${code}`;
  const mapped = t(key, vars);
  return mapped === key ? fallbackMessage : mapped;
}

// ---- React 绑定 ----

/**
 * 组件内查词：订阅语言状态，切换即重渲染。返回的 t 是同一个模块级
 * 函数（查词时读当前语言），hook 的作用只是在语言变化时触发重渲染。
 */
export function useT(): TFn {
  useSyncExternalStore(subscribeLang, getLang, getLang);
  return t;
}

/**
 * 启动语言初始化（App.tsx 挂载期调用一次）：读 `config.ui.language`
 * 同步内存态。IPC 失败（纯浏览器环境）保持默认 zh-CN，不阻断启动。
 */
export function initLangFromConfig(language: Lang): void {
  setLang(language);
}
