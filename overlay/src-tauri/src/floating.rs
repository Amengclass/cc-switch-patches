//! 悬浮窗（加速球）模块
//!
//! 提供桌面常驻的透明小圆球 + 悬停展开的用量面板：
//! - 小球窗口 `floating-ball`（240×56 横向胶囊条，本身即信息条：app/模型/余量），置顶、无边框、透明、跳过任务栏，可原生拖动
//! - 面板窗口 `floating-panel`（300×320），悬停小球时在旁展开，列出各 app 当前
//!   供应商 / 模型 / 用量
//!
//! 数据全部通过 `AppState`（db + usage_cache）聚合，不依赖主窗口的 React 状态，
//! 因此轻量模式销毁主窗口后悬浮窗依然独立工作。

use tauri::{Emitter, Manager, WebviewWindow, WebviewWindowBuilder};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::settings::FloatingWindowPosition;
use crate::store::AppState;

pub const BALL_LABEL: &str = "floating-ball";
pub const PANEL_LABEL: &str = "floating-panel";
/// 右键菜单窗口（与面板同款透明样式，自定义 HTML 菜单）
pub const MENU_LABEL: &str = "floating-menu";
/// 收起后的色条窗口
const STRIP_LABEL: &str = "floating-strip";
/// 悬浮球窗口内容尺寸（方案C：横向胶囊条 180×40）。
/// 窗口实际尺寸 = 内容 + 四周 FLOATING_MARGIN 留白（见 ball_window_size）。
const BALL_WIDTH: f64 = 192.0;
const BALL_HEIGHT: f64 = 40.0;
const PANEL_WIDTH: f64 = 300.0;
const PANEL_HEIGHT: f64 = 320.0;
/// 球/面板内容四周留白（逻辑像素）：透明窗口必须比内容大一圈。
/// 窗口与内容同尺寸时，DPI 缩放下的亚像素舍入会把贴窗口边缘的 1px 边框
/// 裁掉（实测：上/左/右边框可见，唯独下边框消失）。留白让边框远离窗口边缘。
/// 与 MENU_SHADOW_MARGIN 同理——菜单留 10px 是为阴影，这里只画 1px 边框，6px 足够。
const FLOATING_MARGIN: f64 = 6.0;
/// 球窗口实际尺寸 = 内容尺寸 + 四周留白（创建时用窗口尺寸，定位/吸附时用内容尺寸）
fn ball_window_size() -> (f64, f64) {
    (
        BALL_WIDTH + FLOATING_MARGIN * 2.0,
        BALL_HEIGHT + FLOATING_MARGIN * 2.0,
    )
}
/// 面板窗口实际尺寸 = 内容尺寸 + 四周留白
fn panel_window_size() -> (f64, f64) {
    (
        PANEL_WIDTH + FLOATING_MARGIN * 2.0,
        PANEL_HEIGHT + FLOATING_MARGIN * 2.0,
    )
}
/// 右键菜单窗口尺寸（瘦高样式，与 .floating-menu CSS 保持一致：
/// padding 2+2、2×28 项、2×2 gap、1 分隔线、1 边框；带图标列。
/// 比最窄内容（项内容 53 + padding 4 + 边框 2 = 59）留一些余量，取 80）
const MENU_WIDTH: f64 = 80.0;
const MENU_HEIGHT: f64 = 68.0;
/// 菜单窗口四周留白（逻辑像素）：box-shadow 羽化超出透明窗口会被裁成方形，
/// 窗口必须比菜单内容大一圈才能完整显示阴影/描边。
const MENU_SHADOW_MARGIN: f64 = 10.0;

/// 菜单窗口实际尺寸 = 内容尺寸 + 四周阴影留白（创建/显示时用窗口尺寸，定位时用内容尺寸）
fn menu_window_size() -> (f64, f64) {
    (
        MENU_WIDTH + MENU_SHADOW_MARGIN * 2.0,
        MENU_HEIGHT + MENU_SHADOW_MARGIN * 2.0,
    )
}
/// 面板/菜单与小球之间的间距（逻辑像素）
const PANEL_GAP: f64 = 8.0;
/// 吸附后与屏幕边缘保留的一小段间距（逻辑像素）
const EDGE_GAP: f64 = 2.0;
/// 小球→面板跨窗移动时的隐藏宽限期
const HOVER_GRACE_MS: u64 = 500;

/// 悬浮球是否固定当前位置（设置页「固定当前位置」；固定后不可拖动/不吸附）
fn is_floating_locked() -> bool {
    crate::settings::get_settings()
        .floating_locked
        .unwrap_or(false)
}

/// 悬停状态：小球 / 面板任一处于悬停时面板保持显示
struct FloatingHoverState {
    ball: bool,
    panel: bool,
}

static HOVER_STATE: std::sync::Mutex<FloatingHoverState> =
    std::sync::Mutex::new(FloatingHoverState {
        ball: false,
        panel: false,
    });

/// 球位置落盘防抖门控（镜像 tray::schedule_tray_refresh 的做法）
static POSITION_SAVE_SCHEDULED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// 右键菜单是否打开：打开期间悬停球不展开面板、面板互斥不收起菜单
static MENU_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 右键菜单最近一次收起的时刻（区分「点击球关菜单」与正常单击球）
static MENU_CLOSED_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

// ============================================================
// 拖动（Rust 端全局光标轮询，绕开 WebView 事件）
//
// 原生 startDragging / data-tauri-drag-region 在这个透明置顶窗口上不可靠，
// 前端 pointermove 在按住拖动时也收不到事件。这里改用系统 API：
// - 按下左键（前端 pointerdown 或 Rust WindowEvent::MouseInput）→ 记录起点
// - Rust 循环轮询 GetCursorPos 全局光标 → set_position 移动窗口
// - 松开左键（GetAsyncKeyState 检测 或 前端 pointerup）→ 停止 + 边缘吸附 + 保存
// ============================================================

static FLOATING_DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// (起点光标 x, 起点光标 y, 起点窗口 x, 起点窗口 y) —— 均为物理像素
static FLOATING_DRAG_START: std::sync::Mutex<Option<(i32, i32, f64, f64)>> =
    std::sync::Mutex::new(None);
/// 本次拖动是否实际移动了窗口（用于区分单击/拖动）
static FLOATING_DRAG_MOVED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// 取消正在进行的吸附动画（用户再次按下时置 true）
static SNAP_ANIMATION_CANCEL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ============================================================
// 边缘自动收起
// ============================================================

/// 收起方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollapseEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// 球是否处于收起状态
static COLLAPSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 收起方向
static COLLAPSED_EDGE: std::sync::Mutex<Option<CollapseEdge>> = std::sync::Mutex::new(None);
/// 收起前的球位置（逻辑坐标），展开时恢复
static COLLAPSED_PREV_POS: std::sync::Mutex<Option<(f64, f64)>> = std::sync::Mutex::new(None);
/// 拖拽预览状态：当前是否在显示收起预览
static PREVIEW_SHOWING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 拖拽预览当前边缘（用于检测边缘切换时重建 strip）
static PREVIEW_EDGE: std::sync::Mutex<Option<CollapseEdge>> = std::sync::Mutex::new(None);
/// 缓存的显示器工作区（物理像素），只在首次拖拽或跨屏时刷新
static CACHED_WORK_AREA: std::sync::Mutex<Option<(f64, f64, f64, f64)>> = std::sync::Mutex::new(None);
/// 缓存的 DPI 缩放比
static CACHED_SCALE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10000);
/// 远端上下文：前端切换远端目标时写入，悬浮球据此读取远端供应商。
/// (host_id, container_id, current_provider_id)
static REMOTE_CONTEXT: std::sync::Mutex<Option<(String, Option<String>, String)>> =
    std::sync::Mutex::new(None);

/// 前端推送的「各 app 路由接管状态」快照（本机与远端统一，与主窗口 UI 同源）。
/// 主窗口已知真值（本机 takeoverStatus / 远端 routeProxyApps），推送后悬浮窗
/// 立即可用，无需等 DB proxy_config 落库或查询往返。
/// 为 None 时回退读本机 DB —— 冷启动尚未收到推送时的兜底。
static ROUTE_PROXY_MAP: std::sync::Mutex<Option<std::collections::BTreeMap<String, bool>>> =
    std::sync::Mutex::new(None);


/// 胶囊厚度（逻辑像素）：贴边缘的宽度（温度计指示器风格）
/// 8px 足够显示小字百分比标签，又不会太宽遮挡屏幕内容
const CAPSULE_THICKNESS: f64 = 8.0;
/// 胶囊长度（逻辑像素）：左/右竖条高度，上横条宽度
const CAPSULE_LENGTH: f64 = 64.0;
/// 展开触发延迟（毫秒）
const CAPSULE_EXPAND_DELAY_MS: u64 = 100;

/// 胶囊命中区域外扩（逻辑像素）：胶囊本身只有5px，太难命中，
/// 四周扩大20px的隐形热区，鼠标进入即开始展开计时。
const CAPSULE_HIT_PADDING: f64 = 0.0;
/// strip 窗口四周留白（逻辑像素）：窗口必须比胶囊内容大一圈，
/// 让 CSS 圆角在更大的 WebView2 渲染表面上抗锯齿，否则 7.9px 厚的小窗口
/// 会把 4px 圆角物理渲染切方（"两端不够圆"）。胶囊画在窗口正中央。
const STRIP_PAD: f64 = 4.0;

const DRAG_POLL_MS: u64 = 16;
/// 边缘吸附阈值（逻辑像素）：拖到多近才判定「在边缘」触发收起预览/吸附。
/// 40 偏大——球离屏幕边缘还很远就弹预览了，收窄到 24。
const SNAP_THRESHOLD: f64 = 24.0;
/// 判定单击：窗口有任何位移即视为拖动，完全没动才是单击
const CLICK_MOVE_THRESHOLD_PX: f64 = 0.0;
/// 吸附动画默认时长（毫秒）；0 = 立即
const DEFAULT_SNAP_SPEED_MS: u32 = 160;

#[cfg(target_os = "windows")]
fn global_cursor_physical() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        if GetCursorPos(&mut pt) != 0 {
            Some((pt.x, pt.y))
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn global_cursor_physical() -> Option<(i32, i32)> {
    None
}

#[cfg(target_os = "windows")]
fn is_left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(target_os = "windows"))]
fn is_left_button_down() -> bool {
    false
}

