import { cloneElement, useCallback, useEffect, useState } from "react";
import type { CSSProperties, PointerEvent, ReactElement } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { Pin } from "lucide-react";
import { APP_ICON_MAP } from "@/config/appConfig";
import { type FloatingEntry, type FloatingUsageData } from "./types";

/** app 小图标：按当前 appType 取主应用对应品牌图标，并强制成指定尺寸。
 *  APP_ICON_MAP 里的元素是固定 size（14）的 ReactElement，不能直接塞进更小
 *  (10px) 容器否则会被裁切，用 cloneElement 统一成目标尺寸。 */
function AppIcon({ size, appType }: { size: number; appType: string }) {
  const cfg = APP_ICON_MAP[appType as keyof typeof APP_ICON_MAP];
  if (!cfg?.icon) return null;
  const icon = cloneElement(cfg.icon as ReactElement<{ size?: number }>, {
    size,
  });
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
        color: "currentColor",
      }}
    >
      {icon}
    </span>
  );
}

interface Summary {
  level: UsageLevel;
  color: string;
  /** app 类型 key（claude/codex/gemini/...），用于取对应品牌图标 */
  appType: string;
  app: string;
  /** 是否手动置顶（面板/设置页图钉圈定）；置顶时球显示小图钉 */
  isPinned: boolean;
  /** 当前 app 是否处「远端接管」；是则球显示流动边框 */
  takeoverActive: boolean;
  provider: string;
  model: string;
  /** 余量数值（状态色；单套餐用） */
  usageValue: string;
  /** 单位（如 CNY / GB，灰色，与主窗口一致） */
  usageUnit: string;
  /** 多套餐订阅：每行一个套餐缩写（如 h5% / d20% / m30%），右半区三行显示 */
  usageLines: string[] | null;
  /** 多套餐时每行独立颜色等级（与 usageLines 一一对应） */
  usageLineLevels: UsageLevel[] | null;
  /** 悬浮窗不透明度（0.2~1.0，设置页滑块调节） */
  opacity: number;
}

type UsageLevel = "danger" | "warn" | "good" | "none";

/** 单个用量条目的颜色等级（与供应商面板 usageValueClass 阈值一致） */
function usageLevel(d: FloatingUsageData): UsageLevel {
  if (d.isValid === false) return "danger";
  const used = d.used;
  if (used != null && isFinite(used)) {
    if (used >= 90) return "danger";
    if (used >= 70) return "warn";
  }
  // 余额型（remaining ≤ 0 = 已透支）按 danger 处理，避免显示灰点
  const rem = d.remaining;
  if (rem != null && isFinite(rem) && rem <= 0) return "danger";
  return "good";
}

/**
 * 悬浮球状态色分级（与主窗口 UsageFooter 语义一致）：
 * `isValid === false` → danger（红）；`used >= 90` → warn（橙）；否则 good（绿）。
 * 多套餐时取所有套餐里最差的一个。
 * 注：悬浮球只做 3 级（红/橙/绿），>=90 是 warn 不是 danger，
 * 与供应商面板的 3 级（>=90 红、>=70 橙）不同——球体小，不需要那么细。
 */
function worstUsageLevel(usage: FloatingUsageData[] | null): UsageLevel {
  if (!usage || usage.length === 0) return "none";
  let warn = false;
  for (const d of usage) {
    if (d.isValid === false) return "danger";
    const used = d.used;
    if (used != null && isFinite(used)) {
      if (used >= 90) return "danger";
      if (used >= 70) warn = true;
    }
    // 余额型透支
    const rem = d.remaining;
    if (rem != null && isFinite(rem) && rem <= 0) return "danger";
  }
  return warn ? "warn" : "good";
}

/** 圆点色号：danger/warn/good 与主窗口 red/orange/green 一致，none 用灰 */
function ballStatusColor(level: UsageLevel): string {
  switch (level) {
    case "danger":
      return "#ef4444";
    case "warn":
      return "#f97316";
    case "good":
      return "#16a34a";
    default:
      return "#94a3b8";
  }
}

/** 套餐名 → 缩写（小时/天/周/月/年；匹配不到返回空） */
function planAbbr(name?: string | null): string {
  const n = name?.toLowerCase() ?? "";
  if (/hour|小时/.test(n)) return "h";
  if (/week|周/.test(n)) return "w";
  if (/day|天/.test(n)) return "d";
  if (/month|月/.test(n)) return "m";
  if (/year|年/.test(n)) return "y";
  return "";
}

