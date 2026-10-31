//! 阶段 2.B 冒烟测试（项目规范：UI 测试放独立 `_test.rs`，`#[gpui_kit::test]`）。
//!
//! 覆盖 spec 2.1/2.2/2.3 的验收点：
//! 1. 主题 Global 挂载后 `ActiveTheme` 取值正确（亮/暗各一，token 值对账
//!    theme.md §3 定稿值）；
//! 2. 缩放切档后全局基准字号正确（§2 公式）；
//! 3. 三栏根布局可渲染一帧不 panic；
//! 4. 附带：reduce_motion 降级路径、i18n 取词与回退、窗口几何纯函数
//!    （损坏丢弃 / 越界校验 / 夹屏）。
//!
//! 测试全部走内存态（`theme::set_zoom` 不落盘、不读用户偏好），不会写
//! 真实 `~/.craftwave`。

use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{Hsla, TestAppContext, px, rgb};

use crate::theme::{self, ActiveCraftwave as _, CraftwaveTheme};
use crate::window_state::{self, WindowState};
use crate::{MainWindowView, i18n};

/// 测试装配：与 main() 的启动序一致（init → mount → apply）。
/// 不读 ui-prefs.json（ui_zoom::init 不调，zoom 保持缺省标准档）。
fn boot(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
}

/// 亮色：ActiveTheme 取到的就是 CraftWave 定稿色（theme.md §3 亮色列）。
#[gpui_kit::test]
fn light_theme_slots_take_craftwave_values(cx: &mut TestAppContext) {
    boot(cx);
    cx.update(|cx| {
        assert_eq!(cx.theme().mode, ThemeMode::Light);
        assert_eq!(cx.theme().background, rgb(0xf3f3f2).into());
        assert_eq!(cx.theme().foreground, rgb(0x1f2328).into());
        // 自建槽位同步生成
        assert_eq!(cx.craftwave().colors.main_surface, rgb(0xf8f8f8).into());
        assert_eq!(cx.craftwave().colors.composer_surface, rgb(0xffffff).into());
        assert_eq!(cx.craftwave().colors.dim, rgb(0x8a939e).into());
        // 边框 = foreground @ 10%（§4 生成式）
        assert_eq!(
            cx.theme().border,
            Hsla {
                a: theme::BORDER_ALPHA,
                ..rgb(0x1f2328).into()
            }
        );
    });
}

/// 暗色：切模式后重套（apply 必须跟着 Theme::change 调，否则被默认主题
/// 顶掉——这正是 appearance 订阅里两步走的理由）。
#[gpui_kit::test]
fn dark_theme_slots_take_craftwave_values(cx: &mut TestAppContext) {
    boot(cx);
    cx.update(|cx| {
        Theme::change(ThemeMode::Dark, None, cx);
        theme::apply(cx);
        assert!(cx.theme().mode.is_dark());
        assert_eq!(cx.theme().background, rgb(0x272728).into());
        assert_eq!(cx.theme().foreground, rgb(0xe8e8e9).into());
        assert_eq!(cx.craftwave().colors.main_surface, rgb(0x161616).into());
        assert_eq!(cx.craftwave().colors.composer_surface, rgb(0x272728).into());
        assert_eq!(cx.theme().danger, rgb(0xef4444).into());
        assert_eq!(cx.craftwave().colors.danger_text, rgb(0xf87171).into());
        assert_eq!(cx.theme().primary_foreground, rgb(0x191919).into());
    });
}

/// 缩放四档：基准字号 `Z = round(14 × zoom)`（theme.md §2 档位表逐档对账），
/// 切档后 `Theme::font_size`（rem 源头）随动。
#[gpui_kit::test]
fn zoom_levels_rescale_base_font(cx: &mut TestAppContext) {
    boot(cx);
    // 档位公式逐档对账：0.85→12 / 1.00→14 / 1.15→16 / 1.30→18
    assert_eq!(theme::base_font_size(0.85), 12.0);
    assert_eq!(theme::base_font_size(1.00), 14.0);
    assert_eq!(theme::base_font_size(1.15), 16.0);
    assert_eq!(theme::base_font_size(1.30), 18.0);
    cx.update(|cx| {
        theme::set_zoom(1.30, cx);
        assert_eq!(cx.craftwave().zoom, 1.30);
        assert_eq!(cx.theme().font_size, px(18.0));
        // 字号档公式：font(档) = round(Z + offset)（ui-lg 在特大档 = 20）
        assert_eq!(theme::font_size(theme::FONT_UI_LG, 1.30), 20.0);
        theme::set_zoom(0.85, cx);
        assert_eq!(cx.theme().font_size, px(12.0));
    });
}

