use digital_human::shared::config::AppConfig;

#[test]
fn parses_vendor_neutral_vector_store_configuration() {
    let config: AppConfig = toml::from_str(
        r#"
            [vector_store]
            enabled = true
            url = "http://127.0.0.1:6333"
            rag_index_name = "rag_test"
            memory_index_name = "memory_test"
            summary_index_name = "summary_test"

            [embedding]
            base_url = "http://127.0.0.1:11434/v1"
            vector_index_name = "rag_test"

            [web_ingestion]
            vector_index_name = "web_test"
        "#,
    )
    .unwrap();

    assert!(config.vector_store.enabled);
    assert_eq!(config.vector_store.rag_index_name, "rag_test");
    assert_eq!(config.embedding.vector_index_name, "rag_test");
    assert_eq!(config.web_ingestion.vector_index_name, "web_test");
}

#[test]
fn rejects_the_removed_qdrant_configuration_section() {
    let parsed = toml::from_str::<AppConfig>(
        r#"
            [qdrant]
            enabled = true
            url = "http://127.0.0.1:6333"
        "#,
    );

    assert!(parsed.is_err(), "旧 [qdrant] 配置必须显式报错");
}
