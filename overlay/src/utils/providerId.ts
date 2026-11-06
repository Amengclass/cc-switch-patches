import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import { generateUUID } from "@/utils/uuid";

/**
 * 计算新增供应商的 id —— 本机 `useAddProviderMutation` 与远端 add 路径共用，
 * 保证两端「同一个供应商 → 同一个 id 规则」：
 * - opencode / openclaw / hermes / pi / mcode：用 providerKey（OMO 用前缀 + uuid）
 * - 其余（claude / codex / gemini / grokbuild / claude-desktop）：uuid
 *
 * 注意：官方本机逻辑一字未动，这里只是把它的判定抽出来给远端复用。
 */
export function buildProviderId(
  appId: AppId,
  providerInput: Omit<Provider, "id"> & { providerKey?: string },
): string {
  if (
    appId === "opencode" ||
    appId === "openclaw" ||
    appId === "hermes" ||
    appId === "pi" ||
    appId === "mcode"
  ) {
    if (
      providerInput.category === "omo" ||
      providerInput.category === "omo-slim"
    ) {
      const prefix = providerInput.category === "omo" ? "omo" : "omo-slim";
      return `${prefix}-${generateUUID()}`;
    }
    if (!providerInput.providerKey) {
      throw new Error(`Provider key is required for ${appId}`);
    }
    return providerInput.providerKey;
  }
  return generateUUID();
}