/// 开始拖动：记录起点并启动全局光标轮询循环移动窗口（幂等）
#[tauri::command]
pub async fn floating_drag_begin(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;

    // 左键按下进入拖动：先收起面板；若右键菜单开着也一并收起（点击球视为点击外部）
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        let _ = panel.hide();
    }
    if app
        .get_webview_window(MENU_LABEL)
        .map(|m| m.is_visible().unwrap_or(false))
        .unwrap_or(false)
    {
        hide_floating_menu_sync(&app);
    }
    // 取消可能正在进行的吸附动画（用户又按下了）
    SNAP_ANIMATION_CANCEL.store(true, Ordering::Release);

    // 收起状态下不响应拖动（展开由 expand_polling 处理）
    if is_ball_collapsed() {
        return Ok(());
    }

    if FLOATING_DRAGGING.load(Ordering::Acquire) {
        return Ok(());
    }
    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return Ok(());
    };
    let Some((cx, cy)) = global_cursor_physical() else {
        return Ok(());
    };
    let pos = ball.outer_position().map_err(|e| e.to_string())?;
    *FLOATING_DRAG_START
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some((cx, cy, pos.x as f64, pos.y as f64));
    FLOATING_DRAGGING.store(true, Ordering::Release);
    FLOATING_DRAG_MOVED.store(false, Ordering::Release);
    log::info!("[Floating] 开始拖动");

    let app2 = app.clone();
    // 固定状态在按下瞬间读取一次：拖拽期间不可达设置页，状态不会变。
    // 固定时（locked=true）不移动窗口也不标记「已拖动」，松手按单击处理（仍打开主窗口）。
    let locked = is_floating_locked();
    // 拖拽开始时刷新一次 monitor 缓存（后续用缓存，不再每帧调 current_monitor）
    if let Some(ball) = app.get_webview_window(BALL_LABEL) {
        refresh_monitor_cache(&ball);
    }
    tauri::async_runtime::spawn(async move {
        loop {
            if !FLOATING_DRAGGING.load(Ordering::Acquire) {
                break;
            }
            // 左键松开判定（GetAsyncKeyState 是异步状态位，快速拖动时可能瞬时读到 0，
            // 必须延迟一帧二次确认，否则误判松手 → collision 把球隐藏 → "拖拽中消失"）
            if !is_left_button_down() {
                tokio::time::sleep(std::time::Duration::from_millis(24)).await;
                if !is_left_button_down() {
                    FLOATING_DRAGGING.store(false, Ordering::Release);
                    finish_drag(&app2);
                    break;
                }
                // 抖动恢复：左键其实还按着，继续拖
                continue;
            }
            if locked {
                continue;
            }
            let moved = {
                let guard = FLOATING_DRAG_START
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                *guard
            };
            if let Some((scx, scy, swx, swy)) = moved {
                if let Some((cx, cy)) = global_cursor_physical() {
                    let nx = swx + (cx - scx) as f64;
                    let ny = swy + (cy - scy) as f64;
                    if (nx - swx).abs() + (ny - swy).abs() > CLICK_MOVE_THRESHOLD_PX {
                        FLOATING_DRAG_MOVED.store(true, Ordering::Release);
                    }
                    if let Some(ball) = app2.get_webview_window(BALL_LABEL) {
                        // Clamp 球位置到缓存的工作区内（不再调 current_monitor）
                        let (cx_clamped, cy_clamped) = if let Some((wl, wt, ww, wh)) = cached_work_area(&ball) {
                            let scale = cached_scale();
                            let bw = BALL_WIDTH * scale;
                            let bh = BALL_HEIGHT * scale;
                            (
                                nx.max(wl - 50.0).min(wl + ww - bw + 50.0),
                                ny.max(wt - 50.0).min(wt + wh - bh + 50.0),
                            )
                        } else {
                            // 缓存为空时硬 clamp 到合理范围（防止球飞到屏幕外）
                            (nx.max(-200.0).min(4000.0), ny.max(-200.0).min(3000.0))
                        };
                        let _ = ball.set_position(tauri::PhysicalPosition::new(
                            cx_clamped.round() as i32,
                            cy_clamped.round() as i32,
                        ));
                        // 边缘收起预览：状态机模式，只在边缘状态翻转时操作窗口
                        if is_auto_collapse_enabled() && FLOATING_DRAG_MOVED.load(Ordering::Acquire) {
                            // detect_edge 现在用缓存数据，开销极低，每帧调用没问题
                            let new_edge = detect_edge(&ball);
                            let old_edge = *PREVIEW_EDGE.lock().unwrap();
                            if new_edge != old_edge {
                                match (old_edge, new_edge) {
                                    (None, Some(edge)) => {
                                        // 进入边缘：显示 strip
                                        show_preview_strip(&app2, &ball, edge);
                                        PREVIEW_SHOWING.store(true, Ordering::Release);
                                        *PREVIEW_EDGE.lock().unwrap() = Some(edge);
                                        let _ = app2.emit("floating-drag-near-edge", true);
                                    }
                                    (Some(_), None) => {
                                        // 离开边缘：隐藏 strip
                                        hide_preview_strip(&app2);
                                        PREVIEW_SHOWING.store(false, Ordering::Release);
                                        *PREVIEW_EDGE.lock().unwrap() = None;
                                        let _ = app2.emit("floating-drag-near-edge", false);
                                    }
                                    (Some(_), Some(new_e)) => {
                                        // 边缘切换（top→left）：重新定位 + resize
                                        show_preview_strip(&app2, &ball, new_e);
                                        *PREVIEW_EDGE.lock().unwrap() = Some(new_e);
                                    }
                                    (None, None) => {}
                                }
                            } else if new_edge.is_some() {
                                // 同一边缘持续拖拽：仅重新定位（最轻量操作）
                                reposition_preview_strip(&app2, &ball, new_edge.unwrap());
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(DRAG_POLL_MS)).await;
        }
    });
    Ok(())
}

/// 结束拖动（前端 pointerup 通知；轮询循环也会在左键松开时自行结束）
#[tauri::command]
pub async fn floating_drag_end(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if FLOATING_DRAGGING.swap(false, Ordering::AcqRel) {
        finish_drag(&app);
    }
    Ok(())
}

/// 点击色条展开悬浮球
#[tauri::command]
pub async fn floating_expand_from_strip(app: tauri::AppHandle) -> Result<(), String> {
    expand_ball(&app);
    Ok(())
}

/// 计算指定边缘的胶囊预览位置和尺寸（物理像素，全局坐标）。
/// 与 collapse_ball 中的胶囊定位逻辑一致，供拖拽预览复用。
fn compute_capsule_preview(
    ball: &WebviewWindow,
    edge: CollapseEdge,
) -> Option<(f64, f64, f64, f64)> {
    let scale = ball.scale_factor().ok()?;
    let pos = ball.outer_position().ok()?;
    let margin_px = FLOATING_MARGIN * scale;
    let ball_cx = pos.x as f64 + margin_px + BALL_WIDTH * scale / 2.0;
    let ball_cy = pos.y as f64 + margin_px + BALL_HEIGHT * scale / 2.0;

    let monitor = ball.current_monitor().ok().flatten()?;
    let work = monitor.work_area();
    let wl = work.position.x as f64;
    let wt = work.position.y as f64;
    let ww = work.size.width as f64;
    let wh = work.size.height as f64;
    let gap = EDGE_GAP * scale;
    let capsule_thick = CAPSULE_THICKNESS * scale;
    let capsule_len = CAPSULE_LENGTH * scale;

    let (cw, ch, cx, cy) = match edge {
        CollapseEdge::Left => {
            let y = (ball_cy - capsule_len / 2.0).max(wt + gap).min(wt + wh - capsule_len - gap);
            (capsule_thick, capsule_len, wl + gap, y)
        }
        CollapseEdge::Right => {
            let y = (ball_cy - capsule_len / 2.0).max(wt + gap).min(wt + wh - capsule_len - gap);
            (capsule_thick, capsule_len, wl + ww - capsule_thick - gap, y)
        }
        CollapseEdge::Top => {
            let x = (ball_cx - capsule_len / 2.0).max(wl + gap).min(wl + ww - capsule_len - gap);
            (capsule_len, capsule_thick, x, wt + gap)
        }
        CollapseEdge::Bottom => {
            let x = (ball_cx - capsule_len / 2.0).max(wl + gap).min(wl + ww - capsule_len - gap);
            (capsule_len, capsule_thick, x, wt + wh - capsule_thick - gap)
        }
    };
    Some((cx, cy, cw, ch))
}

/// 重新定位已显示的预览 strip 到新的胶囊位置（不销毁重建，仅移动）
/// 供拖拽轮询每帧调用，让预览跟随球移动
fn reposition_preview_strip(app: &tauri::AppHandle, ball: &WebviewWindow, edge: CollapseEdge) {
    let Some(strip) = app.get_webview_window(STRIP_LABEL) else {
        return;
    };
    let Some((cx, cy, _cw, _ch)) = compute_capsule_preview(ball, edge) else {
        return;
    };
    let pad_px = STRIP_PAD * cached_scale();
    let _ = strip.set_position(tauri::PhysicalPosition::new(
        (cx - pad_px).round() as i32,
        cy.round() as i32,
    ));
}

/// 预创建 strip 窗口（启动时调用一次），停在屏幕外等待复用。
/// 拖拽中只做 set_position + show/hide，绝不 destroy/recreate。
fn ensure_preview_strip(app: &tauri::AppHandle) {
    if app.get_webview_window(STRIP_LABEL).is_some() {
        return; // 已创建
    }
    let logic_w = CAPSULE_THICKNESS; // 5px，稍后会被 set_size 覆盖
    let logic_h = CAPSULE_LENGTH;    // 40px
    let clw = logic_w;
    let clh = logic_h;
    if let Ok(strip) = WebviewWindowBuilder::new(
        app,
        STRIP_LABEL,
        tauri::WebviewUrl::App("floating.html".into()),
    )
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .transparent(true)
    .resizable(false)
    .inner_size(logic_w, logic_h)
    .visible(false)
    .on_page_load(move |window, _| {
        let _ = window.set_size(tauri::LogicalSize::new(clw, clh));
        let w = window.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let _ = w.set_size(tauri::LogicalSize::new(clw, clh));
        });
    })
    .build()
    {
        // 停在屏幕外，不显示
        let _ = strip.set_position(tauri::PhysicalPosition::new(-10000, -10000));
        log::info!("[Floating] strip 窗口已预创建");
    }
}

/// WebView2 在窗口 show 后可能把窄窗口撑成内容默认宽度（"刚拖上去一瞬很大"），
/// 延迟强制设回尺寸（位置由调用方持续维护，这里不动，避免拖拽预览跳变）
fn stabilize_strip_window(app: &tauri::AppHandle, w: f64, h: f64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
        if let Some(strip) = app.get_webview_window(STRIP_LABEL) {
            let _ = strip.set_size(tauri::LogicalSize::new(w, h));
        }
    });
}

/// 显示预览 strip 到指定边缘位置（预创建的窗口只做 move + show + resize）
fn show_preview_strip(app: &tauri::AppHandle, ball: &WebviewWindow, edge: CollapseEdge) {
    let Some(strip) = app.get_webview_window(STRIP_LABEL) else {
        return;
    };
    let Some((cx, cy, cw, ch)) = compute_capsule_preview(ball, edge) else {
        return;
    };
    let scale = cached_scale();
    let logic_w = cw / scale;
    let logic_h = ch / scale;
    // 窗口 = 胶囊 + 四周留白（与 collapse_ball 一致），位置外移留白
    let window_logic_w = logic_w + STRIP_PAD * 2.0;
    let window_logic_h = logic_h + STRIP_PAD * 2.0;
    let pad_px = STRIP_PAD * scale;
    let _ = strip.set_size(tauri::LogicalSize::new(window_logic_w, window_logic_h));
    let _ = strip.set_position(tauri::PhysicalPosition::new(
        (cx - pad_px).round() as i32,
        (cy - pad_px).round() as i32,
    ));
    let _ = strip.show();
    // 兜底：show 后 WebView2 可能撑开窗口，180ms 后强制设回尺寸
    stabilize_strip_window(app, window_logic_w, window_logic_h);
    // 悬浮球变半透明（收起预览提示）。用 eval 直接注入，
    // 不依赖 floating-drag-near-edge 事件——跨窗口事件到悬浮窗 webview 不可靠（实测）。
    let _ = ball.eval(
        "var el=document.querySelector('.ball'); if(el){el.style.setProperty('opacity','0.55');}",
    );
}

/// 隐藏拖拽预览 strip（移到屏幕外 + hide，不销毁，保留复用）
fn hide_preview_strip(app: &tauri::AppHandle) {
    if let Some(strip) = app.get_webview_window(STRIP_LABEL) {
        let _ = strip.set_position(tauri::PhysicalPosition::new(-10000, -10000));
        let _ = strip.hide();
    }
    // 恢复悬浮球透明度（eval 注入的预览半透明）
    if let Some(ball) = app.get_webview_window(BALL_LABEL) {
        let _ = ball.eval(
            "var el=document.querySelector('.ball'); if(el){el.style.removeProperty('opacity');}",
        );
    }
}

