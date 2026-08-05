use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::NapCatError;

/// Deserialize a JSON value that may be either a string or a number into `Option<String>`.
fn deserialize_opt_string_or_number<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    use serde::de;

    struct V;

    impl<'de> de::Visitor<'de> for V {
        type Value = Option<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, number, or null")
        }

        fn visit_unit<E: de::Error>(self) -> Result<Option<String>, E> {
            Ok(None)
        }

        fn visit_none<E: de::Error>(self) -> Result<Option<String>, E> {
            Ok(None)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<String>, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Option<String>, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Option<String>, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Option<String>, E> {
            Ok(Some(v.to_string()))
        }
    }

    d.deserialize_any(V)
}

fn deserialize_string_or_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(deserialize_opt_string_or_number(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
struct OneBotResponse {
    retcode: i32,
    data: Option<Value>,
}

/// NapCat 历史接口返回的发送者资料。
#[derive(Debug, Clone, Deserialize)]
pub struct HistorySender {
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    pub card: Option<String>,
    pub role: Option<String>,
}

/// 群/私聊历史共用的只读消息表示。
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryMessage {
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    pub self_id: String,
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    pub user_id: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    pub message_id: String,
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    pub message_seq: String,
    #[serde(default)]
    pub message_type: String,
    #[serde(default, deserialize_with = "deserialize_opt_string_or_number")]
    pub group_id: Option<String>,
    #[serde(default)]
    pub raw_message: String,
    #[serde(default)]
    pub message: Value,
    #[serde(default)]
    pub sender: Option<HistorySender>,
}

#[derive(Debug, Clone, Deserialize)]
struct HistoryPage {
    #[serde(default)]
    messages: Vec<HistoryMessage>,
}

/// Data returned by get_group_list.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupInfoData {
    pub group_id: i64,
    pub group_name: String,
    pub member_count: Option<i32>,
    pub max_member_count: Option<i32>,
}

/// Data returned by get_status.
#[derive(Debug, Clone, Deserialize)]
pub struct StatusData {
    pub online: Option<bool>,
    pub good: Option<bool>,
}

/// Data returned by get_version_info（B5 能力探测）。字段缺失时保留默认值。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionInfoData {
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub protocol_version: String,
    /// OneBot 实现类型，例如 "napcat"。
    #[serde(default)]
    pub impl_type: Option<String>,
}

/// Data returned by get_friend_list（B4 会话发现）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FriendInfoData {
    #[serde(default)]
    pub user_id: i64,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub remark: String,
}

/// Data returned by get_recent_contact（B4 会话发现）。字段对齐真实 NapCat 响应
/// （上游 Stapxs NapCat.Onebot.yaml 映射：peerUin/msgTime/chatType/peerName）。
///
/// 实机类型（评审 P0-1 实测确认）：
/// - `peerUin` 返回 JSON **String**（如 `"123456"`），不是数字。UIN 可超过 i32 范围，
///   且 napcat 以字符串形式传输。使用 string-or-number 反序列化以无精度损失保留 UIN。
/// - `msgTime` 返回 JSON **String**（如 `"1719421200"`），不是数字。同样使用
///   string-or-number 反序列化。
/// - `chatType` 返回整数（1=私聊，2=群聊）。
/// - `peerName` 返回字符串。
///
/// 字段缺失时保留默认值。UIN 保留为字符串避免精度损失（与 `HistoryMessage.user_id` 一致）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecentContactData {
    /// 会话对端 UIN（NapCat 原始字段 `peerUin`，实机为字符串）。
    #[serde(
        default,
        rename = "peerUin",
        deserialize_with = "deserialize_string_or_number"
    )]
    pub peer_uin: String,
    /// 最近消息时间（NapCat 原始字段 `msgTime`，实机为字符串，Unix 秒）。
    #[serde(
        default,
        rename = "msgTime",
        deserialize_with = "deserialize_string_or_number"
    )]
    pub msg_time: String,
    /// 会话类型（NapCat 原始字段 `chatType`，整数）：1=私聊，2=群聊。
    #[serde(default, rename = "chatType")]
    pub chat_type: i32,
    /// 对端名称（NapCat 原始字段 `peerName`）。
    #[serde(default, rename = "peerName")]
    pub peer_name: String,
}

