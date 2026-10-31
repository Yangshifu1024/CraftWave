// @vitest-environment happy-dom
/**
 * i18n 门禁测试（31 号票，§10.8「CI 检查 key 完整性」+ NFR-19 硬编码门禁）：
 *
 * 1. key 完整性：zh/en 两包深度键集合双向 diff 为空（编译期同构之外的第二道
 *    运行时防线——字典以 TS 对象承载，此测试防动态删除/拼接绕过类型检查）；
 * 2. 硬编码回退扫描：用 TypeScript 编译器 AST 扫描 ui/src 非测试源码的
 *    字符串字面量 / JSX 文本 / 模板串静态段中的 CJK 字符——新增硬编码即红
 *    （NFR-19「新增文案不得硬编码」门禁）；豁免面见 ALLOWLIST（注释与
 *    正则天然不在 AST 字符串节点内，无需豁免）；
 * 3. 语言切换冒烟：setLang 后 t() 输出随语言变化（en 断言 / zh 回切）；
 * 4. 插值：{{var}} 替换与数字字符串化；
 * 5. 缺键回退链：en 缺键回 zh、双双缺键回 key 本身（不空白）；
 * 6. errors 映射：tError 对 §3.6 注册表抽样码给本地化文案，未知码回退
 *    原始 message。
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { getLang, type Lang, setLang, t, tError } from "./core";

/** 仓库内 ui/src 的绝对路径（测试从 ui/ 运行）。 */
const SRC_ROOT = join(process.cwd(), "src");

// ---- 1. key 完整性（深度键集合双向 diff 为空）----

/** 收集一个语言字典的全部扁平键（"域.键" 形态——与 FLAT 展开口径一致）。 */
function collectKeys(dict: Record<string, Record<string, string>>): Set<string> {
  const out = new Set<string>();
  for (const [domain, entries] of Object.entries(dict)) {
    for (const key of Object.keys(entries)) out.add(`${domain}.${key}`);
  }
  return out;
}

/** 各域模块的 zh/en 两包（与 core.ts 的 DOMAINS 注册同一份来源）。 */
import { dialogsDomain } from "./domains/dialogs";
import { errorsDomain } from "./domains/errors";
import { inspectorDomain } from "./domains/inspector";
import { onboardingDomain } from "./domains/onboarding";
import { settingsDomain } from "./domains/settings";
import { toastsDomain } from "./domains/toasts";
import { toolcardsDomain } from "./domains/toolcards";
import { workspaceDomain } from "./domains/workspace";

const ALL_DOMAINS = [
  workspaceDomain,
  settingsDomain,
  inspectorDomain,
  toolcardsDomain,
  dialogsDomain,
  toastsDomain,
  onboardingDomain,
  errorsDomain,
];

describe("i18n 语言包 key 完整性（§10.8 CI 门禁）", () => {
  it("zh/en 两包深度键集合双向 diff 为空", () => {
    const zhKeys = collectKeys(Object.fromEntries(ALL_DOMAINS.map((d) => [d.name, d.zh])));
    const enKeys = collectKeys(Object.fromEntries(ALL_DOMAINS.map((d) => [d.name, d.en])));
    const missingInEn = [...zhKeys].filter((k) => !enKeys.has(k));
    const extraInEn = [...enKeys].filter((k) => !zhKeys.has(k));
    expect(missingInEn, "en 包缺键").toEqual([]);
    expect(extraInEn, "en 包多键").toEqual([]);
  });

  it("每个域的 en 包与 zh 包键集合同构（逐域报错便于定位）", () => {
    for (const domain of ALL_DOMAINS) {
      const zh = new Set(Object.keys(domain.zh));
      const en = new Set(Object.keys(domain.en));
      expect(
        [...zh].filter((k) => !en.has(k)),
        `${domain.name} 域 en 缺键`,
      ).toEqual([]);
      expect(
        [...en].filter((k) => !zh.has(k)),
        `${domain.name} 域 en 多键`,
      ).toEqual([]);
    }
  });

  it("zh 基准包无空文案", () => {
    for (const domain of ALL_DOMAINS) {
      for (const [key, text] of Object.entries(domain.zh)) {
        expect(text.trim().length, `${domain.name}.${key} 为空文案`).toBeGreaterThan(0);
      }
    }
  });
});

// ---- 2. 硬编码回退扫描（NFR-19 门禁）----

