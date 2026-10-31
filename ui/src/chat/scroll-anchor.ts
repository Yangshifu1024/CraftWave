/**
 * 会话滚动锚点（29 号票，FR-01-11 收口）：滚动位置随会话持久化。
 *
 * 锚点语义：`{ leafId, offset }`——leafId 是「视口顶上方最后一条消息」的
 * id（恢复时按 id 找回同一条消息），offset 是视口顶相对该消息顶的像素
 * 偏移。以消息为锚而不是裸 scrollTop：会话历史是活的（流式增量、压缩
 * 分隔、分支切换都会改变总高度），裸 scrollTop 在恢复时几乎必然对不上，
 * 消息锚 + 相对偏移才能稳定还原「当时看到的画面」。
 *
 * 持久化：sessions store 的 scrollAnchors 桶（ui-state 随既有 300ms 防抖
 * 落盘）；记账侧由 SessionTab 的滚动监听防抖写入，运行中 autoScroll 跟随
 * 时不记（避免回放打底——跟随态的滚动位置是程序推的，不是用户摆的）。
 *
 * 失效回落：leafId 对应消息已不存在（消息被删除/树重载后 id 变化/锚点
 * 桶损坏）→ resolve 返回 null，调用方滚回底部。
 */

/** 锚点载荷（ui-state.scrollAnchors 的元素形状）。 */
export interface ScrollAnchor {
  leafId: string;
  offset: number;
}

/** 消息锚的抽离形态（collectAnchorElements 从 DOM 收集；测试用假元素）。 */
export interface AnchorElement {
  messageId: string;
  /** 消息顶相对滚动内容顶部的偏移（getBoundingClientRect 相对换算）。 */
  top: number;
}

/**
 * 记账：由当前消息序列与滚动位置算锚点。elements 按 DOM 序；无消息
 * （空会话）返回 null（调用方跳过记账）。
 */
export function computeAnchor(elements: AnchorElement[], scrollTop: number): ScrollAnchor | null {
  if (elements.length === 0) return null;
  // 视口顶上方（含恰好压线）的最后一条消息；一条都没过线用第一条
  let chosen = elements[0];
  for (const element of elements) {
    if (element.top <= scrollTop) chosen = element;
    else break;
  }
  return { leafId: chosen.messageId, offset: Math.max(0, scrollTop - chosen.top) };
}

/**
 * 恢复：由锚点算目标 scrollTop。leafId 已不存在 → null（调用方回底部，
 * Math.max 防锚点越顶——内容被删短时顶到 0）。
 */
export function resolveScrollTop(elements: AnchorElement[], anchor: ScrollAnchor): number | null {
  const target = elements.find((element) => element.messageId === anchor.leafId);
  if (target === undefined) return null;
  return Math.max(0, target.top + anchor.offset);
}

/**
 * 从滚动容器收集消息锚（DOM 面）：容器内全部 [data-message-id] 元素按
 * DOM 序转 AnchorElement——top 用 getBoundingClientRect 相对滚动容器换算
 * （`elRect.top - containerRect.top + scrollTop`），在任何 offsetParent
 * 嵌套下都与「相对滚动内容顶部」一致。
 */
export function collectAnchorElements(container: HTMLElement): AnchorElement[] {
  const containerRect = container.getBoundingClientRect();
  const elements: AnchorElement[] = [];
  for (const node of container.querySelectorAll<HTMLElement>("[data-message-id]")) {
    elements.push({
      messageId: node.dataset.messageId ?? "",
      top: node.getBoundingClientRect().top - containerRect.top + container.scrollTop,
    });
  }
  return elements;
}

/**
 * 恢复落位（DOM 面）：按锚点设置容器 scrollTop——锚点失效（resolve 为
 * null）回底部。返回是否按锚点定位（false = 已回底；测试断言用）。
 */
export function applyAnchor(container: HTMLElement, anchor: ScrollAnchor | undefined): boolean {
  if (anchor === undefined) {
    container.scrollTop = container.scrollHeight;
    return false;
  }
  const top = resolveScrollTop(collectAnchorElements(container), anchor);
  if (top === null) {
    container.scrollTop = container.scrollHeight;
    return false;
  }
  // 内容比锚点位置短（消息被删）时浏览器钳到 scrollHeight，无越界面
  container.scrollTop = top;
  return true;
}

/**
 * 锚点桶清洗（ui-state 载入面）：ui-state.json 是前端自管的任意 JSON，
 * 载入时不信任形状——非对象/缺字段/类型不符的条目剔除（键保留合法条目）。
 */
export function sanitizeScrollAnchors(raw: unknown): Record<string, ScrollAnchor> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
  const out: Record<string, ScrollAnchor> = {};
  for (const [sessionId, value] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof value !== "object" || value === null) continue;
    const candidate = value as { leafId?: unknown; offset?: unknown };
    if (typeof candidate.leafId === "string" && typeof candidate.offset === "number") {
      out[sessionId] = { leafId: candidate.leafId, offset: candidate.offset };
    }
  }
  return out;
}
