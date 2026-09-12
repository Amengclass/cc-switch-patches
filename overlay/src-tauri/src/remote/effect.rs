//! 供应商切换完成后,返回给前端的「生效方式」报告。

use serde::{Deserialize, Serialize};

/// 切换后的生效说明。原则:必须明确告知「在哪、以何种方式生效」,
/// 而不是只报「写入成功」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReport {
    /// 目标标识:本机为 "local",远程为 host 名称。
    pub target: String,
    /// 当前生效的供应商名称。
    pub provider_name: String,
    /// 本次切换后生效的供应商 id（远程切换时返回，前端省一次 get_remote_current_provider 调用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_id: Option<String>,
    /// 已清理的冲突环境变量数量。
    pub conflicts_cleaned: usize,
    /// 面向用户的生效方式说明(每条一句,前端逐条展示)。
    pub notes: Vec<String>,
    /// 需要用户注意的警告（如「隧道未建立，按直连写入」），前端以 warning 样式展示。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}
