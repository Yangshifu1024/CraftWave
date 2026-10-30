/**
 * Composer 按键体系（§10.6 / FR-13-05，17 号票；06 号票起 Enter 三态路由）。
 *
 * 覆盖：`@` 路径补全菜单（search_workspace_paths 驱动，光标锚定，Tab 优先级
 * 菜单 > ghost hint）、↑↓ 空输入翻阅本人历史消息、Esc 停止运行/关闭浮层、
 * Shift+Tab 循环权限档（内存态）、Tab 接受 ghost hint。
 * Enter 路由（06 号票，FR-01-07 两种语义界面明确）：空闲 Enter=普通发送、
 * 运行中 Enter=steer 插话（inject_run_message，turn 边界消费不终止 run）、
 * Alt+Enter=followUp 排队（任何时刻可排，run 正常完成后自动投递）；
 * Shift+Enter 换行 / IME 组合期防误发由 ComposerPrimitive.Input 原生承担
 * （0.15.31 内建 isComposing 检查，17 号票实证）。
 *
 * 同时承担草稿记账：composer 文本变化即写 sessions store（落盘防抖在
 * store 的 persistUiState）——§10.6「草稿按 Tab 存 ui-state」。
 */

import { useAui, useAuiState } from "@assistant-ui/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { queueFollowUp, sendSteer } from "@/chat/followup-store";
import { tierOf, usePermissionTierStore } from "@/chat/permission-tier";
import { DEFAULT_RUN_CONFIG } from "@/chat/runtime";
import { modelOf } from "@/chat/session-model";
import { useSessionScope } from "@/chat/session-scope";
import { thinkingOf } from "@/chat/thinking-level";
import { cancelRun, searchWorkspacePaths, skillList } from "@/ipc/client";
import type { SkillListEntry, WorkspacePathHit } from "@/ipc/types";
import { matchGhost, useAskStore } from "@/stores/ask";
import { useSessionsStore } from "@/stores/sessions";

/** @ 菜单快照（浮层渲染面）。 */
export interface AtMenuState {
  query: string;
  items: WorkspacePathHit[];
  activeIndex: number;
}

/** / 技能菜单快照（FR-01-17 技能半边，09 号票；浮层渲染面）。 */
export interface SkillMenuState {
  query: string;
  items: SkillListEntry[];
  activeIndex: number;
}

/** 内置子代理角色（FR-07-02，10 号票：$ 触发符菜单的静态清单；与 Rust
 *  agent/roles.rs 的 ROLES 同名单——7 角色由契约测试对照 subagent 工具
 *  description 双侧钉住）。 */
export const SUBAGENT_ROLES: { name: string; summary: string }[] = [
  { name: "explore", summary: "只读探索：大范围检索代码与文档，返回调研结论" },
  { name: "planner", summary: "只读规划：分析现状并产出可执行的分步计划" },
  { name: "implementer", summary: "实现者：在独立工作区修改代码完成指定任务" },
  { name: "reviewer", summary: "只读评审：审查代码/改动质量并给出风险清单" },
  { name: "tester", summary: "测试者：在独立工作区运行局部测试并回报证据" },
  { name: "frontend-dev", summary: "前端实现者：在独立工作区修改界面层代码" },
  { name: "backend-dev", summary: "后端实现者：在独立工作区修改服务端代码" },
];

/** $ 菜单快照（FR-07-02，10 号票；浮层渲染面）。 */
export interface DollarMenuState {
  query: string;
  items: { name: string; summary: string }[];
  activeIndex: number;
}

/**
 * Enter 键的三态路由判定（纯函数，FR-01-07 两种语义界面明确）：
 * - Alt+Enter → followUp 排队（任何时刻可排，含运行中）；
 * - 运行中 Enter → steer 插话（注入当前 run，不终止）；
 * - 其余 → none（空闲 Enter 的普通发送 / Shift+Enter 换行交给
 *   ComposerPrimitive.Input 原生处理）。
 * IME 组合期（isComposing）一律 none——防误发与原生口径一致。
 */
export function routeEnterKey(opts: {
  altKey: boolean;
  isRunning: boolean;
  isComposing: boolean;
}): "steer" | "followUp" | "none" {
  if (opts.isComposing) return "none";
  if (opts.altKey) return "followUp";
  if (opts.isRunning) return "steer";
  return "none";
}