/// 三栏根布局可渲染一帧不 panic（亮色装配 + TitleBar + 三栏子树全量走
/// render；GPUI 测试窗口无头绘制，布局/样式错误会直接 panic）。
/// 中栏走真的 chat::install（消息流 + Composer 装置）——阶段 2.C-1 起
/// 占位被对话流替换，冒烟跟着升级。
#[gpui_kit::test]
fn three_pane_layout_renders_a_frame(cx: &mut TestAppContext) {
    boot(cx);
    let (_view, visual) = cx.add_window_view(|window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: crate::chat::install(window, cx),
    });
    // run_until_parked 把首帧 layout/prepaint/paint 跑完——到这就是不 panic
    visual.run_until_parked();
}

/// reduce_motion 生效路径：flag 开启后动效时长归零（theme.md §9）。
#[gpui_kit::test]
fn reduce_motion_zeroes_motion_durations(cx: &mut TestAppContext) {
    boot(cx);
    cx.update(|cx| {
        assert_eq!(theme::motion_duration(theme::DURATION_BASE, cx), theme::DURATION_BASE);
        cx.set_reduce_motion(true);
        assert_eq!(theme::motion_duration(theme::DURATION_BASE, cx), Duration::ZERO);
    });
}

/// i18n：表内键取中文译文，表外键回退英文原文（漏翻译无害）。
#[test]
fn i18n_translates_and_falls_back() {
    assert_eq!(i18n::t("Settings…"), "设置…");
    assert_eq!(i18n::t("Quit CraftWave"), "退出 CraftWave");
    assert_eq!(i18n::t("No Such Key"), "No Such Key");
}

// ---- 窗口几何纯函数（照旧壳 window_state.rs 的单测语义） ----

/// 几何文件：损坏（非法 JSON / 缺字段 / 版本不符 / 宽高非正）整体丢弃。
#[test]
fn corrupted_geometry_is_discarded_whole() {
    let good = WindowState {
        version: window_state::STATE_VERSION,
        x: 10.0,
        y: 20.0,
        width: 1200.0,
        height: 800.0,
        maximized: false,
    };
    let bytes = serde_json::to_vec(&good).unwrap();
    assert_eq!(window_state::load_state(&bytes), Some(good.clone()));
    // 非法 JSON
    assert_eq!(window_state::load_state(b"{"), None);
    // 缺字段
    assert_eq!(window_state::load_state(br#"{"version":1,"x":0.0}"#), None);
    // 版本不符
    let drifted = serde_json::to_string(&good)
        .unwrap()
        .replace(
            &format!("\"version\":{}", window_state::STATE_VERSION),
            "\"version\":99",
        );
    assert_eq!(window_state::load_state(drifted.as_bytes()), None);
    // 宽高非正
    let zero = WindowState {
        height: 0.0,
        ..good
    };
    assert_eq!(
        window_state::load_state(&serde_json::to_vec(&zero).unwrap()),
        None
    );
}

/// 越界校验：与任一显示器的可见交集 ≥ 120×80 才可恢复。
#[test]
fn recoverable_requires_visible_intersection() {
    let screen = [(0.0, 0.0, 1920.0, 1080.0)];
    // 完全在屏内
    assert!(window_state::is_recoverable((100.0, 100.0, 800.0, 600.0), &screen));
    // 大部分出屏但交集恰够 120（-680 + 800 - 0 = 120）
    assert!(window_state::is_recoverable((-680.0, 0.0, 800.0, 600.0), &screen));
    // 交集宽不足 120（-700 + 800 - 0 = 100）
    assert!(!window_state::is_recoverable((-700.0, 0.0, 800.0, 600.0), &screen));
    // 完全出屏
    assert!(!window_state::is_recoverable((5000.0, 5000.0, 800.0, 600.0), &screen));
    // 任一屏命中即可（双屏，窗口跨在第二屏上）
    let two = [(0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0, 1920.0, 1080.0)];
    assert!(window_state::is_recoverable((2000.0, 100.0, 800.0, 600.0), &two));
}

/// 夹屏：尺寸先按屏幕封顶，再把原点限制在屏内。
#[test]
fn fit_in_clamps_to_screen() {
    let screen = (0.0, 0.0, 1920.0, 1080.0);
    // 尺寸超屏 → 封顶
    assert_eq!(window_state::fit_in((0.0, 0.0, 3000.0, 2000.0), screen), (0.0, 0.0, 1920.0, 1080.0));
    // 原点出屏右侧 → 夹回屏内右缘
    assert_eq!(window_state::fit_in((5000.0, 0.0, 800.0, 600.0), screen), (1120.0, 0.0, 800.0, 600.0));
    // 负原点 → 夹回 0
    assert_eq!(window_state::fit_in((-50.0, -50.0, 800.0, 600.0), screen), (0.0, 0.0, 800.0, 600.0));
}
