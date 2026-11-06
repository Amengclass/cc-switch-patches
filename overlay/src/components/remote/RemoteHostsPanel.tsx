import React, {
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Terminal,
  Pencil,
  Trash2,
  PlugZap,
  Loader2,
  Ban,
  CircleDot,
  ShieldAlert,
  Eye,
  EyeOff,
  Plus,
  ChevronUp,
  ChevronDown,
  MoreHorizontal,
  Check,
  CircleAlert,
  Send,
} from "lucide-react";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import { copyText } from "@/lib/clipboard";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
import { DialogDescription, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Sheet,
  SheetBody,
  SheetFooter,
  SheetPageContent,
} from "@/components/ui/sheet";
import { AppPageHeader } from "@/components/shell/AppPageHeader";
import { HelpTip } from "@/components/ui/help-tip";
import { HoverTip } from "@/components/ui/hover-tip";
import { MatrixSearch } from "@/components/mcp/AppMatrix";
import {
  LABEL_CLASS,
  RequiredMark,
  V7ConfirmDialog,
  V7Dialog,
} from "@/components/mcp/formBits";
import { SegmentedControl } from "@/components/ui/segmented-control";
import {
  bootstrapRemoteKey,
  cleanRemoteEnvConflicts,
  deleteRemoteHost,
  detectLocalSshKeys,
  listRemoteHosts,
  saveRemoteHost,
  scanRemoteEnvConflicts,
  testRemoteConnection,
  testRemoteConnectionInfo,
} from "@/lib/api/remote";
import type { RemoteEnvConflict, RemoteHost } from "@/types/remote";
import { extractErrorMessage } from "@/utils/errorUtils";

interface HostFormState {
  name: string;
  host: string;
  port: string;
  username: string;
  authMethod: "password" | "key";
  /** 私钥文件路径（密钥认证） */
  privateKeyPath: string;
  password: string;
  savePassword: boolean;
  /** 首次密码登录后自动装公钥、改用私钥登录（以后免密码） */
  bootstrapKey: boolean;
  routeThroughLocalProxy: boolean;
  /** per-app 远端接管（claude/codex/gemini/grokbuild） */
  routeProxyApps: Record<string, boolean>;
}

const EMPTY_FORM: HostFormState = {
  name: "",
  host: "",
  port: "22",
  username: "",
  authMethod: "password",
  privateKeyPath: "",
  password: "",
  savePassword: true,
  bootstrapKey: true,
  routeThroughLocalProxy: false,
  routeProxyApps: {
    claude: false,
    codex: false,
    gemini: false,
    grokbuild: false,
  },
};

function formToDraft(f: HostFormState) {
  return {
    // 名称为选填：留空时回退为主机地址
    name: f.name.trim() || f.host.trim(),
    host: f.host.trim(),
    port: Math.max(1, Number(f.port) || 22),
    username: f.username.trim(),
    authMethod: f.authMethod,
    privateKeyPath: f.authMethod === "key" ? f.privateKeyPath.trim() : undefined,
    // 凭据一律加密保存（没有「不保存」选项：连接时没有地方临时输入密码）
    savePassword: true,
    routeThroughLocalProxy: f.routeThroughLocalProxy,
    routeProxyApps: f.routeProxyApps,
    password: f.password,
  };
}

export interface RemoteHostsPanelHandle {
  openAdd: () => void;
}

export const RemoteHostsPanel = React.forwardRef<
  RemoteHostsPanelHandle,
  { app: string; onBatchApply?: () => void }
