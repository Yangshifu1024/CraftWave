/**
 * 设置页注册表（照抄 CodeWave settingsRegistry 的纯数据形态——只放数据，
 * 不含渲染逻辑）：页集合 / 页序 / 左导航三组 / 页名，全部集中在本模块。
 * 页面 JSX 仍手写（不做数据驱动渲染）；本表供左导航与脏标记消费。
 */

/** 设置页的键：每页一个，本表所有结构（页序/页名/分组）共用的标识。 */
export type PageKey =
  | "appearance"
  | "providers"
  | "network"
  | "security"
  | "mcp"
  | "skills"
  | "tools"
  | "agent"
  | "logs"
  | "about";

/** 页序（= 左导航逐组展开后的顺序） */
export const PAGE_ORDER: PageKey[] = [
  "appearance",
  "providers",
  "network",
  "security",
  "mcp",
  "skills",
  "tools",
  "agent",
  "logs",
  "about",
];

/** 默认页：设置页打开时的落点 */
export const DEFAULT_PAGE: PageKey = "appearance";

/** 页名（导航项与操作条标题共用）：值为 i18n 键（31 号票——消费点经 t()
 *  查词渲染，双语随界面语言切换）。 */
export const PAGE_LABEL: Record<PageKey, string> = {
  appearance: "settings.pageAppearance",
  providers: "settings.pageProviders",
  network: "settings.pageNetwork",
  security: "settings.pageSecurity",
  mcp: "settings.pageMcp",
  skills: "settings.pageSkills",
  tools: "settings.pageTools",
  agent: "settings.pageAgent",
  logs: "settings.pageLogs",
  about: "settings.pageAbout",
};

/** 左导航三组（组内页序 = PAGE_ORDER 中的相对序）；组名同为 i18n 键。 */
export const PAGE_GROUPS: { title: string; pages: PageKey[] }[] = [
  { title: "settings.groupAppearance", pages: ["appearance", "providers", "network"] },
  { title: "settings.groupSecurity", pages: ["security", "mcp", "skills", "tools", "agent"] },
  { title: "settings.groupDiagnosis", pages: ["logs", "about"] },
];

/** 未知页 key 回退默认页 */
export function normalizePageKey(raw: string | null | undefined): PageKey {
  return PAGE_ORDER.includes(raw as PageKey) ? (raw as PageKey) : DEFAULT_PAGE;
}
