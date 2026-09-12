/** 远程主机认证方式（M1 仅支持密码，密钥二期实现） */
export type RemoteAuthMethod = "password" | "key";

/** 远程主机（对应后端 remote::RemoteHost，camelCase 序列化） */
export interface RemoteHost {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authMethod: RemoteAuthMethod;
  savePassword: boolean;
  routeThroughLocalProxy: boolean;
  /** per-app 远端接管开关（claude/codex/gemini/grokbuild）——宿主机目标 */
  routeProxyApps: Record<string, boolean>;
  /** per-container×app 远端接管开关：{"<容器名>":{"claude":true,...}}——容器目标各自独立 */
  routeProxyContainerApps?: Record<string, Record<string, boolean>>;
  /** 是否被软禁用（来自后端 remote_hosts.disabled）：禁用主机不进目标选择/操作，管理页可见可恢复 */
  disabled?: boolean;
  createdAt: number;
  updatedAt: number;
}

/** 测试连接的返回信息 */
export interface RemoteConnectionInfo {
  connected: boolean;
  home: string;
  /** 当前 app 的主配置文件（settings.json / config.toml / .env…）是否存在 */
  settingsExists: boolean;
  /** 当前 app 的 CLI 是否安装：true=已安装,false=未安装,null=检测失败 */
  cliInstalled: boolean | null;
}

/** 新增/编辑主机的表单（不含 id 的输入项） */
export interface RemoteHostDraft {
  name: string;
  host: string;
  port: number;
  username: string;
  authMethod: RemoteAuthMethod;
  savePassword: boolean;
  routeThroughLocalProxy: boolean;
  password?: string;
}

/** 供应商切换后的「生效方式」报告 */
export interface EffectReport {
  target: string;
  providerName: string;
  /** 本次切换后生效的供应商 id（远程切换时返回，前端省一次 getRemoteCurrentProvider 调用） */
  currentProviderId?: string | null;
  conflictsCleaned: number;
  notes: string[];
  /** 需要用户注意的警告（如「隧道未建立，按直连写入」），前端以 warning 样式展示 */
  warnings?: string[];
}

/** 远端 shell 配置中的冲突环境变量 */
export interface RemoteEnvConflict {
  varName: string;
  varValue: string;
  sourceType: string;
  sourcePath: string;
}

/** 远端会话文件信息 */
export interface RemoteSessionInfo {
  path: string;
  name: string;
  size: number;
  modified: number | null;
}
