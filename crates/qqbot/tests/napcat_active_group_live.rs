use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use qqbot::napcat::{
    MessageSegment, NapCatConnectionObserver, NapCatError, NapCatEvent, NapCatEventHandler,
    NapCatListener,
};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing_subscriber::EnvFilter;

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the ignored live test"))
}

#[derive(Clone)]
struct DebugAdapter {
    client: Client,
    base_url: String,
    credential: String,
    adapter_name: String,
}

impl DebugAdapter {
    async fn create(base_url: String, credential: String) -> TestResult<Self> {
        let client = Client::new();
        let response: Value = client
            .post(format!("{base_url}/api/Debug/create"))
            .bearer_auth(&credential)
            .json(&json!({}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let adapter_name = response
            .pointer("/data/adapterName")
            .and_then(Value::as_str)
            .ok_or("debug adapter name is missing")?
            .to_owned();
        Ok(Self {
            client,
            base_url,
            credential,
            adapter_name,
        })
    }

    async fn call_raw(&self, action: &str, params: Value) -> TestResult<Value> {
        let response: Value = self
            .client
            .post(format!(
                "{}/api/Debug/call/{}",
                self.base_url, self.adapter_name
            ))
            .bearer_auth(&self.credential)
            .json(&json!({"action": action, "params": params}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if response.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(format!("WebUI debug action {action} failed").into());
        }
        Ok(response["data"].clone())
    }

    async fn call(&self, action: &str, params: Value) -> TestResult<Value> {
        let response = self.call_raw(action, params).await?;
        if response.get("status").and_then(Value::as_str) != Some("ok")
            || response.get("retcode").and_then(Value::as_i64) != Some(0)
        {
            return Err(format!(
                "OneBot action {action} failed with retcode {:?}",
                response.get("retcode")
            )
            .into());
        }
        Ok(response["data"].clone())
    }

    async fn login_id(&self) -> TestResult<String> {
        value_id(&self.call("get_login_info", json!({})).await?["user_id"])
    }

    async fn send_group_segments(&self, group_id: &str, segments: Value) -> TestResult<String> {
        tracing::debug!(
            segment_count = segments.as_array().map_or(0, Vec::len),
            "发送测试群消息"
        );
        let data = self
            .call(
                "send_group_msg",
                json!({"group_id": group_id, "message": segments}),
            )
            .await?;
        value_id(&data["message_id"])
    }

    async fn delete_message(&self, message_id: &str) {
        let result = self
            .call_raw("delete_msg", json!({"message_id": message_id}))
            .await;
        tracing::debug!(
            success = result
                .as_ref()
                .ok()
                .and_then(|value| value.get("status"))
                .and_then(|value| value.as_str())
                == Some("ok"),
            "清理测试群消息"
        );
    }

    async fn history(&self, group_id: &str, message_seq: Option<&str>) -> TestResult<Vec<Value>> {
        let mut params = json!({
            "group_id": group_id,
            "count": 100,
            "reverse_order": false,
            "disable_get_url": true,
            "parse_mult_msg": true,
            "quick_reply": false,
            "reverseOrder": false
        });
        if let Some(message_seq) = message_seq {
            params["message_seq"] = Value::String(message_seq.to_owned());
        }
        Ok(self
            .call("get_group_msg_history", params)
            .await?
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

fn value_id(value: &Value) -> TestResult<String> {
    match value {
        Value::String(value) if !value.is_empty() => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err("expected a non-empty string or numeric ID".into()),
    }
}

struct RecordingHandler {
    sender: mpsc::UnboundedSender<NapCatEvent>,
}

#[async_trait]
impl NapCatEventHandler for RecordingHandler {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
        self.sender
            .send(event)
            .map_err(|_| NapCatError::Handler("active test event receiver closed".into()))
    }
}

struct ConnectedObserver {
    notify: Arc<Notify>,
}

#[async_trait]
impl NapCatConnectionObserver for ConnectedObserver {
    async fn connected(&self) -> Result<(), NapCatError> {
        self.notify.notify_one();
        Ok(())
    }
}

struct LiveListener {
    events: mpsc::UnboundedReceiver<NapCatEvent>,
    task: JoinHandle<Result<(), NapCatError>>,
}

impl LiveListener {
    async fn connect(ws_url: String, self_id: i64) -> TestResult<Self> {
        let (sender, events) = mpsc::unbounded_channel();
        let notify = Arc::new(Notify::new());
        let observer: Arc<dyn NapCatConnectionObserver> = Arc::new(ConnectedObserver {
            notify: Arc::clone(&notify),
        });
        let handler: Arc<dyn NapCatEventHandler> = Arc::new(RecordingHandler { sender });
        let listener =
            NapCatListener::new(ws_url, self_id, handler).with_connection_observer(observer);
        let task = tokio::spawn(async move { listener.run_forward().await });
        tokio::time::timeout(Duration::from_secs(10), notify.notified())
            .await
            .map_err(|_| "NapCat listener handshake timed out")?;
        Ok(Self { events, task })
    }

    async fn wait_group_message(&mut self, group_id: i64, marker: &str) -> TestResult<GroupView> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = self.events.recv().await.ok_or("event channel closed")?;
                if let NapCatEvent::GroupMessage(event) = event
                    && event.group_id == group_id
                    && event.normalized_text.contains(marker)
                {
                    let protocol_reply_id = onebot_reply_id(&event.raw_event);
                    return Ok(GroupView {
                        message_id: event.message_id,
                        user_id: event.user_id,
                        is_self: event.is_self,
                        at_bot: event.at_bot,
                        segments: event.segments,
                        sender_present: event.sender.is_some(),
                        protocol_reply_id,
                    });
                }
            }
        })
        .await
        .map_err(|_| format!("timed out waiting for group marker {marker}"))?
    }

    async fn reconnect(&mut self, ws_url: String, self_id: i64) -> TestResult<()> {
        self.task.abort();
        let replacement = Self::connect(ws_url, self_id).await?;
        *self = replacement;
        Ok(())
    }

    fn stop(&self) {
        self.task.abort();
    }
}

struct GroupView {
    message_id: String,
    user_id: i64,
    is_self: bool,
    at_bot: bool,
    segments: Vec<MessageSegment>,
    sender_present: bool,
    protocol_reply_id: Option<String>,
}

struct SentMessages {
    by_a: Vec<String>,
    by_b: Vec<String>,
}

impl SentMessages {
    fn new() -> Self {
        Self {
            by_a: Vec::new(),
            by_b: Vec::new(),
        }
    }

