import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy } from "lucide-react";
import { toast } from "sonner";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { copyText } from "@/lib/clipboard";

interface InstallCommandPopoverProps {
  /** 需要复制的安装命令 */
  command: string;
}

/**
 * 「安装」按钮：点击弹出安装命令，命令可选中复制，并提供「复制命令」一键复制。
 *
 * 用 Popover 而非 Tooltip —— Tooltip 内容 pointer-events-none 无法选中/复制，
 * Popover 是可交互面板，命令既可手选复制，也有一键复制按钮。
 */
export function InstallCommandPopover({ command }: InstallCommandPopoverProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [open, setOpen] = useState(false);

  const handleCopy = async () => {
    try {
      await copyText(command);
      setCopied(true);
      // 复制成功自动收起面板
      setOpen(false);
      toast.success(t("common.copied", { defaultValue: "已复制" }), {
        closeButton: true,
      });
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error(t("common.copyFailed", { defaultValue: "复制失败" }));
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center rounded-full border border-amber-600/20 bg-amber-500/10 px-2.5 py-0.5 text-xs text-amber-600 transition-colors hover:bg-amber-500/15"
          title={t("remote.showInstallCmd", {
            defaultValue: "查看安装命令",
          })}
        >
          {t("remote.install", { defaultValue: "安装" })}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-[320px]">
        <div className="flex flex-col gap-2">
          <code className="block break-all rounded-md bg-muted p-2 font-mono text-xs leading-relaxed text-foreground">
            {command}
          </code>
          <Button
            size="sm"
            variant="outline"
            onClick={() => void handleCopy()}
            className="w-full"
          >
            {copied ? (
              <Check className="mr-1.5 h-3.5 w-3.5" />
            ) : (
              <Copy className="mr-1.5 h-3.5 w-3.5" />
            )}
            {copied
              ? t("common.copied", { defaultValue: "已复制" })
              : t("remote.copyInstallCmd", { defaultValue: "复制命令" })}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
