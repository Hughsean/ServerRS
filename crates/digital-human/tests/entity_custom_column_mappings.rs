use digital_human::infra::repo::entities::{
    community_post_media, fresh_items, fresh_topics, knowledge_ingestion_runs, music,
    psychology_articles, psychology_qna, psychology_resources,
};
use sea_orm::{ActiveValue, DatabaseBackend, EntityTrait, QueryTrait};

fn assert_active_string(_: ActiveValue<String>) {}

fn assert_active_optional_string(_: ActiveValue<Option<String>>) {}

fn assert_active_optional_bytes(_: ActiveValue<Option<Vec<u8>>>) {}

#[test]
fn mysql_custom_text_and_blob_columns_have_expected_active_types() {
    let community_post_media = community_post_media::ActiveModel::default();
    assert_active_string(community_post_media.media_data);
    let _ = community_post_media::Column::MediaData;

    let fresh_items = fresh_items::ActiveModel::default();
    assert_active_optional_string(fresh_items.raw_text);
    assert_active_optional_string(fresh_items.clean_text);
    assert_active_optional_string(fresh_items.summary);
    let _ = fresh_items::Column::RawText;
    let _ = fresh_items::Column::CleanText;
    let _ = fresh_items::Column::Summary;

    let fresh_topics = fresh_topics::ActiveModel::default();
    assert_active_optional_string(fresh_topics.summary);
    let _ = fresh_topics::Column::Summary;

    let knowledge_ingestion_runs = knowledge_ingestion_runs::ActiveModel::default();
    assert_active_optional_string(knowledge_ingestion_runs.fetched_body_text);
    assert_active_optional_string(knowledge_ingestion_runs.clean_text);
    let _ = knowledge_ingestion_runs::Column::FetchedBodyText;
    let _ = knowledge_ingestion_runs::Column::CleanText;

    let music = music::ActiveModel::default();
    assert_active_string(music.file_data);
    let _ = music::Column::FileData;

    let psychology_articles = psychology_articles::ActiveModel::default();
    assert_active_string(psychology_articles.content);
    let _ = psychology_articles::Column::Content;

    let psychology_qna = psychology_qna::ActiveModel::default();
    assert_active_string(psychology_qna.answer);
    let _ = psychology_qna::Column::Answer;

    let psychology_resources = psychology_resources::ActiveModel::default();
    assert_active_optional_bytes(psychology_resources.file_data);
    let _ = psychology_resources::Column::FileData;
}

#[test]
fn psychology_mysql_selects_do_not_cast_columns_as_text() {
    let statements = [
        psychology_articles::Entity::find()
            .build(DatabaseBackend::MySql)
            .to_string(),
        psychology_qna::Entity::find()
            .build(DatabaseBackend::MySql)
            .to_string(),
        psychology_resources::Entity::find()
            .build(DatabaseBackend::MySql)
            .to_string(),
    ];

    for statement in statements {
        assert!(
            !statement.to_ascii_lowercase().contains(" as text"),
            "MySQL query contains unsupported CAST target: {statement}"
        );
    }
}