    async fn cleanup(&self, adapter_a: &DebugAdapter, adapter_b: &DebugAdapter) {
        for message_id in &self.by_a {
            adapter_a.delete_message(message_id).await;
        }
        for message_id in &self.by_b {
            adapter_b.delete_message(message_id).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "sends and recalls messages only in an explicitly approved QQ test group"]
async fn active_group_contract_covers_mentions_reply_self_recall_and_reconnect() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("qqbot=debug,napcat_active_group_live=debug")),
        )
        .with_test_writer()
        .try_init();

    let webui_a = required_env("NAPCAT_ACTIVE_WEBUI_A");
    let webui_b = required_env("NAPCAT_ACTIVE_WEBUI_B");
    let credential_a = required_env("NAPCAT_ACTIVE_CREDENTIAL_A");
    let credential_b = required_env("NAPCAT_ACTIVE_CREDENTIAL_B");
    let ws_a = required_env("NAPCAT_ACTIVE_WS_A");
    let ws_b = required_env("NAPCAT_ACTIVE_WS_B");
    let group_id = required_env("NAPCAT_ACTIVE_GROUP_ID");
    let group_id_number = group_id.parse::<i64>().unwrap();
    let run_id = required_env("NAPCAT_ACTIVE_RUN_ID");

    let adapter_a = DebugAdapter::create(webui_a, credential_a).await.unwrap();
    let adapter_b = DebugAdapter::create(webui_b, credential_b).await.unwrap();
    let self_a = adapter_a.login_id().await.unwrap();
    let self_b = adapter_b.login_id().await.unwrap();
    assert_ne!(self_a, self_b);
    let self_a_number = self_a.parse::<i64>().unwrap();
    let self_b_number = self_b.parse::<i64>().unwrap();

    let mut listener_a = LiveListener::connect(ws_a.clone(), self_a_number)
        .await
        .unwrap();
    let mut listener_b = LiveListener::connect(ws_b, self_b_number).await.unwrap();
    let (mut raw_recall_stream, _) = connect_async(&ws_a).await.unwrap();
    let mut sent = SentMessages::new();

    let result = run_active_contract(
        &adapter_a,
        &adapter_b,
        &mut listener_a,
        &mut listener_b,
        &mut raw_recall_stream,
        &mut sent,
        &group_id,
        group_id_number,
        &self_a,
        self_a_number,
        self_b_number,
        &run_id,
        &ws_a,
    )
    .await;

