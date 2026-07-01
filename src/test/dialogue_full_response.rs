#[cfg(feature = "qdrant")]
mod full_response {
    use std::sync::Arc;

    use chrono::Utc;

    use crate::test::support::{assertions, config, dialogue, infra, logging, tunnels, users};

    struct FullResponseScenario {
        marker: String,
        user_text: String,
        expected_tool: &'static str,
    }

    impl FullResponseScenario {
        fn get_time() -> Self {
            let marker = format!("full-response-test-{}", Utc::now().timestamp_millis());
            let user_text = format!(
                "集成测试标记 {marker}。现在几点？今天是几号、星期几？请先调用 get_time 工具获取当前日期时间，再用中文回复工具返回的日期、时间和星期。不要凭常识或系统时间直接回答。"
            );
            Self {
                marker,
                user_text,
                expected_tool: "get_time",
            }
        }
    }

    #[tokio::test]
    #[ignore = "需要 config.toml、test/123123123 用户、数据库、Qdrant、embedding 和 LLM；手动运行: cargo test dialogue_full_response --lib -- --ignored --nocapture"]
    async fn chat_service_returns_llm_reply_tool_trace_and_persists_test_user_turn() {
        logging::init();

        let config = config::load();
        config::require_agent(&config, "完整 response 测试");
        config::require_tools(&config, "完整 response 测试");
        config::require_qdrant(&config, "完整 response 测试");
        config::require_context_routing(&config, "完整 response 测试");

        let tunnel_manager = tunnels::ensure(
            &config,
            &[
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Database),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Qdrant),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Embedding),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Llm),
            ],
            "完整 response 测试",
        )
        .await;

        let db = infra::connect_db(&config).await;
        let repos = infra::repos(&db, &config);
        let embedding_provider = infra::embedding_provider(&config);
        let llm_provider = infra::llm_provider(&config);
        let vector_store = infra::vector_store(&config).await;
        let context_routing_service =
            infra::context_routing_service(&config, Arc::clone(&embedding_provider)).await;

        let test_user = users::load_test_user(&repos).await;
        let before = assertions::ConversationSnapshot::for_user(&repos, test_user.id).await;
        let harness = dialogue::chat_service_with_time_tool(
            &config,
            &db,
            &repos,
            Arc::clone(&embedding_provider),
            Arc::clone(&llm_provider),
            Arc::clone(&vector_store),
            context_routing_service,
        )
        .await;

        let scenario = FullResponseScenario::get_time();
        let response = harness
            .chat_service
            .send_message(test_user.id, scenario.user_text.clone(), None, None)
            .await
            .unwrap_or_else(|error| panic!("ChatService 完整 response 调用失败: {error}"));

        assert_active_conversation(&repos, test_user.id, response.conversation_id).await;
        assert!(
            !response.reply.trim().is_empty(),
            "完整 response 必须返回非空 LLM 回复"
        );
        assert!(
            response
                .tool_calls
                .iter()
                .any(|trace| trace.tool_name == scenario.expected_tool),
            "完整 response 必须经过 {} 工具调用，实际工具: {:?}",
            scenario.expected_tool,
            response
                .tool_calls
                .iter()
                .map(|trace| trace.tool_name.as_str())
                .collect::<Vec<_>>()
        );

        let after_messages = repos
            .conv_repo
            .find_messages_by_conversation_id(response.conversation_id)
            .await
            .unwrap_or_else(|error| panic!("读取完整 response 后会话消息失败: {error}"));
        assert_eq!(
            before.messages.len() + 2,
            after_messages.len(),
            "完整 response 应只为 test 用户新增一轮 user + assistant 消息"
        );
        let turn = assertions::find_persisted_turn(
            &before,
            &after_messages,
            test_user.id,
            &scenario.marker,
        );

        let turn_closed = harness
            .task_publisher
            .turn_closed_events()
            .into_iter()
            .find(|event| event.conversation_id == response.conversation_id)
            .unwrap_or_else(|| panic!("完整 response 必须发布 TurnClosedEvent"));
        assertions::assert_turn_closed_event(
            &turn_closed,
            test_user.id,
            response.conversation_id,
            &turn,
        );

        println!(
            "完整 response 测试完成: user_id={}, conversation_id={}, reply_chars={}, tool_calls={}, marker={}",
            test_user.id,
            response.conversation_id,
            response.reply.chars().count(),
            response.tool_calls.len(),
            scenario.marker
        );

        if let Some(manager) = tunnel_manager {
            manager.shutdown().await;
        }
    }

    async fn assert_active_conversation(
        repos: &crate::bootstrap::repos::RepoGraph,
        user_id: u64,
        conversation_id: u64,
    ) {
        let active_conversation = repos
            .conv_repo
            .find_single_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("重新读取 test 用户会话失败: {error}"))
            .unwrap_or_else(|| panic!("test 用户会话在发送消息后不存在"));
        assert_eq!(
            conversation_id, active_conversation.id,
            "完整 response 只能写入 test 用户自己的 conversation"
        );

        assertions::assert_test_user_conversation(repos, conversation_id, user_id).await;
    }
}

#[cfg(not(feature = "qdrant"))]
mod full_response {
    #[tokio::test]
    #[ignore = "默认 feature 已包含 qdrant；如果关闭默认 feature，需要手动启用 qdrant"]
    async fn chat_service_full_response_requires_qdrant_feature() {
        panic!("完整 response 测试需要 qdrant feature");
    }
}
