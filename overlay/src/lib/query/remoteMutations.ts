import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  addRemoteProvider,
  deleteRemoteProvider,
  getRemoteProviders,
  switchRemoteProvider,
  updateRemoteProvider,
  updateRemoteProviderSortOrder,
} from "@/lib/api/remote";
import type { RemoteProvidersView } from "@/lib/api/remote";
import { extractErrorMessage } from "@/utils/errorUtils";
import type { Provider } from "@/types";
import type { ProviderEditorSave, ProviderSortUpdate } from "@/lib/api/providers";

export interface SwitchRemoteProviderVars {
  hostId: string;
  providerId: string;
  app: string;
  container?: string;
}

/** 远端供应商写操作的公共变量：目标 + app + container */
interface RemoteProviderBaseVars {
  hostId: string;
  app: string;
  container?: string;
}

/** 远端 providers 视图缓存 key（与 useRemoteProvidersQuery 逐字一致） */
const remoteProvidersKey = (
  hostId: string,
  container: string | undefined,
  app: string,
) => ["remoteProviders", hostId, container || "__host__", app] as const;

/**
 * 把写操作返回的最新视图直接写入缓存（免第二次 SSH 往返）。
 * 与 useRemoteProvidersQuery 的 keepPreviousData 配合，UI 立即刷新不闪骨架屏。
 */
const setRemoteProvidersData = (
  queryClient: ReturnType<typeof useQueryClient>,
  vars: RemoteProviderBaseVars,
  view: RemoteProvidersView,
) => {
  queryClient.setQueryData(
    remoteProvidersKey(vars.hostId, vars.container, vars.app),
    view,
  );
};

/**
 * 远端供应商面板数据源 query（per-target 独立）：
 * 目标选择器指向哪，列表就显示哪台机器的供应商（该目标自己的 SSOT）。
 * 本机目标（remoteTargetId 为空）不启用，走本机 useProvidersQuery。
 */
export const useRemoteProvidersQuery = (
  hostId?: string,
  container?: string,
  app?: string,
  autoImportDefault?: boolean,
  /** 目标选择器已探明该主机离线：跳过连接请求，直接用离线状态（不再尝试建连） */
  knownOffline?: boolean,
) => {
  return useQuery({
    queryKey: ["remoteProviders", hostId, container || "__host__", app],
    // 与本机 useProvidersQuery 一致：refetch 时保留旧数据，避免切换供应商 / 刷新时
    // 闪「正在读取」骨架屏。**但只保留「同一目标」的旧数据**：切目标（host/container
    // 变化）时新 key 无数据，若照搬旧数据会把 A 目标器的列表显示在 B 目标器上，
    // 直到 B 的 SSH 请求回来才替换 —— 这正是「切目标后先显示旧目标内容、过一会才刷新」
    // 的根因。这里比对 previousQuery 的 key，目标不同就返回 undefined（走 loading）。
    placeholderData: (previousData, previousQuery) => {
      const prevKey = previousQuery?.queryKey;
      const sameTarget =
        Array.isArray(prevKey) &&
        prevKey[0] === "remoteProviders" &&
        prevKey[1] === hostId &&
        prevKey[2] === (container || "__host__") &&
        prevKey[3] === app;
      return sameTarget ? previousData : undefined;
    },
    queryFn: () =>
      getRemoteProviders(hostId!, app!, container, autoImportDefault ?? true),
    enabled: Boolean(hostId && app && !knownOffline),
  });
};

/**
 * 远程切换供应商 mutation —— 与本机 `useSwitchProviderMutation` 同构：
 * - onSuccess：回写当前供应商高亮 + invalidateQueries（providers 列表）+ 集中 toast
 * - onError：统一 toast（不再散落 try/catch）
 * - 后端 `EffectReport.currentProviderId` 直接带回当前供应商 id，前端省一次
 *   `get_remote_current_provider` IPC
 * - `app` 参数化：claude / codex 等，invalidate 对应 app 的 providers 缓存
 */
