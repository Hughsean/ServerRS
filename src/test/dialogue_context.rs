#[cfg(feature = "qdrant")]
mod readonly_context {
    use std::sync::Arc;

    use serde_json::Value;

    use crate::app::context_routing::build_routing_input;
    use crate::domain::llm::ChatMessage;
    use crate::domain::vector_store::VectorStoreT;
    use crate::test::support::vector_store as test_vector_store;
    use crate::test::support::{assertions, config, dialogue, infra, logging, messages, tunnels};

    #[tokio::test]
    #[ignore = "需要 config.toml、数据库、Qdrant 和 embedding 服务；手动运行: cargo test dialogue_context --lib -- --ignored --nocapture"]
    async fn app_context_builder_uses_dependencies_without_writing_conversation() {
        logging::init();

        let config = config::load();
        config::require_qdrant(&config, "app 层上下文测试");
        config::require_context_routing(&config, "app 层上下文测试");
        config::require_memory(&config, "app 层上下文测试");

        let tunnel_manager = tunnels::ensure(
            &config,
            &[
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Database),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Qdrant),
                tunnels::TunnelRequirement::Optional(tunnels::ServiceTunnel::Embedding),
            ],
            "app 层上下文测试",
        )
        .await;

        let db = infra::connect_db(&config).await;
        let repos = infra::repos(&db, &config);
        let embedding_provider = infra::embedding_provider(&config);
        let llm_provider = infra::llm_provider(&config);

        let base_vector_store = infra::vector_store(&config).await;
        let counted_vector_store = Arc::new(test_vector_store::ReadOnlyCountingVectorStore::new(
            base_vector_store,
        ));
        let vector_store: Arc<dyn VectorStoreT> = counted_vector_store.clone();

        test_vector_store::assert_searchable(
            &vector_store,
            &embedding_provider,
            &config.qdrant.memory_collection,
        )
        .await;
        let search_count_before = counted_vector_store.search_count();

        let context_routing_service =
            infra::context_routing_service(&config, Arc::clone(&embedding_provider)).await;
        let context_builder = dialogue::context_builder(
            &config,
            &db,
            &repos,
            Arc::clone(&embedding_provider),
            Arc::clone(&llm_provider),
            Arc::clone(&vector_store),
            Arc::clone(&context_routing_service),
        )
        .await;

        let conversation = messages::select_test_conversation(&db, &repos).await;
        let user_id = conversation.user_id;
        let conversation_id = conversation.id;

        let before_conversation = repos
            .conv_repo
            .find_by_id(conversation_id)
            .await
            .unwrap_or_else(|error| panic!("读取测试会话失败: {error}"))
            .unwrap_or_else(|| panic!("测试会话不存在: {conversation_id}"));
        let before_messages = repos
            .conv_repo
            .find_messages_by_conversation_id(conversation_id)
            .await
            .unwrap_or_else(|error| panic!("读取测试会话消息失败: {error}"));

        let recent_messages =
            messages::load_recent_chat_messages(&repos, conversation_id, &config).await;
        let synthetic_query = std::env::var(messages::TEST_QUERY_ENV)
            .unwrap_or_else(|_| "请根据我的长期偏好和项目背景来回答".into());
        let mut turn_messages = recent_messages;
        turn_messages.push(ChatMessage {
            role: "user".into(),
            content: synthetic_query.clone(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let routing_decision = context_routing_service
            .route(
                build_routing_input(&turn_messages),
                config.agent.max_memory_items,
                u64::from(config.agent.max_rag_chunks),
            )
            .await;
        assert!(
            routing_decision.memory.top_k > 0,
            "测试 query 必须触发 Memory 召回预算，实际路由决策: {:?}",
            routing_decision
        );

        let context = context_builder
            .build(
                user_id,
                Some(conversation_id),
                turn_messages,
                None,
                Vec::new(),
                None::<Value>,
                config.agent.max_memory_items,
                u64::from(config.agent.max_rag_chunks),
                config.agent.summary_enabled,
                config.agent.memory_enabled,
                config.agent.rag_enabled,
            )
            .await;

        assert_eq!(context.user_id, user_id);
        assert_eq!(context.conversation_id, Some(conversation_id));
        assert!(
            context
                .recent_messages
                .last()
                .is_some_and(|message| message.role == "user" && message.content == synthetic_query),
            "app 层上下文必须包含当前轮用户消息"
        );
        assert!(
            counted_vector_store.search_count() > search_count_before,
            "AgentContextBuilder 应通过 Qdrant 执行至少一次向量检索"
        );
        assert_eq!(
            counted_vector_store.write_count(),
            0,
            "app 层上下文测试不允许写 Qdrant"
        );

        let after_conversation = repos
            .conv_repo
            .find_by_id(conversation_id)
            .await
            .unwrap_or_else(|error| panic!("重新读取测试会话失败: {error}"))
            .unwrap_or_else(|| panic!("测试会话在构建上下文后不存在: {conversation_id}"));
        let after_messages = repos
            .conv_repo
            .find_messages_by_conversation_id(conversation_id)
            .await
            .unwrap_or_else(|error| panic!("重新读取测试会话消息失败: {error}"));

        assertions::assert_conversation_unchanged(
            &before_conversation,
            before_messages.len(),
            &after_conversation,
            after_messages.len(),
        );

        println!(
            "只读 app 层上下文构建完成: user_id={}, conversation_id={}, memories={}, rag_chunks={}, fresh_chunks={}, qdrant_searches={}",
            context.user_id,
            conversation_id,
            context.memories.len(),
            context.rag_chunks.len(),
            context.fresh_chunks.len(),
            counted_vector_store.search_count() - search_count_before
        );

        if let Some(manager) = tunnel_manager {
            manager.shutdown().await;
        }
    }
}

#[cfg(not(feature = "qdrant"))]
mod readonly_context {
    #[tokio::test]
    #[ignore = "默认 feature 已包含 qdrant；如果关闭默认 feature，需要手动启用 qdrant"]
    async fn app_context_builder_readonly_requires_qdrant_feature() {
        panic!("app 层上下文测试需要启用 qdrant feature");
    }
}
