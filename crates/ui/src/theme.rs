//! CraftWave 主题（gpui-rewrite 阶段 2.2）。
//!
//! 本文件是 [`docs/specs/gpui-rewrite/theme.md`](../../docs/specs/gpui-rewrite/theme.md)
//! 的直接实现：token 数值全部照抄该表（改任何值走显式提交，spec 定稿原则 6），
//! 各节编号与文档一一对应，便于对账。
//!
//! ## 主题怎么「挂」进 gpui-component
//!
//! gpui-component（gpui-kit 门面内）有一个全局 [`Theme`]（实现 `Global`），
//! 组件全部经 `ActiveTheme::theme()`（即 `cx.theme()`）读色。我们的做法不是
//! 另起炉灶，而是：
//!
//! 1. `gpui_kit::init(cx)` 已把 gpui-component 默认主题挂进 Global（含
//!    `Theme::change(Light)` 加载的默认 `ThemeColor`）；
//! 2. [`apply`] 在此之上**覆写**我们表里列出的槽位——表未列的槽位保持
//!    gpui-component 默认值（theme.md §1「未列槽位保持默认」），避免从
//!    `ThemeColor::default()`（全透明黑）打底导致未列槽位整体透明；
//! 3. `ActiveTheme` 取到的就是 CraftWave 色（theme.md §11 验收条目）。
//!
//! gpui-component 没有的槽位（三档背景面、前景淡档、dim 等）落自建 Global
//! [`CraftwaveTheme`]（theme.md §3「自建」列 + §11.1），由同一份亮/暗定义
//! 生成，保证与 ThemeColor 槽位永远同源同模式。
//!
//! ## 与缩放（ui_zoom）的关系
//!
//! 字号类 token 不存死值：实现里只存 offset 常量（§5），运行时经
//! `Z = round(14 × zoom)` 换算（§2）。缩放档住在 [`CraftwaveTheme::zoom`]，
//! 切档由 `ui_zoom` 模块驱动（重算 `Theme::font_size` 并刷新窗口）。

use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeColor, ThemeMode};
use gpui_kit::{App, Global, Hsla, Window, px};

// ======================================================================
// §2 缩放规则（ui_zoom，spec 2.3）
// ======================================================================

/// 全局字号基准系数（theme.md §2：`Z(px) = round(14 × zoom)` 里的 14）。
pub const Z_BASE_PX: f32 = 14.0;

/// 全局字号基准 Z：随缩放档位变化的排版根（`Theme::font_size` 即取此值，
/// gpui-component 的组件按 rem 排版、Root 每帧把 rem 设成 font_size）。
pub fn base_font_size(zoom: f32) -> f32 {
    (Z_BASE_PX * zoom).round()
}

/// 字号档取值（theme.md §2：`font(档) = round(Z + offset)`）。
///
/// 实现里只存 offset 常量（§5），运行时以本公式换算——禁止在调用点写死 px。
pub fn font_size(offset: f32, zoom: f32) -> f32 {
    (base_font_size(zoom) + offset).round()
}

/// Markdown 标题字号（theme.md §2 乘法体系：`h1 = 1.45Z` 等，round 取整）。
pub fn markdown_heading(coefficient: f32, zoom: f32) -> f32 {
    (base_font_size(zoom) * coefficient).round()
}

// ======================================================================
// 颜色基础工具
// ======================================================================

/// `#rrggbb` 整形字面量 → Hsla（theme.md §3 的值格式）。
fn hex(value: u32) -> Hsla {
    gpui_kit::rgb(value).into()
}

/// 透明度混合（theme.md §4：边框与淡档的生成式——亮暗一套 alpha 逻辑，
/// 前景反相自动成立）。`color` 必须是 foreground 这类「基色」。
fn alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

// ---- §4 透明度常量（边框与淡档的生成参数，单点实现，组件不手混） ----

