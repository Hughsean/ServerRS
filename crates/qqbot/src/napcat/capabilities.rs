//! NapCat 能力与版本探测（B5）。
//!
//! 只读探测 NapCat/OneBot 实现能力，建立类型化 capability snapshot：
//! - 实现/版本（`get_version_info`）
//! - Heartbeat 是否可用
//! - 结构化消息是否可用
//! - recent contact / friend / group / history API 可用性
//! - forward / file / record 元数据读取能力
//!
//! 关键规则：
//! - API 不存在时按功能降级，不得导致所有入站失效。
//! - **能力探测不得阻塞实时 WebSocket 入站**（评审 P0-2）：探测并发执行且受严格整体
//!   超时约束；超时后未完成的能力标记为 Unknown，立即让出执行权。
//! - **探测不拉取完整列表**（评审第三轮 P1-1）：探测只调用轻量接口 `get_version_info` 与
//!   `get_status` 确认实现与在线状态。`get_recent_contact`/`get_friend_list`/`get_group_list`
//!   会下载并反序列化完整数组，大账号或异常响应会造成不必要的内存占用；这些列表能力
//!   延迟到 B4 会话同步流程使用时验证，探测阶段标记为"需使用时验证"。
//! - 关键能力缺失必须有结构化 warning。
//! - 不得通过动态字符串开放任意 NapCat Action（探测只用固定只读调用）。
//! - 能力探测不得调用任何写接口。

use std::time::Duration;

use super::api::NapCatApiClient;
use super::event::MessageSegment;

/// 探测整体超时上限（评审 P0-2：3~5 秒，取 5 秒）。
/// 超时后未完成的能力标记为 Unknown，绝不拖延实时 WebSocket 入站。
const PROBE_TOTAL_TIMEOUT_SECS: u64 = 5;

/// 单个只读 API 的可用性结果。
#[derive(Debug, Clone)]
pub enum ApiAvailability {
    /// API 可用。
    Available,
    /// API 不存在或不可用（已确认降级，不致命）。
    Unavailable(String),
    /// 探测超时或被取消，无法判定可用性（评审 P0-2）。
    /// 与 Unavailable 区分：超时不代表接口不可用，只代表本次探测未在时限内完成。
    Unknown,
    /// 探测阶段未验证，延迟到业务首次使用时确认（评审第四轮 P2）。
    /// 与 Unavailable 区分：Deferred 表示"尚未验证"，而非"已确认不支持"。
    /// 列表 API（friend/group/recent_contact）因大账号内存占用风险不在探测时拉取，
    /// 延迟到 B4 会话同步流程首次调用时确认可达性。
    Deferred(String),
}

impl ApiAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, ApiAvailability::Available)
    }

    /// 是否已确认不可用（Unavailable）。Deferred/Unknown 不算已确认不可用。
    pub fn is_unavailable(&self) -> bool {
        matches!(self, ApiAvailability::Unavailable(_))
    }
}

/// 类型化能力快照。
#[derive(Debug, Clone)]
pub struct CapabilitySnapshot {
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub protocol_version: Option<String>,
    pub impl_type: Option<String>,
    /// Heartbeat meta_event 是否可用（探测时无法确定则标记未知）。
    pub heartbeat_supported: ApiAvailability,
    /// 结构化 message 数组是否可用。
    pub structured_message_supported: ApiAvailability,
    pub recent_contact_api: ApiAvailability,
    pub friend_list_api: ApiAvailability,
    pub group_list_api: ApiAvailability,
    pub history_api: ApiAvailability,
    /// forward / file / record 元数据读取能力（保守标记为未知，需实机验证）。
    pub forward_file_record_metadata: ApiAvailability,
    /// 是否在线（get_status.online）。
    pub online: Option<bool>,
    /// 探测是否在整体超时前完成。`false` 表示有部分能力被标记为 Unknown。
    pub probe_completed: bool,
}

