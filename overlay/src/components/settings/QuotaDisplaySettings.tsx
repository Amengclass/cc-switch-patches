import { useTranslation } from "react-i18next";
import { BarChart3, ToggleLeft } from "lucide-react";
import { ToggleRow } from "@/components/ui/toggle-row";

interface QuotaDisplaySettingsProps {
  value: string;
  onChange: (mode: string) => void;
}

export function QuotaDisplaySettings({
  value,
  onChange,
}: QuotaDisplaySettingsProps) {
  const { t } = useTranslation();

  return (
    <section className="space-y-2">
      <header className="flex items-center gap-2 space-y-1">
        <BarChart3 className="h-4 w-4 text-emerald-500" />
        <div className="space-y-1">
          <h3 className="text-sm font-medium">
            {t("settings.advanced.quotaDisplay.title", {
              defaultValue: "套餐用量显示",
            })}
          </h3>
          <p className="text-xs text-muted-foreground">
            {t("settings.advanced.quotaDisplay.description", {
              defaultValue: "控制供应商卡片上套餐用量的显示格式",
            })}
          </p>
        </div>
      </header>
      <ToggleRow
        icon={<ToggleLeft className="h-4 w-4 text-emerald-500" />}
        title={t("settings.advanced.quotaDisplay.expanded", {
          defaultValue: "展开模式",
        })}
        description={t("settings.advanced.quotaDisplay.expandedDescription", {
          defaultValue:
            "开启后：套餐用量在卡片下方独立面板展开显示（含总额/已用/剩余/重置时间）；关闭时：套餐用量直接嵌入供应商卡片（如「5小时: 5%」）",
        })}
        checked={value === "expanded"}
        onCheckedChange={(checked) =>
          onChange(checked ? "expanded" : "compact")
        }
      />
    </section>
  );
}