/// 全部 1px 边框、分隔线、滚动条 thumb：foreground @ 10%。
pub const BORDER_ALPHA: f32 = 0.10;
/// 悬停边框（choice-card hover 等）：foreground @ 15%。
pub const BORDER_HOVER_ALPHA: f32 = 0.15;
/// 亮色悬停面：foreground @ 5%。
pub const HOVER_FACE_ALPHA_LIGHT: f32 = 0.05;
/// 暗色悬停面：foreground @ 10%。
pub const HOVER_FACE_ALPHA_DARK: f32 = 0.10;
/// 淡一档前景（项目名等）：foreground @ 60%。
pub const SUBTLE_ALPHA: f32 = 0.60;
/// 最淡前景（组标题、列表 marker）亮色：foreground @ 40%。
pub const SUBTLEST_ALPHA_LIGHT: f32 = 0.40;
/// 最淡前景暗色：foreground @ 30%（ZCode 同款差异，theme.md §3.1）。
pub const SUBTLEST_ALPHA_DARK: f32 = 0.30;

/// 边框色生成函数（§4 单点实现）：`border = foreground @ 10%`。
pub fn border_color(foreground: Hsla) -> Hsla {
    alpha(foreground, BORDER_ALPHA)
}

/// 悬停边框色生成函数：`border_hover = foreground @ 15%`。
pub fn border_hover_color(foreground: Hsla) -> Hsla {
    alpha(foreground, BORDER_HOVER_ALPHA)
}

// ======================================================================
// §3 颜色 token 表
// ======================================================================

/// 亮色一套（theme.md §3 各表「亮色」列，逐槽照抄）。
fn apply_light(c: &mut ThemeColor) -> CraftwaveColors {
    let fg = hex(0x1f2328);
    let focus = hex(0x2563eb);

    c.background = hex(0xf3f3f2);
    c.foreground = fg;
    c.sidebar = hex(0xf3f3f2);
    c.popover = hex(0xffffff);
    c.muted = hex(0xeef0f3);
    c.muted_foreground = hex(0x57606a);
    c.input = hex(0xcfd5db);
    // selected（亮）= foreground @ 5%（§4 hover_face 亮色规则）
    c.list_hover = alpha(fg, HOVER_FACE_ALPHA_LIGHT);
    c.primary = fg;
    c.primary_foreground = hex(0xffffff);
    c.accent = hex(0xe8eaee);
    c.accent_foreground = fg;
    // focus（强调蓝）：ring + link 同值双槽
    c.ring = focus;
    c.link = focus;
    c.danger = hex(0xdc2626);
    c.warning = hex(0xb45309);
    c.warning_foreground = hex(0xffffff);
    c.success = hex(0x16a34a);
    c.border = border_color(fg);
    c.title_bar = hex(0xf3f3f2);
    c.title_bar_border = c.border;
    c.caret = fg;
    // 侧栏选中态另用 sidebar_accent（值同 selected，槽位独立以便分化）
    c.sidebar_accent = c.list_hover;
    c.sidebar_accent_foreground = fg;
    c.sidebar_border = c.border;
    c.sidebar_foreground = fg;

    // 自建槽位（亮色）
    CraftwaveColors {
        main_surface: hex(0xf8f8f8),
        composer_surface: hex(0xffffff),
        foreground_subtle: alpha(fg, SUBTLE_ALPHA),
        foreground_subtlest: alpha(fg, SUBTLEST_ALPHA_LIGHT),
        dim: hex(0x8a939e),
        danger_text: hex(0xd03066),
    }
}