/// 拖动结束统一收尾：窗口基本没动视为单击（打开主窗口），随后做边缘吸附。
/// 若右键菜单刚收起（点击球关菜单），本次单击只关菜单、不打开主窗口。
fn finish_drag(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    PREVIEW_SHOWING.store(false, Ordering::Release);
    *PREVIEW_EDGE.lock().unwrap() = None;
    // 拖拽结束：销毁预览 strip（collapse_ball 会重新创建正式的）
    hide_preview_strip(app);
    let menu_just_closed = MENU_CLOSED_AT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300));
    if !FLOATING_DRAG_MOVED.load(Ordering::Acquire) && !menu_just_closed {
        log::info!("[Floating] 单击悬浮球，打开主窗口");
        open_main_window_impl(app);
    }
    snap_and_save_ball_position(app);
}

/// 计算边缘吸附后的目标位置（物理像素，全局坐标）。
/// 全部用物理像素（窗口位置 / 显示器尺寸 / 阈值统一），避免 scale 不一致导致右/下不吸附。
/// 球内容相对窗口居中（CSS #floating-root flex 居中），吸附判断以**内容**边缘为准，
/// 返回的窗口位置 = 内容位置 − 留白，保证视觉上球内容贴屏幕边缘。
fn compute_snap_target(ball: &WebviewWindow) -> Option<(f64, f64)> {
    let scale = ball.scale_factor().ok()?;
    let pos = ball.outer_position().ok()?;
    let margin_px = FLOATING_MARGIN * scale;
    // 内容左缘/顶缘（物理像素）：窗口位置 + 留白
    let mut cx = pos.x as f64 + margin_px;
    let mut cy = pos.y as f64 + margin_px;
    let ball_w = BALL_WIDTH * scale;
    let ball_h = BALL_HEIGHT * scale;
    let thresh_px = SNAP_THRESHOLD * scale;
    // 吸附后与边缘保留的一小段间距（物理像素）
    let gap_px = EDGE_GAP * scale;

    if let Some(monitor) = ball.current_monitor().ok().flatten() {
        // 用显示器**工作区**（去掉任务栏的区域）作吸附边界：球贴底时吸附到
        // 任务栏顶端，而不是屏幕最底部（会被任务栏遮住）。任务栏在顶/左/右侧
        // 时同理，球始终留在工作区内。
        let work = monitor.work_area();
        let left = work.position.x as f64;
        let top = work.position.y as f64;
        let right = left + work.size.width as f64;
        let bottom = top + work.size.height as f64;

        if cx - left <= thresh_px {
            cx = left + gap_px;
        } else if right - (cx + ball_w) <= thresh_px {
            cx = right - ball_w - gap_px;
        }
        if cy - top <= thresh_px {
            cy = top + gap_px;
        } else if bottom - (cy + ball_h) <= thresh_px {
            cy = bottom - ball_h - gap_px;
        }
        cx = cx.max(left).min(right - ball_w);
        cy = cy.max(top).min(bottom - ball_h);
        log::info!(
            "[Floating] 吸附计算: content=({cx:.0},{cy:.0}) work_area=({left:.0},{top:.0},{right:.0},{bottom:.0}) gap={gap_px:.0} scale={scale}"
        );
    }
    // 窗口位置 = 内容位置 − 留白
    Some((cx - margin_px, cy - margin_px))
}

/// 松手吸附：按设置的动画速度平滑吸附到边缘。
/// 速度为 0（关闭）= 不自动吸附，只保存当前位置。
fn snap_and_save_ball_position(app: &tauri::AppHandle) {
    if is_floating_locked() {
        // 固定当前位置：不吸附、不保存
        log::debug!("[Floating] 已固定位置，跳过吸附");
        return;
    }
    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return;
    };
    let speed_ms = crate::settings::get_settings()
        .floating_snap_speed_ms
        .unwrap_or(DEFAULT_SNAP_SPEED_MS);
    if speed_ms == 0 {
        // 关闭吸附：不吸附，仅保存当前位置
        if let Ok(pos) = ball.outer_position() {
            save_ball_logical_position(app, pos.x as f64, pos.y as f64);
        }
        // 即使不吸附也要检查边缘收起
        collapse_ball(app);
        return;
    }
    let Some((tx, ty)) = compute_snap_target(&ball) else {
        return;
    };
    animate_ball_snap(app, (tx, ty), speed_ms as u64);
}

/// 保存球位置（物理像素 → 逻辑坐标，供启动时 apply_saved_ball_position 恢复）
fn save_ball_logical_position(app: &tauri::AppHandle, px: f64, py: f64) {
    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return;
    };
    let Ok(scale) = ball.scale_factor() else {
        return;
    };
    let lx = (px / scale).round();
    let ly = (py / scale).round();
    let _ = crate::settings::set_floating_window_position(Some(FloatingWindowPosition {
        x: lx,
        y: ly,
    }));
    log::info!("[Floating] 拖动结束，保存位置: ({lx:.0}, {ly:.0})");
}

/// 平滑吸附动画：ease-out 从当前位置插值到目标，结束后保存位置。
/// 用户再次按下（SNAP_ANIMATION_CANCEL）时中止，交给新的拖动。
fn animate_ball_snap(app: &tauri::AppHandle, target: (f64, f64), duration_ms: u64) {
    use std::sync::atomic::Ordering;
    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return;
    };
    let Ok(start) = ball.outer_position() else {
        return;
    };
    let (sx, sy) = (start.x as f64, start.y as f64);
    let (tx, ty) = target;
    SNAP_ANIMATION_CANCEL.store(false, Ordering::Release);
    let steps = 24u32;
    let interval_ms = (duration_ms / steps as u64).max(1);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for i in 1..=steps {
            if SNAP_ANIMATION_CANCEL.load(Ordering::Acquire) {
                return;
            }
            let t = i as f64 / steps as f64;
            let e = 1.0 - (1.0 - t) * (1.0 - t); // ease-out
            let x = sx + (tx - sx) * e;
            let y = sy + (ty - sy) * e;
            if let Some(ball) = app.get_webview_window(BALL_LABEL) {
                let _ = ball.set_position(tauri::PhysicalPosition::new(
                    x.round() as i32,
                    y.round() as i32,
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
        }
        // 动画结束：精确落在目标并保存
        if let Some(ball) = app.get_webview_window(BALL_LABEL) {
            let _ = ball.set_position(tauri::PhysicalPosition::new(
                tx.round() as i32,
                ty.round() as i32,
            ));
        }
        save_ball_logical_position(&app, tx, ty);
        // 吸附完成后检查是否需要边缘收起
        collapse_ball(&app);
    });
}

// ============================================================
// 窗口创建/销毁
// ============================================================

fn build_floating_window(
    app: &tauri::AppHandle,
    label: &str,
    width: f64,
    height: f64,
) -> Result<WebviewWindow, AppError> {
    let window =
        WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("floating.html".into()))
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .transparent(true)
            // 固定尺寸：禁止用户拖边缘调整大小（避免出现边缘调整光标/误操作）
            .resizable(false)
            .inner_size(width, height)
            .visible(false)
            // WebView2 加载内容后会把悬浮窗窗口 resize 成异常宽度（实测球/菜单
            // 被拉宽成 133 逻辑），这里在页面加载完成时强制设回逻辑尺寸。
            .on_page_load(move |window, _| {
                let _ = window.set_size(tauri::LogicalSize::new(width, height));
                // 再延迟一次，兜底 WebView 加载后的异步 resize
                let w = window.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    let _ = w.set_size(tauri::LogicalSize::new(width, height));
                });
            })
            .build()
            .map_err(|e| AppError::Message(format!("创建悬浮窗 {label} 失败: {e}")))?;
    Ok(window)
}

/// 确保悬浮球 / 面板窗口存在并显示（幂等）。启用开关打开或启动时调用。
pub(crate) fn ensure_floating_window(app: &tauri::AppHandle) {
    let ball_exists = app.get_webview_window(BALL_LABEL).is_some();
    let panel_exists = app.get_webview_window(PANEL_LABEL).is_some();

    // 创建小球窗口（含坐标初始化）。窗口尺寸 = 内容 + 四周留白（边框渲染留白）
    if !ball_exists {
        let (bw, bh) = ball_window_size();
        let Ok(ball) = build_floating_window(app, BALL_LABEL, bw, bh) else {
            return;
        };
        apply_saved_ball_position(&ball);
        // 不 set_focus：悬浮球常驻桌面，抢焦点会打断用户正在输入/操作的应用
        let _ = ball.show();
        log::info!("[Floating] 悬浮球窗口已创建");
    }

    // 创建面板窗口（保持隐藏，悬停时才显示）
    if !panel_exists {
        let (pw, ph) = panel_window_size();
        if let Ok(panel) = build_floating_window(app, PANEL_LABEL, pw, ph) {
            let _ = panel.set_position(tauri::LogicalPosition::new(-20000.0, -20000.0));
            log::info!("[Floating] 面板窗口已创建");
        }
    }

    // 创建右键菜单窗口（保持隐藏，右键时才显示）
    if app.get_webview_window(MENU_LABEL).is_none() {
        let (mw, mh) = menu_window_size();
        if let Ok(menu) = build_floating_window(app, MENU_LABEL, mw, mh) {
            let _ = menu.set_position(tauri::LogicalPosition::new(-20000.0, -20000.0));
            log::info!("[Floating] 右键菜单窗口已创建");
        }
    }

    // 预创建 strip 窗口（拖拽预览用），停在屏幕外等待复用
    ensure_preview_strip(app);
}

/// 应用保存的球位置；无保存位置或位置非法时使用主显示器右下角默认位。
/// 创建后立刻覆盖 window-state 插件可能恢复的旧/坏坐标。
/// 若保存位置超出当前屏幕工作区，clamp 进边界内（防止切屏/分辨率变化后球消失）。
fn apply_saved_ball_position(ball: &WebviewWindow) {
    let saved = crate::settings::get_settings()
        .floating_window_position
        .map(|p| (p.x, p.y))
        .filter(|(x, y)| x.is_finite() && y.is_finite() && *x > -1000.0 && *y > -1000.0);

    let pos = match saved {
        Some((x, y)) => {
            // Clamp 到当前显示器工作区内，防止球跑到屏幕外
            if let Some(monitor) = ball.primary_monitor().ok().flatten() {
                let scale = monitor.scale_factor();
                let work = monitor.work_area();
                let left = work.position.x as f64 / scale;
                let top = work.position.y as f64 / scale;
                let right = left + work.size.width as f64 / scale - BALL_WIDTH - FLOATING_MARGIN * 2.0;
                let bottom = top + work.size.height as f64 / scale - BALL_HEIGHT - FLOATING_MARGIN * 2.0;
                let cx = x.clamp(left.max(0.0), right.max(left));
                let cy = y.clamp(top.max(0.0), bottom.max(top));
                tauri::LogicalPosition::new(cx, cy)
            } else {
                tauri::LogicalPosition::new(x, y)
            }
        }
        None => default_ball_position(ball),
    };
    let _ = ball.set_position(pos);
}

/// 主显示器右下角默认位置（逻辑坐标）
fn default_ball_position(ball: &WebviewWindow) -> tauri::LogicalPosition<f64> {
    let Some(monitor) = ball.primary_monitor().ok().flatten() else {
        return tauri::LogicalPosition::new(200.0, 200.0);
    };
    let scale = monitor.scale_factor();
    // 用工作区（去掉任务栏）算默认右下角位置：首次启动也落在任务栏之上
    let work = monitor.work_area();
    let w = work.size.width as f64 / scale;
    let h = work.size.height as f64 / scale;
    tauri::LogicalPosition::new(
        work.position.x as f64 / scale + (w - BALL_WIDTH - 24.0 - FLOATING_MARGIN).max(0.0),
        work.position.y as f64 / scale + (h - BALL_HEIGHT - 48.0 - FLOATING_MARGIN).max(0.0),
    )
}