/// 单次 NapCat HTTP 请求的超时上限。防止 NapCat 卡住时回补 Worker 或实时读取永久挂起。
/// 此值独立于回补租约（`lease_secs`）：租约覆盖整个运行生命周期，本超时只保护单次 HTTP 调用。
const HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 单次 HTTP 响应字节上限（评审第三轮 P1-1）。
/// 防止异常或恶意 NapCat 响应造成无界内存占用。正常 OneBot 响应远小于此值；
/// 超限响应在反序列化之前即被拒绝，不进入 `.json()` 无上限解析。
/// 评审第五轮：`pub` 使集成测试可构造恰好等于上限的响应验证边界条件。
pub const MAX_RESPONSE_BYTES: usize = 1_048_576; // 1 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadOnlyAction {
    VersionInfo,
    Status,
    FriendList,
    GroupList,
    RecentContact,
    GroupMessageHistory,
    FriendMessageHistory,
}

impl ReadOnlyAction {
    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::VersionInfo,
        Self::Status,
        Self::FriendList,
        Self::GroupList,
        Self::RecentContact,
        Self::GroupMessageHistory,
        Self::FriendMessageHistory,
    ];

    const fn as_path(self) -> &'static str {
        match self {
            Self::VersionInfo => "get_version_info",
            Self::Status => "get_status",
            Self::FriendList => "get_friend_list",
            Self::GroupList => "get_group_list",
            Self::RecentContact => "get_recent_contact",
            Self::GroupMessageHistory => "get_group_msg_history",
            Self::FriendMessageHistory => "get_friend_msg_history",
        }
    }
}

#[derive(Debug, Clone)]
struct HistoryQuery {
    scope_id: String,
    message_seq: Option<String>,
    count: u32,
    direction: HistoryReadDirection,
}

/// 历史查询的类型化读取方向。调用者不能直接传递 OneBot `reverseOrder` 布尔值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryReadDirection {
    TowardOlder,
    TowardNewer,
}

impl HistoryReadDirection {
    /// OneBot 协议字段映射仅在本模块内可见。
    const fn onebot_reverse_order(self) -> bool {
        match self {
            Self::TowardOlder => true,
            Self::TowardNewer => false,
        }
    }
}

impl HistoryQuery {
    fn new(
        scope_kind: &str,
        scope_id: impl Into<String>,
        message_seq: Option<String>,
        count: u32,
        direction: HistoryReadDirection,
    ) -> Result<Self, NapCatError> {
        let scope_id = scope_id.into();
        validate_history_query(scope_kind, &scope_id, count)?;
        Ok(Self {
            scope_id,
            message_seq,
            count,
            direction,
        })
    }
}

/// Validated parameters for one group history read.
#[derive(Debug, Clone)]
pub struct GroupHistoryQuery(HistoryQuery);

impl GroupHistoryQuery {
    pub fn new(
        group_id: impl Into<String>,
        message_seq: Option<String>,
        count: u32,
        direction: HistoryReadDirection,
    ) -> Result<Self, NapCatError> {
        HistoryQuery::new("group_id", group_id, message_seq, count, direction).map(Self)
    }

    pub fn message_seq(&self) -> Option<&str> {
        self.0.message_seq.as_deref()
    }

    pub fn direction(&self) -> HistoryReadDirection {
        self.0.direction
    }
}

/// Validated parameters for one friend history read.
#[derive(Debug, Clone)]
pub struct FriendHistoryQuery(HistoryQuery);

impl FriendHistoryQuery {
    pub fn new(
        user_id: impl Into<String>,
        message_seq: Option<String>,
        count: u32,
        direction: HistoryReadDirection,
    ) -> Result<Self, NapCatError> {
        HistoryQuery::new("user_id", user_id, message_seq, count, direction).map(Self)
    }

    pub fn message_seq(&self) -> Option<&str> {
        self.0.message_seq.as_deref()
    }

    pub fn direction(&self) -> HistoryReadDirection {
        self.0.direction
    }
}

#[async_trait]
pub trait NapCatCapabilityReadT: Send + Sync {
    async fn get_version_info(&self) -> Result<VersionInfoData, NapCatError>;
    async fn get_status(&self) -> Result<StatusData, NapCatError>;
}

