import React, { useEffect, useState } from "react";

import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import {
  type FloatingEntry,
  type FloatingUsageData,
} from "./floating/types";
import { FloatingBall } from "./floating/FloatingBall";
import { FloatingPanel } from "./floating/FloatingPanel";
import { FloatingContextMenu } from "./floating/FloatingContextMenu";
import "./floating/floating.css";
// 悬浮窗是独立入口（floating.html），需自行初始化 i18n，否则 useTranslation() 读不到 key 会显示 key 名。
import "./i18n";

const THEME_STORAGE_KEY = "cc-switch-theme";

/**
 * 与主应用共享外观主题（localStorage 同源共享）：
 * - 读取 `cc-switch-theme`（light / dark / system），在悬浮窗 html 上打 light/dark 类
 * - 主窗口改主题会触发本窗口的 `storage` 事件 → 立即同步（无需重启）
 * - system 模式下监听系统深浅色变化
 * - 同步原生窗口主题，保证 matchMedia 跟随 OS
 */
function FloatingThemeSync() {
  useEffect(() => {
    const apply = () => {
      const root = document.documentElement;
      root.classList.remove("light", "dark");

      let theme: string | null = "system";
      try {
        const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
        if (stored === "light" || stored === "dark" || stored === "system") {
          theme = stored;
        }
      } catch {
        // localStorage 不可用时默认跟随系统
      }

      let isDark = false;
      if (theme === "dark") {
        isDark = true;
      } else if (theme === "system") {
        isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      }
      root.classList.add(isDark ? "dark" : "light");

      void invoke("set_window_theme", { theme }).catch(() => {});
    };

    apply();

    // 主窗口切换主题时 localStorage 变更会在其他同源窗口触发 storage 事件
    window.addEventListener("storage", apply);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", apply);
    return () => {
      window.removeEventListener("storage", apply);
      mq.removeEventListener("change", apply);
    };
  }, []);

  return null;
}

/**
 * 悬浮窗独立入口。与主应用（main.tsx → App）完全分离，
 * 后端用 `WebviewUrl::App("floating.html")` 加载本页，
 * 再按窗口 label 分派渲染小球或面板组件。
 */

/**
 * 窗口级 hover 覆盖层：占满整个透明窗口（含四周 FLOATING_MARGIN 留白）。
 * hover 判定绑在窗口层而非内容元素（.ball/.panel）上——否则鼠标移到球/面板
 * 边缘的透明留白区（视觉上仍在悬浮窗内）就会触发 mouseleave，面板随即消失。
 * 球窗口 192×52 vs 球内容 180×40、面板窗口 312×332 vs 面板内容 300×320。
 */
function FloatingHoverLayer({
  source,
  onEnter,
  children,
  enterDelayMs = 0,
}: {
  source: "ball" | "panel";
  onEnter?: () => void;
  children: React.ReactNode;
  /** 鼠标移入后延迟多少毫秒才视为「悬停」（用于球展开后停留一会再弹面板） */
  enterDelayMs?: number;
}) {
  const timerRef = React.useRef<number | null>(null);
  return (
    <div
      className="floating-hover-layer"
      onMouseEnter={() => {
        // hover 状态立即上报（不能让球滑动动画期间的 enter/leave 交替把它清掉）
        void invoke("floating_set_hover", { source, active: true });
        // 只有「弹面板」动作延迟（等球滑出动画到位，位置才正确）
        if (onEnter) {
          if (timerRef.current) window.clearTimeout(timerRef.current);
          timerRef.current = window.setTimeout(() => onEnter(), enterDelayMs);
        }
      }}
      onMouseLeave={() => {
        if (timerRef.current) {
          window.clearTimeout(timerRef.current);
          timerRef.current = null;
        }
        void invoke("floating_set_hover", { source, active: false });
      }}
    >
      {children}
    </div>
  );
}

/**
 * 侧边栏温度计指示器（纯色，无文字）：
 * - 暗色半透明底 + 圆角矩形容器
 * - 彩色填充条表示用量（绿 ≤70% / 橙 70-90% / 红 ≥90%）
 * - 多套餐：填充取最差等级颜色
 * - 只显示当前悬浮窗置顶 app 的用量（与悬浮面板置顶逻辑一致）
 */
