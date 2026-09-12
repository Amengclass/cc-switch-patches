import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SettingsFormState } from "@/hooks/useSettings";
import { AppWindow, ArrowDownToLine, CircleDot, Eye, Gauge, Lock, Pin } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { ToggleRow } from "@/components/ui/toggle-row";
import { AnimatePresence, motion } from "framer-motion";

/** 悬浮窗可置顶显示的 app 列表（key 与 AppType::as_str 一致） */
const FLOATING_PIN_APPS: Array<{ key: string; label: string }> = [
  { key: "claude", label: "Claude" },
  { key: "claude-desktop", label: "Claude Desktop" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
  { key: "grokbuild", label: "Grok Build" },
  { key: "opencode", label: "OpenCode" },
  { key: "openclaw", label: "OpenClaw" },
  { key: "hermes", label: "Hermes" },
];

interface FloatingWindowSettingsProps {
  settings: SettingsFormState;
  onChange: (updates: Partial<SettingsFormState>) => void;
}

/**
 * 悬浮窗（加速球）设置：独立区块，与主窗口的「窗口行为」分离。
 */
export function FloatingWindowSettings({
  settings,
  onChange,
}: FloatingWindowSettingsProps) {
  const { t } = useTranslation();
  // 置顶 app（None = 跟随最近活跃）：直接从后端读，避免与主设置表单耦合；
  // 面板每行图钉与这里共用 floating_set_pin_app，保持一个数据源。
  const [pinApp, setPinApp] = useState<string | null>(null);
  const [pinLoaded, setPinLoaded] = useState(false);

  useEffect(() => {
    let alive = true;
    (
      invoke("get_floating_ball_target") as Promise<{
        appType: string;
        isPinned: boolean;
      } | null>
    )
      .then((target) => {
        if (!alive) return;
        setPinApp(target?.isPinned ? target.appType : null);
        setPinLoaded(true);
      })
      .catch((e) => console.error("[Settings] 读取悬浮窗置顶 app 失败", e));
    return () => {
      alive = false;
    };
  }, []);

  const changePin = (value: string) => {
    setPinApp(value || null);
    void invoke("floating_set_pin_app", { appType: value || null }).catch((e) =>
      console.error("[Settings] 设置置顶 app 失败", e),
    );
  };

  return (
    <section className="space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40">
        <AppWindow className="h-4 w-4 text-rose-500" />
        <h3 className="text-sm font-medium">{t("settings.floatingWindow")}</h3>
      </div>

      <div className="space-y-3">
        <ToggleRow
          icon={<CircleDot className="h-4 w-4 text-rose-500" />}
          title={t("settings.enableFloatingWindow")}
          description={t("settings.enableFloatingWindowDescription")}
          checked={!!settings.enableFloatingWindow}
          onCheckedChange={(value) => onChange({ enableFloatingWindow: value })}
        />

        <AnimatePresence initial={false}>
          {settings.enableFloatingWindow && (
            <motion.div
              key="floating-extra"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 10 }}
              transition={{ duration: 0.3 }}
            >
              {/* 悬浮窗行为配置组：四项统一放进灰底圆角框，视觉成整体 */}
              <div className="mt-3 space-y-3 rounded-lg border border-border/40 bg-muted/40 p-3">
                <div className="text-xs font-semibold text-muted-foreground">
                  悬浮窗
                </div>
                {/* 固定当前位置 */}
                <div className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2">
                    <Lock className="h-3.5 w-3.5 shrink-0 text-rose-500" />
                    <div className="space-y-0.5">
                      <div className="text-sm font-medium leading-none">
                        {t("settings.floatingLockedTitle")}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t("settings.floatingLockedDesc")}
                      </div>
                    </div>
                  </div>
                  <Switch
                    checked={!!settings.floatingLocked}
                    onCheckedChange={(value) =>
                      onChange({ floatingLocked: value })
                    }
                  />
                </div>
                {/* 边缘自动收起 */}
                <div className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2">
                    <ArrowDownToLine className="h-3.5 w-3.5 shrink-0 text-rose-500" />
                    <div className="space-y-0.5">
                      <div className="text-sm font-medium leading-none">
                        {t("settings.floatingAutoCollapseTitle")}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t("settings.floatingAutoCollapseDesc")}
                      </div>
                    </div>
                  </div>
                  <Switch
                    checked={!!settings.floatingAutoCollapse}
                    onCheckedChange={(value) =>
                      onChange({ floatingAutoCollapse: value })
                    }
                  />
                </div>
                {/* 悬浮窗内容显示 */}
                <div className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2">
                    <Pin className="h-3.5 w-3.5 shrink-0 text-rose-500" />
                    <div className="space-y-0.5">
                      <div className="text-sm font-medium leading-none">
                        {t("settings.floatingPinTitle")}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t("settings.floatingPinDesc")}
                      </div>
                    </div>
                  </div>
                  <select
                    value={pinLoaded ? (pinApp ?? "") : ""}
                    onChange={(e) => changePin(e.target.value)}
                    className="h-8 rounded-md border border-border/40 bg-background px-2 text-sm text-foreground outline-none"
                  >
                    <option value="">{t("settings.floatingPinFollow")}</option>
                    {FLOATING_PIN_APPS.map((a) => (
                      <option key={a.key} value={a.key}>
                        {a.label}
                      </option>
                    ))}
                  </select>
                </div>
                {/* 吸附动画速度 */}
                <div className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2">
                    <Gauge className="h-3.5 w-3.5 shrink-0 text-rose-500" />
                    <div className="space-y-0.5">
                      <div className="text-sm font-medium leading-none">
                        {t("settings.floatingSnapSpeed")}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t("settings.floatingSnapSpeedDescription")}
                      </div>
                    </div>
                  </div>
                  <select
                    value={settings.floatingSnapSpeedMs ?? 160}
                    onChange={(e) =>
                      onChange({ floatingSnapSpeedMs: Number(e.target.value) })
                    }
                    className="h-8 rounded-md border border-border/40 bg-background px-2 text-sm text-foreground outline-none"
                  >
                    <option value={0}>
                      {t("settings.floatingSnapSpeedOff")}
                    </option>
                    <option value={80}>
                      {t("settings.floatingSnapSpeedFast")}
                    </option>
                    <option value={160}>
                      {t("settings.floatingSnapSpeedMedium")}
                    </option>
                    <option value={300}>
                      {t("settings.floatingSnapSpeedSlow")}
                    </option>
                  </select>
                </div>
                {/* 悬浮窗透明度 */}
                <div className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2">
                    <Eye className="h-3.5 w-3.5 shrink-0 text-rose-500" />
                    <div className="space-y-0.5">
                      <div className="text-sm font-medium leading-none">
                        {t("settings.floatingOpacityTitle")}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t("settings.floatingOpacityDesc")}
                      </div>
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="range"
                      min={0.2}
                      max={1}
                      step={0.05}
                      value={settings.floatingOpacity ?? 0.97}
                      onChange={(e) =>
                        onChange({ floatingOpacity: Number(e.target.value) })
                      }
                      className="w-28 h-1.5 cursor-pointer accent-rose-500"
                    />
                    <span className="w-9 text-right text-sm tabular-nums text-muted-foreground">
                      {Math.round((settings.floatingOpacity ?? 0.97) * 100)}%
                    </span>
                  </div>
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </section>
  );
}
