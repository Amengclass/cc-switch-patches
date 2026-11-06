import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Laptop, Loader2, Search, Server, Target, X } from "lucide-react";
import { DockerIcon } from "@/components/icons/DockerIcon";
import { Input } from "@/components/ui/input";
import { HoverTip } from "@/components/ui/hover-tip";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import type { RemoteHost } from "@/types/remote";

interface TargetBreadcrumbProps {
  remoteTargetId: string;
  remoteContainerId: string;
  /** 侧栏收起（72px 图标轨）：渲染成图标按钮，点击弹出同一套下拉 */
  collapsed?: boolean;
  setRemoteTargetId: (value: string) => void;
  setRemoteContainerId: (value: string) => void;
  servers: RemoteHost[];
  containers: string[];
  /** 容器列表正在加载中 */
  containersLoading?: boolean;
  /** 主机在线状态（host_id → 是否在线）；打开下拉时探活填充 */
  hostsOnline?: Record<string, boolean>;
  /** 下拉打开时触发的批量探活回调 */
  onProbeHosts?: () => void;
  /** 下拉打开时刷新主机列表（在远程主机页禁用/删除后，下拉里立刻同步） */
  onRefreshServers?: () => void;
}

/**
 * 连体胶囊：面包屑式「目标」选择器（本机 / 服务器 / 容器）。
 *
 * 主机段 + 容器段两段一体，中间竖分隔线；点击任一段弹对应下拉。
 * - 本机：只有主机段，显示「本机」；
 * - 服务器：主机段显示服务器名，容器段默认「宿主机」，可选具体容器；
 * - 切换主机时清空容器（父级变更，子级重置回「宿主机」）。
 */
