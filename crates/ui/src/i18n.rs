//! CraftWave 最小 i18n 层（gpui-rewrite 阶段 2.1，Wake `i18n.rs` 模式）。
//!
//! ## Wake 模式的要点（照抄其设计，规模收到本阶段最小）
//!
//! - **key 就是英文原文**：漏翻译天然回退英文（查不到即返回 key 自身），
//!   改文案不必同步改 key，代码里读到的还是那句话；
//! - `t()` 返回 `&'static str` 而非 String：调用点是 `.child(t("Settings"))`
//!   的**原地替换**，所有既有签名（`impl Into<SharedString>` 这类）一字不改；
//! - 译文串常驻（Wake 用 leak，本阶段词表是 `const` 静态数据，天然 'static，
//!   零分配零泄漏）。
//!
//! 本阶段只有 zh-CN 一门语言、一张内置词表（仅收录骨架用到的键）。加语言
//! 的路径与 Wake 一致：词表改成「(语言 tag, 表)」数组 + 运行时选择即可，
//! 调用点 `t(key)` 不动。
//!
//! 与 Wake 的差异：Wake 支持 prefs 里存语言偏好、启动选表；本阶段无偏好
//! 面（spec 2.1 只要求「最小 zh-CN 词表」），固定挂 zh-CN，`t()` 直接查表。

use std::collections::HashMap;
use std::sync::OnceLock;

/// zh-CN 词表：(英文原文 key, 中文译文)。只收录本阶段用到的键；
/// 新文案先加表再使用（不加也能跑——回退显示英文原文）。
const ZH_CN: &[(&str, &str)] = &[
    // 应用菜单：CraftWave 菜单
    ("About CraftWave", "关于 CraftWave"),
    ("Settings…", "设置…"),
    ("Hide CraftWave", "隐藏 CraftWave"),
    ("Hide Others", "隐藏其他"),
    ("Show All", "全部显示"),
    ("Quit CraftWave", "退出 CraftWave"),
    // File 菜单
    ("File", "文件"),
    ("New Session", "新会话"),
    ("Close Window", "关闭窗口"),
    // Edit 菜单（os_action 标准项）
    ("Edit", "编辑"),
    ("Undo", "撤销"),
    ("Redo", "重做"),
    ("Cut", "剪切"),
    ("Copy", "拷贝"),
    ("Paste", "粘贴"),
    ("Select All", "全选"),
    // View 菜单
    ("View", "显示"),
    ("Interface Zoom", "界面缩放"),
    // Window 菜单（菜单名 "Window" 逐字英文不译——见 main.rs app_menus 注释；
    // 菜单项照常翻译）
    ("Minimize", "最小化"),
    ("Zoom", "缩放"),
    ("Main Window", "主窗口"),
    // Help 菜单
    ("Help", "帮助"),
    ("CraftWave Help", "CraftWave 帮助"),
    // 界面骨架占位文案
    ("Sidebar placeholder", "会话列表（阶段 3.1）"),
    ("Panel placeholder", "面板（阶段 3.5）"),
    // 对话流（阶段 2.C-1）
    ("Thinking", "思考过程"),
    ("tool calls", "次工具调用"),
    ("failed", "失败"),
    ("Running…", "运行中…"),
    ("Reply or type @ for files…", "输入消息，@ 可引用文件…"),
    ("Attach image", "添加图片附件"),
    // 对话流（阶段 2.C-2b：运行中停止钮）
    ("Stop", "停止"),
];

/// 查表结构：`OnceLock` 首次访问构建，之后零开销哈希查词。
/// 静态表 → 表内 &str 全部 'static，`t()` 才能免分配返回 `&'static str`。
fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| ZH_CN.iter().copied().collect())
}

/// 翻译一句 UI 文案（Wake `t(key)` 形态）。查不到就是英文原文——
/// **永远不会显示 key 名以外的怪东西**，漏翻译是可见但无害的回退。
pub fn t(key: &'static str) -> &'static str {
    table().get(key).copied().unwrap_or(key)
}
