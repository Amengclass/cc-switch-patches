import { useTranslation } from "react-i18next";
import {
  SettingsBlock,
  SettingsCard,
  SettingsSwitchRow,
} from "@/components/settings/SettingsLayout";

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
 */
export function RemoteProviderSettings({
  featureEnabled,
  onFeatureEnabledChange,
  value,
  onChange,
}: RemoteProviderSettingsProps) {
  const { t } = useTranslation();
  const featureTitle = t("settings.remote.featureEnabled.title", {
    defaultValue: "启用 SSH 远端功能",
  });
  const autoImportTitle = t("settings.remote.autoImportDefault.title", {
    defaultValue: "SSH 远端自动读入当前配置",
  });
  return (
    <SettingsBlock
      title={t("settings.section.remote", { defaultValue: "SSH 远端" })}
      help={{
        title: t("settings.section.remote", { defaultValue: "SSH 远端" }),
        body: t("settings.section.remoteHint", {
          defaultValue: "针对 SSH 远端宿主机/容器的供应商面板行为",
        }),
      }}
    >
      <SettingsCard>
        <SettingsSwitchRow
          label={featureTitle}
          help={{
            title: featureTitle,
            body: t("settings.remote.featureEnabled.description", {
              defaultValue:
                "开启使用 SSH 远端宿主机/容器的接管、批量应用等增强功能；关闭则还原原生 cc-switch，隐藏所有 SSH 远端入口。",
            }),
          }}
          checked={featureEnabled}
          onCheckedChange={onFeatureEnabledChange}
        />
        <SettingsSwitchRow
          label={autoImportTitle}
          help={{
            title: autoImportTitle,
            body: t("settings.remote.autoImportDefault.description", {
              defaultValue:
                "开启后将实时展示 SSH 远端机器的当前应用配置（default 卡）；关闭后不再主动拉取最新配置，界面刷新更快。",
            }),
          }}
          description={t("settings.remote.autoImportDefault.note", {
            defaultValue:
              "仅对 Claude、Codex、Gemini、Grok Build 生效；OpenCode、OpenClaw、Hermes 始终跟随当前应用。",
          })}
          checked={value && featureEnabled}
          onCheckedChange={onChange}
          disabled={!featureEnabled}
        />
      </SettingsCard>
    </SettingsBlock>
  );
}
