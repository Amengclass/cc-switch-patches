/**
 * 远端目标（本机 / 服务器 / 容器）的状态与副作用 —— **P2 收敛点**。
 *
 * 为什么抽出来：`App.tsx` 是官方文件、且我们的改动量大（App.tsx 的补丁 ~1500 行）。
 * 把「远端状态 + 拉取 + 探活 + 持久化」整块搬到这个 overlay 文件后，
 * 官方 App.tsx 里只留一次 hook 调用（App.tsx 依然会长，但至少少 300 行）。
 *
 * 调用点：`App.tsx` 的 `App()` 组件。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { AppId } from "@/lib/api";
import {
  checkLocalCliInstalled,
  checkRemoteCliInstalled,
  getRemoteCurrentProvider,
  listDockerContainers,
  listRemoteHosts,
  probeHostsOnline,
} from "@/lib/api/remote";
import type { RemoteHost } from "@/types/remote";

export interface UseRemoteTargetOptions {
  activeApp: AppId;
  sharedFeatureApp: AppId;
  /** 当前主视图（仅用于比较是否停留在 "remote"） */
  currentView: string;
  setActiveApp: (app: AppId) => void;
  queryClient: QueryClient;
  /** 关闭「远端功能」总开关时的回调（由 App 还原主视图） */
  onRemoteFeatureDisabled?: () => void;
}

export interface RemoteTargetBundle {
  batchApplyOpen: boolean;
  setBatchApplyOpen: (open: boolean) => void;
  batchApplyApp: string;
  setBatchApplyApp: (app: string) => void;
  servers: RemoteHost[];
  setServers: React.Dispatch<React.SetStateAction<RemoteHost[]>>;
  remoteTargetId: string;
  setRemoteTargetId: React.Dispatch<React.SetStateAction<string>>;
  remoteCurrentProviderId: string | null;
  setRemoteCurrentProviderId: React.Dispatch<React.SetStateAction<string | null>>;
  remoteInstalled: boolean | null;
  setRemoteInstalled: React.Dispatch<React.SetStateAction<boolean | null>>;
  localInstalled: boolean | null;
  setLocalInstalled: React.Dispatch<React.SetStateAction<boolean | null>>;
  containers: string[];
  setContainers: React.Dispatch<React.SetStateAction<string[]>>;
  containersLoading: boolean;
  remoteContainerId: string;
  setRemoteContainerId: React.Dispatch<React.SetStateAction<string>>;
  hostsOnline: Record<string, boolean>;
  setHostsOnline: React.Dispatch<React.SetStateAction<Record<string, boolean>>>;
  targetKnownOffline: boolean;
  retryRemoteTarget: () => void;
  probeHosts: () => Promise<void>;
  autoImportDefault: boolean;
  handleAutoImportDefaultChange: (next: boolean) => void;
  remoteFeatureEnabled: boolean;
  handleRemoteFeatureEnabledChange: (next: boolean) => void;
  remoteAvailableForApp: boolean;
  activeRemoteHost: RemoteHost | null;
  currentInstalled: boolean | null;
  refreshInstallStatus: () => void;
}