/// 暗色一套（theme.md §3 各表「暗色」列，逐槽照抄）。
fn apply_dark(c: &mut ThemeColor) -> CraftwaveColors {
    let fg = hex(0xe8e8e9);
    let focus = hex(0x3b82f6);

    c.background = hex(0x272728);
    c.foreground = fg;
    c.sidebar = hex(0x272728);
    c.popover = hex(0x161616);
    c.muted = hex(0x262626);
    c.muted_foreground = hex(0x9b9b9d);
    c.input = hex(0x3f3f41);
    // selected（暗）= foreground @ 10%（§4 hover_face 暗色规则）
    c.list_hover = alpha(fg, HOVER_FACE_ALPHA_DARK);
    c.primary = fg;
    c.primary_foreground = hex(0x191919);
    c.accent = hex(0x313133);
    c.accent_foreground = fg;
    c.ring = focus;
    c.link = focus;
    c.danger = hex(0xef4444);
    c.warning = hex(0xf59e0b);
    c.warning_foreground = hex(0x17191c);
    c.success = hex(0x22c55e);
    c.border = border_color(fg);
    c.title_bar = hex(0x272728);
    c.title_bar_border = c.border;
    c.caret = fg;
    c.sidebar_accent = c.list_hover;
    c.sidebar_accent_foreground = fg;
    c.sidebar_border = c.border;
    c.sidebar_foreground = fg;

    // 自建槽位（暗色）
    CraftwaveColors {
        main_surface: hex(0x161616),
        composer_surface: hex(0x272728),
        foreground_subtle: alpha(fg, SUBTLE_ALPHA),
        foreground_subtlest: alpha(fg, SUBTLEST_ALPHA_DARK),
        dim: hex(0x7c7c7e),
        danger_text: hex(0xf87171),
    }
}

/// 构造指定模式的完整主题色（theme.md §11.2 的 `fn theme_colors(mode)`）。
///
/// 与文档签名的一处偏差：多收一个 `base`（gpui-component 当前模式的默认
/// `ThemeColor`）。原因：`ThemeColor` 有一百多个槽位而 token 表只钉了其中
/// 二十多个，`ThemeColor::default()` 是全透明黑，从零打底会让未列槽位
/// （按钮 hover 族、表格、弹层遮罩……）整体透明；以默认主题为底覆写，
/// 正好落实 theme.md §1「未列槽位保持 gpui-component 默认或由派生规则
/// 自动补」。`selection`（系统选区色）与 `overlay` 等未列槽位因此自然保留。
pub fn theme_colors(mode: ThemeMode, base: &ThemeColor) -> (ThemeColor, CraftwaveColors) {
    let mut colors = *base;
    let extra = if mode.is_dark() {
        apply_dark(&mut colors)
    } else {
        apply_light(&mut colors)
    };
    (colors, extra)
}

// ======================================================================
// CraftwaveTheme Global（自建槽位 + 缩放状态，theme.md §11.1）
// ======================================================================

/// gpui-component `ThemeColor` 没有的自建槽位（theme.md §3 标「自建」的行）。
///
/// 由 [`theme_colors`] 与 ThemeColor 同源生成、同模式切换，消费方经
/// [`ActiveCraftwave::craftwave`] 取用。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CraftwaveColors {
    /// 中部会话区底（比外围浅/深一档的定稿层次）。
    pub main_surface: Hsla,
    /// composer 卡底（亮=卡白，暗=同侧栏 #272728）。
    pub composer_surface: Hsla,
    /// 项目名等淡一档前景（fg @60%）。
    pub foreground_subtle: Hsla,
    /// 最淡前景（组标题、列表 marker；亮 @40% / 暗 @30%）。
    pub foreground_subtlest: Hsla,
    /// 设置组标题 / 次级图标（cw-dim）。
    pub dim: Hsla,
    /// 表单错误红 / diff 统计红（cw-err，亮色偏品红的定稿值）。
    pub danger_text: Hsla,
}

/// 自建主题 Global：自建颜色槽位（当前激活模式）+ 缩放档状态（§11.1）。
pub struct CraftwaveTheme {
    pub colors: CraftwaveColors,
    /// 当前缩放倍率（0.85/1.00/1.15/1.30，spec 2.3）。切档由 ui_zoom 模块
    /// 经 [`set_zoom`] 驱动：重算全局字号并刷新窗口。
    pub zoom: f32,
}

impl Global for CraftwaveTheme {}

/// 按缩放档重算全局字号（`Theme::font_size` 是 rem 的源头）与等宽字号。
/// 变高列表的 remeasure 钩子本阶段无列表（spec 2.3），阶段 2.4 消息流
/// 底座接入时在 ListState 侧按 zoom 缓存行高重测（见 ui_zoom::apply_zoom 注释）。
fn write_font_metrics(zoom: f32, theme: &mut Theme) {
    theme.font_size = px(font_size(0.0, zoom));
    // 等宽字号随 ui-sm 档（辅助说明、代码块内，theme.md §5）
    theme.mono_font_size = px(font_size(FONT_UI_SM, zoom));
}