#[async_trait]
pub trait NapCatDirectoryReadT: Send + Sync {
    async fn get_friend_list(&self) -> Result<Vec<FriendInfoData>, NapCatError>;
    async fn get_group_list(&self) -> Result<Vec<GroupInfoData>, NapCatError>;
    async fn get_recent_contact(&self) -> Result<Vec<RecentContactData>, NapCatError>;
}

#[async_trait]
pub trait NapCatHistoryReadT: Send + Sync {
    async fn get_group_msg_history(
        &self,
        query: &GroupHistoryQuery,
    ) -> Result<Vec<HistoryMessage>, NapCatError>;
    async fn get_friend_msg_history(
        &self,
        query: &FriendHistoryQuery,
    ) -> Result<Vec<HistoryMessage>, NapCatError>;
}

/// NapCat 只读 HTTP 客户端。个人秘书不得通过本类型执行发送、撤回或群管理操作。
pub struct NapCatReadOnlyClient {
    /// Base URL for OneBot HTTP API, e.g. "http://127.0.0.1:3000".
    base_url: String,
    http_client: reqwest::Client,
}

impl NapCatReadOnlyClient {
    pub fn new(base_url: String) -> Self {
        Self::with_timeout(base_url, HTTP_REQUEST_TIMEOUT)
    }

    fn with_timeout(base_url: String, timeout: std::time::Duration) -> Self {
        Self {
            base_url,
            http_client: reqwest::Client::builder()
                .timeout(timeout)
                .connect_timeout(timeout)
                .build()
                .expect("NapCat HTTP client uses a statically valid timeout configuration"),
        }
    }

    async fn request(
        &self,
        action: ReadOnlyAction,
        params: Value,
    ) -> Result<OneBotResponse, NapCatError> {
        let action_path = action.as_path();
        let url = format!("{}/{}", self.base_url, action_path);
        let resp = self
            .http_client
            .post(&url)
            .json(&params)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    NapCatError::Connection(format!("NapCat {action_path} request timed out"))
                } else {
                    NapCatError::Connection(format!("NapCat {action_path} request failed"))
                }
            })?;

        if !resp.status().is_success() {
            return Err(NapCatError::Api {
                action: action_path.into(),
                code: resp.status().as_u16() as i32,
                message: format!("HTTP {}", resp.status()),
            });
        }

        // 评审第四轮 P1：流式限流，防止异常/恶意响应造成无界内存分配。
        // resp.bytes().await 会先把整个响应缓冲到内存，再执行 len 检查，无法防止无界下载。
        // 改为：(1) 先检查可信的 Content-Length 作为快速拒绝路径；
        //       (2) 用 bytes_stream() 分块读取，每次追加前检查累计大小，超限立即停止。
        let content_length = resp.content_length();
        if let Some(len) = content_length
            && len > MAX_RESPONSE_BYTES as u64
        {
            return Err(NapCatError::Protocol(format!(
                "NapCat {} Content-Length {} exceeds {} bytes; rejected before reading body",
                action_path, len, MAX_RESPONSE_BYTES
            )));
        }

        let mut body_bytes = Vec::new();
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|_| {
                NapCatError::Protocol(format!("read {action_path} response failed"))
            })?;
            // 在追加当前 chunk 前检查累计大小，保证已分配内存严格有界。
            let next_len = body_bytes.len().saturating_add(chunk.len());
            if next_len > MAX_RESPONSE_BYTES {
                return Err(NapCatError::Protocol(format!(
                    "NapCat {} response exceeds {} bytes during stream read (got {}); aborted",
                    action_path, MAX_RESPONSE_BYTES, next_len
                )));
            }
            body_bytes.extend_from_slice(&chunk);
        }

        let body: OneBotResponse = serde_json::from_slice(&body_bytes).map_err(|error| {
            NapCatError::Protocol(format!(
                "parse {action_path} response failed at line {}, column {}",
                error.line(),
                error.column()
            ))
        })?;

        if body.retcode != 0 {
            // 错误响应 data 可能包含消息、联系人或其它敏感载荷。只记录其存在性，禁止
            // 把完整响应传播到日志或回补证据 JSON。
            let data_detail = body
                .data
                .as_ref()
                .map(|_| "; data_present=true")
                .unwrap_or_default();
            return Err(NapCatError::Api {
                action: action_path.into(),
                code: body.retcode,
                message: format!("OneBot action failed{data_detail}"),
            });
        }

        Ok(body)
    }
}

