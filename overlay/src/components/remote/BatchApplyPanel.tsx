import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  Loader2,
  Send,
  Server,
  XCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ManagementListSearch } from "@/components/common/ManagementListSearch";
import { ProviderIcon } from "@/components/ProviderIcon";
import { DockerIcon } from "@/components/icons/DockerIcon";
import { cn } from "@/lib/utils";
import type { RemoteHost } from "@/types/remote";
import type { Provider } from "@/types";
import {
  broadcastSwitchProvider,
  listDockerContainers,
  type BroadcastSwitchResult,
} from "@/lib/api/remote";
import { useRemoteProvidersQuery } from "@/lib/query/remoteMutations";
import { listen } from "@tauri-apps/api/event";
// 官方 v4.0.4 的 popover 只导出 Popover / PopoverTrigger / PopoverContent /
// PopoverAnchor 四个（没有 PopoverArrow），箭头是纯装饰，去掉。
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface Target {
  hostId: string;
  hostName: string;
  container?: string;
}

interface BatchApplyPanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 可选的 app 列表；为空时固定使用当前 app */
  availableApps?: string[];
  app: string;
  /** app 切换回调：父组件据此重新获取对应 app 的 providers */
  onAppChange?: (app: string) => void;
  hosts: RemoteHost[];
  providers: Record<string, Provider>;
  defaultProviderId?: string | null;
}

interface ResultRow {
  key: string;
  label: string;
  /** 容器名；宿主机落点为 null/undefined（用于渲染「宿主机」标识） */
  container?: string | null;
  status: "pending" | "running" | "ok" | "fail";
  message?: string;
}

const contKey = (id: string, c: string) => `${id}::${c}`;
const splitKey = (key: string) => key.split("::");

/** 根据容器列表拉取的错误信息，判定失败层。
 *  后端 list_docker_containers 已结构化加前缀：SSH 层失败 = `SSH_ERR:<detail>`，
 *  docker 命令失败 = `DOCKER_ERR:<detail>`。前端只认前缀，不依赖具体文案。 */
const classifyHostError = (err: unknown): "ssh" | "docker" => {
  const msg = String(err);
  if (msg.startsWith("SSH_ERR:")) return "ssh";
  if (msg.startsWith("DOCKER_ERR:")) return "docker";
  // 老后端（未加前缀）兜底：仅凭文案含连接类关键词判为 ssh
  if (
    /连接.*失败|连接.*超时|SSH 连接失败|认证失败|最近一次连接未成功/.test(msg)
  ) {
    return "ssh";
  }
  return "docker";
};