/// 取自建主题（自建槽位 + zoom）的访问口，与 gpui-component 的
/// `ActiveTheme` 同款形态（为 `App` 实现，`Context` 经 Deref 自动可用）。
pub trait ActiveCraftwave {
    fn craftwave(&self) -> &CraftwaveTheme;
}

impl ActiveCraftwave for App {
    #[inline(always)]
    fn craftwave(&self) -> &CraftwaveTheme {
        self.global::<CraftwaveTheme>()
    }
}

impl CraftwaveTheme {
    /// 首次挂载（启动/测试装置）：zoom 缺省 1.00，颜色先落亮色一套，
    /// 随后的 [`apply`] 会按实际模式重算。
    pub fn mount(cx: &mut App) {
        cx.set_global(Self {
            colors: CraftwaveColors {
                main_surface: Hsla::default(),
                composer_surface: Hsla::default(),
                foreground_subtle: Hsla::default(),
                foreground_subtlest: Hsla::default(),
                dim: Hsla::default(),
                danger_text: Hsla::default(),
            },
            zoom: 1.00,
        });
    }

    /// 换缩放档（不落盘、不刷新窗口——落盘与窗口刷新由 ui_zoom 编排）。
    fn set_zoom_state(&mut self, zoom: f32) {
        self.zoom = zoom;
    }
}

// ======================================================================
// 挂载与外观切换
// ======================================================================

/// 按当前模式把 CraftWave token 覆写进全局 [`Theme`]，并同步自建 Global。
///
/// 启动后调一次；此后每次外观变化（系统深浅色切换）都要重新调用——
/// `Theme::change` / `sync_system_appearance` 会重载注册表默认主题，把
/// 覆写冲掉（Wake 同款时序）。
pub fn apply(cx: &mut App) {
    let mode = cx.theme().mode;
    let base = cx.theme().colors;
    let (colors, extra) = theme_colors(mode, &base);
    // zoom 在 CraftwaveTheme 上（main 的装配顺序是 mount → apply，测试装置同样）
    let zoom = cx.global::<CraftwaveTheme>().zoom;
    // Theme::update 是 gpui-component 的统一写路径：colors 与 tokens（渲染
    // 面底色）同步、Base 投影重建、全部窗口刷新，见其文档（不能只写
    // global_mut，那会留下 colors/tokens/Base 三处不同步的旧值）
    Theme::update(cx, |theme| {
        theme.colors = colors;
        // 系统字体栈（theme.md §10 font-ui）：`.SystemUIFont` 是 gpui 各平台
        // 后端都认的别名（macOS 解到 SF，中文回落 PingFang SC 由系统字体
        // 级联负责），写死 `.AppleSystemUIFont` 会让非 mac 平台走回落错误路径
        theme.font_family = ".SystemUIFont".into();
        // 等宽族（theme.md §10 font-mono）：macOS Menlo 即 ui-monospace 栈的
        // 首个可用项；gpui-component 默认已按平台选好，此处不覆写
        write_font_metrics(zoom, theme);
        // 圆角两档随 zoom（组件默认圆角 = rounded-lg 8 / rounded-xl 12，
        // theme.md §8；组件直接读像素值，一并乘缩放）
        theme.radius = px(rounded_lg(zoom));
        theme.radius_lg = px(rounded_xl(zoom));
        // Wake 踩坑同款（2026-09-03）：聚焦环画在元素框外 2px，任何裁切
        // 祖先都会切掉一截；关掉后聚焦态只染边框，不占空间
        theme.focus_ring = false;
        theme.shadow = true;
    });
    cx.global_mut::<CraftwaveTheme>().colors = extra;
}

/// 换缩放档（不落盘——落盘与窗口刷新由 ui_zoom 编排）：更新 zoom 状态并
/// 让字号/圆角档按新倍率重算（theme.md §11.1「setter 触发全局字号失效」）。
pub fn set_zoom(zoom: f32, cx: &mut App) {
    cx.global_mut::<CraftwaveTheme>().set_zoom_state(zoom);
    apply(cx);
}

