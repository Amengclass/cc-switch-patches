import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SettingsFormState } from "@/hooks/useSettings";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  SettingsCard,
  SettingsRow,
  SettingsSwitchRow,
} from "@/components/settings/SettingsLayout";

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

/** 吸附动画速度档位（毫秒）；0 = 不播放吸附动画 */
const SNAP_SPEED_OPTIONS: Array<{ value: number; labelKey: string }> = [
  { value: 0, labelKey: "settings.floatingSnapSpeedOff" },
  { value: 80, labelKey: "settings.floatingSnapSpeedFast" },
  { value: 160, labelKey: "settings.floatingSnapSpeedMedium" },
  { value: 300, labelKey: "settings.floatingSnapSpeedSlow" },
];

/** 置顶下拉里代表「跟随当前应用」的哨兵值（空串不能做 Radix SelectItem 的 value） */
const PIN_FOLLOW = "__follow__";

interface FloatingWindowSettingsProps {
  settings: SettingsFormState;
  onChange: (updates: Partial<SettingsFormState>) => void;
}

/**
 * 悬浮窗（加速球）设置。
 *
 * 用官方 v4.0.4 的 `SettingsCard` / `SettingsSwitchRow` / `SettingsRow` 排版，
 * 与设置页其它分节视觉一致：**不带图标**；说明文字按官方惯例放进「?」提示
 * （`help`），不写在名称下面（名称下只放真正的数据，如路径、地址）。
 */
export function FloatingWindowSettings({
  settings,
  onChange,
}: FloatingWindowSettingsProps) {
  const { t } = useTranslation();
  // 置顶 app（空 = 跟随最近活跃）：直接从后端读，避免与主设置表单耦合；
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
    const next = value === PIN_FOLLOW ? null : value;
    setPinApp(next);
    void invoke("floating_set_pin_app", { appType: next }).catch((e) =>
      console.error("[Settings] 设置置顶 app 失败", e),
    );
  };

  const opacity = settings.floatingOpacity ?? 0.97;

  return (
    <SettingsCard>
      <SettingsSwitchRow
        label={t("settings.enableFloatingWindow")}
        help={{
          title: t("settings.enableFloatingWindow"),
          body: t("settings.enableFloatingWindowDescription"),
        }}
        checked={!!settings.enableFloatingWindow}
        onCheckedChange={(value) => onChange({ enableFloatingWindow: value })}
      />

      {/* 开关关掉后其余项没有意义，按官方惯例直接不渲染这一组 */}
      {settings.enableFloatingWindow && (
        <>
          <SettingsSwitchRow
            label={t("settings.floatingLockedTitle")}
            help={{
              title: t("settings.floatingLockedTitle"),
              body: t("settings.floatingLockedDesc"),
            }}
            checked={!!settings.floatingLocked}
            onCheckedChange={(value) => onChange({ floatingLocked: value })}
          />
          <SettingsSwitchRow
            label={t("settings.floatingAutoCollapseTitle")}
            help={{
              title: t("settings.floatingAutoCollapseTitle"),
              body: t("settings.floatingAutoCollapseDesc"),
            }}
            checked={!!settings.floatingAutoCollapse}
            onCheckedChange={(value) =>
              onChange({ floatingAutoCollapse: value })
            }
          />
          <SettingsRow
            label={t("settings.floatingPinTitle")}
            help={{
              title: t("settings.floatingPinTitle"),
              body: t("settings.floatingPinDesc"),
            }}
            control={
              <Select
                value={pinLoaded ? (pinApp ?? PIN_FOLLOW) : PIN_FOLLOW}
                onValueChange={changePin}
              >
                <SelectTrigger
                  className="h-8 w-[160px]"
                  aria-label={t("settings.floatingPinTitle")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={PIN_FOLLOW}>
                    {t("settings.floatingPinFollow")}
                  </SelectItem>
                  {FLOATING_PIN_APPS.map((app) => (
                    <SelectItem key={app.key} value={app.key}>
                      {app.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            }
          />
          <SettingsRow
            label={t("settings.floatingSnapSpeed")}
            help={{
              title: t("settings.floatingSnapSpeed"),
              body: t("settings.floatingSnapSpeedDescription"),
            }}
            control={
              <Select
                value={String(settings.floatingSnapSpeedMs ?? 160)}
                onValueChange={(value) =>
                  onChange({ floatingSnapSpeedMs: Number(value) })
                }
              >
                <SelectTrigger
                  className="h-8 w-[160px]"
                  aria-label={t("settings.floatingSnapSpeed")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SNAP_SPEED_OPTIONS.map((option) => (
                    <SelectItem key={option.value} value={String(option.value)}>
                      {t(option.labelKey)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            }
          />
          <SettingsRow
            label={t("settings.floatingOpacityTitle")}
            help={{
              title: t("settings.floatingOpacityTitle"),
              body: t("settings.floatingOpacityDesc"),
            }}
            control={
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={0.2}
                  max={1}
                  step={0.05}
                  value={opacity}
                  onChange={(event) =>
                    onChange({ floatingOpacity: Number(event.target.value) })
                  }
                  aria-label={t("settings.floatingOpacityTitle")}
                  className="h-1.5 w-28 cursor-pointer accent-primary"
                />
                <span className="w-9 text-right text-caption tabular-nums text-fg-2">
                  {Math.round(opacity * 100)}%
                </span>
              </div>
            }
          />
        </>
      )}
    </SettingsCard>
  );
}
