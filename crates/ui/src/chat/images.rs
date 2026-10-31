//! 图片装载与三态渲染面（gpui-rewrite 2.9，rendering.md「图片」逐条）。
//!
//! ## 三态（Wake 9486-9588 蓝本，不假装有图）
//!
//! - [`ImageLoad::Ready`]：blob 字节已装载且头探测通过 → 渲染缩略图，
//!   点击开 zoom（记消息行 + 图索引）；
//! - [`ImageLoad::Unsupported`]：探测/解码失败（坏字节、声明 mime 与头不符、
//!   超尺寸）或 blob 读取失败 → 「预览不可用」卡；`original` 有字节时给
//!   「下载原图」按钮（复制原始字节到剪贴板文件语义在 3.6 前先以保存为
//!   文件的提示占位——保存对话框属壳能力，阶段 4 接入）；
//! - [`ImageLoad::Omitted`]：转录上限语义（转录引用了图但内容不可得）→
//!   明示「转录已达上限」卡。生产装载器本批不产出该态（blob 在 attach 时
//!   已按 5MiB 白名单入盘，转录里不存在超限图引用），由 fixture 产出驱动
//!   渲染面；阶段 3 会话装载引入「超预算转录按到达序省略」时（Wake
//!   take_image_slots 同语义）改由装载器产出。
//!
//! ## 装载通路
//!
//! 图片字节不在会话日志里（WirePart::Image 只存 blobHash + mime），渲染层
//! 按 hash 从会话 imgblob 目录取（[`craftwave_engine::storage::get_blob`]，
//! 阻塞文件 IO 走 gpui 后台线程池）。[`ImageLoader`] 是注入缝：生产 =
//! [`EngineImageLoader`]（读引擎存储），测试 = fixture 假源（内存字节）。
//! 宽高比从 blob 头解析（[`probe_image`]，image crate 的 ImageReader 只读
//! 头不解码全图）——rendering.md「按宽高比决定约束方向」的数据源。

use std::io::Cursor;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Div, ElementId, ImageFormat, Img, Pixels, SharedString, Task, WeakEntity, div, img, px,
};

use craftwave_engine::app_state::AppState;

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};

use super::service::SessionService;

/// 缩略图边长（rendering.md「图片」钉值 104px，白名单②）。
pub(crate) const IMAGE_THUMB: f32 = 104.0;

/// 头探测的最大边长（Wake image_probe 同值：8K 档）。超过即按 Unsupported
/// 降级（原图字节保留，可下载原图）。
const MAX_DIMENSION: u32 = 8_192;
/// 头探测的最大像素总量（Wake 同值：16M）。防解码爆炸（一张 8000×8000 的
/// 合法头也是 256MB 的 RGBA 缓冲）。
const MAX_PIXELS: u64 = 16_000_000;

/// 图片装载结果（三态；渲染层只认它，不假装有图）。
#[derive(Debug, Clone)]
pub(crate) enum ImageLoad {
    /// 可渲染：图字节 + 头探测出的宽高（竖图定宽、横图定高的判据）。
    Ready {
        image: Arc<gpui_kit::Image>,
        dims: (u32, u32),
    },
    /// 预览不可用：`original` 有字节时用户仍可下载原图。
    Unsupported {
        mime: String,
        original: Option<Arc<Vec<u8>>>,
    },
    /// 转录已达上限（明示卡；见模块注释的生产语义说明——生产装载器本批
    /// 不产出该态，由 fixture 构造驱动渲染面，故生产构建报 dead_code 放行）。
    #[allow(dead_code)]
    Omitted,
}

/// 图片装载源注入缝：blobHash + mime → 装载任务。生产 = 引擎存储读盘，
/// 测试 = fixture 假源（内存字节）；composer 附件预览与消息流图片共用。
pub(crate) trait ImageLoader {
    fn load(&self, blob_hash: &str, mime_type: &str, cx: &App) -> Task<ImageLoad>;
}