#[async_trait]
impl NapCatDirectoryReadT for NapCatReadOnlyClient {
    async fn get_group_list(&self) -> Result<Vec<GroupInfoData>, NapCatError> {
        let resp = self
            .request(ReadOnlyAction::GroupList, serde_json::json!({}))
            .await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(data)
            .map_err(|_| NapCatError::Protocol("parse group_list response data failed".into()))
    }

    /// 只读获取好友列表（B4 会话发现）。字段缺失时保留默认值。
    async fn get_friend_list(&self) -> Result<Vec<FriendInfoData>, NapCatError> {
        let resp = self
            .request(ReadOnlyAction::FriendList, serde_json::json!({}))
            .await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(data)
            .map_err(|_| NapCatError::Protocol("parse friend_list response data failed".into()))
    }

    /// 只读获取最近会话列表（B4 会话发现）。NapCat 专有接口，可能不存在。
    async fn get_recent_contact(&self) -> Result<Vec<RecentContactData>, NapCatError> {
        let resp = self
            .request(ReadOnlyAction::RecentContact, serde_json::json!({}))
            .await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(data)
            .map_err(|_| NapCatError::Protocol("parse recent_contact response data failed".into()))
    }
}

#[async_trait]
impl NapCatCapabilityReadT for NapCatReadOnlyClient {
    async fn get_status(&self) -> Result<StatusData, NapCatError> {
        let resp = self
            .request(ReadOnlyAction::Status, serde_json::json!({}))
            .await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|_| NapCatError::Protocol("parse status response data failed".into()))
    }

    async fn get_version_info(&self) -> Result<VersionInfoData, NapCatError> {
        let resp = self
            .request(ReadOnlyAction::VersionInfo, serde_json::json!({}))
            .await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|_| NapCatError::Protocol("parse version_info response data failed".into()))
    }
}

#[async_trait]
impl NapCatHistoryReadT for NapCatReadOnlyClient {
    async fn get_group_msg_history(
        &self,
        query: &GroupHistoryQuery,
    ) -> Result<Vec<HistoryMessage>, NapCatError> {
        let query = &query.0;
        let response = self
            .request(
                ReadOnlyAction::GroupMessageHistory,
                serde_json::json!({
                    "group_id": &query.scope_id,
                    "message_seq": query.message_seq.as_deref().unwrap_or("0"),
                    "count": query.count,
                    "reverseOrder": query.direction.onebot_reverse_order(),
                }),
            )
            .await?;
        parse_history_page(response, "group", query.direction)
    }

    async fn get_friend_msg_history(
        &self,
        query: &FriendHistoryQuery,
    ) -> Result<Vec<HistoryMessage>, NapCatError> {
        let query = &query.0;
        let response = self
            .request(
                ReadOnlyAction::FriendMessageHistory,
                serde_json::json!({
                    "user_id": &query.scope_id,
                    "message_seq": query.message_seq.as_deref().unwrap_or("0"),
                    "count": query.count,
                    "reverseOrder": query.direction.onebot_reverse_order(),
                }),
            )
            .await?;
        parse_history_page(response, "friend", query.direction)
    }
}

fn validate_history_query(field: &str, value: &str, count: u32) -> Result<(), NapCatError> {
    if value.trim().is_empty() {
        return Err(NapCatError::Protocol(format!(
            "history query requires a non-empty {field}"
        )));
    }
    if !(1..=100).contains(&count) {
        return Err(NapCatError::Protocol(
            "history query count must be between 1 and 100".into(),
        ));
    }
    Ok(())
}

