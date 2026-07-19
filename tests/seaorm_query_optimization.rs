use std::fs;
use std::path::Path;

fn source(relative_path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path))
        .unwrap_or_else(|error| panic!("无法读取 {relative_path}: {error}"))
}

#[test]
fn review_list_uses_joined_partial_model_instead_of_manual_hydration() {
    let review = source("src/infra/web_ingestion/review_repository.rs");

    assert!(
        review.contains("DerivePartialModel"),
        "审核列表应只选择需要的关联字段"
    );
    assert!(
        review.contains("into_partial_model::<KnowledgeReviewRow>()"),
        "审核列表应通过 Partial Model 映射关联查询结果"
    );
    assert!(
        !review.contains("async fn hydrate("),
        "审核列表不应再手工查询并拼装关联对象"
    );
    assert!(
        !review.contains("std::collections::HashMap"),
        "审核列表不应再依赖 HashMap 批量拼装"
    );
}

#[test]
fn indexable_chunks_load_parent_documents_without_n_plus_one_queries() {
    let rag = source("src/infra/repo/seaorm_impl/rag.rs");
    let start = rag
        .find("async fn list_indexable_chunks(")
        .expect("缺少 list_indexable_chunks");
    let section = &rag[start..];

    assert!(
        section.contains("knowledge_chunks::Entity::load()"),
        "索引候选应由 Entity Loader 一次加载父文档"
    );
    assert!(
        section.contains(".with(knowledge_documents::Entity)"),
        "索引候选应声明知识文档关联"
    );
    assert!(
        !section.contains("find_by_id(chunk.document_id)"),
        "索引候选不得逐 chunk 查询父文档"
    );
}

#[test]
fn list_queries_use_partial_models_to_exclude_large_or_sensitive_columns() {
    let user = source("src/infra/repo/seaorm_impl/user.rs");
    let music = source("src/infra/repo/seaorm_impl/music.rs");
    let psychology = source("src/infra/repo/seaorm_impl/psychology.rs");
    let admin_handler = source("src/api/handlers/admin_handler.rs");

    assert!(user.contains("UserListRow"));
    assert!(user.contains("into_partial_model::<UserListRow>()"));
    assert!(music.contains("MusicTrackListRow"));
    assert!(music.contains("into_partial_model::<MusicTrackListRow>()"));
    assert!(psychology.contains("PsychologyResourceListRow"));
    assert!(psychology.contains("into_partial_model::<PsychologyResourceListRow>()"));
    assert!(
        !admin_handler.contains("state.user.list_users().await?"),
        "管理员分页必须在数据库执行，不能先读取所有用户"
    );
}