/** 批量应用 Provider 面板：搜索 + 整台勾选（含容器）+ 顶部已选 chips + 逐落点切换 */
export function BatchApplyPanel({
  open,
  onOpenChange,
  availableApps,
  app: initialApp,
  onAppChange,
  hosts,
  providers,
  defaultProviderId,
}: BatchApplyPanelProps) {
  const { t } = useTranslation();
  const [selectedApp, setSelectedApp] = useState(initialApp);
  const app = selectedApp;
  // 当前应用标识：sharedFeatureApp 已把 claude-desktop 映射为 claude，这里再兜底一次。
  // 图标名复用 AppSwitcher 的 APP_ICON_NAME 映射（claude→claude / codex→openai /
  // gemini→gemini / grokbuild→grok / opencode→opencode / openclaw→openclaw /
  // hermes→hermes），经 ProviderIcon 渲染品牌图标；文本名用 t(`apps.<app>`) 多语言。
  const APP_ICON_NAME: Record<string, string> = {
    claude: "claude",
    "claude-desktop": "claude",
    codex: "openai",
    gemini: "gemini",
    grokbuild: "grok",
    opencode: "opencode",
    openclaw: "openclaw",
    hermes: "hermes",
  };
  const appDisplayName = t(`apps.${app}`, { defaultValue: app });
  const appIconName = APP_ICON_NAME[app];

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [containersMap, setContainersMap] = useState<
    Record<string, string[] | null>
  >({});
  // 每台主机的连接异常类型："ssh" = SSH 层连接/认证/超时失败（宿主机本体也不可用）；
  // "docker" = SSH 通但容器列表拉取失败（宿主机本体仍可应用，只是候选容器未知）。
  const [hostErrors, setHostErrors] = useState<
    Record<string, "ssh" | "docker" | undefined>
  >({});
  const [search, setSearch] = useState<string>("");
  const [providerId, setProviderId] = useState<string>("");
  const [results, setResults] = useState<ResultRow[]>([]);
  const [running, setRunning] = useState(false);
  // Provider 池来源：本机（默认）/ 某台宿主机 / 某台宿主机的容器。
  const [sourceKey, setSourceKey] = useState<string>("local");

  const source = useMemo(() => {
    if (sourceKey === "local") return null;
    const [hostId, container] = splitKey(sourceKey);
    return { hostId, container: container || undefined };
  }, [sourceKey]);
  const remoteSourceQuery = useRemoteProvidersQuery(
    source?.hostId,
    source?.container,
    app,
  );

  // 有效供应商池：本机 DB 或所选来源的 SSOT
  const effectiveProviders = source
    ? (remoteSourceQuery.data?.providers ?? {})
    : providers;
  const effectiveDefaultProviderId = source
    ? (remoteSourceQuery.data?.currentProviderId ?? undefined)
    : defaultProviderId;

  const providerList = useMemo(
    () => Object.values(effectiveProviders),
    [effectiveProviders],
  );

  useEffect(() => {
    if (open) {
      setProviderId(defaultProviderId ?? providerList[0]?.id ?? "");
      setSourceKey("local");
      setSelected(new Set());
      setExpanded(new Set());
      setContainersMap({});
      setSearch("");
      setResults([]);
      setRunning(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // 面板内切 app / 切 Provider 池来源后 providers 变了：原选中的供应商不在新列表里 →
  // 回退到该来源的默认 / 首个（否则新的 Select 会显示占位、批量应用按钮置灰）。
  useEffect(() => {
    if (!open) return;
    if (providerId && providerList.some((p) => p.id === providerId)) return;
    setProviderId(effectiveDefaultProviderId ?? providerList[0]?.id ?? "");
  }, [open, providerId, providerList, effectiveDefaultProviderId]);

  // 打开面板即批量拉取所有主机的容器列表：每台行尾常驻显示「N 容器」，无需逐个展开。
  // 拉取失败记录到 hostErrors（区分 SSH 挂 / docker 挂），供展开区与主行提示。
  useEffect(() => {
    if (!open || hosts.length === 0) return;
    let alive = true;
    setContainersMap({});
    setHostErrors({});
    hosts.forEach((host) => {
      listDockerContainers(host.id)
        .then((list) => {
          if (alive) setContainersMap((prev) => ({ ...prev, [host.id]: list }));
        })
        .catch((e) => {
          if (alive) {
            setContainersMap((prev) => ({ ...prev, [host.id]: [] }));
            setHostErrors((prev) => ({
              ...prev,
              [host.id]: classifyHostError(e),
            }));
          }
        });
    });
    return () => {
      alive = false;
    };
  }, [open, hosts]);

  const toggleExpand = async (host: RemoteHost) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(host.id)) next.delete(host.id);
      else next.add(host.id);
      return next;
    });
    if (!(host.id in containersMap)) {
      setContainersMap((prev) => ({ ...prev, [host.id]: null }));
      try {
        const list = await listDockerContainers(host.id);
        setContainersMap((prev) => ({ ...prev, [host.id]: list }));
        setHostErrors((prev) => ({ ...prev, [host.id]: undefined }));
      } catch (e) {
        setContainersMap((prev) => ({ ...prev, [host.id]: [] }));
        setHostErrors((prev) => ({
          ...prev,
          [host.id]: classifyHostError(e),
        }));
      }
    }
  };

  const filteredHosts = useMemo(() => {
    const kw = search.trim().toLowerCase();
    if (!kw) return hosts;
    return hosts.filter(
      (h) =>
        h.name.toLowerCase().includes(kw) ||
        h.host.toLowerCase().includes(kw) ||
        (containersMap[h.id] ?? []).some((c) =>
          c.toLowerCase().includes(kw),
        ),
    );
  }, [hosts, search, containersMap]);

  const toggleContainer = (hostId: string, container: string) => {
    const key = contKey(hostId, container);
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleHost = (host: RemoteHost) => {
    const cs = containersMap[host.id] ?? [];
    setSelected((prev) => {
      const next = new Set(prev);
      const wasFull = next.has(host.id);
      if (wasFull) {
        next.delete(host.id);
        cs.forEach((c) => next.delete(contKey(host.id, c)));
      } else {
        next.add(host.id);
        cs.forEach((c) => next.add(contKey(host.id, c)));
      }
      return next;
    });
  };

  const isHostFullyChecked = (host: RemoteHost) => {
    const cs = containersMap[host.id] ?? [];
    if (!selected.has(host.id)) return false;
    return (
      cs.length === 0 || cs.every((c) => selected.has(contKey(host.id, c)))
    );
  };

  // 展开区「宿主机账号」行：只切换 host.id（不连容器）
  const toggleHostOnly = (hostId: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(hostId)) next.delete(hostId);
      else next.add(hostId);
      return next;
    });
  };

  // 清空所有选中的落点
  const clearSelected = () => setSelected(new Set());

  const selectedTargets: Target[] = useMemo(() => {
    const list: Target[] = [];
    for (const key of selected) {
      const [hostId, container] = splitKey(key);
      const host = hosts.find((h) => h.id === hostId);
      if (!host) continue;
      list.push({ hostId, hostName: host.name, container });
    }
    return list;
  }, [selected, hosts]);

  const doApply = async () => {
    if (selectedTargets.length === 0 || !providerId) return;
    setRunning(true);
    const makeRows = (): ResultRow[] =>
      selectedTargets.map((t) => ({
        key: t.container ? `${t.hostId}::${t.container}` : t.hostId,
        label: t.container ? `${t.hostName} / ${t.container}` : t.hostName,
        container: t.container,
        status: "pending",
      }));
    setResults(makeRows());

    const fromResult = (res: BroadcastSwitchResult): ResultRow => ({
      key: res.container ? `${res.hostId}::${res.container}` : res.hostId,
      label: res.label,
      container: res.container,
      status: res.ok ? "ok" : "fail",
      message: res.ok
        ? res.providerName
        : (res.error ?? t("batchApply.failed")),
    });

    // 监听后端逐台进度事件：每切完一台立即更新对应落点，实时反馈
    const un = await listen<BroadcastSwitchResult>(
      "broadcast-progress",
      (ev) => {
        const row = fromResult(ev.payload);
        setResults((prev) => prev.map((r) => (r.key === row.key ? row : r)));
      },
    );

    // 一次调用后端广播命令（后端逐落点建连切换并推进度，失败不阻断其它）
    try {
      const results = await broadcastSwitchProvider(
        selectedTargets.map((t) => ({
          hostId: t.hostId,
          container: t.container ?? null,
        })),
        providerId,
        app,
        source?.hostId,
        source?.container,
      );
      // 权威结果兜底：以 invoke 返回为准（即使事件漏了也能对齐最终态）
      setResults(results.map(fromResult));
    } catch (e) {
      // 整个广播调用崩了（如参数错误）：当前未完成的全标失败
      setResults((prev) =>
        prev.map((r) =>
          r.status === "pending"
            ? { ...r, status: "fail", message: String(e) }
            : r,
        ),
      );
    }
    un();
    setRunning(false);
  };

  return (
    <FullScreenPanel
      isOpen={open}
      // 大面积全屏页淡入会让底层内容透出来（像「页面抖一下」），这里直接切换
      motionPreset="none"
      title={t("batchApply.title")}
      onClose={() => onOpenChange(false)}
      contentClassName="px-6 py-6 w-full flex flex-col gap-4"
      footer={
        <div className="flex w-full flex-wrap items-center gap-x-4 gap-y-2 relative">
          {/* 底部固定条·左侧：Provider 选择（始终可见，不用滚动即可选） */}
          <div className="flex min-w-0 flex-1 items-center gap-3">
            {/* App 选择器：可选时显示下拉，否则显示固定标识 */}
            {/* App 选择器 */}
            {availableApps && availableApps.length > 1 ? (
              <Popover>
                <PopoverTrigger asChild>
                  <button
                    type="button"
                    className="inline-flex shrink-0 items-center gap-1.5 rounded-control border border-border bg-subtle px-2 py-1 text-body font-medium text-fg-1 transition-colors hover:bg-selected"
                  >
                    {appIconName ? (
                      <ProviderIcon
                        icon={appIconName}
                        name={appDisplayName}
                        size={16}
                      />
                    ) : null}
                    <span className="whitespace-nowrap">{appDisplayName}</span>
                  </button>
                </PopoverTrigger>
                <PopoverContent align="start" className="w-auto min-w-0 p-1.5" sideOffset={6}>
                  <div className="flex flex-wrap gap-1">
                    {availableApps.map((a) => (
                      <button
                        key={a}
                        type="button"
                        onClick={() => {
                          setSelectedApp(a);
                          setSelected(new Set());
                          setExpanded(new Set());
                          onAppChange?.(a);
                        }}
                        className={cn(
                          "rounded-control px-2 py-1.5 text-left text-body transition-colors whitespace-nowrap",
                          a === app
                            ? "bg-selected text-fg-1 font-medium"
                            : "text-fg-1 hover:bg-subtle",
                        )}
                      >
                        {t(`apps.${a}`, { defaultValue: a })}
                      </button>
                    ))}
                  </div>
                </PopoverContent>
              </Popover>
            ) : (
              <span className="inline-flex shrink-0 items-center gap-1.5 rounded-control border border-border bg-subtle px-2 py-1 text-body font-medium text-fg-1">
                {appIconName ? (
                  <ProviderIcon
                    icon={appIconName}
                    name={appDisplayName}
                    size={16}
                  />
                ) : null}
                <span className="whitespace-nowrap">{appDisplayName}</span>
              </span>
            )}
            <span className="whitespace-nowrap text-body text-fg-2">
              {t("batchApply.source")}
            </span>
            <Select value={sourceKey} onValueChange={setSourceKey}>
              <SelectTrigger className="h-9 max-w-[200px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">
                  {t("batchApply.sourceLocal")}
                </SelectItem>
                {hosts.flatMap((h) => [
                  <SelectItem key={h.id} value={h.id}>
                    {h.name}
                  </SelectItem>,
                  ...(containersMap[h.id] ?? []).map((c) => (
                    <SelectItem key={contKey(h.id, c)} value={contKey(h.id, c)}>
                      {h.name} / {c}
                    </SelectItem>
                  )),
                ])}
              </SelectContent>
            </Select>
            <span className="whitespace-nowrap text-body text-fg-2">
              {t("batchApply.forApp")}
            </span>
            <Select
              value={providerId || undefined}
              onValueChange={setProviderId}
              disabled={providerList.length === 0}
            >
              <SelectTrigger className="h-9 max-w-[220px]">
                <SelectValue
                  placeholder={t("batchApply.noProvider")}
                />
              </SelectTrigger>
              <SelectContent>
                {providerList.map((p) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {/* 底部固定条·右侧：取消 / 批量应用 */}
          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="outline"
              onClick={() => onOpenChange(false)}
              disabled={running}
            >
              {t("batchApply.cancel")}
            </Button>
            <Button
              disabled={
                running ||
                selectedTargets.length === 0 ||
                !providerId ||
                providerList.length === 0
              }
              onClick={() => void doApply()}
            >
              {running ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Send className="mr-2 h-4 w-4" />
              )}
              {t("batchApply.title")} ({selectedTargets.length})
            </Button>
          </div>
        </div>
      }
    >
      {/* ① 选落点（占满剩余空间，内部列表滚动，底部固定条始终可见） */}
      <div className="flex min-h-0 flex-1 flex-col rounded-panel border border-border bg-surface p-5 space-y-5">
        <div className="flex items-center justify-between gap-2">
          <h4 className="text-caption font-semibold uppercase tracking-wider text-fg-3">
            {t("batchApply.selectTargetsHint")}
          </h4>
          {/* 已选浮层：点开悬浮列表查看所选落点，不占布局空间 */}
          {selectedTargets.length > 0 && (
            <Popover>
              <PopoverTrigger asChild>
                <button
                  type="button"
                  className="inline-flex shrink-0 items-center gap-1 text-caption font-medium text-fg-2 hover:text-fg-1"
                >
                  {t("batchApply.selected")}{" "}
                  <span className="font-semibold text-action-text">
                    {selectedTargets.length}
                  </span>
                  <ChevronRight className="h-3.5 w-3.5 text-fg-3" />
                </button>
              </PopoverTrigger>
              <PopoverContent align="end" className="w-80 p-0">
                <div className="flex items-center justify-between border-b border-border px-3 py-2">
                  <span className="text-caption font-semibold text-fg-1">
                    {t("batchApply.selectedCount", {
                      count: selectedTargets.length,
                    })}
                  </span>
                  <button
                    type="button"
                    onClick={clearSelected}
                    className="text-caption text-fg-3 hover:text-fg-1"
                  >
                    {t("batchApply.clear")}
                  </button>
                </div>
                <div className="max-h-[40vh] overflow-y-auto p-2">
                  <div className="space-y-0.5">
                    {selectedTargets.map((target) => {
                      const isContainer = !!target.container;
                      return (
                        <div
                          key={
                            target.container
                              ? contKey(target.hostId, target.container)
                              : target.hostId
                          }
                          className="flex items-center gap-2 rounded-control border border-border px-2 py-1.5 text-body"
                        >
                          {isContainer ? (
                            <DockerIcon className="h-4 w-4 text-fg-3" />
                          ) : (
                            <Server className="h-4 w-4 shrink-0 text-action-text" />
                          )}
                          <span className="min-w-0 flex-1 truncate text-fg-1">
                            {target.container
                              ? target.container
                              : target.hostName}
                          </span>
                          <span
                            className={cn(
                              "shrink-0 rounded-control px-1.5 py-0.5 text-badge font-medium",
                              isContainer
                                ? "bg-subtle text-fg-2"
                                : "bg-action-soft text-action-text",
                            )}
                          >
                            {isContainer
                              ? t("batchApply.container")
                              : t("batchApply.host")}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                </div>
              </PopoverContent>
            </Popover>
          )}
        </div>

        <div className="space-y-2">
          <ManagementListSearch
            value={search}
            onValueChange={setSearch}
            placeholder={t("batchApply.searchPlaceholder", { defaultValue: "搜索主机名称 / 地址 / 容器名…" })}
            ariaLabel={t("batchApply.searchPlaceholder", { defaultValue: "搜索主机名称 / 地址 / 容器名…" })}
            clearLabel={t("common.clear", { defaultValue: "清除" })}
          />
        </div>

        <div className="min-h-0 flex-1 space-y-1 overflow-y-auto pr-1 [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar]:block [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-border">
          {filteredHosts.length === 0 && (
            <div className="py-8 text-center text-body text-fg-3">
              {hosts.length === 0
                ? t("batchApply.emptyHosts")
                : t("batchApply.noMatch")}
            </div>
          )}
          {filteredHosts.map((host) => {
            const cs = containersMap[host.id];
            const fullyChecked = isHostFullyChecked(host);
            // SSH 层失败：宿主机本体与容器都不可用，禁止勾选并提示
            const sshDown = hostErrors[host.id] === "ssh";
            // 加载中（容器列表检测中，未知是否可用）：禁止勾选，鼠标禁用
            const hostLoading = cs == null;
            return (
              <div
                key={host.id}
                className={cn(
                  "rounded-control border transition-colors",
                  fullyChecked
                    ? "border-border-strong bg-subtle"
                    : "border-border",
                )}
              >
                <div className="flex items-center gap-2 px-2 py-2">
                  <Checkbox
                    checked={fullyChecked}
                    disabled={sshDown || hostLoading}
                    className={cn(
                      (sshDown || hostLoading) && "cursor-not-allowed",
                    )}
                    onCheckedChange={() => toggleHost(host)}
                  />
                  <Server
                    className={cn(
                      "h-4 w-4 shrink-0",
                      sshDown ? "text-danger" : "text-fg-3",
                    )}
                  />
                  <button
                    type="button"
                    disabled={hostLoading}
                    onClick={() => void toggleExpand(host)}
                    className={cn(
                      "flex min-w-0 flex-1 items-center gap-1 text-left",
                      hostLoading && "cursor-not-allowed",
                    )}
                  >
                    <ChevronRight
                      className={cn(
                        "h-3.5 w-3.5 shrink-0 text-fg-3 transition-transform ml-0.5",
                        expanded.has(host.id) && "rotate-90",
                      )}
                    />
                    <div className="flex flex-col min-w-0">
                      <span className="truncate text-strong text-fg-1">{host.name}</span>
                      <span className="text-badge text-fg-3 truncate">{host.host}</span>
                    </div>
                  </button>
                  {cs != null && cs.length > 0 && (
                    <span className="shrink-0 rounded-control bg-subtle px-2 py-0.5 text-badge font-medium text-fg-2">
                      {t("batchApply.containerCount", { count: cs.length })}
                    </span>
                  )}
                  {cs == null && (
                    <span className="ml-auto inline-flex shrink-0 items-center gap-1 text-caption text-fg-3">
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("batchApply.probing")}
                    </span>
                  )}
                  {sshDown && (
                    <span className="ml-auto inline-flex shrink-0 items-center gap-1 text-caption text-danger">
                      <XCircle className="h-3.5 w-3.5" />
                      {t("batchApply.cannotConnect")}
                    </span>
                  )}
                  {!sshDown && hostErrors[host.id] === "docker" && (
                    <span className="ml-auto inline-flex shrink-0 items-center gap-1 text-caption text-warning">
                      <AlertTriangle className="h-3.5 w-3.5" />
                      {t("batchApply.containerUnavailable")}
                    </span>
                  )}
                </div>
                {expanded.has(host.id) && (
                  <div className="space-y-1 border-l border-border pl-4 pb-2">
                    {hostLoading ? (
                      /* 检测中：明确告知正在尝试连接/拉容器，配合主行转圈 */
                      <div className="flex items-center gap-1.5 px-2 py-1 text-caption text-fg-3">
                        <Loader2 className="h-3 w-3 animate-spin" />
                        {t("batchApply.probing")}
                      </div>
                    ) : sshDown ? (
                      /* SSH 层失败：宿主机本体与容器都不可用，不可选 */
                      <div className="px-2 py-1 text-caption text-danger">
                        {t("batchApply.cannotConnectHost")}
                      </div>
                    ) : (
                      <>
                        {/* 宿主机账号：与容器并列，代表该宿主机本身（仅账号，不含容器），主色标识 */}
                        <label className="flex cursor-pointer items-center gap-2 px-2 py-1 text-body">
                          <Checkbox
                            checked={selected.has(host.id)}
                            onCheckedChange={() => toggleHostOnly(host.id)}
                          />
                          <Server className="h-3.5 w-3.5 shrink-0 text-action-text" />
                          <span className="truncate font-medium text-action-text">
                            {t("batchApply.host")}
                          </span>
                        </label>
                        <div className="border-t border-border" />
                        {cs != null && cs.length === 0 && (
                          <div className="px-2 py-1 text-caption text-fg-3">
                            {t("batchApply.noContainer")}
                          </div>
                        )}
                        {(cs ?? [])
                          .filter(
                            (c) =>
                              !search ||
                              c.toLowerCase().includes(search.trim().toLowerCase()),
                          )
                          .map((c) => (
                          <label
                            key={c}
                            className="flex cursor-pointer items-center gap-2 px-2 py-1 text-body text-fg-2"
                          >
                            <Checkbox
                              checked={selected.has(contKey(host.id, c))}
                              onCheckedChange={() =>
                                toggleContainer(host.id, c)
                              }
                            />
                            <DockerIcon className="h-3.5 w-3.5 text-fg-3" />
                            <span className="truncate">{c}</span>
                          </label>
                        ))}
                      </>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* 执行结果（独立展示在落点下方、底部固定条上方） */}
      {results.length > 0 && (
        <div className="flex-shrink-0 space-y-1 rounded-panel border border-border bg-surface p-4">
          <div className="mb-3 flex items-center gap-2 text-caption font-semibold uppercase tracking-wider text-fg-3">
            {t("batchApply.resultTitle")}
            <span className="font-medium normal-case text-success">
              {t("batchApply.succeeded", {
                count: results.filter((r) => r.status === "ok").length,
              })}
            </span>
            <span className="font-medium normal-case text-danger">
              {t("batchApply.failedCount", {
                count: results.filter((r) => r.status === "fail").length,
              })}
            </span>
          </div>
          <div className="max-h-[26vh] space-y-1 overflow-y-auto pr-1 [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar]:block [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-border">
            {results.map((r) => (
              <div
                key={r.key}
                className="flex items-center gap-2 rounded-control border border-border px-2 py-1.5 text-body"
              >
                {r.status === "pending" && (
                  <span className="h-3.5 w-3.5 shrink-0 rounded-full bg-border" />
                )}
                {r.status === "running" && (
                  <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-fg-3" />
                )}
                {r.status === "ok" && (
                  <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-success" />
                )}
                {r.status === "fail" && (
                  <XCircle className="h-3.5 w-3.5 shrink-0 text-danger" />
                )}
                <span className="min-w-0 flex-1 truncate text-fg-1">{r.label}</span>
                <span
                  className={cn(
                    "shrink-0 rounded-control px-1.5 py-0.5 text-badge font-medium",
                    r.container
                      ? "bg-subtle text-fg-2"
                      : "bg-action-soft text-action-text",
                  )}
                >
                  {r.container
                    ? t("batchApply.container")
                    : t("batchApply.host")}
                </span>
                {r.status === "ok" && (
                  <span className="text-caption text-fg-3">
                    {t("batchApply.switched")}
                  </span>
                )}
                {r.status === "fail" && (
                  <span className="max-w-[40%] truncate text-caption text-danger">
                    {r.message}
                  </span>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </FullScreenPanel>
  );
}
