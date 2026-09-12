import { Loader2, Radio } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { Switch } from "@/components/ui/switch";
import {
  listRemoteHosts,
  reapplyRemoteProvider,
  setRemoteRouteProxyApp,
} from "@/lib/api/remote";
import { cn } from "@/lib/utils";
import type { EffectReport, RemoteHost } from "@/types/remote";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import { proxyKeys } from "@/lib/query/proxy";

interface RemoteRouteToggleProps {
  /** 宿主机目标：传入该主机则显示「走本机路由」开关（读/写 routeThroughLocalProxy） */
  host?: RemoteHost;
  /** 容器目标：显示所属宿主机的开关（隧道建在宿主机上），文案按容器场景 */
  container?: boolean;
  /** 当前应用（用于按 app 显示专属文案，与 ProxyToggle/ClaudeDesktopRouteToggle 统一） */
  activeApp?: string;
  /** 调用远端 API 用的 app 参数（claude-desktop 映射为 claude 的 sharedFeatureApp 值） */
  appForApi?: string;
  /** 容器 id（容器目标时传给远端命令做网络探测/DNAT 下发） */
  containerId?: string;
  /** 保存成功后的回调（用于更新上层 hosts 列表状态） */
  onUpdated?: (host: RemoteHost) => void;
  /** 仅刷新开关状态（不 invalidate 远端供应商，避免二次刷新抖两次） */
  onHostRefreshed?: (host: RemoteHost) => void;
  className?: string;
}

/**
 * 顶栏「走本机路由」开关（按目标动态渲染）：
 * - 宿主机目标：显示该主机的 routeThroughLocalProxy 开关，切换直接写主机设置；
 * - 容器目标：隧道建在宿主机 SSH 上，故显示所属宿主机的同一开关（文案按容器场景）；
 *   容器网络自动探测（host→localhost / bridge→网关+DNAT）由后端切换时完成。
 */
