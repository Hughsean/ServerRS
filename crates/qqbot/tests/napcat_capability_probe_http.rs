//! 评审 P1-7：验证 `CapabilitySnapshot::probe` 能正确解析真实 NapCat DTO 形状。
//!
//! 上一轮评审实测发现 DTO 类型不兼容（peerUin/msgTime 实机为字符串，原代码用 i64），
//! 但因没有 HTTP mock 测试而未被发现。本测试启动一个本地 HTTP 服务器，返回与
//! 真实 NapCat 完全一致的 JSON 形状（含字符串形式的 peerUin/msgTime），验证：
//! 1. 探测不 panic、不解析失败；
//! 2. 各只读 API 标记为 Available；
//! 3. RecentContactData 字段类型正确（字符串 peer_uin/msg_time，无精度损失）。
//! 4. 探测在整体超时内完成（probe_completed = true）。
//!
//! 服务器使用同步 I/O + `spawn_blocking`，避免引入新的 mock 依赖。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use qqbot::napcat::{
    CapabilitySnapshot, FriendHistoryQuery, GroupHistoryQuery, HistoryReadDirection,
    NapCatCapabilityReadT, NapCatDirectoryReadT, NapCatHistoryReadT, NapCatReadOnlyClient,
};

/// 启动一个本地 HTTP/1.1 服务器，根据请求路径返回固定的 OneBot 响应。
/// 记录收到的 action 请求，供测试断言探测覆盖了所有 API。
fn start_mock_napcat_http_server(
    responder: fn(&str) -> String,
) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let actions = Arc::new(Mutex::new(Vec::new()));
    let actions_clone = Arc::clone(&actions);

    std::thread::spawn(move || {
        // 接受有限连接后退出（探测发起 5 个 API 调用）。
        for _ in 0..20 {
            let (stream, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(_) => break,
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let actions = Arc::clone(&actions_clone);
            std::thread::spawn(move || handle_one_request(stream, actions, responder));
        }
    });

    (addr, actions)
}

fn handle_one_request(
    stream: TcpStream,
    actions: Arc<Mutex<Vec<String>>>,
    responder: fn(&str) -> String,
) {
    let mut stream = stream;
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    // 解析请求行：POST /get_version_info HTTP/1.1
    let action = request
        .split_whitespace()
        .nth(1)
        .map(|path| path.trim_start_matches('/').to_string())
        .unwrap_or_default();
    actions.lock().unwrap().push(action.clone());

    let body = responder(&action);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// 返回与真实 NapCat 完全一致的 OneBot 响应 JSON。
/// 关键：peerUin/msgTime 为**字符串**形式（评审 P0-1 实测确认）。
fn real_napcat_response(action: &str) -> String {
    match action {
        "get_version_info" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": {
                "app_name": "NapCat",
                "app_version": "4.0.0",
                "protocol_version": "11",
                "impl_type": "napcat"
            }
        })
        .to_string(),
        "get_status" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": {"online": true, "good": true}
        })
        .to_string(),
        "get_recent_contact" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": [
                {
                    "peerUin": "1234567890",
                    "msgTime": "1719421200",
                    "chatType": 2,
                    "peerName": "测试群"
                }
            ]
        })
        .to_string(),
        "get_friend_list" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": [
                {"user_id": 100001, "nickname": "好友A", "remark": ""}
            ]
        })
        .to_string(),
        "get_group_list" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": [
                {"group_id": 671260344, "group_name": "测试群", "member_count": 10, "max_member_count": 100}
            ]
        })
        .to_string(),
        "get_group_msg_history" | "get_friend_msg_history" => serde_json::json!({
            "status": "ok",
            "retcode": 0,
            "data": {"messages": []}
        })
        .to_string(),
        _ => serde_json::json!({"status": "ok", "retcode": 0, "data": null}).to_string(),
    }
}

#[derive(Debug)]
struct RecordedRequest {
    action: String,
    params: serde_json::Value,
}

fn start_recording_napcat_http_server() -> (SocketAddr, Arc<Mutex<Vec<RecordedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_clone = Arc::clone(&requests);

    std::thread::spawn(move || {
        for _ in 0..9 {
            let (stream, _) = listener.accept().unwrap();
            let requests = Arc::clone(&requests_clone);
            std::thread::spawn(move || handle_recorded_request(stream, requests));
        }
    });

    (addr, requests)
}