/** 悬浮窗当前应显示的目标 app（由后端解析：置顶优先，否则最近活跃 app） */
export interface FloatingBallTarget {
  appType: string;
  isPinned: boolean;
  appLabel: string;
  takeoverActive: boolean;
  /** 悬浮窗不透明度（0.2~1.0，设置页滑块调节） */
  opacity: number;
}

/** 读取悬浮窗目标 app 的供应商 / 模型 / 余量。
 *  只用两个轻量命令（get_floating_ball_detail 只查目标一个 app、get_floating_ball_target
 *  只回 app/isPinned），避免面板全量扫描拖慢球的响应。 */
async function loadSummary(
  t: (key: string, opts?: Record<string, unknown>) => string,
): Promise<Summary> {
  const [entry, target] = (await Promise.all([
    invoke("get_floating_ball_detail") as Promise<FloatingEntry | null>,
    invoke("get_floating_ball_target") as Promise<FloatingBallTarget | null>,
  ])) as [FloatingEntry | null, FloatingBallTarget | null];

  const isPinned = !!target?.isPinned;
  const takeoverActive = !!target?.takeoverActive;
  const opacity = target?.opacity ?? 0.97;

  if (!entry)
    return {
      level: "none",
      color: "#94a3b8",
      appType: target?.appType ?? "claude",
      app: target?.appLabel ?? "CC",
      isPinned,
      takeoverActive,
      provider: "—",
      model: "—",
      usageValue: "—",
      usageUnit: "",
      usageLines: null,
      usageLineLevels: null,
      opacity,
    };

  // 多套餐订阅（≥2 个）：右半区每行一个套餐缩写（如 h5% / d20% / m30%）
  let usageValue: string;
  let usageUnit = "";
  let usageLines: string[] | null = null;
  let usageLineLevels: UsageLevel[] | null = null;
  if (entry.usage && entry.usage.length > 1) {
    usageLines = entry.usage.map((u) => {
      const r = u.used ?? 0;
      return `${planAbbr(u.planName)}${Math.round(r)}%`;
    });
    usageLineLevels = entry.usage.map((u) => usageLevel(u));
    usageValue = "";
  } else {
    const d = entry.usage?.[0];
    // 余额型套餐（CNY/GB）只填 remaining，订阅型只填 used；
    // 取有值者，避免「剩余 -2.97 CNY」被显示成未设置
    const val = d ? (d.used ?? d.remaining) : null;
    if (val != null && isFinite(val)) {
      usageValue = val.toFixed(2);
      usageUnit = d?.unit ?? "";
    } else {
      usageValue = entry.usageSummary ?? t("floating.notSet");
    }
  }

  const level = worstUsageLevel(entry.usage);
  return {
    level,
    color: ballStatusColor(level),
    appType: entry.appType,
    app: entry.appLabel,
    isPinned,
    takeoverActive,
    provider: entry.hasProvider ? (entry.providerName ?? "") : "",
    model: entry.model ?? "",
    usageValue,
    usageUnit,
    usageLines,
    usageLineLevels,
    opacity,
  };
}

/**
 * 悬浮窗（方案C：240×56 横向胶囊条，本身就是信息条——左 app/模型，右 余量）：
 * - 拖动：按下/松开只发信号给 Rust，Rust 用 GetCursorPos 轮询全局光标 + set_position
 *   移动窗口（绕开 WebView 事件），松手自动吸附，无位移即单击→打开主窗口
 * - 悬停展开面板，离开时通知后端宽限隐藏
 */