/// 销毁悬浮球与面板窗口（开关关闭时）
pub(crate) fn destroy_floating_window(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    MENU_OPEN.store(false, Ordering::Release);
    // 清除收起状态
    COLLAPSED.store(false, Ordering::Release);
    *COLLAPSED_EDGE.lock().unwrap() = None;
    *COLLAPSED_PREV_POS.lock().unwrap() = None;
    PREVIEW_SHOWING.store(false, Ordering::Release);
    *PREVIEW_EDGE.lock().unwrap() = None;
    *CACHED_WORK_AREA.lock().unwrap() = None;
    if let Some(ball) = app.get_webview_window(BALL_LABEL) {
        let _ = ball.destroy();
    }
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        let _ = panel.destroy();
    }
    if let Some(menu) = app.get_webview_window(MENU_LABEL) {
        let _ = menu.destroy();
    }
    if let Some(strip) = app.get_webview_window(STRIP_LABEL) {
        let _ = strip.destroy();
    }
    *HOVER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = FloatingHoverState {
        ball: false,
        panel: false,
    };
    log::info!("[Floating] 悬浮窗已销毁");
}

/// 根据设置开关应用悬浮窗状态（save_settings 联动）
pub(crate) fn apply_floating_window_setting(app: &tauri::AppHandle, enabled: bool) {
    if enabled {
        ensure_floating_window(app);
    } else {
        destroy_floating_window(app);
    }
}

// ============================================================
// 边缘自动收起：拖到边缘松手 → 收起为色条；鼠标靠近 → 展开
// ============================================================

/// 是否启用边缘自动收起
fn is_auto_collapse_enabled() -> bool {
    crate::settings::get_settings()
        .floating_auto_collapse
        .unwrap_or(false)
}

/// 球是否处于收起状态
pub(crate) fn is_ball_collapsed() -> bool {
    use std::sync::atomic::Ordering;
    COLLAPSED.load(Ordering::Acquire)
}

/// 刷新缓存的显示器工作区和缩放比（只在首次拖拽或跨屏时调用）
fn refresh_monitor_cache(ball: &WebviewWindow) {
    use std::sync::atomic::Ordering;
    // 优先 current_monitor，失败则用 primary_monitor 兜底
    let monitor = ball.current_monitor().ok().flatten()
        .or_else(|| ball.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let work = monitor.work_area();
        let scale = monitor.scale_factor();
        *CACHED_WORK_AREA.lock().unwrap() = Some((
            work.position.x as f64,
            work.position.y as f64,
            work.size.width as f64,
            work.size.height as f64,
        ));
        CACHED_SCALE.store((scale * 10000.0) as u64, Ordering::Relaxed);
    }
}

/// 获取缓存的缩放比
fn cached_scale() -> f64 {
    use std::sync::atomic::Ordering;
    CACHED_SCALE.load(Ordering::Relaxed) as f64 / 10000.0
}

/// 获取缓存的工作区（wl, wt, ww, wh），如果缓存为空则刷新
fn cached_work_area(ball: &WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    {
        let cached = CACHED_WORK_AREA.lock().unwrap();
        if let Some(area) = *cached {
            return Some(area);
        }
    }
    refresh_monitor_cache(ball);
    *CACHED_WORK_AREA.lock().unwrap()
}

/// 检测球当前靠近哪个边缘（返回 None = 不在任何边缘阈值内）
/// 使用缓存的显示器信息，避免每帧调用 current_monitor()
fn detect_edge(ball: &WebviewWindow) -> Option<CollapseEdge> {
    let scale = cached_scale();
    let pos = ball.outer_position().ok()?;
    let margin_px = FLOATING_MARGIN * scale;
    let cx = pos.x as f64 + margin_px;
    let cy = pos.y as f64 + margin_px;
    let ball_w = BALL_WIDTH * scale;
    let ball_h = BALL_HEIGHT * scale;
    let thresh_px = SNAP_THRESHOLD * scale;

    let (left, top, ww, wh) = cached_work_area(ball)?;
    let right = left + ww;
    let bottom = top + wh;

    // 检查四边，优先选距离最近的
    let dist_left = cx - left;
    let dist_right = right - (cx + ball_w);
    let dist_top = cy - top;
    let dist_bottom = bottom - (cy + ball_h);

    let min_dist = dist_left.min(dist_right).min(dist_top).min(dist_bottom);
    if min_dist > thresh_px {
        return None;
    }

    if min_dist == dist_left {
        Some(CollapseEdge::Left)
    } else if min_dist == dist_right {
        Some(CollapseEdge::Right)
    } else if min_dist == dist_top {
        Some(CollapseEdge::Top)
    } else {
        // 底部不收起（任务栏区域），只吸附不折叠
        None
    }
}

/// 收起悬浮球为色条：隐藏球窗口 → 创建独立色条窗口
fn collapse_ball(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;

    if !is_auto_collapse_enabled() {
        return;
    }
    if COLLAPSED.load(Ordering::Acquire) {
        return;
    }
    // 拖拽中绝不收起（否则球消失用户还按着左键）
    if FLOATING_DRAGGING.load(Ordering::Acquire) {
        return;
    }

    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return;
    };

    let Some(edge) = detect_edge(&ball) else {
        return;
    };

    // 保存当前位置供展开恢复
    if let Ok(pos) = ball.outer_position() {
        let scale = ball.scale_factor().unwrap_or(1.0);
        let lx = pos.x as f64 / scale;
        let ly = pos.y as f64 / scale;
        *COLLAPSED_PREV_POS.lock().unwrap() = Some((lx, ly));
    }

    // 全部用物理像素计算
    let scale = ball.scale_factor().unwrap_or(1.0);
    let pos = ball.outer_position().ok().unwrap_or(tauri::PhysicalPosition::new(0, 0));
    let margin_px = FLOATING_MARGIN * scale;
    let ball_cx = pos.x as f64 + margin_px + BALL_WIDTH * scale / 2.0;
    let ball_cy = pos.y as f64 + margin_px + BALL_HEIGHT * scale / 2.0;

    let monitor = match ball.current_monitor().ok().flatten() {
        Some(m) => m,
        None => return,
    };
    let work = monitor.work_area();
    let wl = work.position.x as f64;
    let wt = work.position.y as f64;
    let ww = work.size.width as f64;
    let wh = work.size.height as f64;
    let gap = EDGE_GAP * scale;
    let capsule_thick = CAPSULE_THICKNESS * scale;
    let capsule_len = CAPSULE_LENGTH * scale;

    // 胶囊尺寸和位置（物理像素）
    let (cw, ch, cx, cy) = match edge {
        CollapseEdge::Left => {
            // 竖排胶囊：贴左边缘
            let y = (ball_cy - capsule_len / 2.0).max(wt + gap).min(wt + wh - capsule_len - gap);
            (capsule_thick, capsule_len, wl + gap, y)
        }
        CollapseEdge::Right => {
            // 竖排胶囊：贴右边缘
            let y = (ball_cy - capsule_len / 2.0).max(wt + gap).min(wt + wh - capsule_len - gap);
            (capsule_thick, capsule_len, wl + ww - capsule_thick - gap, y)
        }
        CollapseEdge::Top => {
            // 横排胶囊：贴顶部
            let x = (ball_cx - capsule_len / 2.0).max(wl + gap).min(wl + ww - capsule_len - gap);
            (capsule_len, capsule_thick, x, wt + gap)
        }
        CollapseEdge::Bottom => {
            // 横排胶囊：贴底部
            let x = (ball_cx - capsule_len / 2.0).max(wl + gap).min(wl + ww - capsule_len - gap);
            (capsule_len, capsule_thick, x, wt + wh - capsule_thick - gap)
        }
    };

    // 1. 通知前端：球即将收起，执行淡出动画
    let _ = app.emit("floating-ball-collapse", ());
    // 1.5 隐藏球窗口（淡出动画由前端 CSS transition 处理，200ms 足够）
    let _ = ball.hide();

    // 2. 复用预创建的 strip 窗口（绝不 destroy+create —— destroy 是异步的，
    //    紧跟用相同 label 重建会失败，导致胶囊不可见）
    let capsule_logic_w = cw / scale;
    let capsule_logic_h = ch / scale;
    // 窗口 = 胶囊 + 四周留白（STRIP_PAD），位置整体外移留白；胶囊画在窗口中央
    let window_logic_w = capsule_logic_w + STRIP_PAD * 2.0;
    let window_logic_h = capsule_logic_h + STRIP_PAD * 2.0;
    let pad_px = STRIP_PAD * scale;
    if let Some(capsule) = app.get_webview_window(STRIP_LABEL) {
        let _ = capsule.set_size(tauri::LogicalSize::new(window_logic_w, window_logic_h));
        let _ = capsule.set_position(tauri::PhysicalPosition::new(
            (cx - pad_px).round() as i32,
            (cy - pad_px).round() as i32,
        ));
        // 诊断：set_size 后读回实际窗口尺寸，确认 WebView2 是否夹克了尺寸
        if let Ok(actual) = capsule.inner_size() {
            let scale = capsule.scale_factor().unwrap_or(1.0);
            log::info!(
                "[Floating] 胶囊复用后实际窗口: edge={:?} 期望窗口逻辑=({:.1},{:.1}) 实际逻辑≈({:.1},{:.1})",
                edge, window_logic_w, window_logic_h,
                actual.width as f64 / scale, actual.height as f64 / scale
            );
        }
        let _ = capsule.show();
        // 兜底：show 后 WebView2 可能撑开窗口，180ms 后强制设回尺寸
        stabilize_strip_window(app, window_logic_w, window_logic_h);
        log::info!(
            "[Floating] 胶囊窗口复用: edge={:?} size=({:.1},{:.1}) pos=({:.0},{:.0})",
            edge, capsule_logic_w, capsule_logic_h, cx, cy
        );
    } else {
        // strip 未预创建（极端情况）：原地创建
        let clw = capsule_logic_w;
        let clh = capsule_logic_h;
        if let Ok(capsule) = WebviewWindowBuilder::new(
            app,
            STRIP_LABEL,
            tauri::WebviewUrl::App("floating.html".into()),
        )
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .transparent(true)
        .resizable(false)
        .inner_size(capsule_logic_w, capsule_logic_h)
        .visible(false)
        .on_page_load(move |window, _| {
            let _ = window.set_size(tauri::LogicalSize::new(clw, clh));
            let w = window.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let _ = w.set_size(tauri::LogicalSize::new(clw, clh));
            });
        })
        .build()
        {
            let _ = capsule.set_position(tauri::PhysicalPosition::new(
                cx.round() as i32,
                cy.round() as i32,
            ));
            let _ = capsule.show();
            log::info!(
                "[Floating] 胶囊窗口新建(兜底): edge={:?} size=({:.1},{:.1}) pos=({:.0},{:.0})",
                edge, capsule_logic_w, capsule_logic_h, cx, cy
            );
        }
    }

    *COLLAPSED_EDGE.lock().unwrap() = Some(edge);
    COLLAPSED.store(true, Ordering::Release);

    // 启动展开轮询
    start_expand_polling(app.clone());

    log::info!("[Floating] 悬浮球已收起: edge={:?}", edge);
}

/// 鼠标是否在窗口矩形内（物理像素判断）
fn cursor_in_window(win: &tauri::WebviewWindow) -> bool {
    let Some((cx, cy)) = global_cursor_physical() else {
        return false;
    };
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) else {
        return false;
    };
    (cx as f64 >= pos.x as f64)
        && (cx as f64 <= pos.x as f64 + size.width as f64)
        && (cy as f64 >= pos.y as f64)
        && (cy as f64 <= pos.y as f64 + size.height as f64)
}

