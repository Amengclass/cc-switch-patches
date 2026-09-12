import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { EyeOff, Settings } from "lucide-react";

/**
 * 悬浮窗右键菜单（floating-menu 透明窗，瘦高样式）：
 * - 与面板同款样式（同一套 CSS 变量 + floating.css），非系统原生菜单
 * - 菜单项顺序：设置 / 分隔线 / 隐藏
 * - 点击任意项即收起；不悬停自动关闭（由 Rust 端失焦“点击别处”收起）
 */
export function FloatingContextMenu() {
  const { t } = useTranslation();
  const close = () => void invoke("hide_floating_menu");

  const onSettings = () => {
    void invoke("floating_open_settings").catch((e) =>
      console.error("[Floating] 打开设置失败", e),
    );
    close();
  };
  const onHide = () => {
    void invoke("disable_floating_window").catch((e) =>
      console.error("[Floating] 关闭悬浮窗失败", e),
    );
  };

  return (
    <div
      className="floating-menu"
      // 菜单自身右键不弹任何东西（含 WebView 默认菜单）
      onContextMenu={(e) => e.preventDefault()}
    >
      <button className="menu-item" onClick={onSettings}>
        <Settings size={12} className="menu-icon" />
        <span className="menu-label">{t("floating.menuSettings")}</span>
      </button>
      <div className="menu-sep" />
      <button className="menu-item" onClick={onHide}>
        <EyeOff size={12} className="menu-icon" />
        <span className="menu-label">{t("floating.menuHide")}</span>
      </button>
    </div>
  );
}
