/**
 * 设置页图标：统一从 lucide-react 转发（ZCode 同款图标库），保持原导出
 * 名与 SVGProps 入参不变，消费方零改动。包装层默认 aria-hidden（装饰性
 * 图形，读屏软件忽略），与原手绘口径一致。
 * 页图标口径照旧：appearance=调色板、providers=插头、network=地球、
 * security=盾牌、mcp=积木、skills=闪电、tools=勾选框、agent=机器人、
 * logs=文档、about=信息。
 */

import {
  ArrowLeft,
  Blocks,
  Bot,
  ChevronDown,
  Eye,
  FileText,
  Globe,
  Info,
  Lock,
  Palette,
  Pencil,
  Plug,
  Plus,
  Search,
  Shield,
  SquareCheckBig,
  Trash2,
  Zap,
} from "lucide-react";
import type { ComponentType, SVGProps } from "react";

/** 转发包装：lucide 组件套上装饰性 aria-hidden 默认值。 */
function wrap(Icon: ComponentType<SVGProps<SVGSVGElement>>) {
  return function IconForwarder(props: SVGProps<SVGSVGElement>) {
    return <Icon aria-hidden {...props} />;
  };
}

/** 调色板图标（导航「界面」页）。 */
export const PaletteIcon = wrap(Palette);
/** 插头图标（导航「模型与供应商」页）。 */
export const PlugIcon = wrap(Plug);
/** 地球图标（导航「网络与连接」页）。 */
export const GlobeIcon = wrap(Globe);
/** 盾牌图标（导航「安全与审批」页）。 */
export const ShieldIcon = wrap(Shield);
/** 积木图标（导航「MCP」页）。 */
export const BlocksIcon = wrap(Blocks);
/** 闪电图标（导航「技能」页）。 */
export const ZapIcon = wrap(Zap);
/** 勾选框图标（导航「写入后检查与校验」页）。 */
export const CheckSquareIcon = wrap(SquareCheckBig);
/** 机器人图标（导航「工作区与智能体」页）。 */
export const BotIcon = wrap(Bot);
/** 文档图标（导航「日志」页）。 */
export const FileTextIcon = wrap(FileText);
/** 信息图标（导航「关于」页）。 */
export const InfoIcon = wrap(Info);
/** 左箭头图标（设置页「返回工作区」按钮）。 */
export const ArrowLeftIcon = wrap(ArrowLeft);
/** 下箭头图标（Select 组件右侧下拉指示）。 */
export const ChevronDownIcon = wrap(ChevronDown);
/** 加号图标（「添加供应商/模型/请求头」等新增按钮）。 */
export const PlusIcon = wrap(Plus);
/** 放大镜图标（设置页搜索框）。 */
export const SearchIcon = wrap(Search);
/** 铅笔图标（「编辑」按钮）。 */
export const PencilIcon = wrap(Pencil);
/** 垃圾桶图标（「删除」按钮）。 */
export const TrashIcon = wrap(Trash2);
/** 眼睛图标（模型「图像」输入能力标签）。 */
export const EyeIcon = wrap(Eye);
/** 锁图标（不可更改的锁定标签，如「文本」输入类型）。 */
export const LockIcon = wrap(Lock);
