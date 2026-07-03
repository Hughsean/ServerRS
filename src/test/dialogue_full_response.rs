#[cfg(feature = "qdrant")]
mod full_response {
    use std::sync::Arc;

    use chrono::Utc;

    use crate::app::context_routing::{
        ContextRouteDecision, ContextRoutingService, build_routing_input,
    };
    use crate::domain::llm::ChatMessage;
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

    #[derive(Debug, Clone, Copy)]
    enum RequestStyle {
        Explicit,
        Natural,
    }

    #[derive(Debug, Clone, Copy)]
    enum RouteExpectation {
        Any,
        FreshEnabled,
        MemoryPositive,
        RagPositive,
    }

    #[derive(Debug, Clone, Copy)]
    enum ToolExpectation {
        Required(&'static str),
        NotRequired,
    }

    struct MultiTurnScenario {
        name: &'static str,
        marker: String,
        user_text: String,
        request_style: RequestStyle,
        route_expectation: RouteExpectation,
        tool_expectation: ToolExpectation,
        expect_auxiliary_context: bool,
    }

    impl MultiTurnScenario {
        fn new(
            name: &'static str,
            user_text: impl Into<String>,
            request_style: RequestStyle,
            route_expectation: RouteExpectation,
            tool_expectation: ToolExpectation,
            expect_auxiliary_context: bool,
        ) -> Self {
            let marker = format!(
                "multi-turn-full-response-{name}-{}",
                Utc::now().timestamp_millis()
            );
            let user_text = format!("集成测试标记 {marker}。{}", user_text.into());
            Self {
                name,
                marker,
                user_text,
                request_style,
                route_expectation,
                tool_expectation,
                expect_auxiliary_context,
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

    #[tokio::test]
    #[ignore = "需要 config.toml、test/123123123 用户、数据库、Qdrant、embedding、LLM、工具配置和上下文路由；手动运行: cargo test dialogue_full_response --lib -- --ignored --nocapture"]
    async fn multi_turn_dialogue() {
        logging::init();

        let config = config::load();
        config::require_agent(&config, "多轮完整 response 上下文路由测试");
        config::require_tools(&config, "多轮完整 response 上下文路由测试");
        config::require_qdrant(&config, "多轮完整 response 上下文路由测试");
        config::require_context_routing(&config, "多轮完整 response 上下文路由测试");

        let tunnel_manager = tunnels::ensure(
            &config,
            &[
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Database),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Qdrant),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Embedding),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Llm),
            ],
            "多轮完整 response 上下文路由测试",
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
        let harness = dialogue::chat_service_with_core_test_tools(
            &config,
            &db,
            &repos,
            Arc::clone(&embedding_provider),
            Arc::clone(&llm_provider),
            Arc::clone(&vector_store),
            Arc::clone(&context_routing_service),
        )
        .await;
        harness
            .chat_service
            .clear_transcript(test_user.id)
            .await
            .unwrap_or_else(|error| panic!("清空多轮测试用户历史会话失败: {error}"));
        let mut snapshot = assertions::ConversationSnapshot::for_user(&repos, test_user.id).await;

        let scenarios = vec![
            // MultiTurnScenario::new(
            //     "explicit-time",
            //     "请先调用 get_time 工具获取当前日期时间，再用中文回复日期、时间和星期。",
            //     RequestStyle::Explicit,
            //     RouteExpectation::Any,
            //     ToolExpectation::Required("get_time"),
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "natural-time",
            //     "我准备今晚安排学习计划，现在大概是什么时候了，适合先做哪一段？",
            //     RequestStyle::Natural,
            //     RouteExpectation::Any,
            //     ToolExpectation::Required("get_time"),
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "explicit-rag",
            //     "请先使用 knowledge_search 工具查知识库，再回答压力调节有哪些可靠方法。",
            //     RequestStyle::Explicit,
            //     RouteExpectation::RagPositive,
            //     ToolExpectation::Required("knowledge_search"),
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "natural-rag",
            //     "最近总觉得压力很大，想知道有没有更科学一点的缓解办法。",
            //     RequestStyle::Natural,
            //     RouteExpectation::Any,
            //     ToolExpectation::NotRequired,
            //     false,
            // ),
            // // memory_extraction_async=false in the harness: this pair verifies routing for
            // // memory-style turns, not that a new persisted memory was extracted and recalled.
            // MultiTurnScenario::new(
            //     "memory-seed",
            //     "我比较喜欢晚上学习，也更容易被安静的环境安抚。",
            //     RequestStyle::Natural,
            //     RouteExpectation::Any,
            //     ToolExpectation::NotRequired,
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "memory-recall",
            //     "你还记得我更适合什么样的学习环境吗？",
            //     RequestStyle::Natural,
            //     RouteExpectation::MemoryPositive,
            //     ToolExpectation::NotRequired,
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "current-task-seed",
            //     "我们先制定一个三步复习计划。",
            //     RequestStyle::Natural,
            //     RouteExpectation::Any,
            //     ToolExpectation::NotRequired,
            //     false,
            // ),
            // MultiTurnScenario::new(
            //     "current-task-reference",
            //     "继续按刚才那个方案细化第二步。",
            //     RequestStyle::Natural,
            //     RouteExpectation::Any,
            //     ToolExpectation::NotRequired,
            //     true,
            // ),
            // MultiTurnScenario::new(
            //     "fresh-context",
            //     "今天最新的 AI 新闻有哪些值得关注？",
            //     RequestStyle::Natural,
            //     RouteExpectation::FreshEnabled,
            //     ToolExpectation::NotRequired,
            //     false,
            // ),
            MultiTurnScenario::new(
                "knowledge_search",
                "我想了解人工智能史",
                RequestStyle::Natural,
                RouteExpectation::Any,
                ToolExpectation::NotRequired,
                false,
            ),
        ];