/// 深浅色跟随系统（spec 2.1）：同步模式 → 重套 CraftWave token。
pub fn sync_system_appearance(window: Option<&mut Window>, cx: &mut App) {
    Theme::sync_system_appearance(window, cx);
    apply(cx);
}

// ======================================================================
// §5 字号档（八档 offset，全部经 font_size 公式取值）
// ======================================================================

/// ui-2xs：徽标极小字（当前无消费方，保留体系完整）。
pub const FONT_UI_2XS: f32 = -5.0;
/// ui-xs：徽标小字（保留档）。
pub const FONT_UI_XS: f32 = -4.0;
/// ui-sm：辅助说明、代码块内、组标题。
pub const FONT_UI_SM: f32 = -2.0;
/// ui-caption：时间戳、凭据 radio、表单 hint。
pub const FONT_UI_CAPTION: f32 = -1.0;
/// ui-base：正文基准（offset 0，即 Z 本身）。
pub const FONT_UI_BASE: f32 = 0.0;
/// ui-lg：Markdown h2、section 标题。
pub const FONT_UI_LG: f32 = 2.0;
/// ui-xl：旧栈 h1 档（新壳 h1 走 §2 系数 1.45Z）。
pub const FONT_UI_XL: f32 = 4.0;
/// ui-settings-title：设置页操作条标题。
pub const FONT_UI_SETTINGS_TITLE: f32 = 10.0;
/// thinking 展开小字：`Z − 2.5`（标准档 11.5px，rendering.md 钉值）。
/// 渲染层 round 取整，offset 保持 -2.5 精确表达。
pub const FONT_UI_THINKING: f32 = -2.5;

// ======================================================================
// §2 Markdown 标题系数（rendering.md 定稿，乘法体系）
// ======================================================================

/// h1 = 1.45Z
pub const MD_H1: f32 = 1.45;
/// h2 = 1.28Z
pub const MD_H2: f32 = 1.28;
/// h3 = 1.14Z
pub const MD_H3: f32 = 1.14;
/// h4 = 1.05Z
pub const MD_H4: f32 = 1.05;
// h5+ = Z（系数 1.0，无需常量）

// ======================================================================
// §6 行高档（八档，固定倍数，不随 zoom 变）
// ======================================================================

/// leading-tight（设置页大标题）
pub const LEADING_TIGHT: f32 = 1.3;
/// leading-heading（Markdown 标题）
pub const LEADING_HEADING: f32 = 1.4;
/// leading-row（设置行描述 / section 标题）
pub const LEADING_ROW: f32 = 1.45;
/// leading-code（代码块）
pub const LEADING_CODE: f32 = 1.5;
/// leading-thinking（thinking 展开区左导线纯文本）
pub const LEADING_THINKING: f32 = 1.7;
/// leading-prose（旧栈 Markdown 容器对照基准；新壳正文用 leading-assistant）
pub const LEADING_PROSE: f32 = 1.75;
/// leading-user（User 气泡正文，rendering.md 钉值）
pub const LEADING_USER: f32 = 1.85;
/// leading-assistant（Assistant 正文，rendering.md 钉值）
pub const LEADING_ASSISTANT: f32 = 1.9;

// ======================================================================
// §7 间距档（px 常量，4 基为主；现行 UI 实测值照抄）
// ======================================================================

/// space-1：文案与控件小缝
pub const SPACE_1: f32 = 4.0;
/// space-2：图标与文字、导航头 gap
pub const SPACE_2: f32 = 6.0;
/// space-3：面板 padding 基数、按钮组 gap、行卡间距
pub const SPACE_3: f32 = 8.0;
/// space-4：设置行纵向 padding、端点块 padding
pub const SPACE_4: f32 = 10.0;
/// space-5：卡片网格 gap、代码块体 padding、操作条 gap
pub const SPACE_5: f32 = 12.0;
/// space-6：Markdown 段落 margin、列表行 gap、代码块 margin
pub const SPACE_6: f32 = 16.0;
/// space-7：设置卡横向 padding、列表缩进
pub const SPACE_7: f32 = 20.0;
/// space-8：设置行控件 gap、弹框 mask padding、组间分隔
pub const SPACE_8: f32 = 24.0;
/// space-9：离开确认弹框 padding
pub const SPACE_9: f32 = 28.0;
/// space-10：设置页体横向 padding、操作条 padding
pub const SPACE_10: f32 = 32.0;
/// space-11：设置页体顶部 padding
pub const SPACE_11: f32 = 36.0;
/// space-12：窄容器页体底部 padding
pub const SPACE_12: f32 = 48.0;
/// space-13：设置页体底部 padding（滚动呼吸）
pub const SPACE_13: f32 = 72.0;