    sent.cleanup(&adapter_a, &adapter_b).await;
    listener_a.stop();
    listener_b.stop();
    let _ = raw_recall_stream.send(Message::Close(None)).await;
    result.unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn run_active_contract(
    adapter_a: &DebugAdapter,
    adapter_b: &DebugAdapter,
    listener_a: &mut LiveListener,
    listener_b: &mut LiveListener,
    raw_recall_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    sent: &mut SentMessages,
    group_id: &str,
    group_id_number: i64,
    self_a: &str,
    self_a_number: i64,
    self_b_number: i64,
    run_id: &str,
    ws_a: &str,
) -> TestResult<()> {
    let plain_marker = format!("SRSACT-{run_id}-plain");
    let plain_id = adapter_b
        .send_group_segments(
            group_id,
            json!([{"type":"text","data":{"text":plain_marker}}]),
        )
        .await?;
    sent.by_b.push(plain_id.clone());
    let plain_a = listener_a
        .wait_group_message(group_id_number, &plain_marker)
        .await?;
    let plain_b = listener_b
        .wait_group_message(group_id_number, &plain_marker)
        .await?;
    ensure(
        !plain_a.is_self && plain_b.is_self,
        "plain self classification",
    )?;
    ensure(plain_a.user_id == self_b_number, "plain sender identity")?;
    ensure(plain_a.sender_present, "plain sender metadata")?;

    let mention_marker = format!("SRSACT-{run_id}-mention");
    let mention_id = adapter_b
        .send_group_segments(
            group_id,
            json!([
                {"type":"at","data":{"qq":self_a}},
                {"type":"text","data":{"text":mention_marker}}
            ]),
        )
        .await?;
    sent.by_b.push(mention_id);
    let mention_a = listener_a
        .wait_group_message(group_id_number, &mention_marker)
        .await?;
    let _ = listener_b
        .wait_group_message(group_id_number, &mention_marker)
        .await?;
    ensure(mention_a.at_bot, "mention should target receiving account")?;
    ensure(
        mention_a
            .segments
            .iter()
            .any(|segment| matches!(segment, MessageSegment::At { qq } if qq == self_a)),
        "mention segment target",
    )?;

    let self_marker = format!("SRSACT-{run_id}-self");
    let self_id = adapter_a
        .send_group_segments(
            group_id,
            json!([{"type":"text","data":{"text":self_marker}}]),
        )
        .await?;
    sent.by_a.push(self_id.clone());
    let self_on_a = listener_a
        .wait_group_message(group_id_number, &self_marker)
        .await?;
    let self_on_b = listener_b
        .wait_group_message(group_id_number, &self_marker)
        .await?;
    ensure(
        self_on_a.is_self && !self_on_b.is_self,
        "self message classification",
    )?;
    ensure(self_on_a.user_id == self_a_number, "self sender identity")?;

    let reply_marker = format!("SRSACT-{run_id}-reply");
    // NapCat message IDs are scoped to the observing account. Account B must
    // quote the ID it received, not the ID returned to account A when sending.
    let reply_send_reference = self_on_b.message_id.clone();
    let reply_id = adapter_b
        .send_group_segments(
            group_id,
            json!([
                {"type":"reply","data":{"id":reply_send_reference}},
                {"type":"text","data":{"text":reply_marker}}
            ]),
        )
        .await?;
    sent.by_b.push(reply_id.clone());
    let reply_a = listener_a
        .wait_group_message(group_id_number, &reply_marker)
        .await?;
    let _ = listener_b
        .wait_group_message(group_id_number, &reply_marker)
        .await?;
    let parsed_reply_id = reply_a.segments.iter().find_map(|segment| match segment {
        MessageSegment::Reply { id } => Some(id.clone()),
        _ => None,
    });
    let reply_source_id = parsed_reply_id
        .as_deref()
        .or(reply_a.protocol_reply_id.as_deref())
        .ok_or("reply source is missing from parsed and raw OneBot segments")?;
    let reply_source = adapter_a
        .call("get_msg", json!({"message_id": reply_source_id}))
        .await?;
    tracing::debug!(
        matches_sender_reference = reply_source_id == reply_send_reference,
        matches_origin_send_result = reply_source_id == self_id,
        matches_receiver_event = reply_source_id == self_on_a.message_id,
        parsed_from_raw_message = parsed_reply_id.is_some(),
        present_in_protocol_segments = reply_a.protocol_reply_id.is_some(),
        "已按接收账号视角回查 Reply 来源"
    );
    ensure(
        message_contains_marker(&reply_source, &self_marker),
        "reply source should resolve to the original self message",
    )?;

    let reply_id_on_a = reply_a.message_id.clone();
    adapter_b
        .call("delete_msg", json!({"message_id": reply_id}))
        .await?;
    wait_for_group_recall(raw_recall_stream, group_id_number, &reply_id_on_a).await?;

    listener_a.reconnect(ws_a.to_owned(), self_a_number).await?;
    let reconnect_marker = format!("SRSACT-{run_id}-reconnect");
    let reconnect_id = adapter_b
        .send_group_segments(
            group_id,
            json!([{"type":"text","data":{"text":reconnect_marker}}]),
        )
        .await?;
    sent.by_b.push(reconnect_id);
    let reconnect_a = listener_a
        .wait_group_message(group_id_number, &reconnect_marker)
        .await?;
    ensure(!reconnect_a.is_self, "reconnected listener classification")?;

    let markers = [plain_marker, mention_marker, self_marker, reconnect_marker];
    let history_a = wait_for_history_markers(adapter_a, group_id, &markers).await?;
    let history_b = wait_for_history_markers(adapter_b, group_id, &markers).await?;
    let ids_a = history_ids(&history_a);
    let ids_b = history_ids(&history_b);
    ensure(!ids_a.is_empty() && !ids_b.is_empty(), "history IDs")?;
    tracing::debug!(
        account_a_count = history_a.len(),
        account_b_count = history_b.len(),
        cross_account_id_overlap = ids_a.intersection(&ids_b).count(),
        "主动测试历史页已收敛"
    );

    let sample_a = history_a
        .iter()
        .find(|message| message_contains_marker(message, &markers[0]))
        .ok_or("account A history sample missing")?;
    let sample_a_id = value_id(&sample_a["message_id"])?;
    let sample_a_seq = value_id(&sample_a["message_seq"])?;
    let detail = adapter_a
        .call("get_msg", json!({"message_id": sample_a_id}))
        .await?;
    ensure(
        value_id(&detail["message_id"])? == sample_a_id,
        "get_msg roundtrip",
    )?;
    ensure(
        adapter_a
            .history(group_id, Some(&sample_a_seq))
            .await?
            .iter()
            .any(|message| {
                value_id(&message["message_seq"]).ok().as_deref() == Some(&sample_a_seq)
            }),
        "history anchor must be inclusive",
    )?;

    ensure(
        plain_b.message_id == plain_id,
        "send result and listener message ID should match within one account view",
    )?;
    Ok(())
}

async fn wait_for_history_markers(
    adapter: &DebugAdapter,
    group_id: &str,
    markers: &[String],
) -> TestResult<Vec<Value>> {
    for attempt in 1..=10 {
        let history = adapter.history(group_id, None).await?;
        if markers.iter().all(|marker| {
            history
                .iter()
                .any(|message| message_contains_marker(message, marker))
        }) {
            tracing::debug!(
                attempt,
                history_count = history.len(),
                "历史页已包含全部测试标记"
            );
            return Ok(history);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("history did not contain every active test marker".into())
}

fn message_contains_marker(message: &Value, marker: &str) -> bool {
    message
        .get("raw_message")
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains(marker))
        || message
            .get("message")
            .is_some_and(|segments| segments.to_string().contains(marker))
}

fn onebot_reply_id(event: &Value) -> Option<String> {
    event
        .get("message")?
        .as_array()?
        .iter()
        .find(|segment| segment.get("type").and_then(Value::as_str) == Some("reply"))
        .and_then(|segment| segment.pointer("/data/id"))
        .and_then(|id| value_id(id).ok())
}

fn history_ids(history: &[Value]) -> BTreeSet<String> {
    history
        .iter()
        .filter_map(|message| value_id(&message["message_id"]).ok())
        .collect()
}

async fn wait_for_group_recall(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    group_id: i64,
    message_id: &str,
) -> TestResult<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = stream.next().await {
            let frame = frame?;
            let Message::Text(text) = frame else {
                continue;
            };
            let event: Value = serde_json::from_str(text.as_str())?;
            if event.get("post_type").and_then(Value::as_str) == Some("notice")
                && event.get("notice_type").and_then(Value::as_str) == Some("group_recall")
                && event.get("group_id").and_then(Value::as_i64) == Some(group_id)
                && value_id(&event["message_id"])? == message_id
            {
                return Ok::<(), Box<dyn Error + Send + Sync>>(());
            }
        }
        Err("raw recall WebSocket closed".into())
    })
    .await
    .map_err(|_| "timed out waiting for group recall notice")?
}

fn ensure(condition: bool, label: &str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(format!("active NapCat assertion failed: {label}").into())
    }
}