/// 展开悬浮球：隐藏色条窗口 → 球沿吸附方向从屏幕外原位滑出，贴边停靠（带平滑动画）
pub(crate) fn expand_ball(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;

    if !COLLAPSED.load(Ordering::Acquire) {
        return;
    }

    // 先重置状态（避免球显示后第一次拖拽被 is_ball_collapsed 拒绝）
    let edge = COLLAPSED_EDGE.lock().unwrap().take();
    COLLAPSED.store(false, Ordering::Release);

    // 取消可能正在进行的拖动
    FLOATING_DRAGGING.store(false, Ordering::Release);

    // 1. 获取胶囊当前位置（物理像素）作为展开方向基准
    let strip_pos = app.get_webview_window(STRIP_LABEL).and_then(|s| s.outer_position().ok());
    // 2. 隐藏色条窗口（不销毁，保留复用）
    if let Some(strip) = app.get_webview_window(STRIP_LABEL) {
        let _ = strip.set_position(tauri::PhysicalPosition::new(-10000, -10000));
        let _ = strip.hide();
    }

    // 3. 平滑滑入动画：起点在屏幕外（吸附方向对侧），终点贴吸附边缘（球整体露出）
    if let Some(ball) = app.get_webview_window(BALL_LABEL) {
        let scale = ball.scale_factor().unwrap_or(1.0);
        let prev_pos = COLLAPSED_PREV_POS.lock().unwrap().take();

        // 计算起点/终点（全逻辑坐标）。胶囊中心用于对齐垂直/水平位置。
        // 注意：窗口左上角已外移 STRIP_PAD，胶囊实际位置 = 窗口左上 + 留白
        let capsule_center = strip_pos.map(|p| {
            let (sx, sy) = (p.x as f64 / scale, p.y as f64 / scale);
            match edge {
                // 竖排胶囊：中心 y = sy + 留白 + 长/2
                Some(CollapseEdge::Left) | Some(CollapseEdge::Right) => {
                    (sx + STRIP_PAD, sy + STRIP_PAD + CAPSULE_LENGTH / 2.0)
                }
                // 横排胶囊：中心 x = sx + 留白 + 长/2
                _ => (sx + STRIP_PAD + CAPSULE_LENGTH / 2.0, sy + STRIP_PAD),
            }
        });

        let (start, end) = match (edge, cached_work_area(&ball)) {
            (Some(CollapseEdge::Left), Some((wl, _wt, _ww, _wh))) => {
                let wl_l = wl / scale;
                let end_x = wl_l + EDGE_GAP - FLOATING_MARGIN;
                let y = capsule_center.map(|c| c.1 - BALL_HEIGHT / 2.0).unwrap_or(200.0);
                ((end_x - BALL_WIDTH, y), (end_x, y))
            }
            (Some(CollapseEdge::Right), Some((wl, _wt, ww, _wh))) => {
                let right_l = (wl + ww) / scale;
                let end_x = right_l - EDGE_GAP - BALL_WIDTH - FLOATING_MARGIN;
                let y = capsule_center.map(|c| c.1 - BALL_HEIGHT / 2.0).unwrap_or(200.0);
                ((end_x + BALL_WIDTH, y), (end_x, y))
            }
            (Some(CollapseEdge::Top), Some((_wl, wt, _ww, _wh))) => {
                let wt_l = wt / scale;
                let end_y = wt_l + EDGE_GAP - FLOATING_MARGIN;
                let x = capsule_center.map(|c| c.0 - BALL_WIDTH / 2.0).unwrap_or(200.0);
                ((x, end_y - BALL_HEIGHT), (x, end_y))
            }
            // 兜底：方向未知或缓存缺失 → 从胶囊位置滑回收起前位置
            _ => {
                let p = capsule_center.unwrap_or((0.0, 0.0));
                let e = prev_pos.unwrap_or(p);
                (p, e)
            }
        };

        // 先设置到起点位置并显示
        let _ = ball.set_position(tauri::LogicalPosition::new(start.0, start.1));
        let _ = ball.show();
        let _ = app.emit("floating-ball-expand", ());

        // 异步执行滑入动画（200ms ease-out cubic，20帧）
        let app_clone = app.clone();
        SNAP_ANIMATION_CANCEL.store(false, Ordering::Release);
        tauri::async_runtime::spawn(async move {
            let steps = 10u32;
            let duration_ms = 80u64;
            let interval_ms = (duration_ms / steps as u64).max(1);
            for i in 1..=steps {
                if SNAP_ANIMATION_CANCEL.load(Ordering::Acquire) {
                    return;
                }
                let t = i as f64 / steps as f64;
                let e = 1.0 - (1.0 - t).powi(3); // ease-out cubic
                let x = start.0 + (end.0 - start.0) * e;
                let y = start.1 + (end.1 - start.1) * e;
                if let Some(b) = app_clone.get_webview_window(BALL_LABEL) {
                    let _ = b.set_position(tauri::LogicalPosition::new(x, y));
                }
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            // 精确落在目标
            if let Some(b) = app_clone.get_webview_window(BALL_LABEL) {
                let _ = b.set_position(tauri::LogicalPosition::new(end.0, end.1));
            }
            save_ball_logical_position(&app_clone, end.0, end.1);

            // 兜底弹面板：球展开动画完成，若鼠标仍在球窗口内（窗口滑到鼠标下方时
            // 浏览器不触发 mouseenter，前端那套失效），延迟 200ms 后主动弹面板。
            let app2 = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if let Some(b) = app2.get_webview_window(BALL_LABEL) {
                    let vis = b.is_visible().unwrap_or(false);
                    let in_win = cursor_in_window(&b);
                    if vis && in_win {
                        // 鼠标确实在球上：置 hover 状态为 true，
                        // 否则展开瞬间的 mouseleave 宽限期任务会在 500ms 后把
                        // 刚弹出的面板立即 hide（面板只闪现 ~300ms，用户看不到）。
                        HOVER_STATE
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .ball = true;
                        let _ = show_floating_panel(app2).await;
                    }
                }
            });
        });
    }

    log::info!("[Floating] 悬浮球已展开: edge={:?}", edge);
}

/// 收起状态下轮询鼠标位置，hover 胶囊时延迟展开
fn start_expand_polling(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut hovering = false;
        let mut hover_start: Option<std::time::Instant> = None;

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;

            if !COLLAPSED.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }

            let Some(capsule) = app.get_webview_window(STRIP_LABEL) else {
                break;
            };

            let Some((cursor_x, cursor_y)) = global_cursor_physical() else {
                hovering = false;
                hover_start = None;
                continue;
            };

            let scale = capsule.scale_factor().unwrap_or(1.0);
            let pos = match capsule.outer_position() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let size = match capsule.inner_size() {
                Ok(s) => s,
                Err(_) => continue,
            };

            let cx = pos.x as f64;
            let cy = pos.y as f64;
            let cw = size.width as f64;
            let ch = size.height as f64;

            // 命中区域外扩：胶囊本身只有5px太难命中，四周扩大 CAPSULE_HIT_PADDING
            let pad = CAPSULE_HIT_PADDING * scale;
            let now_hovering = (cursor_x as f64 >= cx - pad)
                && (cursor_x as f64 <= cx + cw + pad)
                && (cursor_y as f64 >= cy - pad)
                && (cursor_y as f64 <= cy + ch + pad);

            if now_hovering {
                if !hovering {
                    // 刚进入胶囊区域，开始计时
                    hovering = true;
                    hover_start = Some(std::time::Instant::now());
                } else if let Some(start) = hover_start {
                    if start.elapsed() >= std::time::Duration::from_millis(CAPSULE_EXPAND_DELAY_MS) {
                        expand_ball(&app);
                        break;
                    }
                }
            } else {
                hovering = false;
                hover_start = None;
            }
        }
    });
}

/// 小球窗口拖动后的位置落盘（防抖）
pub(crate) fn schedule_position_save(x: f64, y: f64) {
    use std::sync::atomic::Ordering;
    if POSITION_SAVE_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
        POSITION_SAVE_SCHEDULED.store(false, Ordering::Release);
        if let Err(e) =
            crate::settings::set_floating_window_position(Some(FloatingWindowPosition { x, y }))
        {
            log::warn!("[Floating] 保存球位置失败: {e}");
        } else {
            log::debug!("[Floating] 球位置已保存: ({x:.0}, {y:.0})");
        }
    });
}

/// 退出前立即把球位置落盘（防抖任务可能尚未执行）
pub(crate) fn save_ball_position_now(app: &tauri::AppHandle) {
    let Some((x, y)) = current_ball_position(app) else {
        return;
    };
    if let Err(e) =
        crate::settings::set_floating_window_position(Some(FloatingWindowPosition { x, y }))
    {
        log::warn!("[Floating] 退出前保存球位置失败: {e}");
    }
}

/// 读取当前球位置（逻辑坐标），用于面板/菜单定位。
/// `outer_position` 返回物理像素，除以 scale_factor 转成逻辑像素；
/// 面板/菜单用 `set_position(LogicalPosition)` 落位时由 Tauri 按各自
/// 窗口的 scale 换算回物理，同一显示器下往返一致，保证右缘/底缘精确对齐。
fn current_ball_position(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    let ball = app.get_webview_window(BALL_LABEL)?;
    let scale = ball.scale_factor().ok()?;
    let pos = ball.outer_position().ok()?;
    Some((pos.x as f64 / scale, pos.y as f64 / scale))
}

/// 读取小球当前**内容**逻辑尺寸（宽/高）。面板/菜单位位时**不直接用 BALL_WIDTH/HEIGHT
/// 常量**，而是读球窗口当前实际尺寸换算（窗口尺寸 − 四周留白 = 内容尺寸）：
/// WebView2 加载内容后可能把窗口临时撑宽（on_page_load 已兜底设回），这里实时读取
/// 可保证弹出位置始终贴着球内容真实外缘；球尺寸日后调整时，面板/菜单位置自动跟随。
fn ball_logical_size(app: &tauri::AppHandle) -> (f64, f64) {
    let Some(ball) = app.get_webview_window(BALL_LABEL) else {
        return (BALL_WIDTH, BALL_HEIGHT);
    };
    let Ok(size) = ball.inner_size() else {
        return (BALL_WIDTH, BALL_HEIGHT);
    };
    let scale = ball.scale_factor().unwrap_or(1.0);
    // 窗口 = 内容 + 四周留白（FLOATING_MARGIN），内容居中
    let w = size.width as f64 / scale - FLOATING_MARGIN * 2.0;
    let h = size.height as f64 / scale - FLOATING_MARGIN * 2.0;
    if w > 1.0 && h > 1.0 {
        (w, h)
    } else {
        (BALL_WIDTH, BALL_HEIGHT)
    }
}

// ============================================================
// 数据聚合
// ============================================================

/// 悬浮窗面板每行条目
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingEntry {
    pub app_type: String,
    pub app_label: &'static str,
    pub provider_name: String,
    /// 是否已设置供应商（未设置时 provider_name 为「未设置」），
    /// 前端据此决定是否给供应商/模型名应用高亮色
    pub has_provider: bool,
    pub model: Option<String>,
    pub usage_summary: Option<String>,
    /// 最高利用率（0-100），供小球状态色
    pub worst_pct: Option<f64>,
    /// 完整用量数据（与主窗口 UsageFooter 展示同一份 UsageCache 结果），
    /// 面板据此渲染「剩余：47.22 CNY」这类余额型详情
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Vec<crate::provider::UsageData>>,
    /// 用量查询时间戳（毫秒），供面板显示「刚刚 / x分钟前」（与主窗口一致）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<i64>,
    /// 该 app 的路由纳管是否开启（proxy_config.enabled，与主窗口 takeoverStatus 同源），
    /// 面板据此每行显示「纳管中」标签。OpenCode/OpenClaw 不支持，恒为 false。
    pub takeover_active: bool,
}

// 无供应商时返回空字符串：是否「未设置」由前端根据 has_provider 走 i18n，
// 避免后端硬编码中文「未设置」导致多语言下也显示中文。
const UNKNOWN_PROVIDER: &str = "";

