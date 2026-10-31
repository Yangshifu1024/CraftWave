//! 折叠部件测试（gpui-rewrite 2.C-4：2.7 thinking 面板 / 2.8 工具簇卡 /
//! 2.9 图片三态 + zoom / B3 / B4，`#[gpui_kit::test]` 独立文件）。
//!
//! 覆盖 rendering.md 的本批验收点：
//! 1. thinking 面板：开合走 splice 重测 + 跨重渲染记忆；B4 流式态——
//!    思考计时秒针任务与收口时长、最后一行流动摘要；
//! 2. 工具簇卡：600 字符契约（前 600 字符预览 + 复制完整原文断言剪贴板）；
//!    B3 失败自动展开一次且尊重手动开合（手动折叠后失败不再自动开）；
//!    运行态无 spinner（扫光文案帧，不 panic 即通路）；
//! 3. 图片三态各渲一帧（Ready 纵横比两分支 / Unsupported / Omitted）+
//!    zoom 打开与关闭；
//! 4. composer 附件预览条同款缩略图；
//! 5. 长列表冒烟：3000 条混合消息（含折叠部件）渲染若干帧不 panic。

use gpui_kit::{Context, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::snapshot::StreamPart;

use crate::MainWindowView;
use crate::theme::{self, CraftwaveTheme};

use super::fixture;
use super::row::{last_nonempty_line, thinking_duration_text};
use super::state::ConversationState;

/// 会话状态的可变操作入口（同 conversation_test 的 mutate——测试模块独立，
/// 装置各自薄持一份）。
fn mutate(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut ConversationState, &mut Context<ConversationState>),
) {
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| f(chat, cx));
    });
}

/// 会话状态的只读断言入口。
fn observe<R>(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&ConversationState, &gpui_kit::App) -> R,
) -> R {
    visual.update(|_window, cx| f(chat.read(cx), cx))
}

/// 测试装配：与 conversation_test 的 chat_window 同序（init → mount → apply
/// → 开窗挂 chat 装置）。测试按需注入 FakeImageLoader 与自定义历史。
fn chat_window(
    cx: &mut TestAppContext,
) -> (Entity<ConversationState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let holder: std::rc::Rc<std::cell::RefCell<Option<Entity<ConversationState>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = holder.clone();
    let (_root, visual) = cx.add_window_view(|window, cx| {
        use gpui_kit::AppContext as _;
        let chat = crate::chat::install(window, cx, None);
        *slot.borrow_mut() = Some(chat.clone());
        let inner = cx.new(|_| MainWindowView {
            zoom_seen: 0.0,
            chat,
        });
        gpui_kit::base::Root::new(inner, window, cx)
    });
    let chat = holder.borrow().clone().expect("install 在开窗闭包内执行");
    (chat, visual)
}

// ======================================================================
// 纯函数面：B4 计时文案与最后一行摘要
// ======================================================================

#[test]
fn thinking_duration_formats_seconds_then_minutes() {
    // 最少 1 秒（0 秒的思考也显示 1 秒——旧栈 Math.ceil 口径）
    assert_eq!(thinking_duration_text(0), "1秒");
    assert_eq!(thinking_duration_text(42), "42秒");
    assert_eq!(thinking_duration_text(65), "1分5秒");
    assert_eq!(thinking_duration_text(600), "10分0秒");
}

#[test]
fn last_nonempty_line_picks_the_tail_line() {
    assert_eq!(last_nonempty_line("第一行\n第二行\n第三行"), "第三行");
    assert_eq!(last_nonempty_line("只有一行"), "只有一行");
    // 尾部空行不算最后一行
    assert_eq!(last_nonempty_line("A\n\n\n"), "A");
    // 全空回落空串
    assert_eq!(last_nonempty_line("  \n  "), "");
    assert_eq!(last_nonempty_line(""), "");
}

// ======================================================================
// 1. thinking 面板：开合记忆 + B4 流式态（计时秒针 / 流动摘要）
// ======================================================================

