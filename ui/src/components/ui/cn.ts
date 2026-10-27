/** 类名拼接（shadcn/ui 同名工具的最小实现：过滤假值后空格连接）。 */
export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