fn app_label(app_type: &AppType) -> &'static str {
    match app_type {
        AppType::Claude => "Claude Code",
        AppType::ClaudeDesktop => "Claude Desktop",
        AppType::Codex => "Codex",
        AppType::Gemini => "Gemini",
        AppType::GrokBuild => "Grok Build",
        AppType::OpenCode => "OpenCode",
        AppType::OpenClaw => "OpenClaw",
        AppType::Hermes => "Hermes",
        AppType::Pi => "Pi",
    }
}

/// 尽力解析各 app 当前模型的显示名。
/// OpenClaw 的模型是 app 级配置（agents.defaults.model.primary），其余 app 从
/// provider 的 settings_config 读取各自的模型键。
fn resolve_model(app_type: &AppType, provider: &crate::provider::Provider) -> Option<String> {
    let settings = &provider.settings_config;
    let str_at = |keys: &[&str]| {
        let env = settings.get("env");
        keys.iter().find_map(|k| {
            env.and_then(|e| e.get(*k))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
    };

    match app_type {
        AppType::OpenClaw => crate::openclaw_config::get_agents_defaults()
            .ok()
            .flatten()
            .and_then(|d| d.model.map(|m| m.primary))
            .filter(|s| !s.is_empty()),
        AppType::Claude | AppType::ClaudeDesktop => str_at(&["ANTHROPIC_MODEL"]).map(String::from),
        AppType::Gemini => str_at(&["GEMINI_MODEL"]).map(String::from),
        AppType::Codex => settings
            .get("config")
            .and_then(|v| v.as_str())
            .and_then(|text| {
                text.lines().find_map(|line| {
                    let line = line.trim();
                    if line.starts_with("model") {
                        line.split_once('=')
                            .map(|(_, v)| v.trim().trim_matches('"').to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
            })
            .filter(|s| !s.is_empty()),
        _ => None,
    }
}

/// 计算当前 provider 的最高利用率（复用托盘的分组标签逻辑）
fn worst_utilization_pct(
    app_state: &AppState,
    app_type: &AppType,
    provider: &crate::provider::Provider,
    provider_id: &str,
) -> Option<f64> {
    let is_official_provider = provider.category.as_deref() == Some("official");
    let can_use_script = provider.has_usage_script_enabled()
        && (!is_official_provider || crate::tray::provider_uses_official_subscription(provider));

    if can_use_script {
        if let Some(Some(result)) =
            app_state
                .usage_cache
                .with_script(app_type, provider_id, |result| -> Option<f64> {
                    let data = result.data.as_ref()?;
                    let entries: Vec<(&str, f64)> = data
                        .iter()
                        .filter_map(|d| Some((d.plan_name.as_deref()?, crate::tray::tier_pct(d)?)))
                        .collect();
                    let parts = crate::tray::labeled_tier_parts(&entries);
                    if !parts.is_empty() {
                        return parts
                            .into_iter()
                            .map(|(_, u)| u)
                            .fold(None, |acc, u| Some(acc.map_or(u, |a: f64| a.max(u))));
                    }
                    entries.first().map(|(_, u)| *u)
                })
        {
            return Some(result);
        }
        if crate::tray::provider_uses_official_subscription(provider) {
            if let Some(Some(quota)) =
                app_state
                    .usage_cache
                    .with_subscription(app_type, |quota| -> Option<f64> {
                        let entries: Vec<(&str, f64)> = quota
                            .tiers
                            .iter()
                            .map(|tier| (tier.name.as_str(), tier.utilization))
                            .collect();
                        let parts = crate::tray::labeled_tier_parts(&entries);
                        parts
                            .into_iter()
                            .map(|(_, u)| u)
                            .fold(None, |acc, u| Some(acc.map_or(u, |a: f64| a.max(u))))
                    })
            {
                return Some(quota);
            }
        }
    }
    None
}

/// 取当前 provider 的完整用量数据，供面板展示与主窗口同一份内容。
/// 与 `format_usage_suffix` 同一优先级：脚本缓存优先（`UsageCache.script`
/// 存的就是 queryProviderUsage 的完整 UsageResult），官方订阅兜底时把
/// `quota.tiers` 展平成 UsageData（镜像 provider.rs 的订阅分支）。
fn floating_usage_data(
    app_state: &AppState,
    app_type: &AppType,
    provider: &crate::provider::Provider,
    provider_id: &str,
) -> Option<Vec<crate::provider::UsageData>> {
    let is_official_provider = provider.category.as_deref() == Some("official");
    let can_use_script = provider.has_usage_script_enabled()
        && (!is_official_provider || crate::tray::provider_uses_official_subscription(provider));
    if !can_use_script {
        return None;
    }

    if let Some(Some(data)) = app_state
        .usage_cache
        .with_script(app_type, provider_id, |r| r.data.clone())
    {
        if !data.is_empty() {
            return Some(data);
        }
    }

    if crate::tray::provider_uses_official_subscription(provider) {
        if let Some(data) = app_state.usage_cache.with_subscription(
            app_type,
            |q| -> Option<Vec<crate::provider::UsageData>> {
                if !q.success {
                    return None;
                }
                let items: Vec<crate::provider::UsageData> = q
                    .tiers
                    .iter()
                    .map(|tier| crate::provider::UsageData {
                        plan_name: Some(tier.name.clone()),
                        remaining: Some(100.0 - tier.utilization),
                        total: Some(100.0),
                        used: Some(tier.utilization),
                        unit: Some("%".to_string()),
                        is_valid: Some(true),
                        invalid_message: None,
                        extra: tier.resets_at.clone(),
                    })
                    .collect();
                (!items.is_empty()).then_some(items)
            },
        ) {
            return data;
        }
    }

    None
}

/// 取当前 provider 用量查询的时间戳（毫秒），与 `floating_usage_data` 同一前提。
/// 脚本缓存优先；官方订阅兜底读 SubscriptionQuota.queried_at。
fn floating_queried_at(
    app_state: &AppState,
    app_type: &AppType,
    provider: &crate::provider::Provider,
    provider_id: &str,
) -> Option<i64> {
    let is_official_provider = provider.category.as_deref() == Some("official");
    let can_use_script = provider.has_usage_script_enabled()
        && (!is_official_provider || crate::tray::provider_uses_official_subscription(provider));
    if !can_use_script {
        return None;
    }

    if let Some(ts) = app_state
        .usage_cache
        .script_queried_at(app_type, provider_id)
    {
        return Some(ts);
    }

    if crate::tray::provider_uses_official_subscription(provider) {
        if let Some(Some(ts)) = app_state
            .usage_cache
            .with_subscription(app_type, |q| q.queried_at)
        {
            return Some(ts);
        }
    }
    None
}

/// 该 app 的路由接管状态：优先用前端推送的快照（ROUTE_PROXY_MAP，本机/远端统一，
/// 与主窗口 UI 同源、即时）；尚未收到推送时返回 None，调用方回退读本机 DB。
/// 悬浮球与悬浮面板共用，保证两者读到同一真值来源。
fn pushed_route_for(app_type_str: &str) -> Option<bool> {
    let map = ROUTE_PROXY_MAP.lock().unwrap();
    map.as_ref().map(|m| m.get(app_type_str).copied().unwrap_or(false))
}

/// 构建单个 app 的悬浮窗行条目（当前供应商 / 模型 / 用量 / 路由纳管）。
/// 面板（`get_floating_window_data` 遍历所有可见 app）与悬浮球
/// （`get_floating_ball_detail` 只取目标 app）共用，保证两者数据一致。
async fn build_floating_entry(state: &AppState, app_type: &AppType) -> FloatingEntry {
    let app_type_str = app_type.as_str();

    // 路由纳管状态：优先用前端推送的快照（即时、本机/远端同源）；
    // 未收到推送时（如冷启动早期）回退读本机 DB proxy_config.enabled
    let takeover_active = match pushed_route_for(app_type_str) {
        Some(active) => active,
        None => state
            .db
            .get_proxy_config_for_app(app_type_str)
            .await
            .map(|c| c.enabled)
            .unwrap_or(false),
    };

    let remote_pid = REMOTE_CONTEXT.lock().unwrap().as_ref().map(|(_, _, pid)| pid.clone());
    let current_id = {
        // 远端模式：用前端传来的远端当前供应商 ID
        if let Some(ref pid) = remote_pid {
            if !pid.is_empty() {
                Some(pid.clone())
            } else {
                crate::settings::get_effective_current_provider(&state.db, app_type).unwrap_or(None)
            }
        } else {
            crate::settings::get_effective_current_provider(&state.db, app_type).unwrap_or(None)
        }
    };

    let (provider_name, model, usage_summary, worst_pct, usage, queried_at, has_provider) =
        match current_id {
            Some(provider_id) => {
                let provider = state
                    .db
                    .get_provider_by_id(&provider_id, app_type_str)
                    .ok()
                    .flatten();
                match provider {
                    Some(provider) => {
                        let model = resolve_model(app_type, &provider);
                        let usage_summary = crate::tray::format_usage_suffix(
                            state,
                            app_type,
                            &provider,
                            &provider_id,
                        );
                        let worst_pct =
                            worst_utilization_pct(state, app_type, &provider, &provider_id);
                        let usage = floating_usage_data(state, app_type, &provider, &provider_id);
                        let queried_at =
                            floating_queried_at(state, app_type, &provider, &provider_id);
                        (
                            provider.name.clone(),
                            model,
                            usage_summary,
                            worst_pct,
                            usage,
                            queried_at,
                            true,
                        )
                    }
                    None => (
                        UNKNOWN_PROVIDER.to_string(),
                        None,
                        None,
                        None,
                        None,
                        None,
                        false,
                    ),
                }
            }
            None => (
                UNKNOWN_PROVIDER.to_string(),
                None,
                None,
                None,
                None,
                None,
                false,
            ),
        };

    FloatingEntry {
        app_type: app_type_str.to_string(),
        app_label: app_label(app_type),
        provider_name,
        has_provider,
        model,
        usage_summary,
        worst_pct,
        usage,
        queried_at,
        takeover_active,
    }
}

/// 拉取悬浮窗面板数据：每个可见 app 一行（当前供应商 / 模型 / 用量）
#[tauri::command]
pub async fn get_floating_window_data(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<FloatingEntry>, String> {
    let visible_apps = crate::settings::get_settings()
        .visible_apps
        .unwrap_or_default();

    let mut entries = Vec::new();
    for app_type in AppType::all() {
        if !visible_apps.is_visible(&app_type) {
            continue;
        }
        entries.push(build_floating_entry(&state, &app_type).await);
    }

    // debug：面板 3s 轮询，每轮都 info 会让日志暴涨（实测 17MB 日志里占据大头）
    log::debug!(
        "[Floating] 面板拉取数据: {}",
        entries
            .iter()
            .map(|e| format!("{}={}", e.app_label, e.provider_name))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(entries)
}

/// 悬浮球详情：只构建当前目标 app（置顶优先，否则最近活跃）的一行数据。
/// 相比 `get_floating_window_data`（遍历全部 app），只查一个 app，球据此立即刷新，
/// 避免面板全量扫描拖慢球的响应。
#[tauri::command]
pub async fn get_floating_ball_detail(
    state: tauri::State<'_, AppState>,
) -> Result<Option<FloatingEntry>, String> {
    let Some(target) = resolve_ball_target() else {
        return Ok(None);
    };
    let Some(app_type) = parse_app_type(&target.app_type) else {
        return Ok(None);
    };
    let entry = build_floating_entry(&state, &app_type).await;
    // debug：球 5s 轮询每次打 info 会让日志暴涨
    log::debug!(
        "[Floating] 悬浮球详情: {}={} pinned={}",
        entry.app_label,
        entry.provider_name,
        target.is_pinned
    );
    Ok(Some(entry))
}

/// 悬浮球当前应显示的 app 目标
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingBallTarget {
    /// 目标 app 类型（as_str）
    pub app_type: String,
    /// 是否处于手动置顶状态（否则是跟随最近活跃 app）
    pub is_pinned: bool,
    /// 目标 app 的显示名
    pub app_label: &'static str,
    /// 该 app 是否处「远端接管」（主窗口算好写入设置，球据此显示流动边框）
    pub takeover_active: bool,
    /// 悬浮球不透明度（0.2~1.0；设置页滑块调节）
    pub opacity: f32,
}

/// 解析 app 类型字符串（非法/未识别返回 None）
fn parse_app_type(s: &str) -> Option<AppType> {
    s.parse::<AppType>().ok()
}

/// 读取悬浮球当前目标 app：置顶优先，否则跟随最近活跃 app。
/// 两者都无时返回 None（球端退化为通用「CC」）。
fn resolve_ball_target() -> Option<FloatingBallTarget> {
    let settings = crate::settings::get_settings();
    let takeover_active = settings.floating_remote_takeover.unwrap_or(false);
    // 不透明度随设置走；None 回退到默认 0.97（与 current --fb-bg 相仿）
    let opacity = settings.floating_opacity.unwrap_or(0.97).clamp(0.2, 1.0);
    if let Some(pin) = settings.floating_pin_app.as_deref() {
        if let Some(app_type) = parse_app_type(pin) {
            return Some(FloatingBallTarget {
                app_type: app_type.as_str().to_string(),
                is_pinned: true,
                app_label: app_label(&app_type),
                takeover_active,
                opacity,
            });
        }
    }
    if let Some(last) = settings.floating_last_app.as_deref() {
        if let Some(app_type) = parse_app_type(last) {
            return Some(FloatingBallTarget {
                app_type: app_type.as_str().to_string(),
                is_pinned: false,
                app_label: app_label(&app_type),
                takeover_active,
                opacity,
            });
        }
    }
    None
}

/// 向悬浮面板与悬浮球窗口**定向**发 `floating-data-refresh`。
/// 全局 broadcast 对隐藏/非聚焦的浮窗 webview 不可靠（历史踩坑：面板收不到
/// 事件只能等下一轮轮询，表现为「路由开关已开但面板迟迟不变」）。
/// 逐窗口定向 emit 才能保证面板即时同步路由/供应商/用量状态。
pub(crate) fn emit_data_refresh(app: &tauri::AppHandle) {
    for label in [PANEL_LABEL, BALL_LABEL, STRIP_LABEL] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.emit("floating-data-refresh", ());
        }
    }
}

/// 主窗口计算「球当前目标 app 是否处远端接管」后写入设置（球 1s 轮询读它显示流动边框）。
#[tauri::command]
pub async fn floating_set_remote_takeover(
    app: tauri::AppHandle,
    active: bool,
) -> Result<(), String> {
    crate::settings::set_floating_remote_takeover(Some(active)).map_err(|e| e.to_string())?;
    // emit_pin_changed 内部已定向 emit_data_refresh，无需重复
    emit_pin_changed(&app);
    Ok(())
}

/// 前端推送「当前上下文」：远端目标 (host_id, container_id, provider_id)，
/// 以及各 app 的路由接管状态 route_proxy_apps（本机与远端统一推送）。
/// host 为空表示回到本机模式（REMOTE_CONTEXT 清空），但 route_proxy_apps 仍会写入
/// —— 本机模式悬浮窗同样需要即时的 per-app 路由状态。
#[tauri::command]
pub async fn floating_set_remote_context(
    app: tauri::AppHandle,
    host_id: Option<String>,
    container_id: Option<String>,
    provider_id: Option<String>,
    route_proxy_apps: Option<std::collections::BTreeMap<String, bool>>,
) -> Result<(), String> {
    // host 存在即视为远端模式：provider_id 可空（远端可能未设置当前供应商），
    // 若因 provider 为空而整体退化为 None，悬浮窗会错误回退到本机路由状态。
    *REMOTE_CONTEXT.lock().unwrap() =
        host_id.map(|h| (h, container_id, provider_id.unwrap_or_default()));
    // 路由状态快照：与主窗口 UI 同源，悬浮窗直接采用，不必等 DB 落库
    if let Some(map) = route_proxy_apps {
        *ROUTE_PROXY_MAP.lock().unwrap() = Some(map);
    }
    emit_data_refresh(&app);
    Ok(())
}

/// 悬浮球拉取「显示哪个 app」：置顶优先，否则最近活跃 app。
/// takeover_active 与悬浮面板同一逻辑（pushed_route_for）：优先用前端推送的
/// per-app 路由快照，未推送时回退本机 DB，保证球与面板显示一致。
#[tauri::command]
pub async fn get_floating_ball_target(
    state: tauri::State<'_, AppState>,
) -> Result<Option<FloatingBallTarget>, String> {
    let Some(mut target) = resolve_ball_target() else {
        return Ok(None);
    };
    let remote_route = pushed_route_for(&target.app_type);
    target.takeover_active = match remote_route {
        Some(active) => active,
        None => state
            .db
            .get_proxy_config_for_app(&target.app_type)
            .await
            .map(|c| c.enabled)
            .unwrap_or(false),
    };
    Ok(Some(target))
}

/// 置顶悬浮球到指定 app（None 取消置顶，恢复跟随最近活跃 app）。
/// 写入设置并通知悬浮窗刷新。
#[tauri::command]
pub async fn floating_set_pin_app(
    app: tauri::AppHandle,
    app_type: Option<String>,
) -> Result<(), String> {
    if let Some(ref at) = app_type {
        if parse_app_type(at).is_none() {
            return Err(format!("未知的 app 类型: {at}"));
        }
    }
    let t0 = std::time::Instant::now();
    crate::settings::set_floating_pin_app(app_type).map_err(|e| e.to_string())?;
    log::info!(
        "[Floating] 设置悬浮球置顶 app: {:?} (写设置耗时 {}ms)",
        resolve_ball_target().map(|t| t.app_type),
        t0.elapsed().as_millis()
    );
    // 球对置顶变化要即时响应：单独发 target 事件（轻量，只带 app/isPinned）。
    // 用「按窗口逐个 emit」而非全局 app.emit——实测全局广播到悬浮窗 webview 的
    // 事件不可靠（球/面板都只按 poll 节奏刷新），逐个 emit_to 保证到达球窗口。
    let t1 = std::time::Instant::now();
    emit_pin_changed(&app);
    emit_data_refresh(&app);
    log::info!(
        "[Floating] 置顶事件已发出 (emit 耗时 {}ms)",
        t1.elapsed().as_millis()
    );
    Ok(())
}

/// 记录最近一次活跃的 app（球未置顶时跟随它）。由悬浮球前端在收到
/// provider-switched 事件时调用；主窗口切 app 即可实时影响球。
#[tauri::command]
pub async fn floating_record_active_app(
    app: tauri::AppHandle,
    app_type: String,
) -> Result<(), String> {
    let Some(parsed) = parse_app_type(&app_type) else {
        log::debug!("[Floating] 忽略未知活跃 app: {app_type}");
        return Ok(());
    };
    crate::settings::set_floating_last_app(parsed.as_str().to_string())
        .map_err(|e| e.to_string())?;
    log::info!("[Floating] 记录最近活跃 app: {}", parsed.as_str());
    // 未置顶时球跟随最近活跃：发 target 事件让球即时更新
    emit_pin_changed(&app);
    Ok(())
}

/// 后端直接记录「最近活跃 app」并通知球（同步版）。
///
/// 各 `provider-switched` 发射点（profile/failover/tray/failover_switch）在切 app
/// 时同步调它。不再依赖事件回传到悬浮球 webview（实测球收不到跨窗口事件）——
/// 球改由 1s 轮询读 backend 的 floating_last_app，因此这里把 last_app 写在后端，
/// 球下一次轮询（≤1s）即可跟随。
pub fn record_active_app_sync(app_handle: &tauri::AppHandle, app_type: &str) {
    let Some(parsed) = parse_app_type(app_type) else {
        return;
    };
    if let Err(e) = crate::settings::set_floating_last_app(parsed.as_str().to_string()) {
        log::warn!("[Floating] 记录活跃 app 失败: {e}");
        return;
    }
    let _ = app_handle.emit("floating-pin-changed", resolve_ball_target());
    let _ = app_handle.emit("floating-data-refresh", ());
}

/// 向悬浮球窗口定向发 `floating-pin-changed`（带目标），并向所有窗口发
/// `floating-data-refresh`。按窗口逐个 emit 比全局广播更可靠地到达浮窗 webview。
fn emit_pin_changed(app: &tauri::AppHandle) {
    let target = resolve_ball_target();
    // 定向发给球窗口
    if let Some(ball) = app.get_webview_window(BALL_LABEL) {
        let _ = ball.emit("floating-pin-changed", target);
    }
    // 定向广播 data-refresh（面板/球都刷新）
    emit_data_refresh(app);
}

// ============================================================
// 面板显示/隐藏与悬停协调
// ============================================================

/// 计算弹窗位置：弹窗与球的水平关系按宽度决定——
/// - **球宽 > 弹窗宽**（如右键菜单 80 < 球 180）：弹窗水平居中于球，即弹窗
///   显示在球的正下方（球偏上时）/正上方（球偏下时），中心对齐。
/// - **球宽 < 弹窗宽**（如面板 300 > 球 180）：保持**朝屏幕中央方向展开**，
///   球贴在弹窗靠屏幕边缘侧的角上（角对齐，不居中）。
/// - 垂直：球偏上 → 弹窗向下展开，弹窗顶缘 = 球底缘 + 间隙；
///   球偏下 → 弹窗向上展开，弹窗底缘 = 球顶缘 - 间隙。
///   某方向放不下翻转该轴。
///   面板与菜单共用同一个函数 → 弹出位置始终一致。
fn position_for_ball(
    app: &tauri::AppHandle,
    ball_pos: (f64, f64),
    width: f64,
    height: f64,
) -> (f64, f64) {
    let monitor = app
        .get_webview_window(BALL_LABEL)
        .and_then(|w| w.current_monitor().ok().flatten());
    // 球所在显示器的逻辑原点与逻辑尺寸。
    // 注意：ball_pos 来自 outer_position，相对主屏左上角为原点；多显示器时
    // 必须减去当前显示器的原点偏移，否则「球偏左/偏右」判断会错乱
    // （例如副屏在主屏左侧时球坐标为负，明明吸附在右缘却走了左分支 → 菜单左侧对齐）。
    let (origin_x, origin_y, screen_w, screen_h) = match monitor {
        Some(m) => {
            let pos = m.position();
            let size = m.size();
            let scale = m.scale_factor();
            (
                pos.x as f64 / scale,
                pos.y as f64 / scale,
                size.width as f64 / scale,
                size.height as f64 / scale,
            )
        }
        None => (0.0, 0.0, 1920.0, 1080.0),
    };

    // 球当前实际逻辑尺寸（运行时读取，球尺寸变化时定位自动跟随）
    let (ball_w, ball_h) = ball_logical_size(app);
    // 球相对所在显示器左上角的逻辑坐标
    let (bx, by) = (ball_pos.0 - origin_x, ball_pos.1 - origin_y);
    // 球窗口比内容大一圈（四周 FLOATING_MARGIN 留白），可见边界 = 内容边界：
    // 对齐/间距一律以内容边缘为基准，否则弹窗会整体偏 6px、上下间距不对称
    // （向上展开间距 = PANEL_GAP + 留白 = 14px，向下 = 8px）。
    let ball_content_left = bx + FLOATING_MARGIN;
    let ball_content_top = by + FLOATING_MARGIN;
    let ball_cx = bx + ball_w / 2.0;
    let ball_cy = by + ball_h / 2.0;

    // 水平：球宽 > 弹窗宽 → 弹窗水平居中于球（正下方/正上方中心对齐）；
    // 否则保持“球偏左→弹窗右展（左缘=球左缘）、球偏右→弹窗左展（右缘=球右缘）”
    let centered = ball_w > width;
    let mut px = if centered {
        ball_content_left + (ball_w - width) / 2.0
    } else if ball_cx < screen_w / 2.0 {
        ball_content_left
    } else {
        ball_content_left + ball_w - width
    };
    // 该方向放不下 → 翻到反方向对齐（居中模式即使偏出也由下方 clamp 收进屏幕）
    if !centered {
        if px + width > screen_w {
            px = ball_content_left + ball_w - width;
        } else if px < 0.0 {
            px = ball_content_left;
        }
    }
    px = px.clamp(0.0, (screen_w - width).max(0.0)) + origin_x;

    // 垂直：球偏上 → 弹窗顶缘=球底缘+间隙（向下展开）；球偏下 → 弹窗底缘=球顶缘-间隙（向上展开）。
    // 上下都以球**内容**边缘为基准（见上），保证上/下方与球的间距一致（PANEL_GAP）。
    let mut py = if ball_cy < screen_h / 2.0 {
        ball_content_top + ball_h + PANEL_GAP
    } else {
        ball_content_top - PANEL_GAP - height
    };
    if py + height > screen_h {
        py = ball_content_top - PANEL_GAP - height;
    } else if py < 0.0 {
        py = ball_content_top + ball_h + PANEL_GAP;
    }
    py = py.clamp(0.0, (screen_h - height).max(0.0)) + origin_y;

    (px, py)
}

/// 面板位置：内容按 position_for_ball 规则对齐球边缘（面板左/右缘=球相应缘），
/// 窗口位置再整体平移留白，使窗口的透明留白区跟随内容一起定位。
fn panel_position_for_ball(app: &tauri::AppHandle, ball_pos: (f64, f64)) -> (f64, f64) {
    let (px, py) = position_for_ball(app, ball_pos, PANEL_WIDTH, PANEL_HEIGHT);
    (px - FLOATING_MARGIN, py - FLOATING_MARGIN)
}

/// 右键菜单位置：内容按 position_for_ball 规则对齐球边缘（菜单左/右缘=球相应缘），
/// 窗口位置再整体平移阴影留白，使窗口的阴影区跟随内容一起定位。
fn menu_position_for_ball(app: &tauri::AppHandle, ball_pos: (f64, f64)) -> (f64, f64) {
    let (px, py) = position_for_ball(app, ball_pos, MENU_WIDTH, MENU_HEIGHT);
    (px - MENU_SHADOW_MARGIN, py - MENU_SHADOW_MARGIN)
}

/// 悬停小球：定位并显示面板，通知面板拉取数据
#[tauri::command]
pub async fn show_floating_panel(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    // 拖动中不展开面板（左键按住时鼠标进出小球会重复触发 mouseenter）
    if FLOATING_DRAGGING.load(Ordering::Acquire) {
        return Ok(());
    }
    // 右键菜单打开时：面板不展开、菜单不收起（菜单保持到点击外部才关闭）
    if MENU_OPEN.load(Ordering::Acquire) {
        return Ok(());
    }
    // 互斥兜底：面板展开时收起菜单（正常流程菜单不会开着）
    if let Some(menu) = app.get_webview_window(MENU_LABEL) {
        let _ = menu.hide();
    }
    let Some(ball_pos) = current_ball_position(&app) else {
        return Ok(());
    };
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return Ok(());
    };

    let (px, py) = panel_position_for_ball(&app, ball_pos);
    let _ = panel.set_position(tauri::LogicalPosition::new(px, py));
    // 不 set_focus：面板纯展示，不抢焦点；Windows 下无焦点窗口仍可接收鼠标事件
    let _ = panel.show();
    log::debug!("[Floating] 悬浮面板已显示 pos=({px:.0},{py:.0})");

    // 定向发给面板（含球/strip），确保展开瞬间即为最新路由/供应商/用量状态，
    // 不必等下一轮兜底轮询。
    emit_data_refresh(&app);

    // 面板绝不主动查询 API：用量查询只由主窗口（useUsageQuery / 手动刷新）
    // 和托盘悬停发起，结果写入 UsageCache 后经 usage-cache-updated 事件 +
    // 面板 3s 轮询同步到这里。这里只发事件让面板重新读缓存。
    log::debug!("[Floating] 面板已显示");
    Ok(())
}

/// 隐藏面板
#[tauri::command]
pub async fn hide_floating_panel(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        let _ = panel.hide();
    }
    Ok(())
}

/// 更新悬停状态。任一端离开后启动宽限期，宽限期内双方都不悬停才真正隐藏，
/// 避免小球→面板跨窗移动时面板闪没。
#[tauri::command]
pub async fn floating_set_hover(
    app: tauri::AppHandle,
    source: String,
    active: bool,
) -> Result<(), String> {
    {
        let mut state = HOVER_STATE.lock().unwrap_or_else(|p| p.into_inner());
        match source.as_str() {
            "ball" => state.ball = active,
            "panel" => state.panel = active,
            _ => return Ok(()),
        }
    }

    // 只要有任何一端悬停，就取消可能挂起的隐藏
    let hovering = {
        let state = HOVER_STATE.lock().unwrap_or_else(|p| p.into_inner());
        state.ball || state.panel
    };
    if hovering {
        return Ok(());
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(HOVER_GRACE_MS)).await;
        let still_hovering = {
            let state = HOVER_STATE.lock().unwrap_or_else(|p| p.into_inner());
            state.ball || state.panel
        };
        if !still_hovering {
            let _ = hide_floating_panel(app.clone()).await;
            // 鼠标离开球区域后，如果开了边缘收起且球在边缘，自动重新收起
            if is_auto_collapse_enabled() && !is_ball_collapsed() {
                if let Some(ball) = app.get_webview_window(BALL_LABEL) {
                    if detect_edge(&ball).is_some() {
                        collapse_ball(&app);
                    }
                }
            }
        }
    });
    Ok(())
}

// ============================================================
// 面板交互
// ============================================================

/// 打开主窗口（命令与单击判定复用）
fn open_main_window_impl(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        let _ = window.set_skip_taskbar(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    } else if crate::lightweight::is_lightweight_mode() {
        if let Err(e) = crate::lightweight::exit_lightweight_mode(app) {
            log::warn!("[Floating] 退出轻量模式失败: {e}");
        }
    }
}

/// 打开主界面
#[tauri::command]
pub async fn open_main_window(app: tauri::AppHandle) -> Result<(), String> {
    open_main_window_impl(&app);
    Ok(())
}

/// 关闭悬浮窗的同步实现（命令与右键菜单共用）
fn disable_floating_window_impl(app: &tauri::AppHandle) -> Result<(), String> {
    destroy_floating_window(app);

    let mut settings = crate::settings::get_settings();
    settings.enable_floating_window = false;
    crate::settings::update_settings(settings).map_err(|e| e.to_string())?;
    log::info!("[Floating] 悬浮窗已关闭");
    Ok(())
}

/// 关闭悬浮窗：销毁窗口并把设置开关写回关闭
#[tauri::command]
pub async fn disable_floating_window(app: tauri::AppHandle) -> Result<(), String> {
    disable_floating_window_impl(&app)
}

/// 显示悬浮球右键菜单（自定义 HTML 菜单窗口，与面板同款样式）。
/// 由前端 `onContextMenu` 调起；与面板互斥（先收起面板）。
/// 菜单打开即抢焦点：点击菜单外部任意处（小球/桌面/其他应用）会触发
/// 菜单窗口失焦 → lib.rs 里 `Focused(false)` 统一收起（“点击别处关闭”）。
#[tauri::command]
pub async fn show_floating_context_menu(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    // 拖动中不弹菜单（左键按住时鼠标进出小球会重复触发）
    if FLOATING_DRAGGING.load(Ordering::Acquire) {
        return Ok(());
    }
    // 互斥：收起面板
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        let _ = panel.hide();
    }
    let Some(ball_pos) = current_ball_position(&app) else {
        return Ok(());
    };
    let Some(menu) = app.get_webview_window(MENU_LABEL) else {
        return Ok(());
    };

    let (px, py) = menu_position_for_ball(&app, ball_pos);
    let _ = menu.set_position(tauri::LogicalPosition::new(px, py));
    // 显示瞬间再强制一次窗口尺寸：WebView2 加载内容后可能把菜单窗口撑宽
    // （on_page_load 已设回，这里兜底到显示时刻）；窗口尺寸含阴影留白
    let (mw, mh) = menu_window_size();
    let _ = menu.set_size(tauri::LogicalSize::new(mw, mh));
    MENU_OPEN.store(true, Ordering::Release);
    let _ = menu.show();
    // 菜单是模态的，必须抢焦点，否则无法靠「失焦」感知点击别处
    let _ = menu.set_focus();
    if let Ok(size) = menu.inner_size() {
        let scale = menu.scale_factor().unwrap_or(1.0);
        log::info!(
            "[Floating] 右键菜单窗口尺寸: {:.0}x{:.0} 逻辑 (内容 {MENU_WIDTH}x{MENU_HEIGHT})",
            size.width as f64 / scale,
            size.height as f64 / scale
        );
    }
    Ok(())
}

/// 收起右键菜单的同步实现（命令与失焦事件共用）。
/// 记录收起时刻，供「点击球关菜单」区分正常单击。
pub fn hide_floating_menu_sync(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if MENU_OPEN.swap(false, Ordering::AcqRel) {
        *MENU_CLOSED_AT.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
    }
    if let Some(menu) = app.get_webview_window(MENU_LABEL) {
        let _ = menu.hide();
    }
}

/// 隐藏右键菜单
#[tauri::command]
pub async fn hide_floating_menu(app: tauri::AppHandle) -> Result<(), String> {
    hide_floating_menu_sync(&app);
    Ok(())
}

/// 菜单「设置」：打开主窗口（轻量模式先退出）并切到设置页
#[tauri::command]
pub async fn floating_open_settings(app: tauri::AppHandle) -> Result<(), String> {
    open_main_window_impl(&app);
    let _ = app.emit("open-settings", ());
    Ok(())
}

/// 保存球位置（前端拖动结束后调用；fallback 到 Rust 端 Moved 事件）
#[tauri::command]
pub async fn set_floating_ball_position(x: f64, y: f64) -> Result<(), String> {
    crate::settings::set_floating_window_position(Some(FloatingWindowPosition { x, y }))
        .map_err(|e| e.to_string())
}

/// 悬浮窗手动刷新用量（悬浮面板每行刷新按钮）：只刷新指定 app 的当前供应商。
/// 复用 queryProviderUsage（写 UsageCache + emit usage-cache-updated →
/// 悬浮面板/球监听自动刷新）。不遍历其他 app，避免「点一行刷全部」。
#[tauri::command]
pub async fn floating_refresh_usage(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    copilot_state: tauri::State<'_, crate::CopilotAuthState>,
    xai_state: tauri::State<'_, crate::XaiOAuthState>,
    app_type: String,
) -> Result<(), String> {
    let Some(parsed) = parse_app_type(&app_type) else {
        return Err(format!("未知 app: {app_type}"));
    };
    // 只刷该 app（不可见/无当前供应商则跳过，不算错误）
    let visible_apps = crate::settings::get_settings()
        .visible_apps
        .unwrap_or_default();
    if !visible_apps.is_visible(&parsed) {
        return Ok(());
    }
    let Ok(Some(provider_id)) =
        crate::settings::get_effective_current_provider(&state.db, &parsed)
    else {
        return Ok(());
    };
    match crate::commands::queryProviderUsage(
        app_handle.clone(),
        state.clone(),
        copilot_state.clone(),
        xai_state.clone(),
        provider_id,
        parsed.as_str().to_string(),
    )
    .await
    {
        Ok(_) => log::info!("[Floating] 刷新用量成功: {}", parsed.as_str()),
        Err(e) => log::warn!("[Floating] 刷新用量失败 {}: {e}", parsed.as_str()),
    }
    Ok(())
}
