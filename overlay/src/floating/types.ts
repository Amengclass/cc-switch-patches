/** 完整用量数据项（与主窗口 UsageFooter 展示的 UsageData camelCase 对应） */
export interface FloatingUsageData {
  planName?: string | null;
  extra?: string | null;
  isValid?: boolean | null;
  invalidMessage?: string | null;
  total?: number | null;
  used?: number | null;
  remaining?: number | null;
  unit?: string | null;
}

/** 悬浮窗面板每行条目（与后端 floating::FloatingEntry 的 camelCase 对应） */
export interface FloatingEntry {
  appType: string;
  appLabel: string;
  providerName: string;
  /** 是否已设置供应商（未设置时不应用高亮色） */
  hasProvider: boolean;
  model: string | null;
  usageSummary: string | null;
  /** 最高利用率 0-100，供状态色 */
  worstPct: number | null;
  /** 完整用量数据（余额型：剩余/已用/单位；订阅型：tier 列表） */
  usage: FloatingUsageData[] | null;
  /** 用量查询时间戳（毫秒），供显示「刚刚 / x分钟前」 */
  queriedAt: number | null;
  /** 该 app 的路由纳管是否开启（与主窗口 takeoverStatus 同源），面板据此显示「纳管中」 */
  takeoverActive: boolean;
}

/** 利用率 → 状态色（与后端 emoji_for_utilization 阈值一致） */
export function statusColor(pct: number | null): string {
  if (pct == null) return "#94a3b8"; // 灰：无数据
  if (pct >= 90) return "#ef4444"; // 红
  if (pct >= 70) return "#f59e0b"; // 橙
  return "#22c55e"; // 绿
}

/**
 * 与主窗口供应商面板 UsageFooter 同款的分级规则（按已使用量 utilization 分档）：
 * 已过期（isValid=false）→ 红；used >= 90 → 红；used >= 70 → 橙；否则绿。
 * 返回 CSS 类名，深浅色主题由 floating.css 内的 .dark 覆盖。
 */
export function usageValueClass(d: FloatingUsageData): string {
  if (d.isValid === false) return "usage-value-danger";
  const used = d.used;
  if (used != null && isFinite(used)) {
    if (used >= 90) return "usage-value-danger";
    if (used >= 70) return "usage-value-warn";
  }
  // 余额型：remaining ≤ 0（透支）→ 红
  const rem = d.remaining;
  if (rem != null && isFinite(rem) && rem <= 0) return "usage-value-danger";
  return "usage-value-good";
}