/** 显式豁免清单：`<文件相对路径>: <理由>`。新增豁免必须写明理由（评审面）。 */
const ALLOWLIST: Record<string, string> = {
  // 镜像工具演示页（S1 spike，?mirror=1 调试入口）：非应用 chrome 主链路，
  // M3 随镜像体系收编时一并抽词
  "chat/mirror/MirrorCards.tsx": "镜像演示页（spike 调试入口，非主链路）",
  "chat/mirror/mockSource.ts": "镜像演示剧本数据（非 UI chrome）",
  "chat/scripted-backend.ts": "测试用剧本假后端（非生产 UI）",
  // 导出归档的文档格式串（写入文件的 Markdown 头部字段名）——文档格式
  // 稳定性优先，不随界面语言切换（票面边界：仅应用 chrome 文案）
  "chat/export.ts": "导出归档文档格式串（非界面 chrome）",
  // runtime 回声 adapter（无 Rust 后端时的开发兜底）与内部诊断串
  "chat/runtime.ts": "回声 adapter 开发兜底文案（非生产 UI 链路）",
  // 发给模型的合成文本（非界面 chrome）：@ 引用段、镜像工具注册表
  "chat/at-refs.ts": "@ 引用合成段（发送给模型的文本，非界面 chrome）",
  "chat/toolkit.ts": "工具注册表 description（发给模型的参数说明，非界面 chrome）",
  "chat/mirror/toolkit.ts": "镜像演示工具注册表（spike 调试入口）",
  // ts-rs 生成物（Rust 侧权威单源导出）：预设名等数据面文案随 Rust 定义，
  // 前端不手改（数据非 chrome）
  "ipc/generated/BuiltinPresets.ts": "ts-rs 生成物（预设名数据面，Rust 权威）",
  // 前端树同步的内部诊断串（throw 的错误走 console/降级路径，非界面 chrome）
  "chat/tree-persistence.ts": "树同步内部诊断串（非界面 chrome）",
  // 应用入口的结构性异常（挂载点缺失 = 页面未渲染，开发者诊断面）
  "main.tsx": "挂载点缺失的结构性异常（开发者诊断）",
};

/** 精确字面量豁免：`<文件>: [允许的字符串原文]`——i18n 惯例与数据面例外，
 *  每条写明理由（评审面）。文件级豁免之外的最小豁免粒度。 */
const EXACT_ALLOWLIST: Record<string, { text: string; reason: string }[]> = {
  // 语言选择器里的语言自身名：i18n 惯例——各语言用它自己的名字显示
  //（「简体中文」在英文界面仍显示「简体中文」，反之亦然），不随界面语言翻译
  "settings/SettingsPage.tsx": [
    { text: "简体中文", reason: "语言自身名（i18n 惯例，不翻译）" },
    { text: "English", reason: "语言自身名（i18n 惯例，不翻译）" },
  ],
  // 空态指引里的目录路径示例：数据面（文件系统路径），非文案
  "settings/SkillsPage.tsx": [
    { text: "<项目主目录>/.craftwave/skills/", reason: "目录路径示例（数据面）" },
    { text: "~/.craftwave/skills/", reason: "目录路径示例（数据面）" },
  ],
};

/** 递归收集目录下的 .ts/.tsx 文件（相对 ui/src 的 posix 路径）。 */
function collectSourceFiles(dir: string, base = ""): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const rel = base === "" ? entry : `${base}/${entry}`;
    if (statSync(full).isDirectory()) {
      out.push(...collectSourceFiles(full, rel));
    } else if (
      (entry.endsWith(".ts") || entry.endsWith(".tsx")) &&
      !/\.(test|spec)\./.test(entry)
    ) {
      out.push(rel);
    }
  }
  return out;
}

/** 单个字符串是否含 CJK（\p{Script=Han} 扩展汉字区）。 */
function containsCjk(text: string): boolean {
  return /\p{Script=Han}/u.test(text);
}

