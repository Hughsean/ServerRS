use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::qq_bot::GroupMessageGateway;

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

/// OneBot send_group_msg request.
#[derive(Debug, Clone, Serialize)]
pub struct SendGroupMsgRequest {
    pub group_id: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_escape: Option<bool>,
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

/// Data returned by send_group_msg.
#[derive(Debug, Clone, Deserialize)]
pub struct SendGroupMsgData {
    #[serde(default, deserialize_with = "deserialize_opt_string_or_number")]
    pub message_id: Option<String>,
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

/// NapCat API client for making HTTP calls to the OneBot endpoint.
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

    async fn call_api(
        &self,
        action: &str,
        params: Value,
    ) -> Result<OneBotResponse, super::super::QqBotError> {
        let url = format!("{}/{}", self.base_url, action);
        let mut req = self.http_client.post(&url).json(&params);
        if let Some(ref token) = self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req.send().await.map_err(|e| {
            super::super::QqBotError::Connection(format!("HTTP request failed: {e}"))
        })?;

        if !resp.status().is_success() {
            return Err(super::super::QqBotError::Api {
                action: action.into(),
                code: resp.status().as_u16() as i32,
                message: format!("HTTP {}", resp.status()),
            });
        }

        let body: OneBotResponse = resp.json().await.map_err(|e| {
            super::super::QqBotError::MessageProcessing(format!("parse response failed: {e}"))
        })?;

        if body.retcode != 0 {
            let data_detail = body
                .data
                .as_ref()
                .map(|data| format!("; data={data}"))
                .unwrap_or_default();
            return Err(super::super::QqBotError::Api {
                action: action.into(),
                code: body.retcode,
                message: format!("{}{}", body.status, data_detail),
            });
        }

        Ok(body)
    }

    /// Poke a user in a group.
    ///
    /// Calls the OneBot `group_poke` action.
    /// Returns Ok(()) on success (retcode == 0).
    pub async fn group_poke(
        &self,
        group_id: i64,
        user_id: i64,
    ) -> Result<(), super::super::QqBotError> {
        let params = serde_json::json!({
            "group_id": group_id,
            "user_id": user_id,
        });
        self.call_api("group_poke", params).await?;
        Ok(())
    }

    pub async fn send_group_msg(
        &self,
        group_id: i64,
        message: &str,
    ) -> Result<SendGroupMsgData, super::super::QqBotError> {
        let params = serde_json::json!({
            "group_id": group_id,
            "message": message,
            "auto_escape": false,
        });
        let resp = self.call_api("send_group_msg", params).await?;
        Ok(
            serde_json::from_value(resp.data.unwrap_or_default()).map_err(|e| {
                super::super::QqBotError::MessageProcessing(format!(
                    "parse send_group_msg data: {e}"
                ))
            })?,
        )
    }

    pub async fn get_login_info(&self) -> Result<LoginInfoData, super::super::QqBotError> {
        let resp = self
            .call_api("get_login_info", serde_json::json!({}))
            .await?;
        Ok(
            serde_json::from_value(resp.data.unwrap_or_default()).map_err(|e| {
                super::super::QqBotError::MessageProcessing(format!("parse login_info: {e}"))
            })?,
        )
    }

    pub async fn get_group_info(
        &self,
        group_id: i64,
    ) -> Result<GroupInfoData, super::super::QqBotError> {
        let params = serde_json::json!({"group_id": group_id, "no_cache": false});
        let resp = self.call_api("get_group_info", params).await?;
        Ok(
            serde_json::from_value(resp.data.unwrap_or_default()).map_err(|e| {
                super::super::QqBotError::MessageProcessing(format!("parse group_info: {e}"))
            })?,
        )
    }

    pub async fn get_group_member_info(
        &self,
        group_id: i64,
        user_id: i64,
    ) -> Result<GroupMemberInfoData, super::super::QqBotError> {
        let params =
            serde_json::json!({"group_id": group_id, "user_id": user_id, "no_cache": false});
        let resp = self.call_api("get_group_member_info", params).await?;
        Ok(
            serde_json::from_value(resp.data.unwrap_or_default()).map_err(|e| {
                super::super::QqBotError::MessageProcessing(format!("parse group_member_info: {e}"))
            })?,
        )
    }

    pub async fn get_group_list(&self) -> Result<Vec<GroupInfoData>, super::super::QqBotError> {
        let resp = self
            .call_api("get_group_list", serde_json::json!({}))
            .await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        Ok(serde_json::from_value(data).map_err(|e| {
            super::super::QqBotError::MessageProcessing(format!("parse group_list: {e}"))
        })?)
    }

    pub async fn get_group_member_list(
        &self,
        group_id: i64,
    ) -> Result<Vec<GroupMemberInfoData>, super::super::QqBotError> {
        let params = serde_json::json!({"group_id": group_id, "no_cache": false});
        let resp = self.call_api("get_group_member_list", params).await?;
        let data = resp.data.unwrap_or(serde_json::Value::Array(vec![]));
        Ok(serde_json::from_value(data).map_err(|e| {
            super::super::QqBotError::MessageProcessing(format!("parse group_member_list: {e}"))
        })?)
    }

    pub async fn get_status(&self) -> Result<StatusData, super::super::QqBotError> {
        let resp = self.call_api("get_status", serde_json::json!({})).await?;
        Ok(
            serde_json::from_value(resp.data.unwrap_or_default()).map_err(|e| {
                super::super::QqBotError::MessageProcessing(format!("parse status: {e}"))
            })?,
        )
    }
}

#[async_trait::async_trait]
impl GroupMessageGateway for NapCatApiClient {
    async fn send_group_msg(
        &self,
        group_id: i64,
        message: &str,
    ) -> Result<Option<String>, super::super::QqBotError> {
        Ok(NapCatApiClient::send_group_msg(self, group_id, message)
            .await?
            .message_id)
    }

    async fn group_poke(
        &self,
        group_id: i64,
        user_id: i64,
    ) -> Result<(), super::super::QqBotError> {
        NapCatApiClient::group_poke(self, group_id, user_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::SendGroupMsgData;

    #[test]
    fn send_group_msg_message_id_accepts_number_string_and_null() {
        let numeric: SendGroupMsgData =
            serde_json::from_value(serde_json::json!({ "message_id": 1965026542 })).unwrap();
        assert_eq!(numeric.message_id.as_deref(), Some("1965026542"));

        let string: SendGroupMsgData =
            serde_json::from_value(serde_json::json!({ "message_id": "1965026542" })).unwrap();
        assert_eq!(string.message_id.as_deref(), Some("1965026542"));

        let null: SendGroupMsgData =
            serde_json::from_value(serde_json::json!({ "message_id": null })).unwrap();
        assert_eq!(null.message_id, None);
    }
}