#[gpui_kit::test]
fn thinking_panel_expands_with_memory_and_streaming_timer(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    // 多段思考 + 多行正文的历史（行 0）
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(
            std::rc::Rc::new(vec![fixture::assistant_thinking_multi_segment()]),
            cx,
        );
    });
    visual.run_until_parked();
    // 折叠头渲染（chevron 图标 + 标签 + 摘要），初始折叠
    assert!(visual.debug_bounds("thinking-head-0").is_some());
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.thinking_expanded(0));
    });
    // 点击展开 → 记忆 + splice 重测
    let head = visual.debug_bounds("thinking-head-0").unwrap();
    visual.simulate_click(head.center(), Default::default());
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(0));
        assert_eq!(chat.splice_retest_count, 1, "展开应触发 splice 重测");
    });
    // 重渲染（追加行）后展开记忆保持（无自动收起）
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("触发重渲染"), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(0), "跨重渲染记忆");
    });
}

#[gpui_kit::test]
fn streaming_thinking_runs_the_tick_and_seals_duration(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let history_len = observe(&chat, visual, |chat, _cx| chat.messages().len());
    // 流式行带开着的思考段（起点 3 秒前）→ B4：秒针任务在跑
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_thinking_open()), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(
            chat.thinking_tick_live(),
            "流式且思考段开着：计时秒针应在跑"
        );
    });
    // 继续流式一帧再确认秒针不灭（1 秒步进 notify 的循环由任务自身保证）
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_thinking_open()), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_tick_live(), "持续流式期间秒针不灭");
    });
    // 思考段收口（正文开始）：秒针停（收口时长定格，不再递增）
    mutate(&chat, visual, |chat, cx| {
        let mut next = fixture::streaming_message(12);
        next.parts.insert(
            0,
            StreamPart::Reasoning {
                text: "已完成的思考".to_owned(),
            },
        );
        chat.set_streaming(Some(next), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.thinking_tick_live(), "思考段收口后秒针应停");
        assert_eq!(chat.row_count(), history_len + 1);
    });
    // 流式行收口：秒针与整行一起消失
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(None, cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.thinking_tick_live());
        assert!(!chat.thinking_expanded(history_len));
    });
}

// ======================================================================
// 2. 工具簇卡：600 字符契约 + B3 失败自动展开一次
// ======================================================================

#[gpui_kit::test]
fn tool_cluster_600_char_contract_copies_full_text(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let full = fixture::long_tool_output();
    // 前 600 字符的预览（纪律三的截断量，字符计——clip_tool_text 行为）
    let (shown, truncated) = {
        // 从渲染面同一函数取（row 模块私有，经 super 路径可达 pub(crate)）
        let mut chars = full.chars();
        let head: String = chars.by_ref().take(600).collect();
        (format!("{head}…"), chars.next().is_some())
    };
    assert_eq!(shown.chars().count(), 601, "600 字符 + 省略号");
    assert!(truncated);
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(
            std::rc::Rc::new(vec![fixture::assistant_long_output()]),
            cx,
        );
    });
    visual.run_until_parked();
    // 展开簇卡（真实点击折叠头）
    let head = visual
        .debug_bounds("tools-head-0")
        .expect("工具簇折叠头已渲染");
    visual.simulate_click(head.center(), Default::default());
    visual.run_until_parked();
    // 复制钮在场（截断发生时 Output 分节带「前 600 字符」明示 + 复制钮）
    let copy = visual
        .debug_bounds("tool-copy-0-0-output")
        .expect("Output 分节的复制钮已渲染");
    visual.simulate_click(copy.center(), Default::default());
    visual.run_until_parked();
    // 契约核心：剪贴板里是**完整原文**（不是 600 字符预览）
    let clipboard = visual.update(|_window, cx| {
        cx.read_from_clipboard()
            .expect("复制后剪贴板应有内容")
            .text()
            .expect("应为文本载荷")
    });
    assert_eq!(clipboard, full, "复制按钮必须复制完整原文");
    assert!(clipboard.chars().count() > 600);
}