export const useSwitchRemoteProviderMutation = (
  onSwitched: (providerId: string | null) => void,
) => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: async ({
      hostId,
      providerId,
      app,
      container,
    }: SwitchRemoteProviderVars) => {
      return await switchRemoteProvider(hostId, providerId, app, container);
    },
    onSuccess: (report, vars) => {
      // 后端已持久化并带回当前供应商 id，直接更新高亮（不再多一次 IPC）
      onSwitched(report.currentProviderId ?? vars.providerId);
      void queryClient.invalidateQueries({
        queryKey: ["providers", vars.app],
      });
      // per-target 独立：切换会写 live（additive 的「添加」= 切换，把供应商写入 live
      // 并启用），因此要同时更新 currentProviderId 与 liveIds（按钮态「添加」→「移除」
      // 立即翻转）。**与本机 useSwitchProviderMutation 一致用 invalidateQueries 全量重取**
      // （本机 invalidate ["providers", appId]）。配合 useRemoteProvidersQuery 的
      // placeholderData: keepPreviousData，refetch 时保留旧列表 → 不闪「正在读取」。
      void queryClient.invalidateQueries({
        queryKey: ["remoteProviders", vars.hostId, vars.container || "__host__", vars.app],
      });
      // openclaw 切换供应商后，失效 defaultModel 查询缓存，使"设为默认"按钮状态更新
      // （openclaw 正常工作的原因是 App.tsx 用原始 remoteContainerId 额外失效了一次）
      if (vars.app === "openclaw") {
        void queryClient.invalidateQueries({
          queryKey: [
            "remoteOpenclawDefaultModel",
            vars.hostId,
            vars.container ?? "",
          ],
        });
      }
      // hermes 切换写远端 config.yaml 的 model.provider：失效远端 modelConfig 缓存，
      // 使「当前激活」高亮 / 按钮态立即刷新（否则点「启用」后高亮不更新，与本机不一致）。
      // 第三位归一化为 `vars.container || "__host__"`，与 ProviderList 查询 key 的
      // `remoteContainerId || "__host__"` 逐字一致（宿主机两边都是 "__host__"；
      // 裸 `?? ""` 与 ProviderList 的 `""` 不一致会失效落空）。
      if (vars.app === "hermes") {
        void queryClient.invalidateQueries({
          queryKey: [
            "remoteHermesModelConfig",
            vars.hostId,
            vars.container || "__host__",
          ],
        });
      }
      // 成功提示 = 「在远端 {{target}} 」+ 本机 useProviderActions.ts 对应 app 文案：
      // 远端场景强调落点（在远端 xxx），正文照抄本机，不额外改写。
      const target =
        report.target + (vars.container ? ` / ${vars.container}` : "");
      let switchMessage = t("remote.switchDone", {
        defaultValue: "在远端 {{target}} 切换成功！",
        target,
      });
      if (vars.app === "codex") {
        switchMessage = t("remote.switchDoneCodex", {
          defaultValue: "在远端 {{target}} 切换成功，请重启客户端以生效",
          target,
        });
      } else if (vars.app === "grokbuild") {
        switchMessage = t("remote.switchDoneGrok", {
          defaultValue: "在远端 {{target}} 切换成功，请重启 Grok Build 以生效",
          target,
        });
      } else if (vars.app === "opencode" || vars.app === "openclaw") {
        // 本机 opencode/openclaw 切换成功文案 = 「已添加到配置」
        switchMessage = t("remote.switchDoneAddToConfig", {
          defaultValue: "在远端 {{target}} 已添加到配置",
          target,
        });
      }
      toast.success(switchMessage, { closeButton: true });
      // 追加展示后端警告（如「远端接管已开启但隧道未建立，按直连写入」）
      if (report.warnings && report.warnings.length > 0) {
        report.warnings.forEach((warning) => {
          toast.warning(warning, { closeButton: true, duration: 8000 });
        });
      }
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(t("remote.switchError", { defaultValue: "远程切换失败" }), {
        description: detail,
        closeButton: true,
      });
    },
  });
};