impl CapabilitySnapshot {
    /// 探测能力。评审第三轮 P1-1：只调用轻量接口 `get_version_info` 与 `get_status`，
    /// 不拉取完整好友/群/最近会话列表（大账号内存占用风险）。
    ///
    /// - 两个轻量 API 并发执行，受整体超时约束。
    /// - 列表 API（friend/group/recent_contact）标记为"需 B4 使用时验证"，
    ///   在 B4 会话同步流程首次调用时确认可达性，不在此处下载完整数组。
    /// - 超时后未完成的探测分支标记为 [`ApiAvailability::Unknown`]，立即返回。
    /// - 探测结果仅用于日志与未来健康状态（B7），不影响实时入站路径。
    pub async fn probe(client: &NapCatApiClient) -> Self {
        let probe_fut = async {
            let (version_r, status_r) = tokio::join!(probe_version(client), probe_status(client),);
            (version_r, status_r)
        };

        // 整体超时：超过 PROBE_TOTAL_TIMEOUT_SECS 后立即放弃未完成的分支。
        let (version_r, status_r) =
            match tokio::time::timeout(Duration::from_secs(PROBE_TOTAL_TIMEOUT_SECS), probe_fut)
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        timeout_secs = PROBE_TOTAL_TIMEOUT_SECS,
                        "NapCat 能力探测整体超时，未完成的能力标记为 Unknown"
                    );
                    return Self::partial_unknown();
                }
            };

        let (app_name, app_version, protocol_version, impl_type, heartbeat_supported) = version_r;
        let online = status_r;

        // 结构化消息能力需在实机事件中验证（收到结构化 message 数组即为可用）。
        let structured_message_supported =
            ApiAvailability::Unavailable("requires runtime event verification".into());
        let forward_file_record_metadata =
            ApiAvailability::Unavailable("requires runtime event verification".into());
        let history_api = ApiAvailability::Unavailable(
            "history api requires conversation-scoped verification".into(),
        );

        // 评审第三轮 P1-1 + 第四轮 P2：列表 API 不在探测时拉取，标记为 Deferred
        // （延迟到 B4 首次使用时验证），而非 Unavailable（已确认不支持）。
        // get_recent_contact/get_friend_list/get_group_list 会下载完整数组，
        // 大账号或异常响应造成不必要内存占用。B4 会话同步首次调用时确认可达性。
        let recent_contact_api =
            ApiAvailability::Deferred("deferred to B4 session sync verification".into());
        let friend_list_api =
            ApiAvailability::Deferred("deferred to B4 session sync verification".into());
        let group_list_api =
            ApiAvailability::Deferred("deferred to B4 session sync verification".into());

        if !heartbeat_supported.is_available() {
            tracing::warn!(
                heartbeat = heartbeat_supported.is_available(),
                "NapCat 部分只读能力不可用，将按功能降级"
            );
        }

        Self {
            app_name,
            app_version,
            protocol_version,
            impl_type,
            heartbeat_supported,
            structured_message_supported,
            recent_contact_api,
            friend_list_api,
            group_list_api,
            history_api,
            forward_file_record_metadata,
            online,
            probe_completed: true,
        }
    }

    /// 探测整体超时时的快照：所有能力标记为 Unknown，`probe_completed = false`。
    fn partial_unknown() -> Self {
        Self {
            app_name: None,
            app_version: None,
            protocol_version: None,
            impl_type: None,
            heartbeat_supported: ApiAvailability::Unknown,
            structured_message_supported: ApiAvailability::Unknown,
            recent_contact_api: ApiAvailability::Unknown,
            friend_list_api: ApiAvailability::Unknown,
            group_list_api: ApiAvailability::Unknown,
            history_api: ApiAvailability::Unknown,
            forward_file_record_metadata: ApiAvailability::Unknown,
            online: None,
            probe_completed: false,
        }
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

/// 探测 version_info：返回版本信息与 Heartbeat 能力（保守未知，需运行时验证）。
async fn probe_version(
    client: &NapCatApiClient,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    ApiAvailability,
) {
    match client.get_version_info().await {
        Ok(v) => (
            non_empty(v.app_name),
            non_empty(v.app_version),
            non_empty(v.protocol_version),
            v.impl_type,
            ApiAvailability::Unavailable(
                "heartbeat requires runtime meta_event verification".into(),
            ),
        ),
        Err(e) => (
            None,
            None,
            None,
            None,
            ApiAvailability::Unavailable(format!("version_info: {e}")),
        ),
    }
}

/// 探测 get_status，返回 online 字段。
async fn probe_status(client: &NapCatApiClient) -> Option<bool> {
    match client.get_status().await {
        Ok(s) => s.online,
        Err(_) => None,
    }
}

/// 判定段是否包含结构化富消息引用（forward/file/record/rich/image/video）。
/// 用于在运行时事件中验证结构化消息与元数据读取能力。
pub fn segment_has_rich_reference(segment: &MessageSegment) -> bool {
    matches!(
        segment,
        MessageSegment::Forward { .. }
            | MessageSegment::File { .. }
            | MessageSegment::Record { .. }
            | MessageSegment::Rich { .. }
            | MessageSegment::Image { .. }
            | MessageSegment::Video { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_has_rich_reference_detects_media_and_forward() {
        assert!(segment_has_rich_reference(&MessageSegment::Forward {
            id: "f".into()
        }));
        assert!(segment_has_rich_reference(&MessageSegment::File {
            file: "f".into(),
            name: None,
            size: None
        }));
        assert!(segment_has_rich_reference(&MessageSegment::Rich {
            kind: crate::napcat::RichKind::Json,
            data: None,
            summary: None
        }));
        assert!(!segment_has_rich_reference(&MessageSegment::Text {
            content: "plain".into()
        }));
    }

    #[test]
    fn api_availability_predicates_distinguish_four_states() {
        // 评审第四轮 P2：Available/Unavailable/Unknown/Deferred 四态明确区分。
        assert!(ApiAvailability::Available.is_available());
        assert!(!ApiAvailability::Unavailable("x".into()).is_available());
        assert!(!ApiAvailability::Unknown.is_available());
        assert!(!ApiAvailability::Deferred("x".into()).is_available());

        // is_unavailable 只对 Unavailable 为真；Deferred/Unknown 不算已确认不可用。
        assert!(ApiAvailability::Unavailable("x".into()).is_unavailable());
        assert!(!ApiAvailability::Deferred("x".into()).is_unavailable());
        assert!(!ApiAvailability::Unknown.is_unavailable());
        assert!(!ApiAvailability::Available.is_unavailable());
    }

    #[test]
    fn partial_unknown_marks_all_as_unknown_and_incomplete() {
        let snap = CapabilitySnapshot::partial_unknown();
        assert!(!snap.probe_completed);
        assert!(matches!(snap.heartbeat_supported, ApiAvailability::Unknown));
        assert!(matches!(snap.recent_contact_api, ApiAvailability::Unknown));
        assert!(matches!(snap.friend_list_api, ApiAvailability::Unknown));
        assert!(matches!(snap.group_list_api, ApiAvailability::Unknown));
    }
}
