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
    // 文本工具（阶段 2.C-3：smart_time 的时间档标签）
    ("Just now", "刚刚"),
    ("min ago", "分钟前"),
    ("Yesterday", "昨天"),
    // thinking 面板与工具簇卡（阶段 2.C-4：2.7 / 2.8）
    ("Thinking…", "思考中…"),
    ("sec", "秒"),
    ("min", "分"),
    ("Input", "输入"),
    ("Output", "输出"),
    ("First 600 characters", "前 600 字符"),
    ("Copy full input", "复制完整输入"),
    ("Copy full output", "复制完整输出"),
    ("Open subagent", "打开子代理"),
    // 图片三态与 zoom（阶段 2.C-4：2.9）
    ("Preview unavailable", "预览不可用"),
    ("Download original image", "下载原图"),
    ("Image omitted", "图片已省略"),
    ("Transcript limit reached", "转录已达上限"),
    ("Zoom image", "放大查看"),
    ("Close", "关闭"),
    // 侧栏（阶段 3.1：多会话 + 侧栏）
    ("Temporary sessions", "临时会话"),
    ("Projects", "项目"),
    ("No sessions yet", "还没有会话"),
    ("Rename", "重命名"),
    ("Delete", "删除"),
    ("Delete session", "删除会话"),
    ("Delete session?", "删除这个会话？"),
    ("This permanently removes the conversation history. This cannot be undone.", "将永久删除该会话的全部历史记录，此操作不可撤销。"),
    ("Cancel", "取消"),
    ("messages", "条消息"),
    ("Session title", "会话标题"),
    // 会话树与分支（阶段 3.3）
    ("Previous branch", "上一个分支"),
    ("Next branch", "下一个分支"),
    ("Edit and resend", "编辑重发"),
    ("Editing history message — sending creates a new branch", "正在编辑历史消息——发送将产生新分支"),
    ("Cancel edit", "取消编辑"),
    ("Session is running; branch switching is unavailable", "会话运行中，暂不能切换分支"),
    // B1 空状态（阶段 3-A）
    ("Good morning", "早上好"),
    ("Good afternoon", "下午好"),
    ("Good evening", "晚上好"),
    ("What can I help you with?", "今天想做点什么？"),
    ("No conversation yet — type below to start a new session, or pick one from the sidebar.", "还没有对话——在下方输入即可开始新会话，或从左侧选择历史会话。"),
    ("Explain this repository's structure", "讲解这个仓库的结构"),
    ("Help me write a build script", "帮我写一个构建脚本"),
    ("Review the recent changes", "审查最近的改动"),
    ("Summarize today's meeting notes", "总结今天的会议记录"),
    // Ask 权限面板（阶段 3-B：3.4 权限与确认）
    ("MCP server trust", "MCP 服务器信任：{server}"),
    ("Project resource trust", "项目资源信任：{resource}"),
    ("External directory access", "外部目录访问"),
    ("Allowed", "已允许"),
    ("Denied", "已拒绝"),
    ("Answered", "已回答"),
    ("Expired", "已失效"),
    ("recommended", "推荐"),
    ("Submit answer", "提交回答"),
    ("Copy full command", "复制完整命令"),
    ("Copy full diff", "复制完整 diff"),
    ("Transport", "传输方式"),
    ("Launch", "启动命令"),
    ("Fingerprint", "指纹"),
    ("Source", "来源"),
    ("Version fingerprint", "版本指纹"),
    ("Directory", "目录"),
    ("stdio", "stdio 子进程"),
    ("http", "HTTP"),
    ("Global skills", "全局技能目录"),
    ("Project skills", "项目技能目录"),
    ("Trust this MCP server before connecting? It runs with your session's permissions.", "连接前信任此 MCP 服务器？它将以本会话的权限运行。"),
    ("Trust this resource's current version? Untrusted skills are not loaded.", "信任此资源的当前版本？未信任的技能不会被加载。"),
    ("Allow read/write/list access to this directory and its subdirectories for this session?", "允许本会话内读取/写入/列出该目录及其子目录？"),
    ("Command audit suggests manual confirmation", "命令审计模型建议人工确认：{reason}"),
    ("Command audit unavailable, confirm manually", "命令审计不可用（{reason}），请人工确认"),
    ("Path outside approved plan", "写入路径「{path}」不在已批准计划范围内"),
    // 权限档（阶段 3-B：3.4 快切，旧栈 tierLabel/tierDescription 同表）
    ("Plan", "计划"),
    ("Confirm each", "逐项确认"),
    ("Auto edit", "自动编辑"),
    ("Full trust", "完全信任"),
    ("Plan before editing.", "编辑前先出计划。"),
    ("Ask before each file change.", "改文件前先问我。"),
    ("Edit files automatically.", "自动编辑文件。"),
    ("Fewer confirmations.", "减少确认次数。"),
    // 子代理（阶段 3-B：3.6 列表 + 详情跳转）
    ("Running", "运行中"),
    ("Completed", "已完成"),
    ("Budget exhausted", "预算耗尽"),
    ("Cancelled", "已取消"),
    ("No report", "无汇报"),
    ("Subagents", "子代理"),
    ("Back to conversation", "返回对话"),
    ("turn N", "第 {n} 轮"),
    ("Report", "汇报"),
    ("Final report", "最终汇报"),
    ("Loading history…", "历史装载中…"),
    ("No history yet — the subagent has not finished a turn", "暂无历史——子代理尚未收口任何轮次"),
    ("Subagent history unavailable", "子代理历史不可用（无引擎通道）"),
    // B6 run 级错误卡（阶段 3-B）
    ("Copy full error", "复制完整错误"),
    ("Copy error", "复制错误"),
    ("Open model settings", "打开模型设置"),
    ("Retry", "重试"),
    // ⌘K 命令面板与全局搜索（阶段 3-C：3.2）
    ("Command Palette…", "命令面板…"),
    ("Search sessions or run a command…", "搜索会话或执行命令…"),
    ("Commands", "命令"),
    ("Sessions", "会话"),
    ("Recent Sessions", "最近会话"),
    ("No matching sessions", "没有匹配的会话"),
    // 会话内查找（阶段 3-C：3.8）
    ("Find in Session…", "在会话中查找…"),
    ("Find in conversation", "在对话中查找"),
    ("Previous match", "上一个匹配"),
    ("Next match", "下一个匹配"),
    ("No matches", "无结果"),
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
