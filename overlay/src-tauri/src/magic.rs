//! 增强层在官方主流程里的**唯一安装点**（P2 收敛）。
//!
//! 为什么独立成模块：`lib.rs` 是官方**高频改动文件**（近 400 个官方提交里被改了 28 次）。
//! 把悬浮窗相关钩子全部收进本模块，官方 `lib.rs` 里只留**几个单行调用**，
//! 升级官方版本时的冲突面随之大幅缩小。
//!
//! 调用点：`lib.rs` 的 `on_window_event` / `window_state` 插件 / `setup`。

use tauri::{App, AppHandle, Manager, Window, WindowEvent};

use crate::floating;

/// window-state 插件必须排除的悬浮窗（球 / 面板 / 菜单）。
///
/// 它们的位置与尺寸完全由 floating 模块自己管理；交给插件恢复会得到历史错误尺寸
/// （实测球/菜单会被恢复成 133px 宽 → 右缘与球不对齐、菜单溢出屏幕，
/// `skip_initial_state` 拦不住）。
pub const DENIED_WINDOW_LABELS: [&str; 3] = [
    floating::BALL_LABEL,
    floating::PANEL_LABEL,
    floating::MENU_LABEL,
];

/// 全局窗口事件钩子 —— 在官方 `on_window_event` 闭包**开头**调用一次即可。
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    // 右键菜单失去焦点 = 点击了菜单外部（小球/桌面/其他应用）→ 收起菜单。
    // 菜单打开时会 set_focus，失焦即「点击别处关闭」。
    if let WindowEvent::Focused(false) = event {
        if window.label() == floating::MENU_LABEL {
            floating::hide_floating_menu_sync(window.app_handle());
        }
    }

    // 悬浮球被拖动时记录新位置（防抖落盘到 settings）
    if let WindowEvent::Moved(position) = event {
        if window.label() == floating::BALL_LABEL {
            if let Ok(scale) = window.scale_factor() {
                floating::schedule_position_save(
                    position.x as f64 / scale,
                    position.y as f64 / scale,
                );
            }
        }
    }
}

/// 悬浮窗创建 —— 在官方 `setup` 里原位置调用一次。
/// 开启时创建悬浮球窗口；独立于主窗口，轻量模式销毁主窗口后仍可工作。
pub fn ensure_floating(app: &AppHandle, enabled: bool) {
    if enabled {
        floating::ensure_floating_window(app);
    }
}

/// 启动后预热用量缓存 —— 在官方 `setup` 里原位置调用一次。
///
/// 悬浮窗按设计只读缓存、从不主动发查询；主窗口又可能未挂载用量组件，
/// 因此启动时兜底查一次（复用托盘刷新逻辑，含 10 秒防抖），
/// 结果写入 UsageCache 后经 `usage-cache-updated` 事件推给悬浮窗。
pub fn warm_usage_cache(app: &App) {
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        // 稍等片刻，等网络 / WebView / DB 就绪再查，减少启动竞态
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        crate::tray::refresh_all_usage_in_tray(&app_handle).await;
    });
}
