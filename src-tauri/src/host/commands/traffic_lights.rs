//! macOS 交通灯精确摆位（复刻 ZCode 桌面端 trafficLightPosition (22, 23)）。
//!
//! 为什么不走 tauri-plugin-decoration 的 set_traffic_lights_inset：该接口的
//! inset_y 语义是「标题栏容器的上下总边距」——插件把标题栏容器高度改成
//! button_height + inset_y 后让按钮在容器内垂直居中（按钮距顶 = inset_y / 2），
//! 想得到距顶 23pt 需要 inset_y = 46，而容器尺寸改动受 AppKit 布局约束，
//! 实测 inset_y 超过 40 后按钮直接消失，摆不出目标位置。这里绕开插件的
//! inset 快照路径，只对三枚标准窗口按钮 setFrameOrigin：x 以 close 按钮对齐
//! 22pt、其余按钮保持 AppKit 原生相对间距；y 由「按钮顶部距窗口顶 23pt」沿
//! 按钮容器链换算，不假设标题栏高度。
//!
//! 与插件 listener 的顺序：activate_decoration 已注册插件的交通灯重摆监听
//! （Resized / ScaleFactorChanged / Focused(true) 时把按钮摆回 inset 快照位）。
//! Tauri 窗口事件按注册顺序分发，所以本模块的监听在 activate_custom_titlebar
//! 命令中于 activate_decoration 完成之后注册——同一事件里插件先摆、本模块
//! 随后纠正为 (22, 23)，最终位置以本模块为准。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use objc2_app_kit::{NSButton, NSWindow, NSWindowButton};
use objc2_foundation::NSPoint;
use tauri::{WebviewWindow, WindowEvent};

/// ZCode 桌面端 trafficLightPosition：close 按钮左上角距窗口左上角 (22, 23)。
const CLOSE_X: f64 = 22.0;
const TOP_GAP: f64 = 23.0;

/// 自绘标题栏激活态：未激活时窗口事件不干预按钮（原生标题栏由系统管理）。
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// 事件重摆监听只装一次（激活可能被前端重复调用，如看门狗重试）。
static LISTENER_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 激活自研摆位：置激活态、装事件重摆监听（晚于插件 listener 注册），
/// 并立即摆一次到位。
pub fn activate(window: &WebviewWindow) -> Result<(), String> {
    ACTIVE.store(true, Ordering::SeqCst);
    if LISTENER_INSTALLED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let watched = window.clone();
        window.on_window_event(move |event| {
            if relevant(event)
                && let Err(err) = position(&watched)
            {
                eprintln!("decoration: 交通灯重摆失败：{err}");
            }
        });
    }
    position(window)
}

/// 停用自研摆位（恢复原生标题栏时）：窗口事件不再干预按钮。
pub fn deactivate() {
    ACTIVE.store(false, Ordering::SeqCst);
}

/// 插件同款的重摆触发面：改尺寸、改缩放、重新聚焦；窗口移动不重摆。
fn relevant(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. }
            | WindowEvent::Focused(true)
    )
}

/// 把三枚交通灯摆到 (22, 23)。AppKit 只认主线程：已在主线程直接执行，
/// 否则转投主线程并等结果（命令上下文可能跑在异步线程池）。
pub fn position(window: &WebviewWindow) -> Result<(), String> {
    if objc2::MainThreadMarker::new().is_some() {
        return position_sync(window);
    }
    let (tx, rx) = mpsc::channel();
    let on_main = window.clone();
    window
        .run_on_main_thread(move || {
            let _ = tx.send(position_sync(&on_main));
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| "交通灯摆位任务超时或未返回".to_string())?
}

fn position_sync(window: &WebviewWindow) -> Result<(), String> {
    let pointer = window.ns_window().map_err(|e| e.to_string())?;
    // SAFETY: ns_window 指针在窗口存续期有效；本函数只在主线程被调用，
    // 借用仅覆盖本次同步摆位，指针不被存储或外传。
    let ns_window = unsafe { pointer.cast::<NSWindow>().as_ref() }
        .ok_or_else(|| "NSWindow 指针为空".to_string())?;

    let close = ns_window
        .standardWindowButton(NSWindowButton::CloseButton)
        .ok_or_else(|| "close 按钮不可用".to_string())?;
    let miniaturize = ns_window.standardWindowButton(NSWindowButton::MiniaturizeButton);
    let zoom = ns_window.standardWindowButton(NSWindowButton::ZoomButton);

    let close_frame = close.frame();
    let button_height = close_frame.size.height;
    // SAFETY: AppKit 持有视图层级，仅主线程同步查询。
    let container =
        unsafe { close.superview() }.ok_or_else(|| "交通灯按钮容器不可用".to_string())?;
    // SAFETY: 同上。
    let titlebar =
        unsafe { container.superview() }.ok_or_else(|| "标题栏容器不可用".to_string())?;

    // 按钮顶部距窗口顶 TOP_GAP：沿「按钮 → 按钮容器 → 标题栏容器」链换算出
    // 按钮自身坐标系里的 y（AppKit 坐标 y 向上，窗口顶 = frame 高度处，
    // 逐层减去容器偏移与按钮高度得到按钮原点）。
    let y = ns_window.frame().size.height
        - TOP_GAP
        - button_height
        - titlebar.frame().origin.y
        - container.frame().origin.y;
    // x：close 对齐 CLOSE_X，其余按钮保持 AppKit 原生相对间距。
    let x_for = |button: &NSButton| CLOSE_X + (button.frame().origin.x - close_frame.origin.x);

    close.setFrameOrigin(NSPoint::new(CLOSE_X, y));
    if let Some(button) = miniaturize.as_ref() {
        button.setFrameOrigin(NSPoint::new(x_for(button), y));
    }
    if let Some(button) = zoom.as_ref() {
        button.setFrameOrigin(NSPoint::new(x_for(button), y));
    }
    Ok(())
}