export function RemoteRouteToggle({
  host,
  container,
  activeApp,
  appForApi,
  containerId,
  onUpdated,
  onHostRefreshed,
  className,
}: RemoteRouteToggleProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [saving, setSaving] = useState(false);
  // 本机路由服务（总开关）运行状态：远端接管依赖它，走马灯边框以此为准
  const { isRunning: proxyRunning } = useProxyStatus();
  // per-app 开关值（key = appForApi，即 claude/codex/gemini/grokbuild）
  // 容器目标读 routeProxyContainerApps[containerId][app]（各自独立）；
  // 宿主机目标读 routeProxyApps[app]。
  const appKey = appForApi ?? activeApp;
  const routeEnabled = Boolean(
    appKey &&
      (containerId
        ? host?.routeProxyContainerApps?.[containerId]?.[appKey]
        : host?.routeProxyApps?.[appKey]),
  );

  // 与 ProxyToggle 相同的 app 显示名映射，保证文案统一
  const appLabel =
    activeApp === "claude-desktop"
      ? "Claude Desktop"
      : activeApp === "claude"
        ? "Claude"
        : activeApp === "codex"
          ? "Codex"
          : activeApp === "gemini"
            ? "Gemini"
            : "Grok Build";

  // 未找到所属主机（异常态）：不渲染
  if (!host) {
    return null;
  }

  const handleToggle = async (checked: boolean) => {
    if (saving || !appKey) return;
    setSaving(true);
    try {
      const updated = await setRemoteRouteProxyApp(
        host.id,
        appKey,
        checked,
        containerId,
      );
      onUpdated?.(updated);
      // 对齐本机 useProxyStatus.setTakeoverForApp 的 onSuccess：开/关都刷新
      // 代理状态与接管状态。注意 running=false 时 useProxyStatusQuery 不轮询，
      // 打开接管后必须主动刷新流心（走马灯）才会及时出现；关闭时后端可能
      // 停止代理服务，主动刷新也能立即反映。
      queryClient.invalidateQueries({ queryKey: proxyKeys.status });
      queryClient.invalidateQueries({ queryKey: proxyKeys.takeoverStatus });
      // 对齐本机 stopWithRestore：关闭接管且后端据此停止代理服务时，
      // 健康/熔断统计已失效，清除缓存避免 UI 显示过期数据。
      if (!checked) {
        queryClient.removeQueries({ queryKey: ["providerHealth"] });
        queryClient.removeQueries({ queryKey: ["circuitBreakerStats"] });
      }
      // 对齐本机「开关即生效」：立即把当前供应商按新意图重写 live，
      // 无需用户再手动切一次供应商（含容器网络探测/DNAT 下发）。
      let report: EffectReport | null = null;
      try {
        report = await reapplyRemoteProvider(host.id, appKey, containerId);
      } catch (error) {
        console.error("[RemoteRouteToggle] reapply failed:", error);
        toast.error(
          t("remote.route.reapplyFailed", {
            defaultValue:
              "开关已保存，但重新应用当前供应商到远端失败，请再切一次供应商",
          }),
        );
      }
      // 重新拉取主机列表，让开关跟随后端真实状态：隧道建立失败时后端会回退
      // DB 开关（revert_route_switch_on_tunnel_failure），必须刷新 UI 归位，
      // 否则开关会一直显示「开」而实际已回退（servers 状态只在切视图时刷新）。
      try {
        const hosts = await listRemoteHosts();
        const fresh = hosts.find((h) => h.id === host.id);
        if (fresh) onHostRefreshed?.(fresh);
      } catch {
        // 刷新失败不阻塞；下次切视图会自动对齐
      }
      if (report?.warnings?.length) {
        // 隧道未建立、按直连写入等：明确告知用户接管未真正生效，
        // 不再弹「已开启」成功提示以免误导。
        report.warnings.forEach((w) => toast.warning(w));
      } else {
        toast.success(
          checked
            ? t("remote.route.enabled", {
                name: host.name,
                appLabel,
                defaultValue: `已开启「${host.name}」的 ${appLabel} 远端接管`,
              })
            : t("remote.route.disabled", {
                name: host.name,
                appLabel,
                defaultValue: `已关闭「${host.name}」的 ${appLabel} 远端接管`,
              }),
        );
      }
    } catch (error) {
      console.error("[RemoteRouteToggle] toggle failed:", error);
      toast.error(
        t("remote.route.toggleFailed", {
          defaultValue: "切换远端接管失败，请重试",
        }),
      );
    } finally {
      setSaving(false);
    }
  };

  const tooltipText = routeEnabled
    ? container
      ? t("remote.route.tooltip.containerActive", {
          name: host.name,
          appLabel,
          defaultValue: `容器内 ${appLabel} 经「${host.name}」宿主机隧道走本机代理（本机代理未运行时自动启动）`,
        })
      : `${t("remote.route.tooltip.active", {
          name: host.name,
          appLabel,
          defaultValue: `「${host.name}」的 ${appLabel} 已启用远端接管，请求经本机代理转发`,
        })}${proxyRunning ? `\n${t("settings.advanced.proxy.marqueeHint")}` : ""}`
    : container
      ? t("remote.route.tooltip.containerInactive", {
          name: host.name,
          appLabel,
          defaultValue: `开启后，容器内 ${appLabel} 经「${host.name}」宿主机隧道走本机代理`,
        })
      : t("remote.route.tooltip.inactive", {
          name: host.name,
          appLabel,
          defaultValue: `开启后「${host.name}」的 ${appLabel} 请求经本机代理转发（本机代理未运行时自动启动）`,
        });

  return (
    <div
      className={cn(
        "flex items-center gap-1 px-1.5 h-8 rounded-lg bg-muted/50 transition-all",
        proxyRunning && "route-service-live",
        className,
      )}
      title={tooltipText}
    >
      {saving ? (
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      ) : (
        <Radio
          className={cn(
            "h-4 w-4 transition-colors",
            routeEnabled
              ? "text-emerald-500 status-heartbeat"
              : "text-muted-foreground",
          )}
        />
      )}
      <Switch
        checked={routeEnabled}
        onCheckedChange={handleToggle}
        disabled={saving || !appKey}
      />
    </div>
  );
}
