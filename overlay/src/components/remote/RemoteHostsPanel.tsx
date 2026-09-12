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
  Server,
  Pencil,
  Trash2,
  PlugZap,
  Loader2,
  Ban,
  CircleDot,
  ListFilter,
  ShieldAlert,
  Eye,
  EyeOff,
  Tag,
  Globe,
  Search,
  User,
  Key,
  Info,
  ChevronUp,
  ChevronDown,
} from "lucide-react";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import { copyText } from "@/lib/clipboard";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ManagementListSearch } from "@/components/common/ManagementListSearch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  cleanRemoteEnvConflicts,
  deleteRemoteHost,
  listRemoteHosts,
  saveRemoteHost,
  scanRemoteEnvConflicts,
  testRemoteConnection,
  testRemoteConnectionInfo,
} from "@/lib/api/remote";
import type { RemoteEnvConflict, RemoteHost } from "@/types/remote";

interface HostFormState {
  name: string;
  host: string;
  port: string;
  username: string;
  password: string;
  savePassword: boolean;
  routeThroughLocalProxy: boolean;
  /** per-app 远端接管（claude/codex/gemini/grokbuild） */
  routeProxyApps: Record<string, boolean>;
}

const EMPTY_FORM: HostFormState = {
  name: "",
  host: "",
  port: "22",
  username: "",
  password: "",
  savePassword: true,
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
    authMethod: "password" as const,
    savePassword: f.savePassword,
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
  { app: string }
>(function RemoteHostsPanel({ app }, ref) {
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
  const [testingId, setTestingId] = useState<string | null>(null);
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

  const openEdit = (host: RemoteHost) => {
    setEditing(host);
    setForm({
      name: host.name,
      host: host.host,
      port: String(host.port),
      username: host.username,
      password: "",
      savePassword: host.savePassword,
      routeThroughLocalProxy: host.routeThroughLocalProxy,
      routeProxyApps: { ...host.routeProxyApps },
    });
    setFormOpen(true);
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
    if (!editing && !form.password.trim()) {
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
      await saveRemoteHost(host, form.password || undefined);
      toast.success(
        editing
          ? t("remote.updated", { defaultValue: "远程主机已更新" })
          : t("remote.added", { defaultValue: "远程主机已添加" }),
      );
      setFormOpen(false);
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

  const handleTest = async (host: RemoteHost) => {
    setTestingId(host.id);
    try {
      await testRemoteConnection(host.id, app);
      toast.success(
        t("remote.reachable", {
          defaultValue: `连通正常 ${host.name}`,
          name: host.name,
        }),
      );
    } catch (error) {
      console.error("Failed to test connection:", error);
      toast.error(t("remote.connectionFailed", { defaultValue: "连接失败" }), {
        description: String(error),
      });
    } finally {
      setTestingId(null);
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
    if (!form.host.trim() || !form.username.trim() || !form.password) {
      toast.error(
        t("remote.formRequired", {
          defaultValue: "请先填写主机地址、用户名和密码",
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
      );
      toast.success(
        t("remote.reachable", {
          defaultValue: "连通正常",
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

  const description = useMemo(
    () =>
      t("remote.description", {
        defaultValue:
          "通过 SSH 直连 Linux 服务器，统一管理远端各应用（Claude Code / Codex / Gemini / Grok Build / OpenCode 等）的配置与供应商切换，实现「一处配置、多机生效」。",
      }),
    [t],
  );

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
      <div className="space-y-4 px-6 pt-4">
        {/* 添加按钮在顶部导航右侧（remote.add），此处仅保留页面描述 */}
        <p className="text-sm text-muted-foreground">{description}</p>

        {/* 搜索 + 状态过滤 + 计数（对齐 MCP 管理面板） */}
        <div className="flex items-center gap-2">
          {/* -mb-4 抵消 ManagementListSearch 自带的 mb-4 下边距，使搜索框与状态下拉垂直对齐 */}
          <div className="-mb-4 flex-1">
            <ManagementListSearch
              value={searchQuery}
              onValueChange={setSearchQuery}
              placeholder={t("remote.searchPlaceholder", {
                defaultValue: "搜索主机名称 / 地址 / 用户名…",
              })}
              ariaLabel={t("remote.searchAriaLabel", {
                defaultValue: "搜索远程主机",
              })}
              clearLabel={t("common.clear", { defaultValue: "清除" })}
            />
          </div>
          {/* 状态过滤：全部 / 仅启用 / 仅禁用（带记录数，shadcn Select） */}
          <Select
            value={statusFilter}
            onValueChange={(v) =>
              setStatusFilter(v as "all" | "enabled" | "disabled")
            }
          >
            <SelectTrigger className="h-9 w-auto shrink-0 gap-1.5 border-border/40 px-2.5">
              <ListFilter className="h-3.5 w-3.5 text-muted-foreground" />
              {/* SelectValue 会自动显示选中项的文本，不要再额外放同名 span，否则文本重复 */}
              <SelectValue placeholder="状态" />
            </SelectTrigger>
            <SelectContent align="end" className="min-w-0 w-auto">
              <SelectItem value="all">
                {t("remote.filterAll", { defaultValue: "全部" })}
              </SelectItem>
              <SelectItem value="enabled">
                {t("remote.filterEnabled", { defaultValue: "仅启用" })}
              </SelectItem>
              <SelectItem value="disabled">
                {t("remote.filterDisabled", { defaultValue: "仅禁用" })}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        {/* 主机列表 */}
        {loading ? (
          <div className="flex items-center justify-center py-12">
            <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
          </div>
        ) : hosts.length === 0 ? (
          <div className="flex flex-col items-center justify-center rounded-xl border border-dashed py-12 text-center">
            <Server className="mb-3 h-10 w-10 text-muted-foreground/50" />
            <p className="text-sm text-muted-foreground">
              {t("remote.empty", { defaultValue: "还没有远程主机" })}
            </p>
            <p className="mt-1 text-xs text-muted-foreground/70">
              {t("remote.emptyHint", {
                defaultValue:
                  "点击「添加远程主机」，连接你的 GPU 服务器 / 云主机",
              })}
            </p>
          </div>
        ) : (
          <div>
            {filteredHosts.length === 0 ? (
              <div className="flex flex-col items-center justify-center rounded-xl border border-dashed py-12 text-center">
                <Search className="mb-3 h-8 w-8 text-muted-foreground/40" />
                <p className="text-sm text-muted-foreground">
                  {t("remote.searchEmpty", {
                    defaultValue: "没有匹配的主机",
                  })}
                </p>
              </div>
            ) : (
              <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
                {filteredHosts.map((host) => (
                  <div
                    key={host.id}
                    className={cn(
                      "group rounded-xl border bg-card p-4 shadow-sm transition-all duration-300 hover:border-border-active hover:shadow-sm",
                      editing?.id === host.id &&
                        "ring-2 ring-primary/60 border-primary/40",
                      // 禁用态：底色明显变暗 + 虚线边框 + 更强降透明，一眼可辨「这台已停用」
                      host.disabled && "border-dashed bg-muted/70 opacity-70",
                    )}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <p className="truncate font-medium">
                          {host.name}
                          {/* 状态 pill（仅状态展示，切换由右上角 Switch 负责）：启用=绿/禁用=红 */}
                          <span
                            className={cn(
                              "ml-2 inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium",
                              host.disabled
                                ? "border-red-500/30 bg-red-500/10 text-red-600"
                                : "border-border/60 bg-muted/60 text-muted-foreground",
                            )}
                          >
                            {host.disabled ? (
                              <>
                                <Ban className="h-3 w-3" />
                                {t("remote.statusDisabled", { defaultValue: "已禁用" })}
                              </>
                            ) : (
                              <>
                                <CircleDot className="h-3 w-3" />
                                {t("remote.statusEnabled", { defaultValue: "启用中" })}
                              </>
                            )}
                          </span>
                        </p>
                        <p className="mt-0.5 text-xs text-muted-foreground">
                          <span className="text-foreground/70">
                            {host.username}
                          </span>
                          <span className="text-muted-foreground/60">@</span>
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
                                className="inline-block font-mono text-blue-500 transition-colors hover:underline dark:text-blue-400 cursor-pointer"
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
                          <span className="text-muted-foreground/60">
                            :{host.port}
                          </span>
                        </p>
                      </div>
                      {/* 右上角禁用/启用开关：Tooltip 悬停提示。
                    注意：TooltipTrigger 用 asChild 直接包 Switch 会破坏 Switch 渲染（轨道变白），
                    因此改包一层中介 <span>，悬停 span 触发提示、Switch 自身正常渲染。 */}
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <span className="inline-flex">
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
                    </div>

                    {/* 测试结果通过 toast 提示，卡片不常驻状态行（仅按钮文案反映已测过） */}

                    <div className="mt-3 flex flex-wrap gap-1.5">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => void handleTest(host)}
                        disabled={host.disabled || testingId === host.id}
                      >
                        {testingId === host.id ? (
                          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <PlugZap className="mr-1 h-3.5 w-3.5" />
                        )}
                        {testingId === host.id
                          ? t("remote.testingShort", {
                              defaultValue: "测试中…",
                            })
                          : t("remote.test", { defaultValue: "测试连接" })}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => void openEnvDialog(host)}
                        disabled={host.disabled}
                      >
                        <ShieldAlert className="mr-1 h-3.5 w-3.5" />
                        {t("remote.env", { defaultValue: "环境变量" })}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="ml-auto h-7 w-7"
                        onClick={() => openEdit(host)}
                        disabled={host.disabled}
                        title={t("common.edit", { defaultValue: "编辑" })}
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-destructive hover:text-destructive"
                        onClick={() => setDeleteConfirm({ open: true, host })}
                        disabled={host.disabled}
                        title={t("common.delete", { defaultValue: "删除" })}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* 添加按钮已移至顶部工具栏（不随列表漂移） */}

        {/* 添加/编辑表单 - 全屏面板（对齐 MCP 的 FullScreenPanel 形态） */}
        <FullScreenPanel
          isOpen={formOpen}
          title={
            editing
              ? t("remote.editTitle", { defaultValue: "编辑远程主机" })
              : t("remote.addTitle", { defaultValue: "添加远程主机" })
          }
          onClose={() => setFormOpen(false)}
          footer={
            <>
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
                {testFormLoading || (editing && testingId === editing.id) ? (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" />
                ) : (
                  <PlugZap className="mr-1 h-4 w-4" />
                )}
                {testFormLoading || (editing && testingId === editing.id)
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
            </>
          }
        >
          <div className="flex flex-col gap-6">
            {/* 基础信息 + 连接信息 + 接管开关 + 提示：整体卡片（对齐 MCP 表单的 glass 卡片样式） */}
            <div className="glass rounded-xl p-6 border border-white/10 space-y-0">
              {/* 基础信息 */}
              <div className="space-y-3 pb-4">
                <h4 className="text-xs font-semibold uppercase tracking-wider text-foreground/60">
                  {t("remote.basicInfo", { defaultValue: "基础信息" })}
                </h4>
                <div className="space-y-2">
                  <Label
                    htmlFor="host-name"
                    className="flex items-center gap-1.5 text-foreground/80"
                  >
                    <Tag className="h-3.5 w-3.5 text-foreground/70" />
                    {t("remote.nameLabel", { defaultValue: "名称" })}
                    <span className="text-xs font-normal text-muted-foreground/60">
                      {t("remote.optional", { defaultValue: "选填" })}
                    </span>
                  </Label>
                  <Input
                    id="host-name"
                    ref={nameInputRef}
                    value={form.name}
                    onChange={(e) => setForm({ ...form, name: e.target.value })}
                    placeholder={t("remote.namePlaceholder", {
                      defaultValue: "GPU 训练机 / 生产服务器…",
                    })}
                  />
                  <p className="text-xs text-muted-foreground/70">
                    {t("remote.nameHint", {
                      defaultValue: "不填时将使用主机地址作为显示名",
                    })}
                  </p>
                </div>
              </div>

              {/* 分隔线 */}
              <div className="border-t border-border/50" />

              {/* 连接信息 */}
              <div className="space-y-3 pt-4 pb-4">
                <h4 className="text-xs font-semibold uppercase tracking-wider text-foreground/60">
                  {t("remote.connectionInfo", { defaultValue: "连接信息" })}
                </h4>
                <div className="space-y-2">
                  {/* 标签行 */}
                  <div className="flex items-center gap-3">
                    <Label
                      htmlFor="host-address"
                      className="flex min-w-0 flex-1 items-center gap-1.5 whitespace-nowrap text-foreground/80"
                    >
                      <Globe className="h-3.5 w-3.5 shrink-0 text-foreground/70" />
                      {t("remote.hostLabel", { defaultValue: "主机地址" })}
                      <span className="text-destructive">*</span>
                    </Label>
                    <Label
                      htmlFor="host-port"
                      className="w-24 shrink-0 text-center text-foreground/80"
                    >
                      {t("remote.portLabel", { defaultValue: "端口" })}
                    </Label>
                  </div>
                  {/* 输入框行 - 与标签行分离，保证两个输入框水平对齐 */}
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
                          className="flex h-3.5 w-4 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground"
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
                          className="flex h-3.5 w-4 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground"
                        >
                          <ChevronDown className="h-3 w-3" />
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
                <div className="space-y-2">
                  <Label
                    htmlFor="host-user"
                    className="flex items-center gap-1.5 text-foreground/80"
                  >
                    <User className="h-3.5 w-3.5 text-foreground/70" />
                    {t("remote.usernameLabel", { defaultValue: "用户名" })}
                    <span className="text-destructive">*</span>
                  </Label>
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
                  {form.username === "root" ? (
                    <p className="flex items-center gap-1 text-xs text-muted-foreground">
                      <Info className="h-3 w-3 shrink-0 text-foreground/50" />
                      {t("remote.rootWarning", {
                        defaultValue:
                          "不建议使用 root，推荐具有 sudo 权限的普通用户",
                      })}
                    </p>
                  ) : (
                    <p className="text-xs text-muted-foreground/70">
                      {t("remote.usernameHint", {
                        defaultValue: "建议使用具有 SSH 权限的普通用户",
                      })}
                    </p>
                  )}
                </div>
              </div>

              {/* 分隔线 */}
              <div className="border-t border-border/50" />

              {/* 认证信息 */}
              <div className="space-y-3 pt-4 pb-3">
                <h4 className="text-xs font-semibold uppercase tracking-wider text-foreground/60">
                  {t("remote.authInfo", { defaultValue: "认证信息" })}
                </h4>
                <div className="space-y-2">
                  <Label
                    htmlFor="host-pass"
                    className="flex items-center gap-1.5 text-foreground/80"
                  >
                    <Key className="h-3.5 w-3.5 text-foreground/70" />
                    {t("remote.passwordLabel", { defaultValue: "密码" })}
                    {!editing && <span className="text-destructive">*</span>}
                  </Label>
                  <div className="relative">
                    <Input
                      id="host-pass"
                      type={showPassword ? "text" : "password"}
                      value={form.password}
                      onChange={(e) =>
                        setForm({ ...form, password: e.target.value })
                      }
                      placeholder={
                        editing
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
                      className="absolute right-2 top-1/2 -translate-y-1/2 text-foreground/60 hover:text-foreground"
                    >
                      {showPassword ? (
                        <EyeOff className="h-4 w-4" />
                      ) : (
                        <Eye className="h-4 w-4" />
                      )}
                    </button>
                  </div>
                </div>
                {/* 保存密码 - 轻量行内样式 */}
                <div className="flex items-center justify-between py-2">
                  <div className="space-y-0.5">
                    <Label
                      className={`text-sm font-normal ${editing || form.password ? "text-foreground/80" : "text-muted-foreground/60"}`}
                    >
                      {t("remote.savePassword", {
                        defaultValue: "加密保存密码",
                      })}
                    </Label>
                    <p className="text-xs text-muted-foreground/70">
                      {t("remote.savePasswordHint", {
                        defaultValue:
                          "密码经 Windows DPAPI 加密，绑定当前账户，用于一键连接 / 切换",
                      })}
                    </p>
                  </div>
                  <Switch
                    checked={form.savePassword}
                    onCheckedChange={(v) =>
                      setForm({ ...form, savePassword: v })
                    }
                    // 新增时无密码不可存；编辑时始终可切（关掉 = 删除已保存密码）
                    disabled={!editing && !form.password}
                  />
                </div>
                {/* 远端接管 per-app - 轻量行内样式 */}
                <div className="space-y-2 py-2">
                  <div className="space-y-0.5">
                    <Label className="text-sm font-normal text-foreground/80">
                      {t("remote.routeThroughLocalProxy", {
                        defaultValue: "远端接管（宿主机）",
                      })}
                    </Label>
                    <p className="text-xs text-muted-foreground/70">
                      {t("remote.routeThroughLocalProxyHint", {
                        defaultValue:
                          "作用于该宿主机：开启后，对应应用的供应商会经 SSH 反向隧道使用你本机正在运行的代理；未运行时会自动启动本机代理。容器目标请到容器下单独配置",
                      })}
                    </p>
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    {(["claude", "codex", "gemini", "grokbuild"] as const).map(
                      (app) => (
                        <label
                          key={app}
                          className="flex items-center justify-between rounded-lg border border-border/60 bg-muted/30 px-3 py-1.5 cursor-pointer"
                        >
                          <span className="text-xs capitalize">{app}</span>
                          <Switch
                            checked={Boolean(form.routeProxyApps?.[app])}
                            onCheckedChange={(v) =>
                              setForm({
                                ...form,
                                routeProxyApps: {
                                  ...form.routeProxyApps,
                                  [app]: v,
                                },
                              })
                            }
                          />
                        </label>
                      ),
                    )}
                  </div>
                </div>
              </div>

              <Alert className="border-amber-200/40 bg-amber-50/40 dark:border-amber-800/20 dark:bg-amber-950/10">
                <Info className="h-4 w-4 text-amber-500/80" />
                <AlertDescription className="text-xs text-muted-foreground/80">
                  {t("remote.formHint", {
                    defaultValue:
                      "M1 阶段仅支持密码认证；密钥认证将在后续版本加入。",
                  })}
                </AlertDescription>
              </Alert>
            </div>
          </div>
        </FullScreenPanel>

        {/* 环境变量清理 */}
        <Dialog
          open={envDialog.open}
          onOpenChange={(open) =>
            !open && setEnvDialog({ open: false, host: null })
          }
        >
          <DialogContent className="max-w-md" zIndex="alert">
            <DialogHeader className="space-y-3 border-b-0 bg-transparent pb-0">
              <DialogTitle className="flex items-center gap-2 text-lg font-semibold">
                <ShieldAlert className="h-5 w-5 text-amber-500" />
                {t("remote.envTitle", { defaultValue: "冲突环境变量" })}
              </DialogTitle>
              <DialogDescription className="text-sm text-muted-foreground">
                {envDialog.host
                  ? t("remote.envSubtitle", {
                      defaultValue: "{{host}} · 清理后远端切换才生效",
                      host: envDialog.host.name,
                    })
                  : ""}
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-3 px-6">
              {envLoading ? (
                <div className="flex items-center justify-center py-8">
                  <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
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
                          className="rounded-lg border border-border bg-muted/30 px-3 py-2.5"
                        >
                          <p className="font-mono text-sm font-medium">
                            {c.varName}
                          </p>
                          <p
                            className="mt-0.5 break-all font-mono text-xs text-muted-foreground"
                            title={c.sourcePath}
                          >
                            {c.sourcePath}
                          </p>
                        </div>
                      ))}
                    </div>
                  </ScrollArea>
                  <p className="text-xs text-muted-foreground">
                    {t("remote.envCleanWarning", {
                      defaultValue:
                        "清理会移除这些变量（含系统级 .env 中的定义）",
                    })}
                  </p>
                </>
              )}
            </div>

            <DialogFooter className="flex gap-2 border-t-0 bg-transparent pt-2 sm:justify-end">
              <Button
                variant="outline"
                onClick={() => setEnvDialog({ open: false, host: null })}
              >
                {t("common.cancel", { defaultValue: "取消" })}
              </Button>
              <Button
                variant="destructive"
                disabled={
                  envLoading || envConflicts.length === 0 || envCleaning
                }
                onClick={() => void handleCleanEnv()}
              >
                {envCleaning && (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" />
                )}
                {t("remote.cleanEnv", { defaultValue: "清理冲突变量" })}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        {/* 删除确认 */}
        <ConfirmDialog
          isOpen={deleteConfirm.open}
          title={t("remote.deleteConfirmTitle", {
            defaultValue: "删除远程主机",
          })}
          message={t("remote.deleteConfirmMessage", {
            defaultValue: `确定要删除 "${deleteConfirm.host?.name}" 吗？已加密保存的密码也会一并清除。`,
            name: deleteConfirm.host?.name ?? "",
          })}
          confirmText={t("common.delete", { defaultValue: "删除" })}
          onConfirm={handleDelete}
          onCancel={() => setDeleteConfirm({ open: false, host: null })}
        />
      </div>
    </TooltipProvider>
  );
});