export function useRemoteTarget({
  activeApp,
  sharedFeatureApp,
  currentView,
  setActiveApp,
  queryClient,
  onRemoteFeatureDisabled,
}: UseRemoteTargetOptions): RemoteTargetBundle {
  // ===== 批量应用 Provider 面板（入口A：目标选择器栏；入口B：远程主机管理页）=====
  const [batchApplyOpen, setBatchApplyOpen] = useState(false);
  const [batchApplyApp, setBatchApplyApp] = useState<string>(sharedFeatureApp);

  // ===== 目标选择器（本机 / 远程服务器）=====
  const [servers, setServers] = useState<RemoteHost[]>([]);
  const [remoteTargetId, setRemoteTargetId] = useState<string>(
    () => localStorage.getItem("cc-switch-remote-target") ?? "",
  );
  const [remoteCurrentProviderId, setRemoteCurrentProviderId] = useState<
    string | null
  >(null);
  const [remoteInstalled, setRemoteInstalled] = useState<boolean | null>(null);
  const [localInstalled, setLocalInstalled] = useState<boolean | null>(null);
  // 目标细化到 Docker 容器：选中服务器后可再选容器，所有远程操作作用于容器内。
  // 按主机记忆容器选择：切回远端时恢复上次选的容器。
  const [containers, setContainers] = useState<string[]>([]);
  const [containersLoading, setContainersLoading] = useState(false);
  const [remoteContainerId, setRemoteContainerId] = useState<string>(() => {
    const target = localStorage.getItem("cc-switch-remote-target") ?? "";
    if (!target) return "";
    return localStorage.getItem(`cc-switch-remote-container:${target}`) ?? "";
  });
  // 主机在线状态（host_id → 是否在线）：目标选择器下拉打开时批量实时探测。
  // 不缓存：状态必须真实反映"此刻"（缓存会让用户看到假在线却连不进去）。
  const [hostsOnline, setHostsOnline] = useState<Record<string, boolean>>({});
  // 当前目标是否被探明离线（软信号：探活结果；重试可清除重新探测）
  const targetKnownOffline = remoteTargetId
    ? hostsOnline[remoteTargetId] === false
    : false;
  // 重试：清除当前目标的离线标记 → 查询重新启用 → 真正重新探测/连接
  const retryRemoteTarget = useCallback(() => {
    if (!remoteTargetId) return;
    setHostsOnline((prev) => {
      if (prev[remoteTargetId] !== false) return prev;
      const next = { ...prev };
      delete next[remoteTargetId];
      return next;
    });
  }, [remoteTargetId]);
  const probeHosts = useCallback(async () => {
    if (servers.length === 0) return;
    // 每次打开下拉都重新实时检测：先清空旧状态（全部转圈）
    setHostsOnline({});
    // 每台单独探测、就绪即更新：在线机器先返回先变绿，
    // 离线机器最后（5 秒超时）才变灰——不互相拖累（后端批量接口是等最慢的才一起返回）
    await Promise.allSettled(
      servers.map(async (s) => {
        try {
          const ok = await probeHostsOnline([s.id]);
          setHostsOnline((prev) => ({ ...prev, ...ok }));
        } catch {
          // 单台探测失败不影响其他台
        }
      }),
    );
  }, [servers]);
  // 设置开关：远端非 additive 面板是否每次自动读入当前 live 配置（default 卡）
  const [autoImportDefault, setAutoImportDefault] = useState<boolean>(
    () => localStorage.getItem("cc-switch-remote-auto-import-default") !== "0",
  );

  // ===== 远端功能总开关：关 = 还原原生 cc-switch（隐藏所有远端入口）=====
  // 默认开启（不存在该 key 时视为开），仅在设置页主动关闭才隐藏远端。
  const [remoteFeatureEnabled, setRemoteFeatureEnabled] = useState<boolean>(
    () => localStorage.getItem("cc-switch-remote-feature-enabled") !== "0",
  );

  // claude-desktop 没有远端概念（远端只有 claude/codex/gemini/grokbuild/openclaw/
  // openclaw/hermes），其配置本就回并到 claude：在 claude-desktop 标签下完全隐藏
  // 远端入口（目标选择器远端项、批量应用、远程主机导航、远端状态栏），还原本机体验。
  const remoteAvailableForApp =
    remoteFeatureEnabled && activeApp !== "claude-desktop";

  const handleRemoteFeatureEnabledChange = useCallback(
    (next: boolean) => {
      setRemoteFeatureEnabled(next);
      localStorage.setItem(
        "cc-switch-remote-feature-enabled",
        next ? "1" : "0",
      );
      // 关闭时若正停留在远端视图/选中远端目标，立刻还原成本机视图
      if (!next) {
        onRemoteFeatureDisabled?.();
        setRemoteTargetId("");
        setRemoteContainerId("");
        setContainers([]);
        setHostsOnline({});
      }
    },
    [onRemoteFeatureDisabled],
  );

  // 开关切换后使远端面板查询失效（refetch 用新值），并持久化
  const handleAutoImportDefaultChange = useCallback(
    (next: boolean) => {
      setAutoImportDefault(next);
      localStorage.setItem(
        "cc-switch-remote-auto-import-default",
        next ? "1" : "0",
      );
      queryClient.invalidateQueries({ queryKey: ["remoteProviders"] });
    },
    [queryClient],
  );

  useEffect(() => {
    localStorage.setItem("cc-switch-remote-target", remoteTargetId);
  }, [remoteTargetId]);

  // 按主机保存容器选择：切走时保存当前容器（含切到本机的场景）
  const prevTargetRef = useRef(remoteTargetId);
  const prevContainerRef = useRef(remoteContainerId);
  useEffect(() => {
    const prev = prevTargetRef.current;
    const prevContainer = prevContainerRef.current;
    // 切走了 → 用 ref 保存上一次的容器（不依赖当前已清空的 remoteContainerId）
    if (prev && prev !== remoteTargetId) {
      localStorage.setItem(
        `cc-switch-remote-container:${prev}`,
        prevContainer,
      );
    }
    // 保存当前主机的容器选择（选中主机时）
    if (remoteTargetId) {
      localStorage.setItem(
        `cc-switch-remote-container:${remoteTargetId}`,
        remoteContainerId,
      );
    }
    prevTargetRef.current = remoteTargetId;
    prevContainerRef.current = remoteContainerId;
  }, [remoteTargetId, remoteContainerId]);

  // 远端目标下没有 claude-desktop 应用：切到远端时若当前 tab 是
  // claude-desktop，自动切回 claude（sharedFeatureApp 本就映射 claude）。
  useEffect(() => {
    if (remoteTargetId && activeApp === "claude-desktop") {
      setActiveApp("claude");
    }
  }, [remoteTargetId, activeApp]);

  // 每次切换视图时刷新服务器列表（远程页面增删后回到主界面能同步）；
  // 若当前选中的目标已被删除，自动重置回「本机」。
  useEffect(() => {
    checkLocalCliInstalled(sharedFeatureApp)
      .then(setLocalInstalled)
      .catch(() => setLocalInstalled(null));
  }, [sharedFeatureApp]);

  useEffect(() => {
    // 远端功能关闭：不加载远端主机列表（还原原生，不与任何主机建连）
    if (!remoteFeatureEnabled) {
      setServers([]);
      setRemoteTargetId("");
      setRemoteContainerId("");
      setContainers([]);
      setHostsOnline({});
      return;
    }
    listRemoteHosts()
      .then((list) => {
        // 禁用的主机从目标选择/操作中排除（不显示、不可选、探活跳过）；
        // 恢复需到远程主机管理页「启用」。
        const enabled = list.filter((s) => !s.disabled);
        setServers(enabled);
        setRemoteTargetId((prev) =>
          prev && !enabled.some((s) => s.id === prev) ? "" : prev,
        );
      })
      .catch(() => {});
    // 注：切回主界面时刷新远端当前供应商 + 安装状态的工作，由下方依赖
    // `[remoteTargetId, remoteContainerId]` 的 effect 统一负责（它拿到最新容器）。
    // 这里不重复刷新，避免两个 effect 用不同 container 并行覆盖 setRemoteInstalled。
  }, [currentView, remoteFeatureEnabled]);

  // 选中服务器时：读取远端当前生效的供应商（按 base_url 匹配本地供应商）
  // 并检测远端 Claude Code 安装状态（用于主面板横幅徽标）。
  // remote 视图（远程主机管理面板）不针对当前目标，跳过探测。
  useEffect(() => {
    if (currentView === "remote" || !remoteTargetId) {
      setRemoteCurrentProviderId(null);
      setRemoteInstalled(null);
      setContainers([]);
      // 不清 remoteContainerId：用户可能切回同一台服务器需要恢复之前的容器选择
      return;
    }
    // 目标选择器已探明该主机离线：跳过连接型调用（不发起建连），直接置空
    if (targetKnownOffline) {
      setRemoteCurrentProviderId(null);
      setRemoteInstalled(null);
      setContainers([]);
      return;
    }
    let active = true;
    const container = remoteContainerId || undefined;
    getRemoteCurrentProvider(remoteTargetId, sharedFeatureApp, container)
      .then((id) => {
        if (active) setRemoteCurrentProviderId(id);
      })
      .catch(() => {
        if (active) setRemoteCurrentProviderId(null);
      });
    checkRemoteCliInstalled(remoteTargetId, sharedFeatureApp, container)
      .then((s) => {
        if (active) setRemoteInstalled(s);
      })
      .catch(() => {
        if (active) setRemoteInstalled(null);
      });
    return () => {
      active = false;
    };
  }, [
    remoteTargetId,
    remoteContainerId,
    currentView,
    sharedFeatureApp,
    targetKnownOffline,
  ]);

  // 容器列表只与主机相关（与 app/容器无关）：独立 effect，仅换主机时重拉，
  // 切 app / 切容器不白跑（D 修复）。
  useEffect(() => {
    if (!remoteTargetId || targetKnownOffline) {
      setContainers([]);
      // 不清 remoteContainerId：切回远端服务器时需要恢复之前的容器选择
      // localStorage 已保存，重新拉取容器列表后由 setRemoteContainerId(prev => ...)
      // 校验是否在列表中，不在才清空
      return;
    }
    // 切主机后先清空旧容器列表再拉取：effect 在渲染之后才跑，若不清空，
    // 拉取期间的渲染窗口会显示上一台主机的容器（下拉串台）。
    setContainers([]);
    setContainersLoading(true);
    let active = true;
    listDockerContainers(remoteTargetId)
      .then((list) => {
        if (active) {
          setContainers(list);
          setContainersLoading(false);
          // 容器列表加载完毕后，若已选容器不在当前主机列表中才清空
          // （不在加载前清空，否则 localStorage 记忆的容器会在拉取期间被误删）
          setRemoteContainerId((prev) =>
            prev && !list.includes(prev) ? "" : prev,
          );
        }
      })
      .catch(() => {
        if (active) {
          setContainers([]);
          setContainersLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [remoteTargetId, targetKnownOffline]);

  const activeRemoteHost = servers.find((s) => s.id === remoteTargetId) ?? null;
  // 当前目标（本机/服务器）的 Claude Code 安装状态
  const currentInstalled = remoteTargetId ? remoteInstalled : localInstalled;

  // 刷新当前 app 的 CLI 安装状态：本机与当前远端目标共用同一策略。
  // 依赖必须含 remoteContainerId：否则切容器后 useCallback 缓存旧闭包，
  // 点刷新会用上一次的 container 检测，导致宿主机/容器状态串扰。
  const refreshInstallStatus = useCallback(() => {
    checkLocalCliInstalled(sharedFeatureApp)
      .then(setLocalInstalled)
      .catch(() => setLocalInstalled(null));
    if (remoteTargetId) {
      const container = remoteContainerId || undefined;
      checkRemoteCliInstalled(remoteTargetId, sharedFeatureApp, container)
        .then(setRemoteInstalled)
        .catch(() => setRemoteInstalled(null));
      getRemoteCurrentProvider(remoteTargetId, sharedFeatureApp, container)
        .then(setRemoteCurrentProviderId)
        .catch(() => setRemoteCurrentProviderId(null));
      listDockerContainers(remoteTargetId)
        .then(setContainers)
        .catch(() => setContainers([]));
    }
  }, [remoteTargetId, remoteContainerId, sharedFeatureApp]);

  // 窗口重新聚焦时自动刷新（如装完 Claude Code 切回应用即更新）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await getCurrentWindow().onFocusChanged(
          ({ payload: focused }) => {
            if (focused) refreshInstallStatus();
          },
        );
      } catch (e) {
        console.error("[App] Failed to listen window focus", e);
      }
    })();
    return () => {
      unlisten?.();
    };
  }, [refreshInstallStatus]);
  return {
    batchApplyOpen,
    setBatchApplyOpen,
    batchApplyApp,
    setBatchApplyApp,
    servers,
    setServers,
    remoteTargetId,
    setRemoteTargetId,
    remoteCurrentProviderId,
    setRemoteCurrentProviderId,
    remoteInstalled,
    setRemoteInstalled,
    localInstalled,
    setLocalInstalled,
    containers,
    setContainers,
    containersLoading,
    remoteContainerId,
    setRemoteContainerId,
    hostsOnline,
    setHostsOnline,
    targetKnownOffline,
    retryRemoteTarget,
    probeHosts,
    autoImportDefault,
    handleAutoImportDefaultChange,
    remoteFeatureEnabled,
    handleRemoteFeatureEnabledChange,
    remoteAvailableForApp,
    activeRemoteHost,
    currentInstalled,
    refreshInstallStatus,
  };
}