// ======================================================================
// §8 圆角档（A3，随嵌套递减，禁止跨级混用；随 zoom 的两档由
// Theme::radius/radius_lg 承载，此处为自绘取值）
// ======================================================================

/// rounded-2xl：仅白名单（composer 输入壳、弹框外壳）
pub fn rounded_2xl(zoom: f32) -> f32 {
    (16.0 * zoom).round()
}
/// rounded-xl：首层面板（会话大圆角卡、设置 section 卡）
pub fn rounded_xl(zoom: f32) -> f32 {
    (12.0 * zoom).round()
}
/// rounded-lg：菜单/下拉外壳、行卡、代码块、虚线按钮
pub fn rounded_lg(zoom: f32) -> f32 {
    (8.0 * zoom).round()
}
/// rounded-md：菜单项、按钮、行内小件
pub fn rounded_md(zoom: f32) -> f32 {
    (6.0 * zoom).round()
}
/// rounded-sm：行内最小件（badge、marker）
pub fn rounded_sm(zoom: f32) -> f32 {
    (4.0 * zoom).round()
}
/// full：单选圆点、滚动条 thumb、脏点（gpui 渲染端自动 clamp，给足即可）
pub const ROUNDED_FULL: f32 = 9999.0;

// ======================================================================
// §9 动效时长档（A6 + B3/B4）+ reduce_motion 降级
// ======================================================================

/// duration-fast：浮层入场、hover 过渡
pub const DURATION_FAST: Duration = Duration::from_millis(120);
/// duration-base：通用状态过渡（卡片边框/底色）
pub const DURATION_BASE: Duration = Duration::from_millis(160);
/// duration-enter：面板/抽屉进入
pub const DURATION_ENTER: Duration = Duration::from_millis(200);
/// duration-rise：空状态建议 chips 瀑布入场（逐个 delay）
pub const DURATION_RISE: Duration = Duration::from_millis(260);

/// CraftWave 统一过渡曲线 `ease_cw = cubic-bezier(0.22, 0.61, 0.36, 1)`
/// 的 gpui 近似（theme.md §9 允许「以 ease_out_quart 近似」）：四次幂
/// ease-out 前段冲得快、尾段缓收，形状与设计曲线肉眼无差。
///
/// shortcut: 近似曲线而非精确贝塞尔（精确解需牛顿迭代三次方程），阶段 2.C
/// 真接入 gpui 动画时若逐帧对比发现偏差再换自写贝塞尔。
pub fn ease_cw(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

/// 动效时长取值：`reduce_motion` 全局降级开关生效路径——开启时全部
/// 过渡/循环动画时长归零，保留可读静态状态（theme.md §9）。
///
/// flag 本体由 `gpui_kit::init`（gpui-base）从系统无障碍设置读入
/// （macOS「减弱动态效果」等），自绘动画一律经本函数取时长即自动降级；
/// 测试里 `cx.set_reduce_motion(true)` 一帧到位。
pub fn motion_duration(base: Duration, cx: &App) -> Duration {
    if cx.reduce_motion() {
        Duration::ZERO
    } else {
        base
    }
}

// ======================================================================
// §10 字体与图标档
// ======================================================================

/// 图标档（A7）：icon-sm 12px
pub const ICON_SM: f32 = 12.0;
/// icon-md 14px
pub const ICON_MD: f32 = 14.0;
/// icon-lg 16px（默认档）
pub const ICON_LG: f32 = 16.0;
/// icon-xl 20px
pub const ICON_XL: f32 = 20.0;