>(function RemoteHostsPanel({ app, onBatchApply }, ref) {
  const { t } = useTranslation();

  const [hosts, setHosts] = useState<RemoteHost[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  // 状态过滤：all=全部 / enabled=仅启用 / disabled=仅禁用。禁用主机始终排在后面（B）。
  const [statusFilter, setStatusFilter] = useState<
    "all" | "enabled" | "disabled"
  >("all");
  const [loading, setLoading] = useState(true);
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<RemoteHost | null>(null);
  const [form, setForm] = useState<HostFormState>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  // 正在测连接的主机 id 集合：单台测试和「测试全部」共用，
  // 行内转圈 / 禁用按钮都看它（用 Set 而不是单个 id，批量时能同时转多行）。
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set());
  // 测试「未保存」的新连接信息
  const [testFormLoading, setTestFormLoading] = useState(false);
  // 抽屉打开时初始焦点指向「名称」输入框（而非右上角 X）
  const nameInputRef = useRef<HTMLInputElement>(null);
  const [deleteConfirm, setDeleteConfirm] = useState<{
    open: boolean;
    host: RemoteHost | null;
  }>({ open: false, host: null });
  // 密码明文显示开关
  const [showPassword, setShowPassword] = useState(false);
  // 环境变量清理
  const [envDialog, setEnvDialog] = useState<{
    open: boolean;
    host: RemoteHost | null;
  }>({ open: false, host: null });
  const [envConflicts, setEnvConflicts] = useState<RemoteEnvConflict[]>([]);
  const [envLoading, setEnvLoading] = useState(false);
  const [envCleaning, setEnvCleaning] = useState(false);

  const load = useCallback(async () => {
    try {
      setLoading(true);
      const data = await listRemoteHosts();
      setHosts(data);
    } catch (error) {
      console.error("Failed to load remote hosts:", error);
      toast.error(t("remote.loadError", { defaultValue: "加载远程主机失败" }));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    load();
  }, [load]);

  const openAdd = () => {
    setEditing(null);
    setForm(EMPTY_FORM);
    setFormOpen(true);
  };

  useImperativeHandle(ref, () => ({ openAdd }), [openAdd]);

  /**
   * 「私钥」认证零输入：本机私钥就那么几个固定位置，选私钥时自动探测并填入，
   * 探测不到才需要手填。
   */
  const autoFillKeyPath = async (alias: string, currentPath: string) => {
    if (currentPath.trim()) return;
    try {
      const keys = await detectLocalSshKeys(alias);
      const first = keys[0];
      if (first) {
        setForm((prev) =>
          prev.privateKeyPath.trim() ? prev : { ...prev, privateKeyPath: first },
        );
      }
    } catch {
      // 探测失败不影响手填
    }
  };

  const openEdit = (host: RemoteHost) => {
    setEditing(host);
    setForm({
      name: host.name,
      host: host.host,
      port: String(host.port),
      username: host.username,
      authMethod: host.authMethod,
      privateKeyPath: host.privateKeyPath ?? "",
      password: "",
      savePassword: host.savePassword,
      // 编辑已有密码主机时默认不重复装密钥（避免每次保存都跑一遍）
      bootstrapKey: false,
      routeThroughLocalProxy: host.routeThroughLocalProxy,
      routeProxyApps: { ...host.routeProxyApps },
    });
    setFormOpen(true);
    // 已有私钥主机但没存路径：打开时自动探测一次
    if (host.authMethod === "key" && !host.privateKeyPath) {
      void autoFillKeyPath(host.host, "");
    }
  };

  const handleSave = async () => {
    if (!form.host.trim()) {
      toast.error(
        t("remote.hostRequired", { defaultValue: "主机地址不能为空" }),
      );
      return;
    }
    if (!form.username.trim()) {
      toast.error(
        t("remote.usernameRequired", { defaultValue: "用户名不能为空" }),
      );
      return;
    }
    if (form.authMethod === "key" && !form.privateKeyPath.trim()) {
      toast.error(
        t("remote.privateKeyRequired", {
          defaultValue: "私钥认证需要填写私钥文件路径",
        }),
      );
      return;
    }
    // 密码认证：新增必须填密码；密钥认证：口令（密码框）可留空
    if (form.authMethod === "password" && !editing && !form.password.trim()) {
      toast.error(
        t("remote.passwordRequired", {
          defaultValue: "新增主机需要填写密码",
        }),
      );
      return;
    }
    const draft = formToDraft(form);
    const now = Date.now();
    const host: RemoteHost = {
      id: editing?.id ?? crypto.randomUUID(),
      name: draft.name,
      host: draft.host,
      port: draft.port,
      username: draft.username,
      authMethod: draft.authMethod,
      privateKeyPath: draft.privateKeyPath,
      savePassword: draft.savePassword,
      // 旧布尔 routeThroughLocalProxy 已废弃（per-app 字段全覆盖）：保存固定 false，
      // 顺带清除旧库升级时的残留 1（否则 host_wants_tunnel 曾误判建隧道）。
      routeThroughLocalProxy: false,
      routeProxyApps: draft.routeProxyApps,
      // 表单不编辑容器开关：编辑主机时保留已有容器开关，避免误清空
      routeProxyContainerApps: editing?.routeProxyContainerApps,
      // 编辑时保留禁用态（表单不编辑禁用；重用行内禁用/启用按钮切换）
      disabled: editing?.disabled ?? false,
      createdAt: editing?.createdAt ?? now,
      updatedAt: now,
    };
    setSaving(true);
    try {
      const saved = await saveRemoteHost(host, form.password || undefined);
      toast.success(
        editing
          ? t("remote.updated", { defaultValue: "远程主机已更新" })
          : t("remote.added", { defaultValue: "远程主机已添加" }),
      );

      // 首次密码登录 → 自动装公钥并改用私钥认证（以后免密码）
      if (
        form.authMethod === "password" &&
        form.bootstrapKey &&
        (form.password.trim().length > 0 || editing)
      ) {
        try {
          await bootstrapRemoteKey(saved.id, form.password);
          toast.success(
            t("remote.keyConfigured", {
              defaultValue: "已自动配置密钥登录，以后无需输入密码",
            }),
          );
        } catch (error) {
          console.error("Failed to bootstrap remote key:", error);
          // 不阻断：密码已保存，仍可正常连接
          toast.warning(
            t("remote.keyConfigureFailed", {
              defaultValue: "自动配置密钥失败，已保留密码登录",
            }),
            { description: extractErrorMessage(error) || undefined },
          );
        }
      }
      setFormOpen(false);
      // 连接信息可能已改，旧测试结果作废
      setTestResults((prev) => {
        const next = { ...prev };
        delete next[host.id];
        return next;
      });
      load();
    } catch (error) {
      console.error("Failed to save remote host:", error);
      toast.error(t("remote.saveError", { defaultValue: "保存远程主机失败" }), {
        description: String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  // 每台主机最近一次测试连接的结果（会话内有效，不落库）：
  // 让列表直接显示「连通 / 失败」，不用再点一次才知道。
  const [testResults, setTestResults] = useState<
    Record<string, "ok" | "fail">
  >({});

  // 标记某台主机「测试中」，返回取消标记的函数
  const markTesting = (id: string, on: boolean) =>
    setTestingIds((prev) => {
      const next = new Set(prev);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const handleTest = async (host: RemoteHost) => {
    markTesting(host.id, true);
    try {
      await testRemoteConnection(host.id, app);
      setTestResults((prev) => ({ ...prev, [host.id]: "ok" }));
      toast.success(
        t("remote.reachable", {
          defaultValue: `连通正常 ${host.name}`,
          name: host.name,
        }),
      );
    } catch (error) {
      console.error("Failed to test connection:", error);
      setTestResults((prev) => ({ ...prev, [host.id]: "fail" }));
      toast.error(t("remote.connectionFailed", { defaultValue: "连接失败" }), {
        description: String(error),
      });
    } finally {
      markTesting(host.id, false);
    }
  };

  // 批量测试所有启用中的主机（⋯ 菜单）。
  // 并发跑（每台独立 SSH 会话，互不依赖），但限并发 6 —— 主机多时
  // 一次开几十个 SSH 会话对端也吃不消，排队跑既能并行又不至于打爆。
  const [testingAll, setTestingAll] = useState(false);
  const handleTestAll = async () => {
    const targets = hosts.filter((h) => !h.disabled);
    if (targets.length === 0) return;
    setTestingAll(true);
    const CONCURRENCY = 6;
    let ok = 0;
    try {
      let cursor = 0;
      const worker = async () => {
        while (cursor < targets.length) {
          const host = targets[cursor++];
          markTesting(host.id, true);
          try {
            await testRemoteConnection(host.id, app);
            ok += 1;
            setTestResults((prev) => ({ ...prev, [host.id]: "ok" }));
          } catch {
            setTestResults((prev) => ({ ...prev, [host.id]: "fail" }));
            // 单台失败不打断整批，最后统一报数
          } finally {
            markTesting(host.id, false);
          }
        }
      };
      await Promise.all(
        Array.from({ length: Math.min(CONCURRENCY, targets.length) }, worker),
      );
      if (ok === targets.length) {
        toast.success(
          t("remote.testAllOk", {
            defaultValue: `{{count}} 台主机全部连通正常`,
            count: targets.length,
          }),
        );
      } else {
        toast.warning(
          t("remote.testAllPartial", {
            defaultValue: `{{ok}}/{{total}} 台连通正常`,
            ok,
            total: targets.length,
          }),
        );
      }
    } finally {
      setTestingAll(false);
    }
  };

  // 禁用/启用某台主机（软禁用：不删除，目标选择/操作排除，管理页仍可恢复）
  const handleToggleDisabled = async (host: RemoteHost) => {
    const next = { ...host, disabled: !host.disabled };
    try {
      await saveRemoteHost(next, undefined);
      toast.success(
        next.disabled
          ? t("remote.disabled", {
              defaultValue: `已禁用 ${host.name}`,
              name: host.name,
            })
          : t("remote.enabled", {
              defaultValue: `已启用 ${host.name}`,
              name: host.name,
            }),
      );
      load();
    } catch (error) {
      console.error("Failed to toggle disabled:", error);
      toast.error(t("remote.disableFailed", { defaultValue: "操作失败" }), {
        description: String(error),
      });
    }
  };

  // 测试「未保存」的新连接信息（新增主机时直接测，无需先保存）
  const handleTestForm = async () => {
    const needsPassword =
      form.authMethod === "password" && !form.password.trim();
    const needsKey =
      form.authMethod === "key" && !form.privateKeyPath.trim();
    if (!form.host.trim() || !form.username.trim() || needsPassword || needsKey) {
      toast.error(
        t("remote.formRequired", {
          defaultValue: "请先填写主机地址、用户名和认证信息",
        }),
      );
      return;
    }
    setTestFormLoading(true);
    try {
      await testRemoteConnectionInfo(
        form.host.trim(),
        Math.max(1, Number(form.port) || 22),
        form.username.trim(),
        form.password,
        app,
        form.authMethod,
        form.privateKeyPath.trim() || undefined,
      );
      toast.success(
        t("remote.reachable", {
          defaultValue: "连通正常",
          // locale 文案是「连通正常 {{name}}」：不传 name 会原样显示 {{name}}。
          // 新增场景还没有正式名称，按保存规则回退为主机地址。
          name: form.name.trim() || form.host.trim(),
        }),
      );
    } catch (error) {
      console.error("Failed to test new connection:", error);
      toast.error(t("remote.connectionFailed", { defaultValue: "连接失败" }), {
        description: String(error),
      });
    } finally {
      setTestFormLoading(false);
    }
  };

  const handleDelete = async () => {
    const host = deleteConfirm.host;
    if (!host) return;
    try {
      await deleteRemoteHost(host.id);
      toast.success(t("remote.deleted", { defaultValue: "远程主机已删除" }));
      load();
    } catch (error) {
      console.error("Failed to delete remote host:", error);
      toast.error(
        t("remote.deleteError", { defaultValue: "删除远程主机失败" }),
      );
    } finally {
      setDeleteConfirm({ open: false, host: null });
    }
  };

  // 打开环境变量扫描
  const openEnvDialog = async (host: RemoteHost) => {
    setEnvDialog({ open: true, host });
    setEnvConflicts([]);
    setEnvLoading(true);
    try {
      const conflicts = await scanRemoteEnvConflicts(host.id);
      setEnvConflicts(conflicts);
    } catch (error) {
      console.error("Failed to scan env conflicts:", error);
      toast.error(
        t("remote.scanEnvError", { defaultValue: "扫描冲突环境变量失败" }),
        { description: String(error) },
      );
    } finally {
      setEnvLoading(false);
    }
  };

  // 清理冲突环境变量
  const handleCleanEnv = async () => {
    const host = envDialog.host;
    if (!host) return;
    setEnvCleaning(true);
    try {
      const result = await cleanRemoteEnvConflicts(host.id);
      toast.success(
        t("remote.envCleaned", {
          defaultValue: "已清理 {{count}} 处冲突环境变量",
          count: result.cleaned,
        }),
      );
      setEnvDialog({ open: false, host: null });
    } catch (error) {
      console.error("Failed to clean env conflicts:", error);
      toast.error(
        t("remote.cleanEnvError", { defaultValue: "清理冲突环境变量失败" }),
        { description: String(error) },
      );
    } finally {
      setEnvCleaning(false);
    }
  };

  // 搜索 + 状态过滤：按名称/地址/用户名匹配；按状态过滤（列表顺序保持原始创建顺序，不沉底）。
  const filteredHosts = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    let list = hosts;
    if (q) {
      list = list.filter(
        (h) =>
          h.name.toLowerCase().includes(q) ||
          h.host.toLowerCase().includes(q) ||
          h.username.toLowerCase().includes(q),
      );
    }
    if (statusFilter === "enabled") list = list.filter((h) => !h.disabled);
    else if (statusFilter === "disabled") list = list.filter((h) => h.disabled);
    return list;
  }, [hosts, searchQuery, statusFilter]);

  return (
    <TooltipProvider>
      <AppPageHeader
        icon={<Terminal className="h-5 w-5" strokeWidth={1.5} />}
        title={t("remote.title")}
        titleExtra={
          <HelpTip title={t("remote.title")}>{t("remote.description")}</HelpTip>
        }
        actions={
          <>
            {onBatchApply && (
              <Button
                type="button"
                variant="quiet"
                size="regular"
                onClick={onBatchApply}
              >
                <Send className="h-4 w-4" strokeWidth={2} />
                {t("batchApply.title", { defaultValue: "批量应用" })}
              </Button>
            )}
            <Button
              type="button"
              variant="solid"
              size="regular"
              onClick={openAdd}
            >
              <Plus className="h-4 w-4" strokeWidth={2} />
              {t("remote.add")}
            </Button>
            <DropdownMenu>
              <HoverTip content={t("common.more", { defaultValue: "更多" })}>
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    variant="quiet"
                    size="icon-compact"
                    className="h-8 w-8"
                    aria-label={t("remote.moreActions", {
                      defaultValue: "更多操作",
                    })}
                  >
                    <MoreHorizontal className="h-4 w-4" strokeWidth={1.5} />
                  </Button>
                </DropdownMenuTrigger>
              </HoverTip>
              <DropdownMenuContent align="end" className="min-w-[200px]">
                <DropdownMenuItem
                  disabled={testingAll || hosts.every((h) => h.disabled)}
                  onSelect={() => void handleTestAll()}
                >
                  {t("remote.testAll", { defaultValue: "测试全部连接" })}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      <div className="flex min-h-0 flex-1 flex-col">
        {/* 搜索 + 状态过滤 + 计数（对齐 MCP 管理面板） */}
        <div className="flex h-14 shrink-0 items-center gap-3 px-6">
          <MatrixSearch
            className="w-[320px] min-w-[180px] shrink"
            value={searchQuery}
            onValueChange={setSearchQuery}
            placeholder={t("remote.searchPlaceholder")}
            ariaLabel={t("remote.searchAriaLabel")}
            status={
              searchQuery.trim()
                ? t("appMatrix.found", { count: filteredHosts.length })
                : ""
            }
          />
          <SegmentedControl<"all" | "enabled" | "disabled">
            aria-label={t("remote.filterAriaLabel")}
            value={statusFilter}
            onValueChange={setStatusFilter}
            size="sm"
            className="shrink-0"
            items={[
              { value: "all", label: t("remote.filterAll") },
              { value: "enabled", label: t("remote.filterEnabled") },
              { value: "disabled", label: t("remote.filterDisabled") },
            ]}
          />
          <span className="shrink-0 whitespace-nowrap text-caption tabular-nums text-fg-2">
            {t("remote.count", { count: filteredHosts.length })}
          </span>
        </div>

        {/* 主机列表 */}
        <div className="flex min-h-0 flex-1 flex-col px-6 pb-5">
          {loading ? (
            <div className="flex flex-1 items-center justify-center gap-2 text-body text-fg-2">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("remote.loading")}
            </div>
          ) : hosts.length === 0 ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 px-10 pb-12 text-center">
              <h2 className="m-0 text-section">{t("remote.empty")}</h2>
              <p className="m-0 text-body text-fg-2">{t("remote.emptyHint")}</p>
              <div className="mt-2 flex gap-2">
                <Button
                  type="button"
                  variant="neutral"
                  size="regular"
                  onClick={openAdd}
                >
                  <Plus className="h-3.5 w-3.5" strokeWidth={2} />
                  {t("remote.add")}
                </Button>
              </div>
            </div>
          ) : (
            <div className="min-h-0 flex-1 overflow-auto scroll-stable rounded-panel border border-border bg-surface">
              {filteredHosts.length === 0 ? (
                <div className="flex h-full flex-col items-center justify-center gap-3 px-6 py-8 text-center">
                  <span className="text-body text-fg-2">
                    {t("remote.searchEmpty")}
                  </span>
                  <div className="flex gap-2">
                    <Button
                      type="button"
                      variant="neutral"
                      size="regular"
                      onClick={() => setSearchQuery("")}
                    >
                      {t("mcpPage.clearSearch")}
                    </Button>
                  </div>
                </div>
              ) : (
                <ul
                  aria-label={t("remote.listLabel")}
                  className="m-0 list-none p-0"
                >
                  {filteredHosts.map((host, index) => (
                    <li
                      key={host.id}
                      className={cn(
                        "flex h-14 items-center gap-3 pe-2 ps-4 transition-colors hover:bg-subtle",
                        index > 0 && "border-t border-border",
                      )}
                    >
                      <div className="flex min-w-0 flex-1 flex-col">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <span
                            className="min-w-0 truncate text-body font-medium"
                            title={host.name}
                          >
                            {host.name}
                          </span>
                          {/* 状态徽标照官方 UsageDataSourcesSheet / PromptListItem 的写法：
                              开 = bg-success-soft + text-success-text，关 = bg-subtle + text-fg-2。
                              比灰描边 chip + 小色点醒目得多。 */}
                          <span
                            className={cn(
                              "inline-flex h-[18px] shrink-0 items-center gap-1 whitespace-nowrap rounded-full px-1.5 text-badge font-medium",
                              host.disabled
                                ? "bg-subtle text-fg-2"
                                : "bg-success-soft text-success-text",
                            )}
                          >
                            {host.disabled ? (
                              <Ban className="h-3 w-3" />
                            ) : (
                              <CircleDot className="h-3 w-3" />
                            )}
                            {host.disabled
                              ? t("remote.statusDisabled")
                              : t("remote.statusEnabled")}
                          </span>
                          {/* 最近一次测试连接的结果（会话内）：连通绿 / 失败红。
                              没测过就不显示，避免和上面的启用/禁用徽标混淆。 */}
                          {testResults[host.id] && (
                            <span
                              className={cn(
                                "inline-flex h-[18px] shrink-0 items-center gap-1 whitespace-nowrap rounded-full px-1.5 text-badge font-medium",
                                testResults[host.id] === "ok"
                                  ? "bg-success-soft text-success-text"
                                  : "bg-danger-soft text-danger-text",
                              )}
                            >
                              {testResults[host.id] === "ok" ? (
                                <Check className="h-3 w-3" />
                              ) : (
                                <CircleAlert className="h-3 w-3" />
                              )}
                              {testResults[host.id] === "ok"
                                ? t("remote.testOkShort", {
                                    defaultValue: "连通",
                                  })
                                : t("remote.testFailShort", {
                                    defaultValue: "失败",
                                  })}
                            </span>
                          )}
                        </div>
                        <div className="flex min-w-0 items-center text-caption text-fg-2">
                          <span className="min-w-0 truncate font-mono">
                            {host.username}@
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <button
                                  type="button"
                                  onClick={() => {
                                    void copyText(host.host).then(() =>
                                      toast.success(
                                        t("remote.copied", {
                                          defaultValue: "主机地址已复制",
                                        }),
                                      ),
                                    );
                                  }}
                                  className="text-fg-2 underline decoration-border underline-offset-[3px] hover:text-fg-1"
                                >
                                  {host.host}
                                </button>
                              </TooltipTrigger>
                              <TooltipContent>
                                {t("remote.copyHost", {
                                  defaultValue: "点击复制主机地址",
                                })}
                              </TooltipContent>
                            </Tooltip>
                            :{host.port}
                          </span>
                        </div>
                      </div>

                      {/* 禁用/启用开关：Tooltip 悬停提示。
                          注意：TooltipTrigger 用 asChild 直接包 Switch 会破坏 Switch 渲染（轨道变白），
                          因此改包一层中介 <span>，悬停 span 触发提示、Switch 自身正常渲染。 */}
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <span className="inline-flex shrink-0">
                            <Switch
                              checked={!host.disabled}
                              onCheckedChange={() =>
                                void handleToggleDisabled(host)
                              }
                              aria-label={
                                host.disabled
                                  ? t("remote.enable", {
                                      defaultValue: "点击启用",
                                    })
                                  : t("remote.disable", {
                                      defaultValue: "点击禁用（操作排除）",
                                    })
                              }
                            />
                          </span>
                        </TooltipTrigger>
                        <TooltipContent>
                          {host.disabled
                            ? t("remote.switchTooltipDisabled", {
                                defaultValue:
                                  "已禁用此主机：点击开关启用（恢复可操作）",
                              })
                            : t("remote.switchTooltipEnabled", {
                                defaultValue:
                                  "启用中：点击开关禁用（操作排除，可随时恢复）",
                              })}
                        </TooltipContent>
                      </Tooltip>

                      {/* 行内操作：测试连接 / 环境变量 / 编辑 / 删除 */}
                      <div className="flex shrink-0 items-center gap-1">
                        <HoverTip content={t("remote.test")}>
                          <Button
                            type="button"
                            variant="quiet"
                            size="icon-compact"
                            aria-label={t("remote.test")}
                            disabled={host.disabled || testingIds.has(host.id)}
                            onClick={() => void handleTest(host)}
                          >
                            {testingIds.has(host.id) ? (
                              <Loader2 className="h-[15px] w-[15px] animate-spin" />
                            ) : (
                              <PlugZap
                                className="h-[15px] w-[15px]"
                                strokeWidth={1.5}
                              />
                            )}
                          </Button>
                        </HoverTip>
                        <HoverTip content={t("remote.env")}>
                          <Button
                            type="button"
                            variant="quiet"
                            size="icon-compact"
                            aria-label={t("remote.env")}
                            disabled={host.disabled}
                            onClick={() => void openEnvDialog(host)}
                          >
                            <ShieldAlert
                              className="h-[15px] w-[15px]"
                              strokeWidth={1.5}
                            />
                          </Button>
                        </HoverTip>
                        <HoverTip content={t("common.edit")}>
                          <Button
                            type="button"
                            variant="quiet"
                            size="icon-compact"
                            aria-label={t("common.edit")}
                            disabled={host.disabled}
                            onClick={() => openEdit(host)}
                          >
                            <Pencil
                              className="h-[15px] w-[15px]"
                              strokeWidth={1.5}
                            />
                          </Button>
                        </HoverTip>
                        <HoverTip content={t("common.delete")}>
                          <Button
                            type="button"
                            variant="quiet"
                            size="icon-compact"
                            className="text-danger-text hover:text-danger-text"
                            aria-label={t("common.delete")}
                            disabled={host.disabled}
                            onClick={() =>
                              setDeleteConfirm({ open: true, host })
                            }
                          >
                            <Trash2
                              className="h-[15px] w-[15px]"
                              strokeWidth={1.5}
                            />
                          </Button>
                        </HoverTip>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </div>

      {/* 添加/编辑表单：右侧整页抽屉（对齐 MCP 的 Sheet 形态） */}
      {formOpen && (
        <Sheet
          open
          modal={false}
          onOpenChange={(open) => {
            if (!open) setFormOpen(false);
          }}
        >
          <SheetPageContent
            title={
              editing
                ? t("remote.editTitle", { defaultValue: "编辑远程主机" })
                : t("remote.addTitle", { defaultValue: "添加远程主机" })
            }
            closeLabel={t("common.back", { defaultValue: "返回" })}
            aria-describedby={undefined}
          >
            <SheetBody className="flex flex-col gap-5 px-6 pb-6 pt-5">
              {/* 名称：照 MCP 表单 —— 标签在上、输入框在下，右侧「?」放说明 */}
              <div className="flex flex-col gap-1.5">
                <div className="flex items-center gap-0.5">
                  <label htmlFor="host-name" className={LABEL_CLASS}>
                    {t("remote.nameLabel", { defaultValue: "名称" })}
                  </label>
                  <span className="text-caption text-fg-3">
                    {t("remote.optional", { defaultValue: "选填" })}
                  </span>
                  <HelpTip
                    title={t("remote.nameLabel", { defaultValue: "名称" })}
                  >
                    {t("remote.nameHint", {
                      defaultValue: "不填时将使用主机地址作为显示名",
                    })}
                  </HelpTip>
                </div>
                <Input
                  id="host-name"
                  ref={nameInputRef}
                  value={form.name}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                  placeholder={t("remote.namePlaceholder", {
                    defaultValue: "生产服务器 / 开发机…",
                  })}
                />
              </div>

              <div aria-hidden="true" className="h-px shrink-0 bg-border" />

              {/* 连接信息 */}
              <section
                aria-labelledby="remote-conn-title"
                className="flex flex-col gap-4"
              >
                <h3
                  id="remote-conn-title"
                  className="m-0 text-body font-semibold text-fg-1"
                >
                  {t("remote.connectionInfo", { defaultValue: "连接信息" })}
                </h3>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="host-address" className={LABEL_CLASS}>
                    {t("remote.hostLabel", { defaultValue: "主机地址" })}
                    <RequiredMark
                      srText={t("remote.required", { defaultValue: "必填" })}
                    />
                  </label>
                  <div className="flex items-center gap-3">
                    <Input
                      id="host-address"
                      value={form.host}
                      onChange={(e) =>
                        setForm({ ...form, host: e.target.value })
                      }
                      placeholder={t("remote.hostPlaceholder", {
                        defaultValue: "例如 192.168.1.10",
                      })}
                      className="min-w-0 flex-1"
                    />
                    <div className="relative w-24 shrink-0">
                      <Input
                        id="host-port"
                        type="text"
                        inputMode="numeric"
                        pattern="[0-9]*"
                        value={form.port}
                        onChange={(e) =>
                          setForm({
                            ...form,
                            port: e.target.value.replace(/[^\d]/g, ""),
                          })
                        }
                        placeholder={t("remote.portPlaceholder", {
                          defaultValue: "例如 22",
                        })}
                        className="pr-8 text-center"
                      />
                      <div className="absolute right-0.5 top-1/2 flex -translate-y-1/2 flex-col">
                        <button
                          type="button"
                          tabIndex={-1}
                          onClick={() =>
                            setForm({
                              ...form,
                              port: String(
                                Math.max(1, (Number(form.port) || 0) + 1),
                              ),
                            })
                          }
                          className="flex h-3.5 w-4 items-center justify-center rounded-sm text-fg-3 hover:bg-subtle hover:text-fg-1"
                        >
                          <ChevronUp className="h-3 w-3" />
                        </button>
                        <button
                          type="button"
                          tabIndex={-1}
                          onClick={() =>
                            setForm({
                              ...form,
                              port: String(
                                Math.max(1, (Number(form.port) || 0) - 1),
                              ),
                            })
                          }
                          className="flex h-3.5 w-4 items-center justify-center rounded-sm text-fg-3 hover:bg-subtle hover:text-fg-1"
                        >
                          <ChevronDown className="h-3 w-3" />
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              </section>
              {/* 认证信息 */}
              <section
                aria-labelledby="remote-auth-title"
                className="flex flex-col gap-4"
              >
                <h3
                  id="remote-auth-title"
                  className="m-0 text-body font-semibold text-fg-1"
                >
                  {t("remote.authInfo", { defaultValue: "认证信息" })}
                </h3>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="host-user" className={LABEL_CLASS}>
                    {t("remote.usernameLabel", { defaultValue: "用户名" })}
                    <RequiredMark
                      srText={t("remote.required", { defaultValue: "必填" })}
                    />
                  </label>
                  <Input
                    id="host-user"
                    value={form.username}
                    onChange={(e) =>
                      setForm({ ...form, username: e.target.value })
                    }
                    placeholder={t("remote.usernamePlaceholder", {
                      defaultValue: "例如 root",
                    })}
                  />
                  <span className="text-caption text-fg-2">
                    {form.username === "root"
                      ? t("remote.rootWarning", {
                          defaultValue:
                            "不建议使用 root，推荐具有 sudo 权限的普通用户",
                        })
                      : t("remote.usernameHint", {
                          defaultValue: "建议使用具有 SSH 权限的普通用户",
                        })}
                  </span>
                </div>
                <div className="flex flex-col gap-1.5">
                  <span className={LABEL_CLASS}>
                    {t("remote.authMethodLabel", {
                      defaultValue: "认证方式",
                    })}
                  </span>
                  <SegmentedControl<"password" | "key">
                    size="sm"
                    aria-label={t("remote.authMethodLabel", {
                      defaultValue: "认证方式",
                    })}
                    value={form.authMethod}
                    onValueChange={(value) => {
                      setForm({ ...form, authMethod: value });
                      if (value === "key") {
                        void autoFillKeyPath(form.host, form.privateKeyPath);
                      }
                    }}
                    items={[
                      {
                        value: "password",
                        label: t("remote.authPassword", {
                          defaultValue: "密码",
                        }),
                      },
                      {
                        value: "key",
                        label: t("remote.authKey", {
                          defaultValue: "私钥",
                        }),
                      },
                    ]}
                  />
                </div>
                {form.authMethod === "key" && (
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="host-key" className={LABEL_CLASS}>
                      {t("remote.privateKeyPathLabel", {
                        defaultValue: "私钥文件路径",
                      })}
                      <RequiredMark
                        srText={t("remote.required", { defaultValue: "必填" })}
                      />
                    </label>
                    <Input
                      id="host-key"
                      value={form.privateKeyPath}
                      onChange={(e) =>
                        setForm({ ...form, privateKeyPath: e.target.value })
                      }
                      placeholder={t("remote.privateKeyPathPlaceholder", {
                        defaultValue: "例如 ~/.ssh/id_ed25519",
                      })}
                    />
                    <span className="text-caption text-fg-2">
                      {form.privateKeyPath
                        ? t("remote.privateKeyPathDetected", {
                            defaultValue:
                              "已自动填入本机私钥；可改成其他路径。有口令时填在下方，会加密保存",
                          })
                        : t("remote.privateKeyPathHint", {
                            defaultValue:
                              "未检测到本机私钥，请手动填写路径；有口令时填在下方密码框",
                          })}
                    </span>
                  </div>
                )}
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="host-pass" className={LABEL_CLASS}>
                    {form.authMethod === "key"
                      ? t("remote.passphraseLabel", {
                          defaultValue: "私钥口令（可选）",
                        })
                      : t("remote.passwordLabel", { defaultValue: "密码" })}
                    {!editing && form.authMethod === "password" && (
                      <RequiredMark
                        srText={t("remote.required", { defaultValue: "必填" })}
                      />
                    )}
                  </label>
                  <div className="relative">
                    <Input
                      id="host-pass"
                      type={showPassword ? "text" : "password"}
                      value={form.password}
                      onChange={(e) =>
                        setForm({ ...form, password: e.target.value })
                      }
                      placeholder={
                        form.authMethod === "key"
                          ? t("remote.passphrasePlaceholder", {
                              defaultValue: "私钥无口令可留空",
                            })
                          : editing
                            ? t("remote.passwordKeepHint", {
                                defaultValue: "留空则保留已保存的密码",
                              })
                            : t("remote.passwordPlaceholder", {
                                defaultValue: "请输入 SSH 登录密码",
                              })
                      }
                      className="pr-9"
                    />
                    <button
                      type="button"
                      onClick={() => setShowPassword((v) => !v)}
                      title={
                        showPassword
                          ? t("remote.hidePassword", {
                              defaultValue: "隐藏密码",
                            })
                          : t("remote.showPassword", {
                              defaultValue: "显示密码",
                            })
                      }
                      className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-3 hover:text-fg-1"
                    >
                      {showPassword ? (
                        <EyeOff className="h-4 w-4" />
                      ) : (
                        <Eye className="h-4 w-4" />
                      )}
                    </button>
                  </div>
                </div>
                {/* 凭据一律加密保存（无「保存密码」开关：关掉后连接时无处可输，
                    只会让远端操作直接失败）。配置密钥成功后密码会被自动删除。 */}
                <p className="m-0 text-caption text-fg-2">
                  {form.authMethod === "key"
                    ? t("remote.passphraseStoredHint", {
                        defaultValue:
                          "口令会用 Windows DPAPI 加密保存在本机（仅当前 Windows 账户可解密），用于自动连接。",
                      })
                    : t("remote.passwordStoredHint", {
                        defaultValue:
                          "密码会以 Windows DPAPI 加密保存在本机（仅当前 Windows 账户可解密），用于一键连接 / 切换；配置密钥成功后会删除。",
                      })}
                </p>
                {form.authMethod === "password" && (
                  <div className="flex items-center justify-between gap-4">
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1">
                        <span className={LABEL_CLASS}>
                          {t("remote.bootstrapKey", {
                            defaultValue: "首次登录后自动配置密钥（推荐）",
                          })}
                        </span>
                        <HelpTip
                          title={t("remote.bootstrapKey", {
                            defaultValue: "首次登录后自动配置密钥（推荐）",
                          })}
                        >
                          {t("remote.bootstrapKeyHint", {
                            defaultValue:
                              "用刚填的密码登录一次：在本机生成密钥、把公钥装到远端，之后连接不再需要密码（对齐 VS Code 的首次体验）",
                          })}
                        </HelpTip>
                      </div>
                    </div>
                    <Switch
                      checked={form.bootstrapKey}
                      onCheckedChange={(v) =>
                        setForm({ ...form, bootstrapKey: v })
                      }
                      aria-label={t("remote.bootstrapKey", {
                        defaultValue: "首次登录后自动配置密钥（推荐）",
                      })}
                    />
                  </div>
                )}
              </section>

              <p className="text-caption text-fg-2">
                {t("remote.formHint", {
                  defaultValue:
                    "M1 阶段仅支持密码认证；密钥认证将在后续版本加入。",
                })}
              </p>
            </SheetBody>
            <SheetFooter className="h-14 px-6 py-0">
              <Button
                variant="outline"
                onClick={() => setFormOpen(false)}
                disabled={saving || testFormLoading}
              >
                {t("common.cancel", { defaultValue: "取消" })}
              </Button>
              <Button
                variant="outline"
                onClick={() => {
                  if (editing) {
                    void handleTest(editing);
                  } else {
                    void handleTestForm();
                  }
                }}
                disabled={saving || testFormLoading}
              >
                {testFormLoading || (editing && testingIds.has(editing.id)) ? (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" />
                ) : (
                  <PlugZap className="mr-1 h-4 w-4" />
                )}
                {testFormLoading || (editing && testingIds.has(editing.id))
                  ? t("remote.testing", { defaultValue: "测试中…" })
                  : t("remote.test", { defaultValue: "测试连接" })}
              </Button>
              <Button
                onClick={() => void handleSave()}
                disabled={saving || testFormLoading}
              >
                {saving && <Loader2 className="mr-1 h-4 w-4 animate-spin" />}
                {t("common.save", { defaultValue: "保存" })}
              </Button>
            </SheetFooter>
          </SheetPageContent>
        </Sheet>
      )}

      {/* 环境变量清理 */}
      <V7Dialog
        open={envDialog.open}
        onOpenChange={(open) => {
          if (!open) setEnvDialog({ open: false, host: null });
        }}
        describedBy="remote-env-dialog-desc"
      >
        <div className="flex items-center gap-2">
          <ShieldAlert className="h-5 w-5 text-warning" strokeWidth={1.5} />
          <DialogTitle className="text-section">
            {t("remote.envTitle", { defaultValue: "冲突环境变量" })}
          </DialogTitle>
        </div>
        <DialogDescription
          id="remote-env-dialog-desc"
          className="text-body text-fg-2"
        >
          {envDialog.host
            ? t("remote.envSubtitle", {
                defaultValue: "{{host}} · 清理后远端切换才生效",
                host: envDialog.host.name,
              })
            : ""}
        </DialogDescription>

        {envLoading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin" />
          </div>
        ) : envConflicts.length === 0 ? (
          <Alert>
            <AlertDescription>
              {t("remote.envClean", {
                defaultValue: "未发现冲突的 ANTHROPIC_* 环境变量",
              })}
            </AlertDescription>
          </Alert>
        ) : (
          <>
            <ScrollArea className="max-h-[45vh]">
              <div className="space-y-2">
                {envConflicts.map((c, i) => (
                  <div
                    key={i}
                    className="rounded-control border border-border bg-subtle px-3 py-2.5"
                  >
                    <p className="font-mono text-body font-medium">
                      {c.varName}
                    </p>
                    <p
                      className="mt-0.5 break-all font-mono text-caption text-fg-2"
                      title={c.sourcePath}
                    >
                      {c.sourcePath}
                    </p>
                  </div>
                ))}
              </div>
            </ScrollArea>
            <p className="text-caption text-fg-2">
              {t("remote.envCleanWarning", {
                defaultValue: "清理会移除这些变量（含系统级 .env 中的定义）",
              })}
            </p>
          </>
        )}

        <div className="flex flex-wrap justify-end gap-2 pt-1">
          <Button
            type="button"
            variant="neutral"
            size="regular"
            onClick={() => setEnvDialog({ open: false, host: null })}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
          <Button
            type="button"
            variant="destructive"
            size="regular"
            disabled={envLoading || envConflicts.length === 0 || envCleaning}
            onClick={() => void handleCleanEnv()}
          >
            {envCleaning && <Loader2 className="mr-1 h-4 w-4 animate-spin" />}
            {t("remote.cleanEnv", { defaultValue: "清理冲突变量" })}
          </Button>
        </div>
      </V7Dialog>

      {/* 删除确认 */}
      <V7ConfirmDialog
        open={deleteConfirm.open}
        title={t("remote.deleteConfirmTitle", {
          defaultValue: "删除远程主机",
        })}
        body={t("remote.deleteConfirmMessage", {
          defaultValue: `确定要删除 "${deleteConfirm.host?.name}" 吗？已加密保存的密码也会一并清除。`,
          name: deleteConfirm.host?.name ?? "",
        })}
        confirmLabel={t("common.delete", { defaultValue: "删除" })}
        onConfirm={handleDelete}
        onCancel={() => setDeleteConfirm({ open: false, host: null })}
      />
    </TooltipProvider>
  );
});
