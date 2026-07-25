use futures_util::SinkExt;
use qqbot::napcat::api::NapCatApiClient;
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the ignored live test"))
}

#[tokio::test]
#[ignore = "requires an explicitly configured local NapCat test account"]
async fn readonly_http_contract_matches_the_local_napcat_instance() {
    let base_url = required_env("NAPCAT_TEST_HTTP_BASE_URL");
    let group_id = required_env("NAPCAT_TEST_GROUP_ID");
    let peer_id = required_env("NAPCAT_TEST_PEER_ID");
    let client = NapCatApiClient::new(base_url);

    let status = client.get_status().await.unwrap();
    assert_eq!(status.online, Some(true));
    assert_eq!(status.good, Some(true));

    let login = client.get_login_info().await.unwrap();
    assert!(login.user_id > 0);
    assert!(!login.nickname.trim().is_empty());

    let parsed_group_id = group_id.parse::<i64>().unwrap();
    let group = client.get_group_info(parsed_group_id).await.unwrap();
    assert_eq!(group.group_id, parsed_group_id);
    assert!(
        client
            .get_group_list()
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.group_id == parsed_group_id)
    );
    let members = client.get_group_member_list(parsed_group_id).await.unwrap();
    assert!(members.iter().any(|member| member.user_id == login.user_id));
    assert!(
        members
            .iter()
            .any(|member| member.user_id.to_string() == peer_id)
    );

    let group_history = client
        .get_group_msg_history(&group_id, None, 100, false)
        .await
        .unwrap();
    let group_sample = group_history
        .first()
        .expect("test group must contain at least one message");
    assert!(!group_sample.message_id.is_empty());
    assert!(!group_sample.message_seq.is_empty());
    assert_eq!(
        client
            .get_msg(&group_sample.message_id)
            .await
            .unwrap()
            .message_id,
        group_sample.message_id
    );

    let friend_history = client
        .get_friend_msg_history(&peer_id, None, 100, false)
        .await
        .unwrap();
    let friend_sample = friend_history
        .first()
        .expect("test friendship must contain at least one message");
    assert!(!friend_sample.message_id.is_empty());
    assert!(!friend_sample.message_seq.is_empty());
}

#[tokio::test]
#[ignore = "requires an explicitly configured local NapCat WebSocket server"]
async fn websocket_server_accepts_a_real_handshake() {
    let ws_url = required_env("NAPCAT_TEST_WS_URL");
    let (mut stream, _) = connect_async(ws_url).await.unwrap();
    stream.send(Message::Close(None)).await.unwrap();
}