/**
 * 从 (text, cursor) 检测未闭合的 `@` 触发（§10.6 光标锚定，28 号票引号升级）：
 * - 光标往回到最近 `@` 之间无引号外空白即视为路径查询中，返回插入位与查询词；
 * - 引号路径（`@"a b/c.txt"`，28 号票）：回扫遇**未闭合** `"` 内空白不终止
 *   （引号闭合即止——闭合的引用段视为完整，不再触发菜单）；引号内的 `@`
 *   不嵌套触发（`@"a@b/c.txt"` 中间的 @ 是普通字符）。
 *
 * 算法：先按光标左侧引号总数定起步状态（奇 = 光标在未闭合引号段内），
 * 再向左逐字符扫描——`"` 翻转引号内态（翻出引号段且起步就在段外 = 闭合点，
 * 即止返回 null）；`@` 在引号外即触发符；引号外空白即分隔符。
 */
export function detectAtQuery(text: string, cursor: number): { at: number; query: string } | null {
  // 起步状态：光标左侧的引号配对余量（奇数 = 光标处于某个未闭合引号内）
  let inQuote = (text.slice(0, cursor).match(/"/g)?.length ?? 0) % 2 === 1;
  const startInQuote = inQuote;
  let i = cursor - 1;
  while (i >= 0) {
    const ch = text[i];
    if (ch === '"') {
      inQuote = !inQuote;
      if (!inQuote && !startInQuote) return null; // 引号闭合即止：引用已完整
      i -= 1;
      continue;
    }
    if (ch === "@") {
      if (!inQuote) return { at: i, query: text.slice(i + 1, cursor) };
      i -= 1;
      continue; // 引号内的 @：普通字符，不嵌套触发
    }
    if (/\s/.test(ch) && !inQuote) return null; // 引号外空白：非触发态
    i -= 1;
  }
  return null;
}

/**
 * 从 (text, cursor) 检测 `/` 技能触发（FR-01-17 技能半边，09 号票；与
 * detectAtQuery 同构的光标锚定 + 首字符互斥）：
 * - 光标往回到最近的 `/` 之间无空白 → 查询中；
 * - `/` 必须在行首或前一字符是空白（排除 `src/main.rs` 这类路径文本误触）；
 * - 与 `@` 互斥：光标前最近的触发符是谁就归谁的菜单（先于本函数的 @ 检测
 *   命中时本函数不会被调用——useComposerKeys 内先测 @ 再测 /）。
 * 返回插入位（`/` 的下标）与查询词；非触发态返回 null。
 */
export function detectSlashQuery(
  text: string,
  cursor: number,
): { at: number; query: string } | null {
  let i = cursor - 1;
  while (i >= 0) {
    const ch = text[i];
    if (ch === "/") {
      // `/` 的前一字符（若有）必须是空白——行首或空白后的 `/` 才是触发符
      if (i > 0 && !/\s/.test(text[i - 1])) return null;
      return { at: i, query: text.slice(i + 1, cursor) };
    }
    if (ch === "@") return null; // 更近的 @ 触发归 @ 菜单
    if (/\s/.test(ch)) return null; // / 与光标间隔了空白：非触发态
    i -= 1;
  }
  return null;
}

/**
 * 从 (text, cursor) 检测 `$` 角色触发（FR-07-02，10 号票；与 detectAtQuery/
 * detectSlashQuery 同构的光标锚定 + 首字符互斥）：
 * - 光标往回到最近的 `$` 之间无空白 → 查询中；
 * - `$` 必须在行首或前一字符是空白（排除 `cost$100` 这类文本误触）；
 * - 与 `@`/`/` 互斥：光标前最近的触发符是谁就归谁的菜单（useComposerKeys
 *   内按 @ → / → $ 顺序检测）。
 * 返回插入位（`$` 的下标）与查询词；非触发态返回 null。
 */
export function detectDollarQuery(
  text: string,
  cursor: number,
): { at: number; query: string } | null {
  let i = cursor - 1;
  while (i >= 0) {
    const ch = text[i];
    if (ch === "$") {
      // `$` 的前一字符（若有）必须是空白——行首或空白后的 `$` 才是触发符
      if (i > 0 && !/\s/.test(text[i - 1])) return null;
      return { at: i, query: text.slice(i + 1, cursor) };
    }
    if (ch === "@" || ch === "/") return null; // 更近的 @// 触发归各自菜单
    if (/\s/.test(ch)) return null; // $ 与光标间隔了空白：非触发态
    i -= 1;
  }
  return null;
}

/** 角色清单的查询过滤（名称前缀优先、其次包含；大小写不敏感）。 */
export function filterRoles(
  roles: { name: string; summary: string }[],
  query: string,
): { name: string; summary: string }[] {
  const q = query.trim().toLowerCase();
  if (q === "") return roles;
  const starts: { name: string; summary: string }[] = [];
  const contains: { name: string; summary: string }[] = [];
  for (const role of roles) {
    const name = role.name.toLowerCase();
    if (name.startsWith(q)) starts.push(role);
    else if (name.includes(q)) contains.push(role);
  }
  return [...starts, ...contains];
}

/** 选中角色的插入文本（票面定案：插入 `$角色名 ` 后用户续写任务，发送
 *  文本原样入会话——模型经 subagent 工具 description 理解点名语义）。 */
export function insertDollarInvocation(
  text: string,
  at: number,
  cursor: number,
  name: string,
): string {
  const rest = text.slice(cursor).replace(/^\s+/, "");
  return `${text.slice(0, at)}$${name} ${rest}`;
}

/** 选中技能的插入文本（票面定案：展开为明确指令文本，避免依赖模型对裸
 *  `/名` 的理解；尾部空格衔接用户后续输入——后续文本若以空白开头则剥掉
 *  首段空白，避免出现双空格）。 */
export function insertSkillInvocation(
  text: string,
  at: number,
  cursor: number,
  name: string,
): string {
  const rest = text.slice(cursor).replace(/^\s+/, "");
  return `${text.slice(0, at)}请用 skill 工具加载技能 ${name} 并按其指引行动 ${rest}`;
}

/** 技能清单的查询过滤（名称前缀优先、其次包含；大小写不敏感）。 */
export function filterSkills(skills: SkillListEntry[], query: string): SkillListEntry[] {
  const q = query.trim().toLowerCase();
  if (q === "") return skills;
  const starts: SkillListEntry[] = [];
  const contains: SkillListEntry[] = [];
  for (const skill of skills) {
    const name = skill.name.toLowerCase();
    if (name.startsWith(q)) starts.push(skill);
    else if (name.includes(q)) contains.push(skill);
  }
  return [...starts, ...contains];
}

/**
 * 补全项的插入文本（28 号票引号升级）：路径含空格 → 引号包裹
 * `@"a b/c.txt"`（发送合成引用段与模型 read 都按整段路径消费）；无空格
 * 维持裸路径。替换范围 = [at+1, cursor)（即查询段整体——含查询中已敲出的
 * 开引号，选中补全后引号形态归一）。目录与文件同形态。
 */
export function insertAtCompletion(text: string, at: number, cursor: number, path: string): string {
  const token = /\s/.test(path) ? `"${path}"` : path;
  return `${text.slice(0, at + 1)}${token} ${text.slice(cursor)}`;
}

/**
 * Composer 键盘体系主 hook：返回输入框要挂的 onKeyDown/onCursor 回调、
 * @ 补全菜单状态（atMenu）与 / 技能菜单状态（skillMenu）（FR-01-17 两半边，
 * 09 号票；触发符按首字符互斥：光标前最近的 @ / / 各归其菜单）与菜单交互
 * 方法（pickActive/pickSkill 选中、hoverIndex/hoverSkill 悬停）。
 * 同时内部承担三件记账：@ 菜单的触发检测与防抖搜索、/ 菜单的技能清单拉取、
 * 草稿实时写入会话 store。键位优先级（§10.6）：触发符菜单 > ghost hint。
 *
 * 17 号票增 interceptEnter：带图附件的 Enter 分流（先于既有路由询问）——
 * 返回 true 表示按键已被附件发送路径消费（调用方内部完成发送/排队），
 * onKeyDown 即 preventDefault 收口；无附件时传入 undefined 走原路由。
 */
export function useComposerKeys(options?: {
  /** 带图附件的 Enter 拦截（(altKey, isRunning) → 是否已消费）。 */
  interceptEnter?: (altKey: boolean, isRunning: boolean) => boolean;
}): {
  onKeyDown: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onCursor: (cursor: number) => void;
  atMenu: AtMenuState | null;
  skillMenu: SkillMenuState | null;
  dollarMenu: DollarMenuState | null;
  pickActive: () => void;
  pickSkill: () => void;
  pickDollar: () => void;
  hoverIndex: (index: number) => void;
  hoverSkill: (index: number) => void;
  hoverDollar: (index: number) => void;
} {
  const sessionId = useSessionScope();
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  const isRunning = useAuiState((s) => s.thread.isRunning);
  const threadMessages = useAuiState((s) => s.thread.messages);
  // 本人历史输入（时间序，非空文本）——翻阅用；useMemo 派生避免 selector 引用抖动
  const userMessages = useMemo(
    () =>
      threadMessages
        .filter((m) => m.role === "user")
        .map((m) => m.content.map((p) => (p.type === "text" ? p.text : "")).join(""))
        .filter((t) => t.length > 0),
    [threadMessages],
  );
  const hint = useAskStore((s) => s.hints[sessionId]);
  const clearHint = useAskStore((s) => s.clearHint);
  const cycleTier = usePermissionTierStore((s) => s.cycle);
  const setDraft = useSessionsStore((s) => s.setDraft);
  // 带图附件的 Enter 拦截（17 号票）：ref 承载避免 options 变化重建回调链
  const interceptRef = useRef(options?.interceptEnter);
  interceptRef.current = options?.interceptEnter;

  /** 报告当前光标位置（onKeyDown/onKeyUp/onSelect 都会回调；存 ref 快照，
   *  供 @ 触发检测与补全插入使用，不参与渲染）。 */
  const cursorRef = useRef(0);
  const onCursor = useCallback((cursor: number) => {
    cursorRef.current = cursor;
  }, []);

  // ---- @ 菜单状态机（与 / 技能菜单互斥：先测 @，命中即不再测 /）----
  const [atMenu, setAtMenu] = useState<AtMenuState | null>(null);
  // 文本变化 → 重检测触发态（光标取 ref 快照）；查询词变化 → 防抖拉补全。
  // detected 在 effect 内现算，避免对象引用进依赖数组。
  useEffect(() => {
    const detected = detectAtQuery(text, cursorRef.current);
    if (detected === null) {
      setAtMenu(null);
      return;
    }
    setSkillMenu(null); // @ 触发优先：关掉可能开着的 / 菜单（互斥）
    setDollarMenu(null); // 同时关掉可能开着的 $ 菜单（10 号票三符互斥）
    setAtMenu((prev) =>
      prev !== null && prev.query === detected.query
        ? prev
        : { query: detected.query, items: [], activeIndex: 0 },
    );
    const timer = setTimeout(() => {
      // 引号路径查询（28 号票）：搜索词剥引号字符（`"a b/c` → `a b/c`，
      // 后端基名子串匹配不含引号形态）
      const needle = detected.query.replace(/"/g, "");
      void searchWorkspacePaths(sessionId, needle, 8)
        .then((items) => {
          setAtMenu((prev) => (prev !== null ? { ...prev, items, activeIndex: 0 } : prev));
        })
        .catch(() => {
          // IPC 异常（纯浏览器等）：菜单留空，不打断输入
        });
    }, 150);
    return () => clearTimeout(timer);
  }, [text, sessionId]);

  // ---- / 技能菜单状态机（FR-01-17 技能半边，09 号票）----
  // 技能清单：挂载时拉一次（skill_list），reload/toggle 后由调用方重开菜单
  // 时刷新（清单数据量小，不轮询）。
  const [skills, setSkills] = useState<SkillListEntry[]>([]);
  useEffect(() => {
    let cancelled = false;
    void skillList()
      .then((entries) => {
        if (!cancelled) setSkills(entries.filter((e) => e.enabled));
      })
      .catch(() => {
        // IPC 异常（纯浏览器等）：清单留空，/ 菜单不出现
      });
    return () => {
      cancelled = true;
    };
  }, []);
  const [skillMenu, setSkillMenu] = useState<SkillMenuState | null>(null);
  useEffect(() => {
    const detected = detectSlashQuery(text, cursorRef.current);
    if (detected === null) {
      setSkillMenu(null);
      return;
    }
    setAtMenu(null); // / 触发：关掉可能开着的 @ 菜单（互斥）
    const items = filterSkills(skills, detected.query);
    setSkillMenu({ query: detected.query, items, activeIndex: 0 });
  }, [text, skills]);

  // ---- $ 角色菜单状态机（FR-07-02，10 号票）：静态 7 角色清单；与 @ //
  // 互斥（先测 @ 再测 /，都未命中才测 $）。
  const [dollarMenu, setDollarMenu] = useState<DollarMenuState | null>(null);
  useEffect(() => {
    const detected = detectDollarQuery(text, cursorRef.current);
    if (detected === null) {
      setDollarMenu(null);
      return;
    }
    setAtMenu(null); // $ 触发：关掉可能开着的 @ / 菜单（互斥）
    setSkillMenu(null);
    const items = filterRoles(SUBAGENT_ROLES, detected.query);
    setDollarMenu({ query: detected.query, items, activeIndex: 0 });
  }, [text]);

  /** 插入当前选中项（键盘 Tab/Enter 与浮层点击共用）。 */
  const pickActive = useCallback(() => {
    const snapshot = atMenu;
    if (snapshot === null) return;
    const hit = snapshot.items[snapshot.activeIndex];
    if (hit !== undefined) {
      const at = cursorRef.current;
      aui.composer.setText(
        insertAtCompletion(text, detectAtQuery(text, at)?.at ?? at - 1, at, hit.path),
        {
          origin: "user",
        },
      );
    }
    setAtMenu(null);
  }, [aui, atMenu, text]);

  /** 插入选中技能的明确指令文本（09 号票定案：展开为「请用 skill 工具加载
   *  技能 X…」+ 保留用户后续输入）。 */
  const pickSkill = useCallback(() => {
    const snapshot = skillMenu;
    if (snapshot === null) return;
    const skill = snapshot.items[snapshot.activeIndex];
    if (skill !== undefined) {
      const at = cursorRef.current;
      aui.composer.setText(
        insertSkillInvocation(text, detectSlashQuery(text, at)?.at ?? at - 1, at, skill.name),
        { origin: "user" },
      );
    }
    setSkillMenu(null);
  }, [aui, skillMenu, text]);

  /** 插入选中角色的 `$角色名 ` 前缀（10 号票定案：点名语义由发送文本原样
   *  承载——模型经 subagent 工具 description 理解）。 */
  const pickDollar = useCallback(() => {
    const snapshot = dollarMenu;
    if (snapshot === null) return;
    const role = snapshot.items[snapshot.activeIndex];
    if (role !== undefined) {
      const at = cursorRef.current;
      aui.composer.setText(
        insertDollarInvocation(text, detectDollarQuery(text, at)?.at ?? at - 1, at, role.name),
        { origin: "user" },
      );
    }
    setDollarMenu(null);
  }, [aui, dollarMenu, text]);

  // ---- 历史翻阅（↑↓ 空输入，§10.6）----
  const historyRef = useRef<{ index: number; draft: string }>({ index: -1, draft: "" });

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    onCursor(event.currentTarget.selectionStart ?? 0);
    // 1) 触发符菜单导航（§10.6 键位优先级：菜单 > ghost hint；@ 与 / 互斥开）
    if (atMenu !== null) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        setAtMenu((prev) =>
          prev === null || prev.items.length === 0
            ? prev
            : {
                ...prev,
                activeIndex: (prev.activeIndex + delta + prev.items.length) % prev.items.length,
              },
        );
        return;
      }
      if (event.key === "Tab" || event.key === "Enter") {
        event.preventDefault();
        pickActive();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setAtMenu(null);
        return;
      }
    }
    if (skillMenu !== null && skillMenu.items.length > 0) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        setSkillMenu((prev) =>
          prev === null
            ? prev
            : {
                ...prev,
                activeIndex: (prev.activeIndex + delta + prev.items.length) % prev.items.length,
              },
        );
        return;
      }
      if (event.key === "Tab" || event.key === "Enter") {
        event.preventDefault();
        pickSkill();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setSkillMenu(null);
        return;
      }
    }
    // 1c) $ 角色菜单导航（10 号票，§10.6：触发符菜单 > ghost hint）
    if (dollarMenu !== null && dollarMenu.items.length > 0) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        setDollarMenu((prev) =>
          prev === null || prev.items.length === 0
            ? prev
            : {
                ...prev,
                activeIndex: (prev.activeIndex + delta + prev.items.length) % prev.items.length,
              },
        );
        return;
      }
      if (event.key === "Tab" || event.key === "Enter") {
        event.preventDefault();
        pickDollar();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDollarMenu(null);
        return;
      }
    }
    // 2) ↑↓ 空输入：翻阅本人历史（Esc/清空退出翻阅恢复原稿）
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && text.length === 0) {
      const stack = userMessages.slice().reverse();
      if (stack.length === 0) return;
      event.preventDefault();
      const h = historyRef.current;
      if (h.index === -1) h.draft = text;
      const next = event.key === "ArrowUp" ? h.index + 1 : h.index - 1;
      if (next < 0) {
        historyRef.current = { index: -1, draft: "" };
        aui.composer.setText(h.draft, { origin: "user" });
        return;
      }
      const clamped = Math.min(next, stack.length - 1);
      historyRef.current = { ...h, index: clamped };
      aui.composer.setText(stack[clamped], { origin: "user" });
      return;
    }
    // 3) Tab 接受 ghost hint（§10.4）
    if (event.key === "Tab" && !event.shiftKey && matchGhost(hint ?? null, text) !== null) {
      event.preventDefault();
      aui.composer.setText(hint ?? "", { origin: "user" });
      clearHint(sessionId);
      return;
    }
    // 4) Shift+Tab 循环权限档（内存态，§10.6）
    if (event.key === "Tab" && event.shiftKey) {
      event.preventDefault();
      cycleTier(sessionId);
      return;
    }
    // 5) Enter 三态路由（06 号票，FR-01-07）：Alt+Enter=followUp 排队、
    //    运行中 Enter=steer 插话；空闲 Enter（普通发送）与 Shift+Enter（换行）
    //    不拦截，交给 ComposerPrimitive.Input 原生。两种排队语义排队成功后
    //    清空输入框（steer 回执后时间线插节点，followUp 进队列条）。
    //    5a) 带图附件拦截（17 号票）：先于既有路由——附件发送/入队由
    //        ComposerBar 的回调统一处理（空闲 thread.append、运行中 inject）。
    if (event.key === "Enter" && !event.shiftKey) {
      if (interceptRef.current?.(event.altKey, isRunning) === true) {
        event.preventDefault();
        return;
      }
      const route = routeEnterKey({
        altKey: event.altKey,
        isRunning,
        isComposing: event.nativeEvent.isComposing,
      });
      const body = text.trim();
      if (route === "none" || body.length === 0) return;
      event.preventDefault();
      if (route === "steer") {
        void sendSteer(sessionId, body).then((accepted) => {
          if (accepted) aui.composer.setText("", { origin: "user" });
          // 未接纳（防御态）：保留输入框文本，不打断输入
        });
        return;
      }
      const runConfig = {
        ...DEFAULT_RUN_CONFIG,
        tier: tierOf(sessionId),
        ...(modelOf(sessionId) !== undefined ? { modelId: modelOf(sessionId) } : {}),
        ...(thinkingOf(sessionId) !== undefined ? { thinking: thinkingOf(sessionId) } : {}),
      };
      void queueFollowUp(sessionId, body, runConfig).then((queued) => {
        if (queued !== null) aui.composer.setText("", { origin: "user" });
      });
      return;
    }
    // 6) Esc：停止运行（终态以 runEnded 帧为权威，S5 语义）
    if (event.key === "Escape") {
      if (isRunning) {
        event.preventDefault();
        void cancelRun(sessionId).catch(() => {});
      } else if (hint != null) {
        clearHint(sessionId);
      }
    }
  };

  // ---- 草稿记账（§10.6 按 Tab 存 ui-state；空串自动清除该键） ----
  useEffect(() => {
    setDraft(sessionId, text);
  }, [sessionId, text, setDraft]);

  /** @ 浮层 hover 高亮（键盘 ↑↓ 导航共享 activeIndex）。 */
  const hoverIndex = useCallback((index: number) => {
    setAtMenu((prev) => (prev === null ? prev : { ...prev, activeIndex: index }));
  }, []);

  /** / 技能菜单浮层 hover 高亮。 */
  const hoverSkill = useCallback((index: number) => {
    setSkillMenu((prev) => (prev === null ? prev : { ...prev, activeIndex: index }));
  }, []);

  /** $ 角色菜单浮层 hover 高亮。 */
  const hoverDollar = useCallback((index: number) => {
    setDollarMenu((prev) => (prev === null ? prev : { ...prev, activeIndex: index }));
  }, []);

  return {
    onKeyDown,
    onCursor,
    atMenu,
    skillMenu,
    dollarMenu,
    pickActive,
    pickSkill,
    pickDollar,
    hoverIndex,
    hoverSkill,
    hoverDollar,
  };
}
