/**
 * 轻量确认弹框（CodeWave 删除操作的 Popconfirm 对应物）：
 * 居中小卡片 + 危险色确认按钮；Esc / 遮罩点击 = 取消。
 * 是 Dialog 的定装组合，供"删除供应商/模型"等二次确认场景复用。
 */
import type { ReactNode } from "react";
import { t } from "@/i18n/core";
import { Dialog, type DialogActionItem } from "./dialog";

/** ConfirmDialog 的 props：open 是否显示；title 标题；description 补充说明；
 *  extra 说明之后的附加内容（如删除会话确认弹窗的备份目录入口
 *  按钮承载位）；confirmLabel 确认按钮文案；onConfirm/onCancel 两个回调。 */
export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description?: string;
  extra?: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

/** 渲染"取消 + 危险色确认"两按钮的确认弹框（宽 340）。 */
export function ConfirmDialog({
  open,
  title,
  description,
  extra,
  confirmLabel,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const actions: DialogActionItem[] = [
    { label: t("workspace.cancel"), variant: "ghost", onClick: onCancel },
    { label: confirmLabel, variant: "danger", onClick: onConfirm },
  ];
  return (
    <Dialog open={open} title={title} onClose={onCancel} actions={actions} width={340}>
      {description}
      {extra}
    </Dialog>
  );
}