fn parse_history_page(
    response: OneBotResponse,
    kind: &str,
    direction: HistoryReadDirection,
) -> Result<Vec<HistoryMessage>, NapCatError> {
    let mut page: HistoryPage = serde_json::from_value(response.data.unwrap_or_default())
        .map_err(|_| NapCatError::Protocol(format!("parse {kind} history response data failed")))?;
    // NapCat 4.18.x returns every history page oldest-to-newest. Keep that
    // protocol detail inside this adapter and expose messages in the requested direction.
    if direction == HistoryReadDirection::TowardOlder {
        page.messages.reverse();
    }
    Ok(page.messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_action_paths_are_exact_and_unique() {
        let paths = ReadOnlyAction::ALL.map(ReadOnlyAction::as_path);
        assert_eq!(
            paths,
            [
                "get_version_info",
                "get_status",
                "get_friend_list",
                "get_group_list",
                "get_recent_contact",
                "get_group_msg_history",
                "get_friend_msg_history",
            ]
        );

        let unique = paths.into_iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), ReadOnlyAction::ALL.len());
    }

    #[tokio::test]
    async fn request_timeout_returns_typed_error_without_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
        });
        let base_url = format!("http://{addr}");
        let client = NapCatReadOnlyClient::with_timeout(
            base_url.clone(),
            std::time::Duration::from_millis(20),
        );

        let error = client.get_status().await.unwrap_err();
        assert!(matches!(&error, NapCatError::Connection(_)));
        let detail = error.to_string();
        assert!(detail.contains("get_status"));
        assert!(detail.contains("timed out"));
        assert!(!detail.contains(&base_url));
    }

    #[test]
    fn history_ids_accept_numbers_and_strings_without_precision_loss() {
        let numeric: HistoryMessage = serde_json::from_value(serde_json::json!({
            "self_id": 10001,
            "user_id": "20002",
            "message_id": 1965026542,
            "message_seq": "1965026543"
        }))
        .unwrap();
        assert_eq!(numeric.self_id, "10001");
        assert_eq!(numeric.user_id, "20002");
        assert_eq!(numeric.message_id, "1965026542");
        assert_eq!(numeric.message_seq, "1965026543");
    }

    #[test]
    fn history_query_rejects_unbounded_or_identity_free_reads() {
        assert!(GroupHistoryQuery::new("", None, 20, HistoryReadDirection::TowardOlder).is_err());
        assert!(GroupHistoryQuery::new("1", None, 0, HistoryReadDirection::TowardOlder).is_err());
        assert!(
            FriendHistoryQuery::new("1", None, 101, HistoryReadDirection::TowardOlder).is_err()
        );
        assert!(
            GroupHistoryQuery::new(
                "1",
                Some("opaque".into()),
                100,
                HistoryReadDirection::TowardNewer,
            )
            .is_ok()
        );
    }

    #[test]
    fn history_page_is_normalized_into_the_requested_direction() {
        fn response() -> OneBotResponse {
            OneBotResponse {
                retcode: 0,
                data: Some(serde_json::json!({
                    "messages": [
                        {"message_id": "older", "message_seq": "10"},
                        {"message_id": "newer", "message_seq": "11"}
                    ]
                })),
            }
        }

        let toward_older =
            parse_history_page(response(), "group", HistoryReadDirection::TowardOlder).unwrap();
        assert_eq!(
            toward_older
                .iter()
                .map(|message| message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );

        let toward_newer =
            parse_history_page(response(), "group", HistoryReadDirection::TowardNewer).unwrap();
        assert_eq!(
            toward_newer
                .iter()
                .map(|message| message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["older", "newer"]
        );
    }

    // P0-1 实测：真实 NapCat `get_recent_contact` 返回的 peerUin/msgTime 为 JSON **字符串**，
    // 不是数字。原 i64 类型会导致 serde 解析失败，把真实可用接口误判为 unavailable。
    // UIN 保留为字符串以避免精度损失（QQ 号可达 10^10，超出 i32 范围）。
    #[test]
    fn recent_contact_parses_real_napcat_string_fields_without_precision_loss() {
        let payload = serde_json::json!([{
            "peerUin": "1234567890",
            "msgTime": "1719421200",
            "chatType": 2,
            "peerName": "测试群"
        }]);
        let contacts: Vec<RecentContactData> = serde_json::from_value(payload).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].peer_uin, "1234567890");
        assert_eq!(contacts[0].msg_time, "1719421200");
        assert_eq!(contacts[0].chat_type, 2);
        assert_eq!(contacts[0].peer_name, "测试群");
    }

    // 兼容性：某些实现可能返回数字形式的 peerUin/msgTime；仍能解析为字符串。
    #[test]
    fn recent_contact_also_accepts_numeric_fields() {
        let payload = serde_json::json!([{
            "peerUin": 9876543210u64,
            "msgTime": 1719421200i64,
            "chatType": 1,
            "peerName": ""
        }]);
        let contacts: Vec<RecentContactData> = serde_json::from_value(payload).unwrap();
        assert_eq!(contacts[0].peer_uin, "9876543210");
        assert_eq!(contacts[0].msg_time, "1719421200");
        assert_eq!(contacts[0].chat_type, 1);
    }
}