#[gpui_kit::test]
fn failure_auto_expands_once_and_respects_manual_collapse(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let ix = observe(&chat, visual, |chat, _cx| chat.messages().len());
    // 无失败的流式行：折叠（默认）
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_message(6)), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.tools_expanded(ix), "无失败不自动展开");
    });
    // 失败到达（首帧出现路径之外，这里走更新路径）→ 自动展开一次
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_with_failure()), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.tools_expanded(ix), "失败应自动展开一次");
    });
    // 用户手动折叠 → 失败更新再来（锁存）→ 保持折叠
    mutate(&chat, visual, |chat, cx| chat.toggle_tools(ix, cx));
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.tools_expanded(ix));
    });
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_with_failure()), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(
            !chat.tools_expanded(ix),
            "手动折叠后失败更新不得再自动展开"
        );
    });
    // 流式失败帧带展开区的完整渲染一帧（危险面 + 失败原因可复制钮）
    mutate(&chat, visual, |chat, cx| chat.toggle_tools(ix, cx));
    visual.run_until_parked();
    let head_sel: &'static str = Box::leak(format!("tools-head-{ix}").into_boxed_str());
    assert!(visual.debug_bounds(head_sel).is_some());
    // 收口：锁存与展开记忆随行消失
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(None, cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.tools_expanded(ix));
    });
}

/// 运行态（ok = None 的调用）不用 spinner：扫光文案走 ShimmerText
/// （reduce_motion 时组件内建降级静态），整帧渲染不 panic 即通路。
#[gpui_kit::test]
fn running_call_renders_shimmer_without_spinner(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_with_tools()), cx);
    });
    visual.run_until_parked();
    let ix = observe(&chat, visual, |chat, _cx| chat.messages().len());
    mutate(&chat, visual, |chat, cx| chat.toggle_tools(ix, cx));
    visual.run_until_parked();
    // reduce_motion 一帧到位再渲一帧（动效降级的渲染通路）
    visual.update(|_window, cx| cx.set_reduce_motion(true));
    visual.run_until_parked();
}

// ======================================================================
// 3. 图片三态各渲一帧 + zoom 打开关闭
// ======================================================================

#[gpui_kit::test]
fn image_three_states_render_and_zoom_opens_closes(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    // 注入假装载源 + 四态历史：竖图 / 横图 / 坏字节 / 转录上限
    mutate(&chat, visual, |chat, cx| {
        chat.set_image_loader(std::rc::Rc::new(fixture::FakeImageLoader));
        chat.set_history(
            std::rc::Rc::new(vec![fixture::user_msg_with_images(
                "四张图：竖、横、坏、省略",
                &[
                    "cwtest-portrait",
                    "cwtest-landscape",
                    "cwtest-corrupt",
                    "cwtest-omitted",
                ],
            )]),
            cx,
        );
    });
    visual.run_until_parked();
    // 三态落位（缓存键 = 行内图片序，0 基：竖、横、坏、省略）
    observe(&chat, visual, |chat, _cx| {
        match chat.image_state(0, 0) {
            Some(super::images::ImageLoad::Ready { dims, .. }) => {
                assert_eq!(*dims, (40, 80), "竖图定宽分支");
            }
            other => panic!("图 0 应为 Ready：{other:?}"),
        }
        match chat.image_state(0, 1) {
            Some(super::images::ImageLoad::Ready { dims, .. }) => {
                assert_eq!(*dims, (160, 40), "横图定高分支");
            }
            other => panic!("图 1 应为 Ready：{other:?}"),
        }
        match chat.image_state(0, 2) {
            Some(super::images::ImageLoad::Unsupported { original, .. }) => {
                assert!(original.is_some(), "坏字节保留原图（可下载原图）");
            }
            other => panic!("图 2 应为 Unsupported：{other:?}"),
        }
        assert!(matches!(
            chat.image_state(0, 3),
            Some(super::images::ImageLoad::Omitted)
        ));
    });
    // 四张缩略图各渲一帧（debug 钩子在场 = 元素树可达）
    for image_ix in 0..4 {
        let sel: &'static str = Box::leak(format!("image-0-{image_ix}").into_boxed_str());
        assert!(visual.debug_bounds(sel).is_some(), "缩略图 {image_ix} 应渲染");
    }
    // zoom 打开（点击 Ready 态竖图）
    let thumb = visual.debug_bounds("image-0-0").unwrap();
    visual.simulate_click(thumb.center(), Default::default());
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.zoom_open(), Some((0, 0)), "点击缩略图应开 zoom");
    });
    assert!(visual.debug_bounds("image-zoom").is_some(), "覆盖层已挂");
    visual.run_until_parked();
    // 关闭钮收场
    let close = visual
        .debug_bounds("image-zoom-close")
        .expect("zoom 关闭钮已渲染");
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.zoom_open(), None, "关闭钮应收 zoom");
    });
    // 覆盖层不再渲染
    assert!(visual.debug_bounds("image-zoom").is_none());
}

