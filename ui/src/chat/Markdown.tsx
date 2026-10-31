import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { Streamdown, type StreamdownProps } from "streamdown";
import { t } from "@/i18n/core";

const plugins = { cjk, code };
const streamingPlugins = { cjk };

/** streamdown 组件文案（31 号票 i18n）：渲染时查词——语言切换后新渲染的
 *  Markdown 块即新语言。 */
function translations(): StreamdownProps["translations"] {
  return {
    close: t("workspace.mdClose"),
    copied: t("workspace.mdCopied"),
    copyCode: t("workspace.mdCopyCode"),
    copyLink: t("workspace.mdCopyLink"),
    copyTable: t("workspace.mdCopyTable"),
    copyTableAsCsv: t("workspace.mdCopyTableAsCsv"),
    copyTableAsMarkdown: t("workspace.mdCopyTableAsMarkdown"),
    copyTableAsTsv: t("workspace.mdCopyTableAsTsv"),
    downloadFile: t("workspace.mdDownloadFile"),
    downloadTable: t("workspace.mdDownloadTable"),
    downloadTableAsCsv: t("workspace.mdDownloadTableAsCsv"),
    downloadTableAsMarkdown: t("workspace.mdDownloadTableAsMarkdown"),
    externalLinkWarning: t("workspace.mdExternalLinkWarning"),
    openExternalLink: t("workspace.mdOpenExternalLink"),
    openLink: t("workspace.mdOpenLink"),
    viewFullscreen: t("workspace.mdViewFullscreen"),
    exitFullscreen: t("workspace.mdExitFullscreen"),
  };
}

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
      translations={translations()}
    >
      {children}
    </Streamdown>
  );
}
