/**
 * @ 引用发送合成（§10.6，28 号票 / FR-10-03）：发送时提取消息正文中的
 * @token（普通形态 `@path` 与引号形态 `@"path with space"`），把 roots 内
 * 的相对路径去重、上限 20，追加为正文尾部的引用段——模型侧据此精确知道
 * 用户指哪个文件，按需走 read 工具读取（不复制内容、不预读，§6.2 wire
 * 重建零改动）。
 *
 * 引用段只在发送变换时合成：输入框原文、树落盘（syncTreeBeforeSend）与
 * ghost hint/↑↓ 历史都不受影响。绝对路径（17 号票拖拽插入形态）与含
 * `..` 段的 token 视为「越界」，原样保留在正文、不进引用段（模型 read
 * 照走授权链）。
 */

/** 引用段准入的 token 上限（超出截断并提示；Toast 随 29 号票，先 console.warn）。 */
export const AT_REF_LIMIT = 20;

/** 引用段格式（§10.6 回写口径）：正文尾部空行 + 标题行 + 逐行 `- @<路径>`。 */
export function buildRefSection(refs: string[]): string {
  return `\n\n[引用文件]\n${refs.map((r) => `- @${r}`).join("\n")}`;
}

/** 单个 @token 是否进引用段（相对路径准入）：非空、不以 `/` 开头、无 `..`
 *  段（防目录穿越）；Windows 反斜杠形态同样拒绝（正斜杠归一口径，16 号票）。 */
export function isRefCandidate(token: string): boolean {
  if (token.length === 0) return false;
  if (token.startsWith("/") || token.startsWith("\\")) return false;
  if (/^[A-Za-z]:/.test(token)) return false; // Windows 盘符绝对路径
  return token.split("/").every((seg) => seg !== "..");
}

/**
 * 从消息正文提取全部 @token（全文扫描，与 detectAtQuery 的光标回扫同语义）：
 * - 引号形态优先：`@"..."`（配对内任意字符，含空格）；
 * - 普通形态：`@` 后到下一个引号外空白前的非空段；
 * - 未闭合引号形态（`@"a b` 输入中）：普通形态吃到空白即止——`"a` 不构成
 *   有效路径 token，剔除引号字符后参与判定（脏字符自然不命中相对路径准入）。
 * 返回顺序保持出现序（去重交给 assembleRefSection）。
 */
export function extractAtTokens(text: string): string[] {
  return extractAtTokenSpans(text).map((s) => s.token);
}

/**
 * @token 提取的区间版（chip 高亮渲染面，28 号票）：返回每个 token 的
 * 字符串与其在原文中的 [start, end) 区间——引号形态的区间含 `@"` 与闭合
 * `"` 整体（高亮整段引用）；普通形态区间含 `@`。
 */
export function extractAtTokenSpans(text: string): Array<{
  token: string;
  start: number;
  end: number;
}> {
  const spans: Array<{ token: string; start: number; end: number }> = [];
  // 引号形态：@ 后紧跟成对引号
  const quoted = /@"([^"]*)"/g;
  const quotedSpans: Array<[number, number]> = [];
  for (const m of text.matchAll(quoted)) {
    const start = m.index ?? 0;
    quotedSpans.push([start, start + m[0].length]);
    if (m[1].length > 0) spans.push({ token: m[1], start, end: start + m[0].length });
  }
  const inQuotedSpan = (pos: number) => quotedSpans.some(([s, e]) => pos >= s && pos < e);
  // 普通形态：@ 开头、到引号外空白/引号字符/行尾为止
  for (const m of text.matchAll(/@(\S*)/g)) {
    const start = m.index ?? 0;
    if (inQuotedSpan(start)) continue;
    const raw = m[1];
    // 含引号字符 = 未闭合引号形态（`@"a b` 输入中）：不产出 token——
    // 不足以构成用户意图的完整路径，交给引号闭合后的下一次发送
    if (raw.includes('"')) continue;
    if (raw.length > 0) {
      spans.push({ token: raw, start, end: start + 1 + raw.length });
    }
  }
  return spans.sort((a, b) => a.start - b.start);
}

/**
 * 引用条目收集（提取 → 相对路径准入过滤 → 去重 → 上限截断）。发送变换与
 * 测试共用：runtime 的 withAtRefSection 取返回数组拼引用段（content part
 * 面组装）；超出上限 console.warn（Toast 随 29 号票）。
 */
export function collectRefs(text: string): string[] {
  if (!text.includes("@")) return [];
  const seen = new Set<string>();
  const refs: string[] = [];
  for (const token of extractAtTokens(text)) {
    if (!isRefCandidate(token) || seen.has(token)) continue;
    if (refs.length >= AT_REF_LIMIT) {
      console.warn(`[at-refs] 引用文件超过上限 ${AT_REF_LIMIT}，超出部分已截断`);
      break;
    }
    seen.add(token);
    refs.push(token);
  }
  return refs;
}

/**
 * 发送变换（消息数组以顶层 `text` 字段承载正文的通用形态）：从最后一条
 * 消息正文提取 @token（collectRefs），命中 ≥1 时正文尾部追加引用段，返回
 * 变换后的消息数组（浅拷贝，仅替换最后一条）；无命中原样返回（同一引用，
 * 零开销路径）。runtime 的 ThreadMessage 形态（文本在 content parts）走
 * collectRefs + buildRefSection 自行组装，不经本函数。
 */
export function assembleRefSection<T>(
  messages: readonly T[],
  textsOf: (message: T) => string,
): T[] {
  const list = [...messages];
  const last = list[list.length - 1];
  if (last === undefined) return list;
  const text = textsOf(last);
  const refs = collectRefs(text);
  if (refs.length === 0) return list;
  list[list.length - 1] = { ...last, text: `${text}${buildRefSection(refs)}` } as T;
  return list;
}