/// 生产装载器：会话 imgblob 目录读字节 → 头探测 → 三态归类。
pub(crate) struct EngineImageLoader {
    state: Arc<AppState>,
    /// 会话 id 的真源（服务在发消息时建会话；weak 回指防所有权环）。
    session: WeakEntity<SessionService>,
}

impl EngineImageLoader {
    pub(crate) fn new(state: Arc<AppState>, session: WeakEntity<SessionService>) -> Self {
        Self { state, session }
    }
}

impl ImageLoader for EngineImageLoader {
    fn load(&self, blob_hash: &str, mime_type: &str, cx: &App) -> Task<ImageLoad> {
        // 无会话（尚未发过消息）或服务已释放：图片不可能属于任何会话历史，
        // 按不可预览降级（不假装有图）
        let Some(session_id) = self
            .session
            .upgrade()
            .and_then(|service| service.read(cx).session_id().map(str::to_owned))
        else {
            return Task::ready(ImageLoad::Unsupported {
                mime: mime_type.to_owned(),
                original: None,
            });
        };
        let state = self.state.clone();
        let hash = blob_hash.to_owned();
        let declared = mime_type.to_owned();
        cx.background_spawn(async move {
            // get_blob 是阻塞文件 IO（P95<300ms 档），跑后台线程池不占主线程
            match craftwave_engine::storage::get_blob(state.store.root(), &session_id, &hash) {
                Ok((bytes, sniffed)) => match probe_image(&bytes, sniffed) {
                    Some((format, dims)) => ImageLoad::Ready {
                        image: Arc::new(gpui_kit::Image::from_bytes(format, bytes)),
                        dims,
                    },
                    None => ImageLoad::Unsupported {
                        mime: sniffed.to_owned(),
                        original: Some(Arc::new(bytes)),
                    },
                },
                Err(_) => ImageLoad::Unsupported {
                    mime: declared,
                    original: None,
                },
            }
        })
    }
}

/// 声明 mime → gpui 图格式（引擎白名单口径：png/jpeg/gif/webp；webp 由
/// image crate 的 webp 特性支撑头探测）。
fn image_format_of(mime: &str) -> Option<ImageFormat> {
    Some(match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => return None,
    })
}

/// 头探测：只读图头拿宽高（不解码全图），格式与声明 mime 互证（防改扩展名
/// 的假 png），超尺寸/零尺寸拒绝。返回（gpui 图格式, (宽, 高)）。
pub(crate) fn probe_image(bytes: &[u8], mime: &str) -> Option<(ImageFormat, (u32, u32))> {
    let format = image_format_of(mime)?;
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // 头格式与声明互证（Wake image_probe 同款：不匹配即 Unsupported）
    let guessed = reader.format()?;
    let expected = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Webp => image::ImageFormat::WebP,
        _ => return None,
    };
    if guessed != expected {
        return None;
    }
    let (width, height) = reader.into_dimensions().ok()?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    Some((format, (width, height)))
}

/// 竖图判据：宽 < 高 → 定宽缩放（横图/方图定高——Wake 9490 同口径）。
fn is_portrait(dims: (u32, u32)) -> bool {
    dims.0 < dims.1
}

/// 缩略图 figure：按宽高比决定约束方向（竖图定宽、横图定高，另一维按原图
/// 比例由 img 元素自动缩放——Wake 9491-9493 手法）。
pub(crate) fn image_figure(image: &Arc<gpui_kit::Image>, dims: (u32, u32)) -> Img {
    let figure = img(image.clone());
    if is_portrait(dims) {
        figure.w(px(IMAGE_THUMB))
    } else {
        figure.h(px(IMAGE_THUMB))
    }
}

/// zoom 的显示尺寸：图片按视口自适应（不放大超过原尺寸——放大看锯齿没有
/// 意义，Wake zoom_fit 同口径）。`avail` 是 overlay 可用区域。
pub(crate) fn zoom_fit(dims: (u32, u32), avail: (f32, f32)) -> (Pixels, Pixels) {
    let (w, h) = (dims.0 as f32, dims.1 as f32);
    let scale = (avail.0 / w).min(avail.1 / h).min(1.0);
    (px(w * scale), px(h * scale))
}

