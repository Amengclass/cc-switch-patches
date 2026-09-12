import { useTranslation } from "react-i18next";
import { Server, RefreshCw, WifiOff } from "lucide-react";
import { Switch } from "@/components/ui/switch";

interface RemoteProviderSettingsProps {
  /** 远端功能总开关：关 = 还原原生 cc-switch（不显示任何远端入口） */
  featureEnabled: boolean;
  onFeatureEnabledChange: (next: boolean) => void;
  /** 是否每次读远端面板时自动读入该机器 live 的实际生效配置（default 卡） */
  value: boolean;
  onChange: (next: boolean) => void;
}

/**
 * 「远端设置」分组：存放远端宿主机/容器的供应商面板个性化选项。
 *
 * 第一项是「远端功能总开关」：关掉即整体隐藏本 fork Magic 的远程入口，
 * 还原成原生 cc-switch 行为（目标选择器、工具栏、导航均无远端项）。
 *
 * 第二项：非 additive app（claude/codex/gemini/grokbuild）的 default 卡
 * 是否每次刷新。开 = 随时可见当前机器实际生效配置（容器下每次读面板多一次
 * docker exec）；关 = 仅空库才导入，更快但看不到外部改动。
 * additive（opencode/openclaw/hermes）不受影响（live 即列表，必须同步）。
 * 后续远端个性化设置追加到本分组即可。
 */
export function RemoteProviderSettings({
  featureEnabled,
  onFeatureEnabledChange,
  value,
  onChange,
}: RemoteProviderSettingsProps) {
  const { t } = useTranslation();
  return (
    <section className="space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40">
        <Server className="h-4 w-4 text-primary" />
        <h3 className="text-sm font-medium">
          {t("settings.section.remote", { defaultValue: "SSH远端设置" })}
        </h3>
      </div>
      <p className="text-xs text-muted-foreground -mt-2">
        {t("settings.section.remoteHint", {
          defaultValue: "针对 SSH 远端宿主机/容器的供应商面板行为",
        })}
      </p>

      {/* 远端功能总开关：关 = 还原原生 cc-switch */}
      <div className="flex items-center justify-between gap-4 rounded-xl border border-border bg-card/50 p-4 transition-colors hover:bg-muted/50">
        <div className="flex items-start gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <WifiOff className="h-4 w-4 text-primary" />
          </div>
          <div className="space-y-1">
            <p className="text-sm font-medium leading-none">
              {t("settings.remote.featureEnabled.title", {
                defaultValue: "启用 SSH 远端功能",
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("settings.remote.featureEnabled.description", {
                defaultValue:
                  "开启使用 SSH 远端宿主机/容器的接管、批量应用等增强功能；关闭则还原原生 cc-switch，隐藏所有 SSH 远端入口。",
              })}
            </p>
          </div>
        </div>
        <Switch
          checked={featureEnabled}
          onCheckedChange={onFeatureEnabledChange}
          aria-label={t("settings.remote.featureEnabled.title", {
            defaultValue: "启用 SSH 远端功能",
          })}
        />
      </div>

      {/* 远端自动读入当前配置（仅远端功能开启时可选） */}
      <div className="flex items-center justify-between gap-4 rounded-xl border border-border bg-card/50 p-4 transition-colors hover:bg-muted/50 opacity-80">
        <div className="flex items-start gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-background ring-1 ring-border">
            <RefreshCw className="h-4 w-4 text-primary" />
          </div>
          <div className="space-y-1">
            <p className="text-sm font-medium leading-none">
              {t("settings.remote.autoImportDefault.title", {
                defaultValue: "SSH 远端自动读入当前配置",
              })}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("settings.remote.autoImportDefault.description", {
                defaultValue:
                  "开启后将实时展示 SSH 远端机器的当前应用配置（default 卡）；关闭后不再主动拉取最新配置，界面刷新更快。",
              })}
            </p>
            <p className="text-[11px] leading-snug text-muted-foreground/70">
              {t("settings.remote.autoImportDefault.note", {
                defaultValue:
                  "注：该设置仅对 Claude、Codex、Gemini、Grok Build 生效，OpenCode、OpenClaw、Hermes 始终跟随当前应用。",
              })}
            </p>
          </div>
        </div>
        <Switch
          checked={value && featureEnabled}
          onCheckedChange={onChange}
          disabled={!featureEnabled}
          aria-label={t("settings.remote.autoImportDefault.title", {
            defaultValue: "远端自动读入当前配置",
          })}
        />
      </div>
    </section>
  );
}
