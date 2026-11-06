import { createContext, type ReactNode } from "react";

/**
 * 远端「目标选择器」（本机 / 服务器 / 容器）的注入点。
 *
 * 为什么用 context 而不是给 Sidebar 加 props：照官方自己的做法 ——
 * 官方的窗口按钮就是这么传的（`WindowControlsContext`，见
 * `components/shell/AppPageHeader.tsx`：App 渲染好节点、页头消费），
 * 这样**不必改官方 Sidebar 的 props 签名**（改签名会让官方文件多一处侵入）。
 *
 * 值：App 渲染好的选择器节点；远端功能关闭时为 `null`（侧栏整行不渲染）。
 *
 * 位置为什么选侧栏：Windows 上 `useAppWindowControls` 恒为 true，
 * 窗口是 `decorations: false`（自绘标题栏），最小化/最大化/关闭全靠官方页头右侧。
 * 任何插在页头**上面**的东西都会把窗口按钮顶下去 —— 所以目标选择器不能占主区高度。
 * 侧栏与主区左右并排，放这里零主区占用，且天然每页常驻。
 */
export const RemoteTargetContext = createContext<ReactNode>(null);