export function FloatingBall() {
  const { t } = useTranslation();
  const [summary, setSummary] = useState<Summary>({
    level: "none",
    color: "#94a3b8",
    appType: "claude",
    app: "CC",
    isPinned: false,
    takeoverActive: false,
    provider: "—",
    model: "—",
    usageValue: "—",
    usageUnit: "",
    usageLines: null,
    usageLineLevels: null,
    opacity: 0.97,
  });
  const [previewEdge, setPreviewEdge] = useState<string | null>(null);
  const [animState, setAnimState] = useState<"idle" | "fading-in" | "fading-out">("idle");

  const refresh = useCallback(() => {
    loadSummary(t)
      .then(setSummary)
      .catch((e) => console.error("[Floating] 刷新数据失败", e));
  }, [t]);

  useEffect(() => {
    refresh();
    const unlisteners: Array<() => void> = [];

    // provider-switched：记录最近活跃 app（供未置顶时跟随）+ 刷新球
    void listen<{ appType?: string }>("provider-switched", (ev) => {
      const appType = ev.payload?.appType;
      if (appType) {
        void invoke("floating_record_active_app", { appType }).catch((e) =>
          console.error("[Floating] 记录活跃 app 失败", e),
        );
      }
      refresh();
    }).then((u) => unlisteners.push(u));
    // 置顶/跟随目标变化：用轻量命令立刻刷新球，不等全量 data-refresh
    void listen("floating-pin-changed", refresh).then((u) =>
      unlisteners.push(u),
    );
    void listen("floating-data-refresh", refresh).then((u) =>
      unlisteners.push(u),
    );
    void listen("usage-cache-updated", refresh).then((u) =>
      unlisteners.push(u),
    );
    // 拖拽靠近边缘：球变半透明（strip 窗口预览由 Rust 端直接管理）
    void listen<boolean>("floating-drag-near-edge", (ev) => {
      setPreviewEdge(ev.payload ? "near" : null);
    }).then((u) => unlisteners.push(u));
    // 展开/收起淡入淡出动画
    void listen("floating-ball-expand", () => {
      setAnimState("fading-in");
      // 250ms 后恢复 idle（与 CSS transition 时长匹配）
      setTimeout(() => setAnimState("idle"), 260);
    }).then((u) => unlisteners.push(u));
    void listen("floating-ball-collapse", () => {
      setAnimState("fading-out");
    }).then((u) => unlisteners.push(u));
    // 1s 轮询是主要可靠路径：实测跨窗口事件（含定向 emit）到悬浮窗 webview 并
    // 不可靠，且隐藏窗口会被 Chromium 节流而推迟事件回调。球虽常年可见，但依赖
    // 事件会导致「有时快、有时慢」；1s 轮询稳定且 get_floating_ball_detail 只查
    // 单个 app（本地缓存读取），成本可忽略。
    // 事件监听保留作加速：能收到时即时刷新，收不到最多 1s 后由轮询补齐。
    const timer = setInterval(refresh, 1_000);
    return () => {
      clearInterval(timer);
      unlisteners.forEach((u) => u());
    };
  }, [refresh]);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    void invoke("floating_drag_begin").catch((err) =>
      console.error("[Floating] 拖动开始失败", err),
    );
  };

  // 拖拽预览：靠近边缘时球变透明 + 边缘显示色条预览
  const showPreview = !!previewEdge;
  const ballOpacity = animState === "fading-in" ? 0 : animState === "fading-out" ? 0 : undefined;

  return (
    <div
      className={[
        "ball",
        summary.takeoverActive ? "route-service-live" : "",
        showPreview ? "drag-previewing" : "",
      ].filter(Boolean).join(" ")}
      title={summary.takeoverActive ? t("floating.takeoverActive") : undefined}
      style={{
        "--ball-alpha": summary.opacity,
        position: "relative",
        opacity: ballOpacity,
      } as CSSProperties}
      onPointerDown={onPointerDown}
      onPointerUp={() => void invoke("floating_drag_end")}
      onPointerCancel={() => void invoke("floating_drag_end")}
      onContextMenu={(e) => {
        e.preventDefault();
        void invoke("show_floating_context_menu").catch((err) =>
          console.error("[Floating] 打开右键菜单失败", err),
        );
      }}
    >
      {/* 中区：app 图标 + 名 + 图钉 + 供应商蓝底球标 */}
      <div className="ball-left">
        <span className="ball-app">
          <AppIcon size={10} appType={summary.appType} />
          <span className="ball-app-text">{summary.app}</span>
          {summary.isPinned && (
            <span
              className="ball-pin"
              title={t("floating.pinnedTitle")}
              aria-label={t("floating.pinnedAria")}
            >
              <Pin size={9} />
            </span>
          )}
        </span>
        <span className="ball-model">
          {/* 只显示供应商；用模型那套淡品牌蓝底蓝字突出，模型名工具内已可见不重复 */}
          {summary.provider ? (
            <span className="ball-tag ball-tag-model">{summary.provider}</span>
          ) : (
            <span>—</span>
          )}
        </span>
      </div>
      <span
        className={
          summary.usageLines ? "ball-usage ball-usage-multi" : "ball-usage"
        }
      >
        {summary.usageLines ? (
          summary.usageLines.map((line, i) => {
            const lineLevel = summary.usageLineLevels?.[i] ?? summary.level;
            return (
              <span
                key={i}
                className={
                  lineLevel === "none"
                    ? undefined
                    : `usage-value-${lineLevel}`
                }
              >
                {line}
              </span>
            );
          })
        ) : (
          <>
            <span
              className={
                summary.level === "none"
                  ? undefined
                  : `usage-value-${summary.level}`
              }
            >
              {summary.usageValue}
            </span>
            {summary.usageUnit && (
              <span className="ball-usage-unit">{summary.usageUnit}</span>
            )}
          </>
        )}
      </span>
    </div>
  );
}