        for scenario in scenarios {
            let routing_input =
                build_test_window_routing_input(&repos, test_user.id, &scenario.user_text).await;
            if scenario.expect_auxiliary_context {
                assert_auxiliary_context_used(&scenario, &routing_input);
            }
            let route_decision =
                route_with_config(&context_routing_service, &config, routing_input).await;
            assert_route_expectation(&scenario, &route_decision);

            let response = harness
                .chat_service
                .send_message(test_user.id, scenario.user_text.clone(), None, None)
                .await
                .unwrap_or_else(|error| {
                    panic!("多轮完整 response 场景 {} 调用失败: {error}", scenario.name)
                });

            assert_active_conversation(&repos, test_user.id, response.conversation_id).await;
            assert!(
                !response.reply.trim().is_empty(),
                "场景 {} 必须返回非空 LLM 回复",
                scenario.name
            );
            assert_tool_expectation(&scenario, &response.tool_calls);

            let after_messages = repos
                .conv_repo
                .find_messages_by_conversation_id(response.conversation_id)
                .await
                .unwrap_or_else(|error| {
                    panic!("读取场景 {} 后会话消息失败: {error}", scenario.name)
                });
            assert_eq!(
                snapshot.messages.len() + 2,
                after_messages.len(),
                "场景 {} 应只新增一轮 user + assistant 消息",
                scenario.name
            );
            let turn = assertions::find_persisted_turn(
                &snapshot,
                &after_messages,
                test_user.id,
                &scenario.marker,
            );

            let turn_closed = harness
                .task_publisher
                .turn_closed_events()
                .into_iter()
                .find(|event| {
                    event.conversation_id == response.conversation_id
                        && event.user_message_id == Some(turn.user_message.id)
                        && event.assistant_message_id == Some(turn.assistant_message.id)
                })
                .unwrap_or_else(|| panic!("场景 {} 必须发布 TurnClosedEvent", scenario.name));
            assertions::assert_turn_closed_event(
                &turn_closed,
                test_user.id,
                response.conversation_id,
                &turn,
            );

            println!(
                "多轮完整 response 场景完成: name={}, style={:?}, route={:?}, tools={:?}, reply_chars={}, marker={}",
                scenario.name,
                scenario.request_style,
                route_decision,
                response
                    .tool_calls
                    .iter()
                    .map(|trace| trace.tool_name.as_str())
                    .collect::<Vec<_>>(),
                response.reply.chars().count(),
                scenario.marker
            );

            snapshot = assertions::ConversationSnapshot::new(None, after_messages);
        }

        if let Some(manager) = tunnel_manager {
            manager.shutdown().await;
        }
    }

    async fn build_test_window_routing_input(
        repos: &crate::bootstrap::repos::RepoGraph,
        user_id: u64,
        pending_user_message: &str,
    ) -> crate::domain::semantic_classification::SemanticInput {
        const CORE_TEST_MAX_CONTEXT_MESSAGES: usize = 6;
        const PERSISTED_MESSAGE_LIMIT: u64 = (CORE_TEST_MAX_CONTEXT_MESSAGES - 1) as u64;

        let mut recent_messages = match repos
            .conv_repo
            .find_single_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("读取 test 用户活跃会话失败: {error}"))
        {
            Some(conversation) => repos
                .conv_repo
                .find_messages_before(conversation.id, None, PERSISTED_MESSAGE_LIMIT)
                .await
                .unwrap_or_else(|error| panic!("读取 test 用户最近会话消息失败: {error}"))
                .into_iter()
                .filter_map(test_conversation_message_to_chat_message)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };

        recent_messages.push(ChatMessage {
            role: "user".into(),
            content: pending_user_message.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        build_routing_input(&apply_test_context_limit(
            recent_messages,
            CORE_TEST_MAX_CONTEXT_MESSAGES,
        ))
    }

    fn test_conversation_message_to_chat_message(
        message: crate::domain::conversation::conversation_message::ConversationMessage,
    ) -> Option<ChatMessage> {
        if !matches!(
            message.sender_role.as_str(),
            "system" | "user" | "assistant"
        ) {
            return None;
        }
        if message.message_type != "text" {
            return None;
        }

        let content = test_conversation_message_text(&message);
        if content.trim().is_empty() {
            return None;
        }

        Some(ChatMessage {
            role: message.sender_role,
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        })
    }

    fn test_conversation_message_text(
        message: &crate::domain::conversation::conversation_message::ConversationMessage,
    ) -> String {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&message.content) else {
            return message.content.clone();
        };

        let mut text = value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| message.content.clone());

        if message.sender_role == "user" {
            if let Some(emotion) = value
                .get("emotion")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|emotion| !emotion.is_empty())
            {
                text = format!("{text}\n\n[user emotion: {emotion}]");
            }
        }

        text
    }

    fn apply_test_context_limit(messages: Vec<ChatMessage>, limit: usize) -> Vec<ChatMessage> {
        if limit == 0 || messages.is_empty() {
            return messages;
        }

        let system_messages = messages
            .iter()
            .filter(|message| message.role == "system")
            .cloned()
            .collect::<Vec<_>>();
        let mut non_system_messages = messages
            .into_iter()
            .filter(|message| message.role != "system")
            .collect::<Vec<_>>();
        let non_system_count = non_system_messages.len();
        if non_system_count > limit {
            let skip = non_system_count.saturating_sub(limit);
            non_system_messages = non_system_messages.into_iter().skip(skip).collect();
        }

        let mut limited_messages = system_messages;
        limited_messages.extend(non_system_messages);
        limited_messages
    }

    async fn route_with_config(
        router: &Arc<ContextRoutingService>,
        config: &crate::shared::config::AppConfig,
        input: crate::domain::semantic_classification::SemanticInput,
    ) -> ContextRouteDecision {
        router
            .route(
                input,
                config.agent.max_memory_items,
                u64::from(config.agent.max_rag_chunks),
            )
            .await
    }

    fn assert_route_expectation(scenario: &MultiTurnScenario, decision: &ContextRouteDecision) {
        match scenario.route_expectation {
            RouteExpectation::Any => {}
            RouteExpectation::FreshEnabled => assert!(
                decision.fresh_context.enabled,
                "场景 {} 应启用 Fresh Context，实际决策: {:?}",
                scenario.name, decision
            ),
            RouteExpectation::MemoryPositive => {
                assert!(
                    decision.memory.top_k > 0,
                    "场景 {} 应保留 Memory 预算，实际决策: {:?}",
                    scenario.name,
                    decision
                );
                assert_eq!(
                    decision.memory.reason, "memory_positive",
                    "场景 {} 应命中 memory_positive，实际决策: {:?}",
                    scenario.name, decision
                );
            }
            RouteExpectation::RagPositive => {
                assert!(
                    decision.rag.top_k > 0,
                    "场景 {} 应保留 RAG 预算，实际决策: {:?}",
                    scenario.name,
                    decision
                );
                assert_ne!(
                    decision.rag.reason, "rag_negative",
                    "场景 {} 不应命中 rag_negative，实际决策: {:?}",
                    scenario.name, decision
                );
            }
        }
    }

    fn assert_tool_expectation(
        scenario: &MultiTurnScenario,
        tool_calls: &[crate::app::agent::agent_runtime::ToolTrace],
    ) {
        match scenario.tool_expectation {
            ToolExpectation::Required(expected_tool) => assert!(
                tool_calls
                    .iter()
                    .any(|trace| trace.tool_name == expected_tool),
                "场景 {} 必须经过 {} 工具调用，实际工具: {:?}",
                scenario.name,
                expected_tool,
                tool_calls
                    .iter()
                    .map(|trace| trace.tool_name.as_str())
                    .collect::<Vec<_>>()
            ),
            ToolExpectation::NotRequired => {}
        }
    }

    fn assert_auxiliary_context_used(
        scenario: &MultiTurnScenario,
        input: &crate::domain::semantic_classification::SemanticInput,
    ) {
        assert!(
            !input.auxiliary_texts.is_empty(),
            "场景 {} 应为指代语输入携带辅助上下文，routing input: {:?}",
            scenario.name,
            input
        );
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