function FloatingStrip() {
  const [entry, setEntry] = useState<FloatingEntry | null>(null);
  // 窗口尺寸变化（Rust 端 set_size 切横/竖）时强制重渲染，
  // 否则 isVertical 停在首次渲染（预创建窗口是竖的 → 顶部横条会误判为竖）
  const [, forceRender] = useState(0);

  useEffect(() => {
    const onResize = () => forceRender((n) => n + 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    let alive = true;
    const refresh = () => {
      void invoke("get_floating_ball_detail").then((data: any) => {
        if (!alive) return;
        setEntry(data as FloatingEntry | null);
      }).catch(() => {});
    };
    refresh();
    const timer = setInterval(refresh, 2000);
    return () => { alive = false; clearInterval(timer); };
  }, []);

  const usage = entry?.usage;
  // 主套餐 = usage[0]（当前置顶 app 的默认套餐，如 5h）。
  // 温度计只反映主套餐：颜色按主套餐等级、填充按主套餐已用量；
  // 不再取"所有套餐最差"——否则别的套餐高用量会把当前套餐的颜色盖掉（用户困惑点）。
  const primary = usage?.[0];
  const primaryUsed = primary?.used;
  const primaryPct = (primaryUsed != null && isFinite(primaryUsed)) ? Math.min(primaryUsed, 100) : 0;
  const fillPct = Math.max(2, primaryPct);

  function statusColor(d: FloatingUsageData | undefined): string {
    if (!d) return "#94a3b8";               // 无数据：灰
    if (d.isValid === false) return "#ef4444";
    const used = d.used;
    if (used != null && isFinite(used)) {
      if (used >= 90) return "#ef4444";
      if (used >= 70) return "#f97316";
    }
    return "#16a34a";
  }

  const fillColor = usage && usage.length > 0 ? statusColor(primary) : "#94a3b8";
  const isVertical = window.innerWidth <= window.innerHeight;

  return (
    <div
      style={{
        // 窗口比胶囊大一圈（STRIP_PAD=4px 留白），胶囊画在窗口正中央，
        // 让 CSS 圆角在更大的渲染表面上抗锯齿（跳过 WebView2 极小窗口切方问题）
        position: "fixed",
        top: 4,
        right: 4,
        bottom: 4,
        left: 4,
        background: "#e3edfe",
        // 边框黑色细线让浅色胶囊在任意壁纸上清晰可辨（浅色底足够醒目，1.5px 观感最细）
        border: "1.5px solid #000",
        boxSizing: "border-box",
        // 超大圆角 = 完美胶囊（浏览器自动按短边一半裁剪）
        borderRadius: 999,
        cursor: "default",
        overflow: "hidden",
      }}
    >
      <div
        style={{
          position: "absolute",
          background: fillColor,
          borderRadius: isVertical ? "0 0 999px 999px" : "0 999px 999px 0",
          transition: "height 0.5s ease, width 0.5s ease, background 0.5s ease",
          ...(isVertical
            ? { bottom: 0, left: 0, right: 0, height: `${fillPct}%` }
            : { top: 0, bottom: 0, left: 0, width: `${fillPct}%` }),
        }}
      />
    </div>
  );
}

function FloatingApp() {
  const label = getCurrentWindow().label;
  // strip 窗口（5×40/40×5）需要去掉 body 的 flex 居中，否则色条内容会偏移
  useEffect(() => {
    if (label === "floating-strip") {
      document.body.classList.add("strip-window");
      return () => document.body.classList.remove("strip-window");
    }
  }, [label]);
  return (
    <>
      <FloatingThemeSync />
      {label === "floating-ball" ? (
        <FloatingHoverLayer
          source="ball"
          enterDelayMs={150}
          onEnter={() => void invoke("show_floating_panel")}
        >
          <FloatingBall />
        </FloatingHoverLayer>
      ) : label === "floating-panel" ? (
        <FloatingHoverLayer source="panel">
          <FloatingPanel />
        </FloatingHoverLayer>
      ) : label === "floating-menu" ? (
        <FloatingContextMenu />
      ) : label === "floating-strip" ? (
        <FloatingStrip />
      ) : (
        <div style={{ color: "#fff", padding: 8 }}>unknown window: {label}</div>
      )}
    </>
  );
}

ReactDOM.createRoot(document.getElementById("floating-root")!).render(
  <React.StrictMode>
    <FloatingApp />
  </React.StrictMode>,
);