/**
 * 在远端目标添加供应商 —— 与本机 `useAddProviderMutation` 同构：
 * 写该目标 SSOT（addToLive=true 时同时写 live），成功 toast + 直接回写视图缓存。
 * 后端返回最新视图，省一次 SSH 往返。
 */
export const useAddRemoteProviderMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: async ({
      hostId,
      app,
      provider,
      addToLive,
      container,
    }: RemoteProviderBaseVars & {
      provider: Provider;
      addToLive?: boolean;
    }) => {
      const view = await addRemoteProvider(
        hostId,
        app,
        provider,
        addToLive,
        container,
      );
      return { view, vars: { hostId, app, container } };
    },
    onSuccess: ({ view, vars }) => {
      setRemoteProvidersData(queryClient, vars, view);
      toast.success(
        t("notifications.providerAdded", { defaultValue: "供应商已添加" }),
        { closeButton: true },
      );
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(
        t("notifications.addFailed", {
          defaultValue: "添加供应商失败: {{error}}",
          error: detail,
        }),
      );
    },
  });
};

/**
 * 编辑远端目标的供应商 —— 更新 SSOT（在生效位置时后端重写远端 live）。
 * 成功 toast + 直接回写视图缓存。
 */
export const useUpdateRemoteProviderMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: async ({
      hostId,
      app,
      provider,
      originalId,
      container,
      editorSave,
    }: RemoteProviderBaseVars & {
      provider: Provider;
      originalId?: string;
      editorSave?: ProviderEditorSave;
    }) => {
      const view = await updateRemoteProvider(
        hostId,
        app,
        provider,
        originalId,
        container,
        editorSave,
      );
      return { view, vars: { hostId, app, container } };
    },
    onSuccess: ({ view, vars }) => {
      setRemoteProvidersData(queryClient, vars, view);
      toast.success(
        t("notifications.updateSuccess", { defaultValue: "供应商更新成功" }),
        { closeButton: true },
      );
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(
        t("notifications.updateFailed", {
          defaultValue: "更新供应商失败: {{error}}",
          error: detail,
        }),
      );
    },
  });
};

/**
 * 删除远端目标的供应商 —— SSOT 移除（additive 且已写 live 时后端同时移除 live）。
 * 成功 toast + 直接回写视图缓存。
 */
export const useDeleteRemoteProviderMutation = () => {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: async ({
      hostId,
      app,
      providerId,
      container,
    }: RemoteProviderBaseVars & { providerId: string }) => {
      const view = await deleteRemoteProvider(hostId, app, providerId, container);
      return { view, vars: { hostId, app, container } };
    },
    onSuccess: ({ view, vars }) => {
      setRemoteProvidersData(queryClient, vars, view);
      toast.success(
        t("notifications.deleteSuccess", { defaultValue: "供应商已删除" }),
        { closeButton: true },
      );
    },
    onError: (error: Error) => {
      const detail = extractErrorMessage(error) || t("common.unknown");
      toast.error(
        t("notifications.deleteFailed", {
          defaultValue: "删除供应商失败: {{error}}",
          error: detail,
        }),
      );
    },
  });
};

/**
 * 更新远端目标供应商的排序索引（拖拽 / 复制时重排）。
 * 后端返回 boolean，成功后 invalidate 视图缓存重新拉取。
 */
export const useUpdateRemoteProviderSortOrder = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      hostId,
      app,
      updates,
      container,
    }: RemoteProviderBaseVars & { updates: ProviderSortUpdate[] }) => {
      await updateRemoteProviderSortOrder(hostId, app, updates, container);
      return { hostId, app, container };
    },
    onSuccess: ({ hostId, app, container }) => {
      void queryClient.invalidateQueries({
        queryKey: remoteProvidersKey(hostId, container, app),
      });
    },
  });
};