export function TargetBreadcrumb({
  remoteTargetId,
  remoteContainerId,
  collapsed = false,
  setRemoteTargetId,
  setRemoteContainerId,
  servers,
  containers,
  containersLoading,
  hostsOnline,
  onProbeHosts,
  onRefreshServers,
}: TargetBreadcrumbProps) {
  const { t } = useTranslation();
  const [hostSearch, setHostSearch] = useState("");
  const [containerSearch, setContainerSearch] = useState("");

  const filteredHosts = hostSearch
    ? servers.filter(
        (s) =>
          s.name.toLowerCase().includes(hostSearch.toLowerCase()) ||
          s.host?.toLowerCase().includes(hostSearch.toLowerCase()),
      )
    : servers;

  const filteredContainers = containerSearch
    ? containers.filter((c) =>
        c.toLowerCase().includes(containerSearch.toLowerCase()),
      )
    : containers;

  // 下拉关闭时清空搜索
  const handleHostOpen = (open: boolean) => {
    if (!open) setHostSearch("");
  };
  const handleContainerOpen = (open: boolean) => {
    if (!open) setContainerSearch("");
  };
  const isLocal = !remoteTargetId;
  const activeHost = servers.find((s) => s.id === remoteTargetId) ?? null;
  const hostLabel = isLocal
    ? t("remote.targetLocal", { defaultValue: "本机" })
    : (activeHost?.name ?? remoteTargetId);
  const containerLabel =
    remoteContainerId || t("remote.targetHost", { defaultValue: "宿主机" });

  // 切换主机时同步清空容器（父级变更，子级重置回「宿主机」）
  // 例外：切回之前访问过的服务器时恢复上次的容器选择
  const handleSelectHost = (value: string) => {
    if (value === remoteTargetId) return;
    // 切到一台新服务器时清空容器（之前没选过容器的服务器）
    // 切回之前访问过的服务器时恢复 localStorage 里保存的容器
    const switchingToDifferentServer =
      value && remoteTargetId && value !== remoteTargetId;
    setRemoteTargetId(value);
    if (switchingToDifferentServer) {
      const saved = localStorage.getItem(`cc-switch-remote-container:${value}`) || "";
      setRemoteContainerId(saved);
    }
  };

  // 段按钮照官方分段控件（`components/ui/segmented-control.tsx`）：
  // 文字角色 text-caption、未选中 text-fg-2 / hover 到 text-fg-1，
  // 按下缩一下（active:scale-[0.96]，官方按钮族同款）。
  // min-w-0 + 内部 truncate：侧栏只有 200 宽，主机名/容器名长时靠收缩+截断，
  // 不能撑破侧栏（否则被 nav 的 overflow-hidden 裁掉）。
  const segmentCls =
    "inline-flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-[5px] px-2.5 text-caption font-medium text-fg-2 transition-[background-color,color,scale] hover:text-fg-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring active:scale-[0.96]";

  // 下拉条目：比官方菜单再紧凑一档（选择器是窄控件，不需要菜单那么高的行）。
  //  - 字 text-caption(12px)：基类自带 text-sm，twMerge 不把两者当同一组去重，
  //    本仓库 CSS 里 .text-sm 排在 .text-caption 之后会赢，所以必须带 ! 强制。
  //  - 行 min-h-6(24px)：比官方 h-8 矮一档，配 12px 字不显空。
  //  - 内边距 px-2：左右收紧；勾选项左侧留 pl-6 给勾选标记（基类自带 pl-8，
  //    用 pl-6 覆盖它 —— ps-* 和 pl-* 是两组，写 ps-* 会双份内边距）。
  const menuItemCls = "min-h-6 !rounded-control px-2 !text-caption";
  const menuCheckItemCls = cn(menuItemCls, "pl-6");

  // 状态标识（实时，不缓存）：检测中=转圈 / 在线=绿点 / 离线=灰点
  const statusDot = (hostId: string) => {
    const online = hostsOnline?.[hostId];
    if (online === undefined) {
      return (
        <Loader2
          className="h-3 w-3 shrink-0 animate-spin text-fg-3"
          aria-label={t("remote.probing")}
        />
      );
    }
    return (
      <span
        className={cn(
          "inline-flex h-2 w-2 shrink-0 rounded-full",
          online ? "bg-success" : "bg-fg-3/40",
        )}
        aria-hidden="true"
      />
    );
  };

  // 收起态（72px 图标轨）：一个图标按钮，点开是「主机 + 容器」合一的下拉。
  // 图标用 Target（唯一，不和「远程主机」的 Terminal、「设置→网络」的 Globe 撞），
  // 当前在远端时叠一个小绿点，一眼能看出「现在不是本机」。
  if (collapsed) {
    return (
      <DropdownMenu
        onOpenChange={(open) => {
          if (open) {
            onRefreshServers?.();
            onProbeHosts?.();
          }
        }}
      >
        <HoverTip content={hostLabel} side="right" disableHoverableContent>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={t("remote.title")}
              className="relative mx-auto flex h-8 w-12 shrink-0 items-center justify-center rounded-control transition-colors hover:bg-subtle"
            >
              <Target className="h-5 w-5" />
              {!isLocal && (
                <span
                  aria-hidden="true"
                  className="absolute end-2.5 top-1 h-2 w-2 rounded-full border-2 border-sidebar bg-success"
                />
              )}
            </button>
          </DropdownMenuTrigger>
        </HoverTip>
        <DropdownMenuContent
          align="start"
          className="min-w-[160px] max-h-[50vh] overflow-hidden rounded-panel p-1 shadow-v7-md [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-border"
        >
          <div className="max-h-[40vh] overflow-y-auto overflow-x-hidden">
            <DropdownMenuCheckboxItem
              checked={isLocal}
              // 「本机」没有容器子项，选中即关菜单（不要拦 onSelect）。
              // 注意别用 isLocal 判断：那是点击前的状态，从远端点「本机」时它是 false，
              // 拦了就变成要点两次才关。
              onCheckedChange={() => handleSelectHost("")}
              className={menuCheckItemCls}
            >
              {t("remote.targetLocal", { defaultValue: "本机" })}
            </DropdownMenuCheckboxItem>
            {servers.length > 0 && (
              <>
                <DropdownMenuSeparator />
                {/* 分组标题：服务器和容器平铺在一列容易分不清，各给一个小标题 */}
                <div className="px-2 pb-0.5 pt-1 text-badge text-fg-3">
                  {t("remote.groupServers", { defaultValue: "服务器" })}
                </div>
                {servers.map((s) => (
                  <DropdownMenuCheckboxItem
                    key={s.id}
                    checked={remoteTargetId === s.id}
                    // 选服务器后不关菜单：下面立刻列出它的容器，一次选完
                    onSelect={(e) => e.preventDefault()}
                    onCheckedChange={() => handleSelectHost(s.id)}
                    className={cn(
                      menuCheckItemCls,
                      "flex items-center justify-between gap-3",
                    )}
                  >
                    <span className="truncate">{s.name}</span>
                    {statusDot(s.id)}
                  </DropdownMenuCheckboxItem>
                ))}
              </>
            )}
            {/* 已选服务器：容器另起一组（带标题 + 胶囊标签），选容器才关菜单 */}
            {!isLocal && (
              <>
                <DropdownMenuSeparator />
                <div className="px-2 pb-0.5 pt-1 text-badge text-fg-3">
                  {t("remote.groupContainers", { defaultValue: "容器" })}
                </div>
                {containersLoading && containers.length === 0 ? (
                  <div className="flex items-center gap-1.5 px-2 py-1.5 text-caption text-fg-3">
                    <Loader2 className="h-3 w-3 animate-spin" />
                    {t("remote.loadingContainers", {
                      defaultValue: "加载容器…",
                    })}
                  </div>
                ) : (
                  <>
                    <DropdownMenuCheckboxItem
                      checked={!remoteContainerId}
                      onCheckedChange={() => setRemoteContainerId("")}
                      className={menuCheckItemCls}
                    >
                      <span className="flex w-full items-center justify-between gap-2">
                        {t("remote.targetHost", { defaultValue: "宿主机" })}
                        <span className="shrink-0 rounded-full bg-subtle px-2 py-0.5 text-badge text-fg-3">
                          {t("remote.targetHost", { defaultValue: "宿主机" })}
                        </span>
                      </span>
                    </DropdownMenuCheckboxItem>
                    {containers.map((c) => (
                      <DropdownMenuCheckboxItem
                        key={c}
                        checked={remoteContainerId === c}
                        onCheckedChange={() => setRemoteContainerId(c)}
                        className={menuCheckItemCls}
                      >
                        <span className="flex w-full items-center justify-between gap-2">
                          <span className="min-w-0 flex-1 truncate">{c}</span>
                          <span className="shrink-0 rounded-full bg-subtle px-2 py-0.5 text-badge text-fg-3">
                            {t("remote.container", { defaultValue: "容器" })}
                          </span>
                        </span>
                      </DropdownMenuCheckboxItem>
                    ))}
                  </>
                )}
              </>
            )}
          </div>
        </DropdownMenuContent>
      </DropdownMenu>
    );
  }

  return (
    // 轨道照官方分段控件：bg-subtle 底 + 3px 内衬 + h-7 / rounded-[8px]（sm 档），
    // 内圆角 = 外圆角 − 内衬 = 5px。选中段自己贴 bg-surface + shadow-v7-sm。
    <div className="inline-flex h-7 w-full min-w-0 items-stretch rounded-[8px] bg-subtle p-[3px]">
      <DropdownMenu onOpenChange={(open) => { handleHostOpen(open); if (open) { onRefreshServers?.(); onProbeHosts?.(); } }}>
        {/* 侧栏窄，主机名会 truncate；悬停显示完整目标名 */}
        <HoverTip content={hostLabel} disableHoverableContent>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className={cn(segmentCls, "bg-surface shadow-v7-sm")}
            >
              {isLocal ? (
                <Laptop className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <Server className="h-3.5 w-3.5 shrink-0" />
              )}
              <span className="min-w-0 truncate">{hostLabel}</span>
            </button>
          </DropdownMenuTrigger>
        </HoverTip>
        <DropdownMenuContent
          align="start"
          className="min-w-[150px] max-h-[50vh] overflow-hidden rounded-panel p-1 shadow-v7-md [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-border"
        >
          {servers.length >= 3 && (
            <div
              className="relative mb-0.5 border-b border-border pb-1"
              onKeyDown={(e) => e.stopPropagation()}
            >
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3 w-3 -translate-y-1/2 text-fg-3" />
              <Input
                value={hostSearch}
                onChange={(e) => setHostSearch(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape" && hostSearch) {
                    e.stopPropagation();
                    setHostSearch("");
                  }
                }}
                placeholder={t("remote.searchHosts", { defaultValue: "搜索主机…" })}
                aria-label={t("remote.searchHosts", { defaultValue: "搜索主机…" })}
                className="h-6 pl-6 pr-6 !text-caption"
                autoFocus
              />
              {hostSearch && (
                <button
                  type="button"
                  onClick={() => setHostSearch("")}
                  className="absolute right-1.5 top-1/2 flex h-4 w-4 -translate-y-1/2 items-center justify-center rounded-control text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
                >
                  <X className="h-3 w-3" />
                </button>
              )}
            </div>
          )}
          <div
            role="group"
            aria-label={t("remote.targetHost", { defaultValue: "宿主机" })}
            className="max-h-[40vh] overflow-y-auto overflow-x-hidden"
          >
            <DropdownMenuCheckboxItem
              checked={isLocal}
              onCheckedChange={() => handleSelectHost("")}
              className={menuCheckItemCls}
            >
              {t("remote.targetLocal", { defaultValue: "本机" })}
            </DropdownMenuCheckboxItem>
            {servers.length > 0 && (
              <>
                <DropdownMenuSeparator />
                {filteredHosts.map((s) => (
                  <DropdownMenuCheckboxItem
                    key={s.id}
                    checked={remoteTargetId === s.id}
                    onCheckedChange={() => handleSelectHost(s.id)}
                    className={cn(
                      menuCheckItemCls,
                      "flex items-center justify-between gap-3",
                    )}
                  >
                    <span className="truncate">{s.name}</span>
                    {statusDot(s.id)}
                  </DropdownMenuCheckboxItem>
                ))}
                {hostSearch && filteredHosts.length === 0 && (
                  <div className="px-3 py-2 text-center text-caption text-fg-3">
                    {t("remote.noMatchingHosts", { defaultValue: "无匹配主机" })}
                  </div>
                )}
              </>
            )}
          </div>
        </DropdownMenuContent>
      </DropdownMenu>

      {!isLocal && (
        <>
          <ChevronRight
            className="h-3.5 w-3.5 shrink-0 self-center text-fg-3"
            aria-hidden="true"
          />
          <DropdownMenu onOpenChange={handleContainerOpen}>
            {/* 容器名同样会 truncate；悬停显示完整名称 */}
            <HoverTip content={containerLabel} disableHoverableContent>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className={cn(segmentCls, "bg-surface shadow-v7-sm")}
                >
                  <DockerIcon className="h-3.5 w-3.5" />
                  {containersLoading && !containers.length ? (
                    <Loader2 className="h-3 w-3 shrink-0 animate-spin text-fg-3" />
                  ) : (
                    <span className="min-w-0 truncate">{containerLabel}</span>
                  )}
                </button>
              </DropdownMenuTrigger>
            </HoverTip>
            <DropdownMenuContent
              align="start"
              className="min-w-[180px] max-h-[50vh] overflow-hidden rounded-panel p-1 shadow-v7-md [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-border"
            >
              {containers.length >= 3 && (
                <div
                  className="relative mb-0.5 border-b border-border pb-1"
                  onKeyDown={(e) => e.stopPropagation()}
                >
                  <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3 w-3 -translate-y-1/2 text-fg-3" />
                  <Input
                    value={containerSearch}
                    onChange={(e) => setContainerSearch(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape" && containerSearch) {
                        e.stopPropagation();
                        setContainerSearch("");
                      }
                    }}
                    placeholder={t("remote.searchContainers", { defaultValue: "搜索容器…" })}
                    aria-label={t("remote.searchContainers", { defaultValue: "搜索容器…" })}
                    className="h-6 pl-6 pr-6 !text-caption"
                    autoFocus
                  />
                  {containerSearch && (
                    <button
                      type="button"
                      onClick={() => setContainerSearch("")}
                      className="absolute right-1.5 top-1/2 flex h-4 w-4 -translate-y-1/2 items-center justify-center rounded-control text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  )}
                </div>
              )}
              <div
                role="group"
                aria-label={t("remote.container", { defaultValue: "容器" })}
                className="max-h-[40vh] overflow-y-auto overflow-x-hidden"
              >
                <DropdownMenuCheckboxItem
                  checked={!remoteContainerId}
                  onCheckedChange={() => setRemoteContainerId("")}
                  className={menuCheckItemCls}
                >
                  <span className="flex w-full items-center justify-between gap-2">
                    {t("remote.targetHost")}
                    <span className="shrink-0 rounded-full bg-subtle px-2 py-0.5 text-badge text-fg-3">
                      {t("remote.targetHost")}
                    </span>
                  </span>
                </DropdownMenuCheckboxItem>
                {filteredContainers.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    {filteredContainers.map((c) => (
                      <DropdownMenuCheckboxItem
                        key={c}
                        checked={remoteContainerId === c}
                        onCheckedChange={() => setRemoteContainerId(c)}
                        className={menuCheckItemCls}
                      >
                        <span className="flex w-full items-center justify-between gap-2">
                          <span className="min-w-0 flex-1 truncate">{c}</span>
                          <span className="shrink-0 rounded-full bg-subtle px-2 py-0.5 text-badge text-fg-3">
                            {t("remote.container")}
                          </span>
                        </span>
                      </DropdownMenuCheckboxItem>
                    ))}
                  </>
                )}
                {containerSearch && filteredContainers.length === 0 && (
                  <div className="px-3 py-2 text-center text-caption text-fg-3">
                    {t("remote.noMatchingContainers", { defaultValue: "无匹配容器" })}
                  </div>
                )}
              </div>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
