/**
 * 轻量确认弹框（CodeWave 删除操作的 Popconfirm 对应物）：
 * 居中小卡片 + 危险色确认按钮；Esc / 遮罩点击 = 取消。
 */
import { Dialog, type DialogActionItem } from "./dialog";

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description?: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const actions: DialogActionItem[] = [
    { label: "取消", variant: "ghost", onClick: onCancel },
    { label: confirmLabel, variant: "danger", onClick: onConfirm },
  ];
  return (
    <Dialog open={open} title={title} onClose={onCancel} actions={actions} width={340}>
      {description}
    </Dialog>
  );
}