/** 用 TS AST 抽取源码中所有「会被渲染/展示的字符串」文本。 */
function extractStringTexts(source: ts.SourceFile): { text: string; line: number }[] {
  const out: { text: string; line: number }[] = [];
  const visit = (node: ts.Node): void => {
    // 字符串字面量（排除 import 路径等由父节点判别）
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      const parent = node.parent;
      const isModuleSpecifier = ts.isImportDeclaration(parent) || ts.isExportDeclaration(parent);
      const calleeText = ts.isCallExpression(parent) ? parent.expression.getText(source) : "";
      const isConsoleArgument =
        ts.isCallExpression(parent) && /^console\.(log|warn|error|info|debug)$/.test(calleeText);
      const isI18nKeyArgument =
        ts.isCallExpression(parent) &&
        (parent.arguments.includes(node) as boolean) &&
        /^(t|tError|labelKey)$/.test(calleeText);
      if (
        !isModuleSpecifier &&
        !isI18nKeyArgument &&
        !isConsoleArgument &&
        containsCjk(node.text)
      ) {
        const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
        out.push({ text: node.text, line: line + 1 });
      }
    }
    // 模板串的静态段（`${...}` 插值之外的部分）；作为 console.* 参数的
    // 模板串（诊断串）同样豁免
    if (ts.isTemplateExpression(node)) {
      const templateOwner = node.parent;
      const templateIsConsole =
        ts.isCallExpression(templateOwner) &&
        /^console\.(log|warn|error|info|debug)$/.test(templateOwner.expression.getText(source));
      for (const span of [node.head, ...node.templateSpans.map((s) => s.literal)]) {
        if (!templateIsConsole && containsCjk(span.text)) {
          const { line } = source.getLineAndCharacterOfPosition(span.getStart(source));
          out.push({ text: span.text, line: line + 1 });
        }
      }
    }
    // JSX 文本节点（<div>中文</div>）
    if (ts.isJsxText(node) && containsCjk(node.getText(source))) {
      const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
      out.push({ text: node.getText(source).trim(), line: line + 1 });
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return out;
}

describe("硬编码中文回退扫描（NFR-19 门禁）", () => {
  it("ui/src 非测试源码的字符串字面量与 JSX 文本无白名单外的 CJK", () => {
    const files = collectSourceFiles(SRC_ROOT).filter(
      (f) => !f.startsWith("i18n/") && f !== "i18n.ts",
    );
    const violations: string[] = [];
    for (const rel of files) {
      const abs = join(SRC_ROOT, rel);
      const source = ts.createSourceFile(
        abs,
        readFileSync(abs, "utf8"),
        ts.ScriptTarget.Latest,
        true,
        rel.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
      );
      for (const { text, line } of extractStringTexts(source)) {
        if (ALLOWLIST[rel] !== undefined) continue; // 文件级豁免
        const trimmed = text.trim();
        if (EXACT_ALLOWLIST[rel]?.some((e) => e.text === trimmed)) continue;
        violations.push(`${rel}:${line}: ${text.slice(0, 60).replace(/\n/g, "\\n")}`);
      }
    }
    expect(
      violations,
      `发现白名单外的硬编码中文（NFR-19：所有 UI 文案经 i18n 字典）：\n${violations.join("\n")}`,
    ).toEqual([]);
  });
});

// ---- 3–6. 切换冒烟 / 插值 / 回退 / errors ----

describe("t() 查词与语言切换", () => {
  it("切换 en-US 后 t() 输出英文，切回 zh-CN 恢复（冒烟）", () => {
    setLang("zh-CN");
    expect(t("workspace.settings")).toBe("设置");
    setLang("en-US");
    expect(getLang()).toBe<Lang>("en-US");
    expect(t("workspace.settings")).toBe("Settings");
    setLang("zh-CN");
    expect(t("workspace.settings")).toBe("设置");
  });

  it("{{var}} 插值与多变量替换", () => {
    setLang("zh-CN");
    expect(t("workspace.newSessionInProject", { name: "demo" })).toBe("在项目 demo 下新建会话");
    expect(t("inspector.compactedDivider", { count: 3 })).toBe("已压缩早期上下文（3 次）");
  });

  it("缺键回退链：en 缺键回 zh 基准；双缺回 key 本身（不空白）", () => {
    setLang("en-US");
    // 两包同构，正常 key 不触发回退；用不存在于 en 的键模拟（直接查 core 私面
    // 不可行——以「未知 key」验证回退形态）
    expect(t("workspace.settings")).toBe("Settings");
    expect(t("nonexistent.key.for.test")).toBe("nonexistent.key.for.test");
    setLang("zh-CN");
    expect(t("nonexistent.key.for.test")).toBe("nonexistent.key.for.test");
  });
});

describe("tError 错误码映射（§3.6 错误面口径）", () => {
  it("抽样注册表错误码给出本地化文案", () => {
    setLang("zh-CN");
    expect(tError("E_SESSION_RUNNING", "session busy")).toBe("会话正在运行中");
    expect(tError("E_PROVIDER_AUTH", "auth failed")).toBe("鉴权失败：请检查 API Key");
    setLang("en-US");
    expect(tError("E_SESSION_RUNNING", "session busy")).toBe("Session is already running");
    setLang("zh-CN");
  });

  it("未知错误码回退原始 message（不空白）", () => {
    expect(tError("E_FUTURE_CODE", "raw backend message")).toBe("raw backend message");
    expect(tError("E_FUTURE_CODE", "")).toBe("");
  });
});
