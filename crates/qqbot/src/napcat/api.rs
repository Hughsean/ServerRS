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

/// OneBot API response.
#[derive(Debug, Clone, Deserialize)]
pub struct OneBotResponse {
    pub status: String,
    pub retcode: i32,
    pub data: Option<Value>,
    #[serde(default)]
    pub echo: Option<String>,
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

/// 群/私聊历史与 `get_msg` 共用的只读消息表示。
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

/// Data returned by get_login_info.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginInfoData {
    pub user_id: i64,
    pub nickname: String,
}

/// Data returned by get_group_info.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupInfoData {
    pub group_id: i64,
    pub group_name: String,
    pub member_count: Option<i32>,
    pub max_member_count: Option<i32>,
}

/// Data returned by get_group_member_info.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GroupMemberInfoData {
    pub group_id: i64,
    pub user_id: i64,
    pub nickname: String,
    pub card: Option<String>,
    pub role: Option<String>,
    pub title: Option<String>,
    pub join_time: Option<i64>,
    pub last_sent_time: Option<i64>,
}

/// Data returned by get_status.
#[derive(Debug, Clone, Deserialize)]
pub struct StatusData {
    pub online: Option<bool>,
    pub good: Option<bool>,
}

/// Data returned by get_version_info.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionInfoData {
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub protocol_version: Option<String>,
}

/// NapCat 只读 HTTP 客户端。个人秘书不得通过本类型执行发送、撤回或群管理操作。
pub struct NapCatApiClient {
    /// Base URL for OneBot HTTP API, e.g. "http://127.0.0.1:3000".
    base_url: String,
    /// Optional authorization token.
    token: Option<String>,
    http_client: reqwest::Client,
}

impl NapCatApiClient {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            base_url,
            token,
            http_client: reqwest::Client::new(),
        }
    }

    async fn call_api(&self, action: &str, params: Value) -> Result<OneBotResponse, NapCatError> {
        let url = format!("{}/{}", self.base_url, action);
        let mut req = self.http_client.post(&url).json(&params);
        if let Some(ref token) = self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| NapCatError::Connection(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(NapCatError::Api {
                action: action.into(),
                code: resp.status().as_u16() as i32,
                message: format!("HTTP {}", resp.status()),
            });
        }

        let body: OneBotResponse = resp
            .json()
            .await
            .map_err(|e| NapCatError::Protocol(format!("parse response failed: {e}")))?;

        if body.retcode != 0 {
            let data_detail = body
                .data
                .as_ref()
                .map(|data| format!("; data={data}"))
                .unwrap_or_default();
            return Err(NapCatError::Api {
                action: action.into(),
                code: body.retcode,
                message: format!("{}{}", body.status, data_detail),
            });
        }

        Ok(body)
    }

    pub async fn get_login_info(&self) -> Result<LoginInfoData, NapCatError> {
        let resp = self
            .call_api("get_login_info", serde_json::json!({}))
            .await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|e| NapCatError::Protocol(format!("parse login_info: {e}")))
    }

    pub async fn get_group_info(&self, group_id: i64) -> Result<GroupInfoData, NapCatError> {
        let params = serde_json::json!({"group_id": group_id, "no_cache": false});
        let resp = self.call_api("get_group_info", params).await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|e| NapCatError::Protocol(format!("parse group_info: {e}")))
    }

    pub async fn get_group_member_info(
        &self,
        group_id: i64,
        user_id: i64,
    ) -> Result<GroupMemberInfoData, NapCatError> {
        let params =
            serde_json::json!({"group_id": group_id, "user_id": user_id, "no_cache": false});
        let resp = self.call_api("get_group_member_info", params).await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|e| NapCatError::Protocol(format!("parse group_member_info: {e}")))
    }

    pub async fn get_group_list(&self) -> Result<Vec<GroupInfoData>, NapCatError> {
        let resp = self
            .call_api("get_group_list", serde_json::json!({}))
            .await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(data)
            .map_err(|e| NapCatError::Protocol(format!("parse group_list: {e}")))
    }

    pub async fn get_group_member_list(
        &self,
        group_id: i64,
    ) -> Result<Vec<GroupMemberInfoData>, NapCatError> {
        let params = serde_json::json!({"group_id": group_id, "no_cache": false});
        let resp = self.call_api("get_group_member_list", params).await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(data)
            .map_err(|e| NapCatError::Protocol(format!("parse group_member_list: {e}")))
    }

    pub async fn get_status(&self) -> Result<StatusData, NapCatError> {
        let resp = self.call_api("get_status", serde_json::json!({})).await?;
        serde_json::from_value(resp.data.unwrap_or_default())
            .map_err(|e| NapCatError::Protocol(format!("parse status: {e}")))
    }

    pub async fn get_group_msg_history(
        &self,
        group_id: &str,
        message_seq: Option<&str>,
        count: u32,
        reverse_order: bool,
    ) -> Result<Vec<HistoryMessage>, NapCatError> {
        validate_history_query("group_id", group_id, count)?;
        let response = self
            .call_api(
                "get_group_msg_history",
                serde_json::json!({
                    "group_id": group_id,
                    "message_seq": message_seq.unwrap_or("0"),
                    "count": count,
                    "reverseOrder": reverse_order,
                }),
            )
            .await?;
        parse_history_page(response, "group")
    }

    pub async fn get_friend_msg_history(
        &self,
        user_id: &str,
        message_seq: Option<&str>,
        count: u32,
        reverse_order: bool,
    ) -> Result<Vec<HistoryMessage>, NapCatError> {
        validate_history_query("user_id", user_id, count)?;
        let response = self
            .call_api(
                "get_friend_msg_history",
                serde_json::json!({
                    "user_id": user_id,
                    "message_seq": message_seq.unwrap_or("0"),
                    "count": count,
                    "reverseOrder": reverse_order,
                }),
            )
            .await?;
        parse_history_page(response, "friend")
    }

    pub async fn get_msg(&self, message_id: &str) -> Result<HistoryMessage, NapCatError> {
        if message_id.trim().is_empty() {
            return Err(NapCatError::Protocol(
                "get_msg requires a non-empty message_id".into(),
            ));
        }
        let response = self
            .call_api("get_msg", serde_json::json!({"message_id": message_id}))
            .await?;
        serde_json::from_value(response.data.unwrap_or_default())
            .map_err(|error| NapCatError::Protocol(format!("parse get_msg data: {error}")))
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
) -> Result<Vec<HistoryMessage>, NapCatError> {
    let page: HistoryPage = serde_json::from_value(response.data.unwrap_or_default())
        .map_err(|error| NapCatError::Protocol(format!("parse {kind} history data: {error}")))?;
    Ok(page.messages)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(validate_history_query("group_id", "", 20).is_err());
        assert!(validate_history_query("group_id", "1", 0).is_err());
        assert!(validate_history_query("group_id", "1", 101).is_err());
        assert!(validate_history_query("group_id", "1", 100).is_ok());
    }
}