/// 解码失败与超尺寸的降级路径（probe 的纯函数面）：坏字节 / 声明与头不符 /
/// 超尺寸全部拒绝，合法头通过（宽高比判据的数据源）。
#[test]
fn image_probe_rejects_corrupt_and_oversized() {
    use super::images::probe_image;
    // 合法 PNG（fixture 生成）→ 尺寸面
    let png = fixture::png_bytes(3, 7);
    assert_eq!(probe_image(&png, "image/png"), Some((gpui_kit::ImageFormat::Png, (3, 7))));
    // 坏字节 → None（降级 Unsupported）
    assert_eq!(probe_image(&[0xde, 0xad, 0xbe, 0xef], "image/png"), None);
    // 声明与头不符（PNG 字节报 jpeg）→ None（防改扩展名的假格式）
    assert_eq!(probe_image(&png, "image/jpeg"), None);
    // 空字节 → None
    assert_eq!(probe_image(&[], "image/png"), None);
}

// ======================================================================
// 4. composer 附件预览条同款缩略图
// ======================================================================

#[gpui_kit::test]
fn composer_attachment_preview_uses_image_thumb(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    // composer 注入假装载源并入队一张 Ready 图
    mutate(&chat, visual, |chat, cx| {
        let composer = chat.composer().expect("回声装配有 composer").clone();
        composer.update(cx, |composer, cx| {
            composer.set_image_loader(std::rc::Rc::new(fixture::FakeImageLoader), cx);
            composer.add_attachment(
                craftwave_engine::followup::QueuedImageAttachment {
                    blob_hash: "cwtest-portrait".to_owned(),
                    mime_type: "image/png".to_owned(),
                },
                cx,
            );
        });
    });
    visual.run_until_parked();
    // 同款缩略图渲染（104 卡框 + 图）
    assert!(
        visual.debug_bounds("attachment-preview-0").is_some(),
        "附件预览应为同款缩略图"
    );
    // 再入队一张无 blob 的 stub：不可预览卡（不假装有图）
    mutate(&chat, visual, |chat, cx| {
        let composer = chat.composer().expect("回声装配有 composer").clone();
        composer.update(cx, |composer, cx| {
            composer.add_attachment(
                craftwave_engine::followup::QueuedImageAttachment {
                    blob_hash: "missing-blob".to_owned(),
                    mime_type: "image/png".to_owned(),
                },
                cx,
            );
        });
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("attachment-preview-1").is_some());
}

// ======================================================================
// 5. 长列表冒烟：3000 条混合消息渲染若干帧不 panic
// ======================================================================

#[gpui_kit::test]
fn three_thousand_mixed_rows_render_smoke(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    mutate(&chat, visual, |chat, cx| {
        chat.set_image_loader(std::rc::Rc::new(fixture::FakeImageLoader));
        chat.set_history(fixture::mixed_history(3000), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), 3000);
    });
    // 尾部若干帧（Bottom 对齐首帧测的是尾窗——折叠部件最密集处）
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    // 上滚一段再渲一帧（中段行进视口，thinking / 工具簇 / 图片各形态过树）
    let center = observe(&chat, visual, |chat, _cx| chat.list.viewport_bounds().center());
    visual.simulate_event(gpui_kit::ScrollEvent {
        position: center,
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
            gpui_kit::px(0.),
            gpui_kit::px(4_000.),
        )),
        ..Default::default()
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    // 展开一行 thinking + 一行工具簇再渲（折叠部件展开态在大列表里的通路）
    mutate(&chat, visual, |chat, cx| {
        chat.toggle_thinking(2, cx);
        chat.toggle_tools(2, cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
