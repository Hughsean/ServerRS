ALTER TABLE knowledge_vector_manifests
    CHANGE COLUMN qdrant_collection vector_index_name VARCHAR(128) NOT NULL COMMENT '向量索引名称',
    CHANGE COLUMN qdrant_point_id vector_point_id CHAR(64) NOT NULL COMMENT '确定性向量 point ID';

ALTER TABLE knowledge_vector_manifests
    RENAME INDEX uk_vector_manifests_qdrant_point TO uk_vector_manifests_vector_point;