// ======================================================================
// 三态缩略图（消息流 User 气泡与 composer 附件预览条共用的形制）
// ======================================================================

/// Ready 态缩略图：图 figure 套 104 卡框，点击回调交给调用方（消息流 =
/// 开 zoom；composer 预览 = 无动作）。`id` 需稳定（消息行 + 图索引）。
pub(crate) fn render_ready_thumb(
    id: impl Into<ElementId>,
    image: &Arc<gpui_kit::Image>,
    dims: (u32, u32),
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    div()
        .id(id)
        .size(px(IMAGE_THUMB))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .cursor_pointer()
        .flex_shrink_0()
        .child(image_figure(image, dims))
}

/// Unsupported 态卡：「预览不可用」+ mime + （有原始字节时）下载原图钮。
/// 下载原图本批复制原始字节到剪贴板（系统保存对话框属壳能力，阶段 4 接
/// prompt_for_new_path 后换真保存）；卡保持 104 形制，不假装有图。
pub(crate) fn render_unsupported_thumb(
    id: impl Into<ElementId>,
    mime: &str,
    original: Option<&Arc<Vec<u8>>>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mime = SharedString::from(mime.to_owned());
    let mime_for_clipboard = mime.clone();
    let original = original.cloned();
    div()
        .id(id)
        .size(px(IMAGE_THUMB))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::SPACE_1))
        .p(px(theme::SPACE_2))
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
        .text_color(theme.muted_foreground)
        .flex_shrink_0()
        .child(gpui_kit::component::Icon::new(gpui_kit::assets::IconName::ImageOff).with_size(px(
            theme::ICON_MD,
        )))
        .child(
            div()
                .max_w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .child(t("Preview unavailable")),
        )
        .child(
            div()
                .max_w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .child(mime),
        )
        .when_some(original, |card, bytes| {
            card.child(
                div()
                    .id("download-original")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px(px(theme::SPACE_1))
                    .rounded(px(theme::rounded_sm(zoom)))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(t("Download original image"))
                            .build(window, cx)
                    })
                    .on_click(move |_, _, cx| {
                        // 保存对话框属壳能力（阶段 4）；本批先把原始字节放上
                        // 剪贴板，用户粘贴即得完整原图（纪律三：不丢原文）
                        let format = image_format_of(&mime_for_clipboard).unwrap_or(ImageFormat::Png);
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
                            &gpui_kit::Image::from_bytes(format, bytes.as_ref().clone()),
                        ));
                    })
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Download)
                            .with_size(px(theme::ICON_SM)),
                    ),
            )
        })
}

/// Omitted 态卡：明示「图片已省略 / 转录已达上限」（不假装有图）。
pub(crate) fn render_omitted_thumb(id: impl Into<ElementId>, cx: &App) -> gpui_kit::Stateful<Div> {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    div()
        .id(id)
        .size(px(IMAGE_THUMB))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::SPACE_1))
        .p(px(theme::SPACE_2))
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
        .text_color(theme.muted_foreground)
        .flex_shrink_0()
        .child(t("Image omitted"))
        .child(t("Transcript limit reached"))
}

/// 图片可用区尺寸（zoom overlay 用）：视口减三栏骨架占用与呼吸边距
/// （与 mod.rs 的 reader 宽度同一套保留常量）。
pub(crate) fn zoom_avail(viewport: (f32, f32)) -> (f32, f32) {
    let reserved = crate::SIDEBAR_PLACEHOLDER_WIDTH
        + crate::PANEL_PLACEHOLDER_WIDTH
        + 2.0
        + theme::SPACE_10 * 2.0;
    let width = (viewport.0 - reserved).max(220.0);
    let height = (viewport.1 - theme::SPACE_10 * 2.0).max(220.0);
    (width, height)
}
