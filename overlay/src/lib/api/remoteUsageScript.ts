/**
 * 远端目标下保存用量脚本（P2 收敛点）。
 *
 * 为什么单独成模块：`useProviderActions.ts` 是官方**高频改动文件**
 * （近 400 个官方提交里被改了 11 次）。把远端分支整体搬到这里之后，
 * 官方 hook 里只留一段 early-return，**官方原文一字不动**，
 * 从而把升级官方时的冲突面降到最低。
 */

import type { QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { toast } from "sonner";

import type { AppId } from "@/lib/api";
import { updateRemoteProviderMeta } from "@/lib/api/remote";
import { usageKeys } from "@/lib/query/usage";
import type { Provider, UsageScript } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

export interface SaveRemoteUsageScriptArgs {
  provider: Provider;
  script: UsageScript;
  remoteTargetId: string;
  remoteContainerId?: string;
  activeApp: AppId;
  queryClient: QueryClient;
  t: TFunction;
}

/**
 * 远端目标：直接写远端 SSOT，不写本地 DB；随后失效远端隔离的查询缓存
 * （key 与 `useUsageQuery` 的远端分支保持一致）。
 */
export async function saveRemoteUsageScript(
  args: SaveRemoteUsageScriptArgs,
): Promise<void> {
  const {
    provider,
    script,
    remoteTargetId,
    remoteContainerId,
    activeApp,
    queryClient,
    t,
  } = args;

  try {
    const updatedMeta = {
      ...provider.meta,
      usage_script: script,
    };

    await updateRemoteProviderMeta(
      remoteTargetId,
      activeApp,
      provider.id,
      updatedMeta as Record<string, unknown>,
      remoteContainerId,
    );
    await queryClient.invalidateQueries({
      queryKey: [
        "remoteProviders",
        remoteTargetId,
        remoteContainerId || "__host__",
        activeApp,
      ],
    });
    // 保存用量脚本后，失效该 provider 的用量查询缓存
    await queryClient.invalidateQueries({
      queryKey: usageKeys.scriptRemote(
        remoteTargetId,
        activeApp,
        provider.id,
        remoteContainerId,
      ),
    });
    await queryClient.invalidateQueries({
      queryKey: ["subscription", "quota", activeApp],
    });
    toast.success(
      t("provider.usageSaved", {
        defaultValue: "用量查询配置已保存",
      }),
      { closeButton: true },
    );
  } catch (error) {
    const detail =
      extractErrorMessage(error) ||
      t("provider.usageSaveFailed", {
        defaultValue: "用量查询配置保存失败",
      });
    toast.error(detail);
  }
}
