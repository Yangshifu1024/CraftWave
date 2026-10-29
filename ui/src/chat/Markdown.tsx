import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { Streamdown, type StreamdownProps } from "streamdown";

const plugins = { cjk, code };
const streamingPlugins = { cjk };
const translations: StreamdownProps["translations"] = {
  close: "关闭",
  copied: "已复制",
  copyCode: "复制代码",
  copyLink: "复制链接",
  copyTable: "复制表格",
  copyTableAsCsv: "复制为 CSV",
  copyTableAsMarkdown: "复制为 Markdown",
  copyTableAsTsv: "复制为 TSV",
  downloadFile: "下载代码",
  downloadTable: "下载表格",
  downloadTableAsCsv: "下载 CSV",
  downloadTableAsMarkdown: "下载 Markdown",
  externalLinkWarning: "即将打开外部链接，请确认地址可信。",
  openExternalLink: "打开外部链接",
  openLink: "打开链接",
  viewFullscreen: "全屏查看",
  exitFullscreen: "退出全屏",
};

/** 流式期间只解析正文，结束后再启用高亮，避免高亮任务与增量渲染竞争。 */
export function Markdown({
  children,
  streaming = false,
}: {
  children: string;
  streaming?: boolean;
}) {
  return (
    <Streamdown
      className="cw-md"
      mode={streaming ? "streaming" : "static"}
      isAnimating={streaming}
      // 表格不限高：组件默认 maxHeight 300px 会把高表格压扁出纵向滚动条，
      // 放开让表格按内容完整展开（横向溢出仍由组件的 overflow-x-auto 兜底）
      tableMaxHeight="none"
      plugins={streaming ? streamingPlugins : plugins}
      translations={translations}
    >
      {children}
    </Streamdown>
  );
}