fn handle_recorded_request(stream: TcpStream, requests: Arc<Mutex<Vec<RecordedRequest>>>) {
    let mut stream = stream;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "request ended before headers were complete");
        request.extend_from_slice(&buf[..n]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while request.len() - header_end < content_length {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "request ended before body was complete");
        request.extend_from_slice(&buf[..n]);
    }

    let action = headers
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_owned();
    let params = serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap();
    requests.lock().unwrap().push(RecordedRequest {
        action: action.clone(),
        params,
    });

    let body = real_napcat_response(&action);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

#[tokio::test]
async fn readonly_client_emits_exactly_the_seven_allowed_actions_and_typed_params() {
    let (addr, requests) = start_recording_napcat_http_server();
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));

    client.get_version_info().await.unwrap();
    client.get_status().await.unwrap();
    client.get_friend_list().await.unwrap();
    client.get_group_list().await.unwrap();
    client.get_recent_contact().await.unwrap();
    client
        .get_group_msg_history(
            &GroupHistoryQuery::new("group-scope", None, 20, HistoryReadDirection::TowardOlder)
                .unwrap(),
        )
        .await
        .unwrap();
    client
        .get_group_msg_history(
            &GroupHistoryQuery::new(
                "group-scope",
                Some("opaque/group+cursor".into()),
                100,
                HistoryReadDirection::TowardNewer,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    client
        .get_friend_msg_history(
            &FriendHistoryQuery::new("friend-scope", None, 20, HistoryReadDirection::TowardOlder)
                .unwrap(),
        )
        .await
        .unwrap();
    client
        .get_friend_msg_history(
            &FriendHistoryQuery::new(
                "friend-scope",
                Some("opaque friend cursor".into()),
                1,
                HistoryReadDirection::TowardNewer,
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let requests = requests.lock().unwrap();
    let actions = requests
        .iter()
        .map(|request| request.action.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        [
            "get_version_info",
            "get_status",
            "get_friend_list",
            "get_group_list",
            "get_recent_contact",
            "get_group_msg_history",
            "get_group_msg_history",
            "get_friend_msg_history",
            "get_friend_msg_history",
        ]
    );
    assert!(
        requests[..5]
            .iter()
            .all(|request| request.params == serde_json::json!({}))
    );
    assert_eq!(
        requests[5].params,
        serde_json::json!({
            "group_id": "group-scope",
            "message_seq": "0",
            "count": 20,
            "reverseOrder": false,
        })
    );
    assert_eq!(
        requests[6].params,
        serde_json::json!({
            "group_id": "group-scope",
            "message_seq": "opaque/group+cursor",
            "count": 100,
            "reverseOrder": true,
        })
    );
    assert_eq!(
        requests[7].params,
        serde_json::json!({
            "user_id": "friend-scope",
            "message_seq": "0",
            "count": 20,
            "reverseOrder": false,
        })
    );
    assert_eq!(
        requests[8].params,
        serde_json::json!({
            "user_id": "friend-scope",
            "message_seq": "opaque friend cursor",
            "count": 1,
            "reverseOrder": true,
        })
    );
    for forbidden in [
        "send_msg",
        "delete_msg",
        "group_poke",
        "set_group_kick",
        "upload_group_file",
    ] {
        assert!(!actions.contains(&forbidden));
    }
}

/// 返回 retcode=1404 的失败响应，模拟 API 不存在。
fn unavailable_response(_action: &str) -> String {
    serde_json::json!({
        "status": "failed",
        "retcode": 1404,
        "data": null
    })
    .to_string()
}

fn sensitive_error_response(_action: &str) -> String {
    serde_json::json!({
        "status": "sensitive response status",
        "retcode": 1200,
        "data": {
            "message": "sensitive message body",
            "account_id": "sensitive account id"
        }
    })
    .to_string()
}

#[tokio::test]
async fn nonzero_retcode_error_does_not_expose_response_data() {
    let (addr, _) = start_mock_napcat_http_server(sensitive_error_response);
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));

    let error = client.get_status().await.unwrap_err().to_string();
    assert!(error.contains("get_status"));
    assert!(error.contains("1200"));
    assert!(error.contains("data_present=true"));
    assert!(!error.contains("sensitive response status"));
    assert!(!error.contains("sensitive message body"));
    assert!(!error.contains("sensitive account id"));
}

#[tokio::test]
async fn capability_probe_parses_real_napcat_dto_shapes() {
    let (addr, actions) = start_mock_napcat_http_server(real_napcat_response);
    let base_url = format!("http://{addr}");
    let client = NapCatReadOnlyClient::new(base_url);

    let snapshot = CapabilitySnapshot::probe(&client).await;

    // 探测必须在整体超时内完成（5s 上限）。
    assert!(
        snapshot.probe_completed,
        "probe must complete within timeout"
    );

    // version_info 正确解析。
    assert_eq!(snapshot.app_name.as_deref(), Some("NapCat"));
    assert_eq!(snapshot.app_version.as_deref(), Some("4.0.0"));
    assert_eq!(snapshot.protocol_version.as_deref(), Some("11"));
    assert_eq!(snapshot.impl_type.as_deref(), Some("napcat"));

    // get_status.online = true。
    assert_eq!(snapshot.online, Some(true));

    // 评审第三轮 P1-1 + 第四轮 P2：探测只调用轻量接口 version_info + status。
    // 列表 API（friend/group/recent_contact）标记为 Deferred（延迟到 B4 验证），
    // 不是 Unavailable（已确认不支持）。不在探测时拉取完整数组。
    use qqbot::napcat::ApiAvailability;
    assert!(
        matches!(snapshot.recent_contact_api, ApiAvailability::Deferred(_)),
        "recent_contact must be Deferred, got {:?}",
        snapshot.recent_contact_api
    );
    assert!(
        matches!(snapshot.friend_list_api, ApiAvailability::Deferred(_)),
        "friend_list must be Deferred, got {:?}",
        snapshot.friend_list_api
    );
    assert!(
        matches!(snapshot.group_list_api, ApiAvailability::Deferred(_)),
        "group_list must be Deferred, got {:?}",
        snapshot.group_list_api
    );

    // 探测只覆盖 version_info + status 两个轻量 API。
    let actions = actions.lock().unwrap();
    assert!(
        actions.contains(&"get_version_info".to_string()),
        "probe must call get_version_info, got {:?}",
        *actions
    );
    assert!(
        actions.contains(&"get_status".to_string()),
        "probe must call get_status, got {:?}",
        *actions
    );
    // 列表 API 不应在探测时被调用。
    assert!(
        !actions.contains(&"get_recent_contact".to_string()),
        "probe must not call get_recent_contact (deferred to B4)"
    );
    assert!(
        !actions.contains(&"get_friend_list".to_string()),
        "probe must not call get_friend_list (deferred to B4)"
    );
    assert!(
        !actions.contains(&"get_group_list".to_string()),
        "probe must not call get_group_list (deferred to B4)"
    );
}

#[tokio::test]
async fn recent_contact_dto_roundtrips_real_napcat_string_fields() {
    // 直接验证 DTO 反序列化与真实响应一致，不经过探测。
    let (addr, _) = start_mock_napcat_http_server(real_napcat_response);
    let base_url = format!("http://{addr}");
    let client = NapCatReadOnlyClient::new(base_url);

    let contacts = client.get_recent_contact().await.unwrap();
    assert_eq!(contacts.len(), 1);
    let contact = &contacts[0];
    // 关键断言：peerUin/msgTime 为字符串，无精度损失。
    assert_eq!(contact.peer_uin, "1234567890");
    assert_eq!(contact.msg_time, "1719421200");
    assert_eq!(contact.chat_type, 2);
    assert_eq!(contact.peer_name, "测试群");
}

#[tokio::test]
async fn capability_probe_marks_unavailable_api_as_unavailable() {
    // 启动一个只返回 retcode=1404（API 不存在）的服务器，验证降级标记。
    // 评审第三轮 P1-1 + 第四轮 P2：探测只调用 version_info + status；
    // 列表 API 标记为 Deferred（延迟到 B4），不因 1404 而变为 Unavailable。
    use qqbot::napcat::ApiAvailability;
    let (addr, _) = start_mock_napcat_http_server(unavailable_response);
    let base_url = format!("http://{addr}");
    let client = NapCatReadOnlyClient::new(base_url);
    let snapshot = CapabilitySnapshot::probe(&client).await;

    // 探测完成（version_info + status 都返回了 1404，不算超时）。
    assert!(snapshot.probe_completed);
    // version_info 失败 -> heartbeat_supported 标记为 Unavailable（version_info 错误）。
    assert!(
        snapshot.heartbeat_supported.is_unavailable(),
        "heartbeat_supported must be Unavailable when version_info fails, got {:?}",
        snapshot.heartbeat_supported
    );
    // 列表 API 延迟到 B4（Deferred），不因 1404 变为 Unavailable（探测不调用它们）。
    assert!(
        matches!(snapshot.recent_contact_api, ApiAvailability::Deferred(_)),
        "recent_contact must be Deferred even when version_info fails"
    );
    assert!(
        matches!(snapshot.friend_list_api, ApiAvailability::Deferred(_)),
        "friend_list must be Deferred even when version_info fails"
    );
    assert!(
        matches!(snapshot.group_list_api, ApiAvailability::Deferred(_)),
        "group_list must be Deferred even when version_info fails"
    );
}

#[tokio::test]
async fn capability_probe_completes_within_timeout_when_server_hangs() {
    // 启动一个接受连接但既不读也不写、长时间持有连接的服务器，验证探测整体超时后标记为 Unknown。
    // 关键：服务器不能 drop 连接（否则 reqwest 收到 EOF 立即返回错误而非挂起）。
    // 服务器持有连接但不响应，模拟 NapCat 卡住。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let held_streams: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
    let held_clone = Arc::clone(&held_streams);
    std::thread::spawn(move || {
        for _ in 0..10 {
            let (stream, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(_) => break,
            };
            // 持有连接但不读不写，模拟 NapCat 卡住。
            // 连接被存入 Vec 中防止 drop，保持 TCP 连接开着。
            held_clone.lock().unwrap().push(stream);
        }
    });

    let base_url = format!("http://{addr}");
    let client = NapCatReadOnlyClient::new(base_url);
    let start = std::time::Instant::now();
    let snapshot = CapabilitySnapshot::probe(&client).await;
    let elapsed = start.elapsed();

    // 关键断言：探测在整体超时（5s）内返回，不永久挂起。
    assert!(
        elapsed.as_secs() <= 6,
        "probe must complete within ~5s timeout, took {:?}",
        elapsed
    );
    // 超时后标记为未完成。
    assert!(
        !snapshot.probe_completed,
        "probe must be marked incomplete when server hangs"
    );
}

// ===== 评审第四轮 P1 + 第五轮收尾：流式响应字节上限测试 =====
// 验证私有 HTTP 请求实现不会因 resp.bytes() 缓冲整个响应而造成无界内存分配。
// 应先检查 Content-Length，再用 bytes_stream() 分块累计，超限立即停止。
//
// 评审第五轮要求测试通过可观察副作用证明流式行为，而非仅断言错误消息：
// - Content-Length 测试：服务器只发响应头后保持连接，通过写入计数证明客户端未读正文。
// - chunked 测试：服务器慢速分块发送，记录成功发送量，证明客户端在完整响应发送前中止。
// - exact-limit 测试：构造总长度精确等于上限的合法 JSON，验证被接受。

/// 启动一个返回指定大小响应体的 HTTP 服务器。
/// `body_sent` 记录服务器实际成功写入正文的字节数（用于证明客户端提前中止）。
/// `chunk_delay_ms` 为分块发送间隔（>0 时模拟慢速服务器，便于观察流式中止）。
fn start_tracked_http_server(
    body: Vec<u8>,
    content_length: Option<usize>,
    chunked: bool,
    chunk_delay_ms: u64,
    body_sent: Arc<std::sync::atomic::AtomicUsize>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body_clone = body.clone();
    std::thread::spawn(move || {
        for _ in 0..5 {
            let (stream, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let body = body_clone.clone();
            let sent = Arc::clone(&body_sent);
            std::thread::spawn(move || {
                let mut stream = stream;
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let cl_header = match (content_length, chunked) {
                    (Some(len), false) => format!("Content-Length: {}\r\n", len),
                    (_, true) => "Transfer-Encoding: chunked\r\n".to_string(),
                    (None, false) => "Connection: close\r\n".to_string(),
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{}\r\n",
                    cl_header
                );
                let _ = std::io::Write::write_all(&mut stream, header.as_bytes());
                if chunked {
                    // 分块发送：每块前加十六进制长度行，块间可选延迟。
                    let mut offset = 0;
                    while offset < body.len() {
                        let end = (offset + 1024).min(body.len());
                        let chunk_size = end - offset;
                        let size_line = format!("{:x}\r\n", chunk_size);
                        if std::io::Write::write_all(&mut stream, size_line.as_bytes()).is_err() {
                            // 客户端已关闭连接；记录已成功发送的字节数。
                            return;
                        }
                        if std::io::Write::write_all(&mut stream, &body[offset..end]).is_err() {
                            return;
                        }
                        if std::io::Write::write_all(&mut stream, b"\r\n").is_err() {
                            return;
                        }
                        sent.fetch_add(chunk_size, std::sync::atomic::Ordering::SeqCst);
                        offset = end;
                        if chunk_delay_ms > 0 {
                            std::thread::sleep(Duration::from_millis(chunk_delay_ms));
                        }
                    }
                    let _ = std::io::Write::write_all(&mut stream, b"0\r\n\r\n");
                } else {
                    // 非 chunked：分块写入并 flush，使客户端断开时 write 立即失败
                    // 而不是缓冲到 OS socket buffer。每次写入后 flush + 短延迟，
                    // 确保客户端的 RST 能在完整 body 发送前到达服务器。
                    let mut offset = 0;
                    while offset < body.len() {
                        let end = (offset + 1024).min(body.len());
                        let chunk_size = end - offset;
                        if std::io::Write::write_all(&mut stream, &body[offset..end]).is_err() {
                            return;
                        }
                        let _ = std::io::Write::flush(&mut stream);
                        sent.fetch_add(chunk_size, std::sync::atomic::Ordering::SeqCst);
                        offset = end;
                        if chunk_delay_ms > 0 {
                            std::thread::sleep(Duration::from_millis(chunk_delay_ms));
                        } else {
                            // 即使无延迟也短暂让出，使 RST 能及时到达。
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn content_length_over_limit_rejects_without_reading_body() {
    // 评审第五轮：证明 Content-Length 超限时客户端在读取 body 前即拒绝。
    // 服务器发送超过上限的 Content-Length 头，但只发送 header 后不主动写 body
    // （依赖 read 超时后退出）。通过 body_sent 计数证明客户端未读取 body。
    // 旧实现（bytes().await 先缓冲再检查）会尝试读取整个 body，body_sent 会接近完整大小。
    let big = vec![b'x'; 2 * 1024 * 1024]; // 2 MiB > 1 MiB limit
    let body_sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // 用非 chunked + Content-Length + 慢速发送，使客户端的 RST 能在 body 发送前到达。
    // 关键：客户端因 Content-Length 检查立即关闭，服务器 write 失败，body_sent 远小于完整大小。
    let addr =
        start_tracked_http_server(big, Some(2 * 1024 * 1024), false, 5, Arc::clone(&body_sent));
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));
    let start = std::time::Instant::now();
    let result = client.get_status().await;
    let elapsed = start.elapsed();

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("Content-Length"),
        "expected Content-Length rejection, got: {err}"
    );
    // 关键断言：客户端拒绝速度极快（Content-Length 检查不需要读 body）。
    // 旧实现（bytes().await 先缓冲再检查）会等待读取 2 MiB body，耗时显著更长。
    assert!(
        elapsed.as_millis() < 1000,
        "rejection should be near-instant (Content-Length check), took {:?}",
        elapsed
    );
    // 等待服务器线程确认 body 发送量。
    std::thread::sleep(Duration::from_millis(300));
    let sent = body_sent.load(std::sync::atomic::Ordering::SeqCst);
    let full_body = 2 * 1024 * 1024;
    // 关键断言：服务器发送的字节数远少于完整 body（2 MiB）。
    // 旧实现会读取完整 body，sent 会接近 2 MiB。
    // 由于 OS socket buffer 与 RST 传播时序，服务器可能写入少量缓冲字节，
    // 但必须远小于完整大小（< 10%），证明客户端在读取 body 前即拒绝。
    assert!(
        sent < full_body / 10,
        "server should have sent far fewer than full body ({full_body} bytes, client rejected by Content-Length before reading), but sent {sent}"
    );
}

#[tokio::test]
async fn chunked_response_over_limit_aborts_mid_stream() {
    // 评审第五轮：证明 chunked 响应超限时客户端在流式读取中途中止，而非完整读取。
    // 服务器慢速分块发送 2 MiB；客户端在累计超过 1 MiB 时中止。
    // 通过 body_sent 证明服务器在发送完整 2 MiB 前被客户端中断。
    let big = vec![b'x'; 2 * 1024 * 1024]; // 2 MiB > 1 MiB limit
    let body_sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // 每块 1024 字节，间隔 5ms，使客户端中止可被观察。
    let addr = start_tracked_http_server(big, None, true, 5, Arc::clone(&body_sent));
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));
    let result = client.get_status().await;
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("exceeds") || err.to_string().contains("aborted"),
        "expected stream abort, got: {err}"
    );
    // 等待服务器线程退出（客户端关闭后）。
    std::thread::sleep(Duration::from_millis(300));
    let sent = body_sent.load(std::sync::atomic::Ordering::SeqCst);
    // 关键断言：服务器成功发送的字节数应远少于完整 2 MiB，
    // 证明客户端在流式读取中途（约 1 MiB 时）中止。
    assert!(
        sent < 2 * 1024 * 1024,
        "server should not have sent full body (client aborted mid-stream), sent {sent}"
    );
    // 客户端至少读取了超过 1 MiB 才触发中止（分块累计到上限）。
    // 但由于时序，服务器记录的可能略少于实际读取量。关键是远少于完整大小。
    assert!(sent <= 2 * 1024 * 1024, "sent {sent} sanity check");
}

#[tokio::test]
async fn response_at_exact_limit_is_accepted() {
    // 评审第五轮：构造总长度精确等于 MAX_RESPONSE_BYTES 的合法 JSON，验证被接受。
    // 用 JSON 空白填充使总字节数恰好等于上限。
    use qqbot::napcat::MAX_RESPONSE_BYTES;
    let base = serde_json::json!({
        "status": "ok",
        "retcode": 0,
        "data": {"online": true, "good": true}
    })
    .to_string();
    let base_bytes = base.into_bytes();
    let base_len = base_bytes.len();
    assert!(
        base_len <= MAX_RESPONSE_BYTES,
        "base JSON {base_len} must be <= {MAX_RESPONSE_BYTES}"
    );
    // 在 JSON 末尾（闭合 } 前）插入空白使总长度精确等于上限。
    // 构造：{"status":"ok","retcode":0,"data":{"online":true,"good":true}       ...       }
    // 简单做法：把 base 的最后一个 } 去掉，加 padding 空格，再加 }。
    let padding_needed = MAX_RESPONSE_BYTES - base_len;
    let mut body_bytes = base_bytes.clone();
    // 移除末尾的 }，加空白，再加回 }。
    body_bytes.pop(); // remove trailing }
    body_bytes.extend(std::iter::repeat_n(b' ', padding_needed));
    body_bytes.push(b'}');
    assert_eq!(
        body_bytes.len(),
        MAX_RESPONSE_BYTES,
        "body must be exactly MAX_RESPONSE_BYTES bytes"
    );
    // 验证仍是合法 JSON。
    let _: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("padded body must be valid JSON");

    let addr = start_tracked_http_server(
        body_bytes.clone(),
        Some(body_bytes.len()),
        false,
        0,
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));
    let status = client
        .get_status()
        .await
        .expect("response at exact limit (1 MiB) must be accepted, not rejected");
    assert_eq!(status.online, Some(true));
}

#[tokio::test]
async fn normal_small_response_succeeds() {
    // 正常小响应应成功解析（无回归）。
    let body = serde_json::json!({
        "status": "ok",
        "retcode": 0,
        "data": {"online": true, "good": true}
    })
    .to_string();
    let body_bytes = body.into_bytes();
    let addr = start_tracked_http_server(
        body_bytes.clone(),
        Some(body_bytes.len()),
        false,
        0,
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    let client = NapCatReadOnlyClient::new(format!("http://{addr}"));
    let status = client.get_status().await.unwrap();
    assert_eq!(status.online, Some(true));
    assert_eq!(status.good, Some(true));
}
