-- QQBot / 个人 QQ 智能秘书 Schema Baseline v2
-- 生成时间：2026-08-06（Asia/Shanghai）
-- 仅包含最终结构，不包含业务数据、测试数据、凭据或历史 ALTER/DROP 过程。
-- 仅允许用于全新 QQBot 数据库；既有数据库继续使用历史增量迁移记录升级。
-- 为兼容连接池，先创建全部表，再统一添加外键；不依赖会话级 FOREIGN_KEY_CHECKS。

SET NAMES utf8mb4;

CREATE TABLE IF NOT EXISTS `secretary_accounts` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `source_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_account_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'active',
  `policy_epoch` bigint unsigned NOT NULL DEFAULT '0',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_account_source` (`source_channel`,`platform_account_id`),
  CONSTRAINT `chk_secretary_accounts_source` CHECK ((`source_channel` in (_utf8mb4'napcat',_utf8mb4'qq_open_platform'))),
  CONSTRAINT `chk_secretary_accounts_status` CHECK ((`status` in (_utf8mb4'active',_utf8mb4'disabled')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书接入账号主体；不同 NapCat 账号或官方 Bot 严格隔离';

CREATE TABLE IF NOT EXISTS `secretary_action_audit` (
  `audit_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `event_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `detail_json` json NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`audit_id`),
  KEY `idx_secretary_action_audit_run` (`run_id`,`created_at`),
  CONSTRAINT `chk_secretary_action_audit_kind` CHECK ((`event_kind` in (_utf8mb4'created',_utf8mb4'claimed',_utf8mb4'suspended',_utf8mb4'resumed',_utf8mb4'effect_applied',_utf8mb4'completed',_utf8mb4'failed',_utf8mb4'released')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Action 运行审计；不可变追加，用于排查与合规';

CREATE TABLE IF NOT EXISTS `secretary_action_checkpoints` (
  `checkpoint_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `checkpoint_json` json NOT NULL,
  `checkpoint_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'active',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `consumed_at` datetime(6) DEFAULT NULL,
  PRIMARY KEY (`checkpoint_id`),
  KEY `idx_secretary_action_checkpoint_run` (`run_id`,`created_at`),
  KEY `idx_secretary_action_checkpoint_status` (`checkpoint_status`,`created_at`),
  CONSTRAINT `chk_secretary_action_checkpoint_status` CHECK ((`checkpoint_status` in (_utf8mb4'active',_utf8mb4'consumed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Action Graph 完整 Checkpoint；恢复时 CAS 单次消费（checkpoint_status active->consumed），支持进程重启';

CREATE TABLE IF NOT EXISTS `secretary_action_effect_receipts` (
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_json` json NOT NULL,
  `result_ref` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`effect_id`),
  KEY `idx_secretary_action_effect_run` (`run_id`,`created_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Action Effect 幂等回执；effect_id 全局唯一，INSERT IGNORE 去重';

CREATE TABLE IF NOT EXISTS `secretary_action_responses` (
  `response_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `response_json` json NOT NULL,
  `serialized_bytes` int unsigned NOT NULL,
  `invalidated` tinyint(1) NOT NULL DEFAULT '0',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`response_id`),
  UNIQUE KEY `uk_secretary_action_response_run` (`run_id`),
  CONSTRAINT `chk_secretary_action_response_bytes` CHECK ((`serialized_bytes` <= 65536))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 响应草稿；64KB 限制由应用层验证，来源失效时标记 invalidated';

CREATE TABLE IF NOT EXISTS `secretary_action_runs` (
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_text` varchar(4000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `occurred_at_unix_secs` bigint NOT NULL,
  `timezone_offset_secs` bigint NOT NULL,
  `timezone_name` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'UTC',
  `recent_events_json` json NOT NULL,
  `planner_version` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'v1',
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `worker_id` varchar(191) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `next_eligible_at` datetime(6) DEFAULT NULL,
  `attempt` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(1000) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `last_checkpoint_json` json DEFAULT NULL,
  `response_draft_json` json DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `completed_at` datetime(6) DEFAULT NULL,
  PRIMARY KEY (`run_id`),
  UNIQUE KEY `uk_secretary_action_run_command` (`account_id`,`command_source_event_id`,`planner_version`),
  UNIQUE KEY `uk_secretary_action_run_lease` (`lease_token`),
  KEY `fk_secretary_action_run_command` (`command_source_event_id`),
  KEY `idx_secretary_action_claim` (`status`,`next_eligible_at`,`created_at`),
  KEY `idx_secretary_action_account` (`account_id`,`status`,`created_at`),
  KEY `idx_secretary_action_lease_expiry` (`status`,`lease_expires_at`),
  CONSTRAINT `chk_secretary_action_run_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'running',_utf8mb4'suspended',_utf8mb4'completed',_utf8mb4'failed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Action Planner 运行；CAS 领取 + lease fencing + 业务幂等创建';

CREATE TABLE IF NOT EXISTS `secretary_agenda_items` (
  `item_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `item_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `title` varchar(500) COLLATE utf8mb4_unicode_ci NOT NULL,
  `scheduled_at_unix_secs` bigint DEFAULT NULL,
  `timezone_name` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `item_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'scheduled',
  `version` bigint unsigned NOT NULL DEFAULT '1',
  `created_command_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_command_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `create_idempotency_key` varchar(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`item_id`),
  UNIQUE KEY `uk_secretary_agenda_create` (`account_id`,`create_idempotency_key`),
  KEY `fk_secretary_agenda_created_command` (`created_command_event_id`),
  KEY `fk_secretary_agenda_current_command` (`current_command_event_id`),
  KEY `idx_secretary_agenda_due` (`account_id`,`item_status`,`scheduled_at_unix_secs`,`item_id`),
  CONSTRAINT `chk_secretary_agenda_kind` CHECK ((`item_kind` in (_utf8mb4'schedule',_utf8mb4'task',_utf8mb4'reminder'))),
  CONSTRAINT `chk_secretary_agenda_status` CHECK ((`item_status` in (_utf8mb4'scheduled',_utf8mb4'completed',_utf8mb4'cancelled'))),
  CONSTRAINT `chk_secretary_agenda_version` CHECK ((`version` > 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 日程、任务和提醒；UTC 时间 + IANA timezone + version fencing';

CREATE TABLE IF NOT EXISTS `secretary_agenda_mutation_audit` (
  `audit_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `item_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `mutation_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `from_version` bigint unsigned DEFAULT NULL,
  `to_version` bigint unsigned NOT NULL,
  `detail_json` json NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`audit_id`),
  UNIQUE KEY `uk_secretary_agenda_effect` (`effect_id`),
  KEY `fk_secretary_agenda_audit_account` (`account_id`),
  KEY `fk_secretary_agenda_audit_command` (`command_source_event_id`),
  KEY `fk_secretary_agenda_audit_run` (`run_id`),
  KEY `idx_secretary_agenda_audit_item` (`item_id`,`to_version`),
  CONSTRAINT `chk_secretary_agenda_mutation_kind` CHECK ((`mutation_kind` in (_utf8mb4'create',_utf8mb4'reschedule',_utf8mb4'complete',_utf8mb4'cancel',_utf8mb4'snooze')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Agenda mutation 不可变审计；effect_id 同时作为业务幂等键';

CREATE TABLE IF NOT EXISTS `secretary_artifact_derivations` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `last_error_code` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_artifact_derivation_claim` (`status`,`lease_expires_at`,`created_at`),
  CONSTRAINT `chk_secretary_artifact_derivation_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'claimed',_utf8mb4'completed',_utf8mb4'failed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Durable derivation jobs for source-message Artifact envelopes';

CREATE TABLE IF NOT EXISTS `secretary_artifact_reprocess_audit` (
  `audit_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `requested_limit` smallint unsigned NOT NULL,
  `requeued_count` smallint unsigned NOT NULL,
  `requeued_source_event_ids` json NOT NULL,
  `reason` varchar(1000) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`audit_id`),
  UNIQUE KEY `uk_secretary_artifact_reprocess_effect` (`effect_id`),
  KEY `idx_secretary_artifact_reprocess_account` (`account_id`,`created_at`),
  KEY `fk_secretary_artifact_reprocess_command` (`command_source_event_id`),
  KEY `fk_secretary_artifact_reprocess_run` (`run_id`),
  CONSTRAINT `chk_secretary_artifact_reprocess_limit` CHECK ((`requested_limit` between 1 and 100)),
  CONSTRAINT `chk_secretary_artifact_reprocess_count` CHECK ((`requeued_count` <= `requested_limit`))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='OPS-004 失败 Artifact 派生重处理不可变审计';

CREATE TABLE IF NOT EXISTS `secretary_artifacts` (
  `artifact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `artifact_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_reference` varchar(500) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `display_name` varchar(500) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `mime_type` varchar(200) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `size_bytes` bigint unsigned DEFAULT NULL,
  `hash_or_source_key` varchar(500) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `description` varchar(2000) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `availability` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'available',
  `content_policy` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'normal',
  `created_at_unix_secs` bigint NOT NULL,
  `ttl_expires_at_unix_secs` bigint DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`artifact_id`),
  KEY `fk_secretary_artifact_conversation` (`conversation_id`),
  KEY `idx_secretary_artifact_event` (`account_id`,`source_event_id`,`availability`),
  KEY `idx_secretary_artifact_ttl` (`ttl_expires_at_unix_secs`),
  KEY `idx_secretary_artifact_source_event` (`source_event_id`,`availability`),
  CONSTRAINT `chk_secretary_artifact_availability` CHECK ((`availability` in (_utf8mb4'available',_utf8mb4'expired',_utf8mb4'recalled',_utf8mb4'owner_deleted',_utf8mb4'policy_blocked'))),
  CONSTRAINT `chk_secretary_artifact_kind` CHECK ((`artifact_kind` in (_utf8mb4'image',_utf8mb4'file',_utf8mb4'record',_utf8mb4'video',_utf8mb4'forward',_utf8mb4'rich_json',_utf8mb4'rich_xml',_utf8mb4'rich_card'))),
  CONSTRAINT `chk_secretary_artifact_policy` CHECK ((`content_policy` in (_utf8mb4'normal',_utf8mb4'local_only',_utf8mb4'envelope_only',_utf8mb4'never_long_term')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='富消息 Artifact 信封（不自动下载；有界；TTL；撤回/删除/策略失效传播）';

CREATE TABLE IF NOT EXISTS `secretary_backfill_leases` (
  `backfill_run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`backfill_run_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='回补运行当前租约持有者的 fencing token；每次过期接管均轮换';

CREATE TABLE IF NOT EXISTS `secretary_backfill_runs` (
  `backfill_run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `completeness` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'unprovable',
  `failure_class` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `pages_read` int unsigned NOT NULL DEFAULT '0',
  `events_read` int unsigned NOT NULL DEFAULT '0',
  `accepted` int unsigned NOT NULL DEFAULT '0',
  `duplicates` int unsigned NOT NULL DEFAULT '0',
  `budget_exhausted` tinyint(1) NOT NULL DEFAULT '0',
  `anomaly_count` int unsigned NOT NULL DEFAULT '0',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `completed_at` datetime(6) DEFAULT NULL,
  PRIMARY KEY (`backfill_run_id`),
  KEY `fk_secretary_backfill_run_connection` (`connection_epoch_id`),
  KEY `idx_secretary_backfill_run_gap` (`gap_id`),
  KEY `idx_secretary_backfill_run_claim` (`status`,`lease_expires_at`),
  KEY `idx_secretary_backfill_run_status` (`account_id`,`status`),
  CONSTRAINT `chk_secretary_backfill_completeness` CHECK ((`completeness` in (_utf8mb4'proven_complete',_utf8mb4'known_scopes_complete',_utf8mb4'unprovable',_utf8mb4'unrecoverable'))),
  CONSTRAINT `chk_secretary_backfill_run_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'backfilling',_utf8mb4'verified_complete',_utf8mb4'unprovable',_utf8mb4'unrecoverable')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='一次历史回补运行：Gap、租约、进度、完整性证据与终态（一个 Gap 可有多条历史运行）';

CREATE TABLE IF NOT EXISTS `secretary_backfill_scopes` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `backfill_run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `scope_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `scope_key` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `last_anchor_message_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `last_anchor_message_seq` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `pages_read` int unsigned NOT NULL DEFAULT '0',
  `events_read` int unsigned NOT NULL DEFAULT '0',
  `accepted` int unsigned NOT NULL DEFAULT '0',
  `duplicates` int unsigned NOT NULL DEFAULT '0',
  `reached_boundary` tinyint(1) NOT NULL DEFAULT '0',
  `anomalies` json DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_backfill_scope` (`backfill_run_id`,`scope_key`),
  KEY `fk_secretary_backfill_scope_account` (`account_id`),
  KEY `fk_secretary_backfill_scope_conversation` (`conversation_id`),
  KEY `idx_secretary_backfill_scope_status` (`backfill_run_id`,`status`),
  CONSTRAINT `chk_secretary_backfill_scope_kind` CHECK ((`scope_kind` in (_utf8mb4'group',_utf8mb4'private',_utf8mb4'owner_control'))),
  CONSTRAINT `chk_secretary_backfill_scope_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'backfilling',_utf8mb4'verified_complete',_utf8mb4'unprovable',_utf8mb4'unrecoverable')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='单个会话 Scope 的回补进度与证据；锚点绑定账号视角平台消息 ID';

CREATE TABLE IF NOT EXISTS `secretary_connection_epochs` (
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'connecting',
  `started_at` datetime(6) NOT NULL,
  `connected_at` datetime(6) DEFAULT NULL,
  `ended_at` datetime(6) DEFAULT NULL,
  `last_event_at` datetime(6) DEFAULT NULL,
  `last_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `end_reason` varchar(32) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`connection_epoch_id`),
  KEY `idx_secretary_connection_account_time` (`account_id`,`started_at`),
  KEY `idx_secretary_connection_status` (`status`,`updated_at`),
  CONSTRAINT `chk_secretary_connection_source` CHECK ((`source_channel` in (_utf8mb4'napcat',_utf8mb4'qq_open_platform'))),
  CONSTRAINT `chk_secretary_connection_status` CHECK ((`status` in (_utf8mb4'connecting',_utf8mb4'connected',_utf8mb4'disconnected',_utf8mb4'shutdown',_utf8mb4'connect_failed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书传输连接周期、断连原因和最后成功事件';

CREATE TABLE IF NOT EXISTS `secretary_conversations` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `account_id` bigint unsigned NOT NULL,
  `conversation_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `memory_mode` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'normal',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_conversation` (`account_id`,`conversation_kind`,`platform_conversation_id`),
  KEY `idx_secretary_conversation_memory` (`account_id`,`memory_mode`),
  CONSTRAINT `chk_secretary_conversation_kind` CHECK ((`conversation_kind` in (_utf8mb4'private',_utf8mb4'group',_utf8mb4'owner_control'))),
  CONSTRAINT `chk_secretary_conversation_memory` CHECK ((`memory_mode` in (_utf8mb4'normal',_utf8mb4'local_only',_utf8mb4'envelope_only',_utf8mb4'never_long_term')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书协议无关会话及数据保留策略';

CREATE TABLE IF NOT EXISTS `secretary_directory_gap_freeze` (
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `snapshot_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `account_id` bigint unsigned NOT NULL,
  `frozen_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`gap_id`),
  KEY `fk_secretary_directory_freeze_snapshot` (`snapshot_id`),
  KEY `idx_secretary_directory_freeze_account` (`account_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Gap 创建时冻结的目录快照引用；回补读此快照而非实时 Cursor';

CREATE TABLE IF NOT EXISTS `secretary_directory_scopes` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `snapshot_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `scope_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `conversation_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `boundary_message_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `boundary_msg_time` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `display_name` varchar(500) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_directory_scope` (`snapshot_id`,`scope_kind`,`platform_conversation_id`),
  KEY `idx_secretary_directory_scope_account` (`account_id`,`scope_kind`),
  CONSTRAINT `chk_secretary_directory_scope_conv_kind` CHECK ((`conversation_kind` in (_utf8mb4'private',_utf8mb4'group',_utf8mb4'owner_control'))),
  CONSTRAINT `chk_secretary_directory_scope_kind` CHECK ((`scope_kind` in (_utf8mb4'friend',_utf8mb4'group',_utf8mb4'recent_unconfirmed',_utf8mb4'deleted',_utf8mb4'exited',_utf8mb4'inaccessible')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='目录快照内的会话 Scope 条目：类别、边界与显示名（平台 ID 字符串保留精度）';

CREATE TABLE IF NOT EXISTS `secretary_directory_snapshots` (
  `snapshot_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_api` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'uncertain',
  `evidence_json` json NOT NULL,
  `scope_count` int unsigned NOT NULL DEFAULT '0',
  `created_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`snapshot_id`),
  KEY `idx_secretary_directory_snapshot_account_time` (`account_id`,`created_at_unix_secs`),
  CONSTRAINT `chk_secretary_directory_snapshot_source` CHECK ((`source_api` in (_utf8mb4'friend_group_recent',_utf8mb4'recent_contact',_utf8mb4'observed_from_history'))),
  CONSTRAINT `chk_secretary_directory_snapshot_status` CHECK ((`status` in (_utf8mb4'known_scopes_complete',_utf8mb4'verified_complete',_utf8mb4'uncertain',_utf8mb4'unavailable',_utf8mb4'api_timeout',_utf8mb4'api_oversized',_utf8mb4'api_deferred')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='账号会话目录快照：来源 API、状态、证据与 Scope 数量（账号作用域、幂等）';

CREATE TABLE IF NOT EXISTS `secretary_event_ingestion` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `observed_at` datetime(6) NOT NULL,
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_event_ingestion_epoch` (`connection_epoch_id`,`observed_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='实时 SourceEvent 到连接周期的不可变来源关联';

CREATE TABLE IF NOT EXISTS `secretary_event_threads` (
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'open',
  `root_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `latest_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `opened_at_unix_secs` bigint NOT NULL,
  `latest_occurred_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`thread_id`),
  KEY `fk_secretary_event_thread_root` (`root_event_id`),
  KEY `fk_secretary_event_thread_latest` (`latest_event_id`),
  KEY `idx_secretary_event_thread_account_status` (`account_id`,`status`,`updated_at`),
  CONSTRAINT `chk_secretary_event_thread_status` CHECK ((`status` in (_utf8mb4'open',_utf8mb4'waiting',_utf8mb4'resolved',_utf8mb4'closed',_utf8mb4'reopened')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书确定性事件线程；生命周期与消息正文分离';

CREATE TABLE IF NOT EXISTS `secretary_follow_up_items` (
  `follow_up_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_memory_fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_version` bigint unsigned NOT NULL DEFAULT '1',
  `reason_code` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `due_at_unix_secs` bigint NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'scheduled',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`follow_up_id`),
  UNIQUE KEY `uk_secretary_follow_up_source` (`source_memory_fact_id`,`reason_code`),
  KEY `fk_secretary_follow_up_account` (`account_id`),
  KEY `idx_secretary_follow_up_due` (`status`,`due_at_unix_secs`,`follow_up_id`),
  KEY `idx_secretary_follow_up_scan_source_version` (`status`,`due_at_unix_secs`,`source_version`,`follow_up_id`),
  CONSTRAINT `chk_secretary_follow_up_reason` CHECK ((`reason_code` in (_utf8mb4'commitment_due',_utf8mb4'project_blocked'))),
  CONSTRAINT `chk_secretary_follow_up_source_version` CHECK ((`source_version` > 0)),
  CONSTRAINT `chk_secretary_follow_up_status` CHECK ((`status` in (_utf8mb4'scheduled',_utf8mb4'completed',_utf8mb4'dismissed',_utf8mb4'superseded')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='由来源化承诺记忆生成的持久化跟进事项';

CREATE TABLE IF NOT EXISTS `secretary_follow_up_owner_controls` (
  `control_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `follow_up_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_source_version` bigint unsigned NOT NULL,
  `current_source_version` bigint unsigned NOT NULL,
  `previous_due_at_unix_secs` bigint DEFAULT NULL,
  `current_due_at_unix_secs` bigint DEFAULT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `control_kind` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'dismiss',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`control_id`),
  UNIQUE KEY `uk_secretary_follow_up_control_effect_item` (`effect_id`,`follow_up_id`),
  KEY `fk_secretary_follow_up_control_run` (`run_id`),
  KEY `fk_secretary_follow_up_control_item` (`follow_up_id`),
  KEY `fk_secretary_follow_up_control_command` (`command_source_event_id`),
  KEY `idx_secretary_follow_up_control_item` (`account_id`,`follow_up_id`,`created_at`),
  CONSTRAINT `chk_secretary_follow_up_control_due` CHECK ((((`control_kind` = _utf8mb4'dismiss') and (`current_status` = _utf8mb4'dismissed')) or ((`control_kind` = _utf8mb4'snooze') and (`current_status` = _utf8mb4'scheduled') and (`previous_due_at_unix_secs` is not null) and (`current_due_at_unix_secs` is not null) and (`current_due_at_unix_secs` > `previous_due_at_unix_secs`)) or ((`control_kind` = _utf8mb4'complete') and (`current_status` = _utf8mb4'completed')))),
  CONSTRAINT `chk_secretary_follow_up_control_kind` CHECK ((`control_kind` in (_utf8mb4'dismiss',_utf8mb4'snooze',_utf8mb4'complete'))),
  CONSTRAINT `chk_secretary_follow_up_control_status` CHECK (((`previous_status` = _utf8mb4'scheduled') and (`current_status` in (_utf8mb4'dismissed',_utf8mb4'scheduled',_utf8mb4'completed')))),
  CONSTRAINT `chk_secretary_follow_up_control_version` CHECK (((`previous_source_version` > 0) and (`current_source_version` = (`previous_source_version` + 1))))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对 FollowUp 的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

CREATE TABLE IF NOT EXISTS `secretary_gap_boundaries` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `conversation_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `boundary_message_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `boundary_occurred_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_gap_boundary` (`gap_id`,`conversation_id`),
  KEY `fk_secretary_gap_boundary_account` (`account_id`),
  KEY `fk_secretary_gap_boundary_conversation` (`conversation_id`),
  KEY `idx_secretary_gap_boundary_gap` (`gap_id`),
  CONSTRAINT `chk_secretary_gap_boundary_kind` CHECK ((`conversation_kind` in (_utf8mb4'private',_utf8mb4'group',_utf8mb4'owner_control')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Gap 创建时冻结的会话游标快照；回补边界按平台消息 ID 匹配，非领取时实时游标';

CREATE TABLE IF NOT EXISTS `secretary_gap_reclaim_schedule` (
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `next_eligible_at` datetime(6) DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`gap_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='uncertain Gap 的再次领取退避时间；为 NULL 或已过期即立即可领取';

CREATE TABLE IF NOT EXISTS `secretary_gap_signal_scopes` (
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `signal_kind` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`gap_id`,`conversation_id`),
  KEY `idx_secretary_gap_signal_scope_conversation` (`conversation_id`),
  CONSTRAINT `chk_secretary_gap_signal_scope_kind` CHECK ((`signal_kind` = _utf8mb4'non_message_reference'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Non-message notice requested conversation repair scopes for an active ingestion gap';

CREATE TABLE IF NOT EXISTS `secretary_ingestion_cursors` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned DEFAULT NULL,
  `scope_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `scope_key` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `last_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `last_platform_event_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `last_occurred_at_unix_secs` bigint NOT NULL,
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uk_secretary_cursor_scope` (`account_id`,`scope_kind`,`scope_key`),
  KEY `fk_secretary_cursor_conversation` (`conversation_id`),
  KEY `fk_secretary_cursor_event` (`last_source_event_id`),
  KEY `fk_secretary_cursor_connection` (`connection_epoch_id`),
  KEY `idx_secretary_cursor_updated` (`account_id`,`updated_at`),
  CONSTRAINT `chk_secretary_cursor_scope` CHECK ((`scope_kind` in (_utf8mb4'account',_utf8mb4'conversation')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='账号及会话级稳定接入游标；为历史回补提供锚点';

CREATE TABLE IF NOT EXISTS `secretary_ingestion_gaps` (
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `gap_started_at` datetime(6) NOT NULL,
  `gap_ended_at` datetime(6) DEFAULT NULL,
  `status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'uncertain',
  `reason` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`gap_id`),
  UNIQUE KEY `uk_secretary_gap_connection` (`connection_epoch_id`),
  KEY `idx_secretary_gap_status` (`account_id`,`status`,`gap_started_at`),
  KEY `idx_secretary_gap_open` (`account_id`,`gap_ended_at`),
  CONSTRAINT `chk_secretary_gap_status` CHECK ((`status` in (_utf8mb4'uncertain',_utf8mb4'backfilling',_utf8mb4'verified_complete',_utf8mb4'unrecoverable')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='无法证明消息连续性的时间窗；回补验证前始终保持 uncertain';

CREATE TABLE IF NOT EXISTS `secretary_memory_candidate_controls` (
  `control_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `control_kind` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_candidate_version` bigint unsigned NOT NULL,
  `current_candidate_version` bigint unsigned NOT NULL,
  `fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`control_id`),
  UNIQUE KEY `uk_secretary_candidate_control_effect` (`effect_id`,`candidate_id`),
  KEY `fk_secretary_candidate_control_run` (`run_id`),
  KEY `fk_secretary_candidate_control_candidate` (`candidate_id`),
  KEY `fk_secretary_candidate_control_fact` (`fact_id`),
  KEY `fk_secretary_candidate_control_command` (`command_source_event_id`),
  KEY `idx_secretary_candidate_control_candidate` (`account_id`,`candidate_id`,`created_at`),
  CONSTRAINT `chk_secretary_candidate_control_kind` CHECK ((`control_kind` in (_utf8mb4'approve',_utf8mb4'approve_conflict',_utf8mb4'reject'))),
  CONSTRAINT `chk_secretary_candidate_control_status` CHECK ((((`control_kind` = _utf8mb4'approve') and (`previous_status` = _utf8mb4'proposed') and (`current_status` = _utf8mb4'approved')) or ((`control_kind` = _utf8mb4'reject') and (`previous_status` = _utf8mb4'proposed') and (`current_status` = _utf8mb4'rejected')) or ((`control_kind` = _utf8mb4'approve_conflict') and (`previous_status` = _utf8mb4'proposed') and (`current_status` = _utf8mb4'proposed')))),
  CONSTRAINT `chk_secretary_candidate_control_version` CHECK (((`previous_candidate_version` > 0) and (((`control_kind` = _utf8mb4'approve_conflict') and (`current_candidate_version` = `previous_candidate_version`)) or ((`control_kind` in (_utf8mb4'approve',_utf8mb4'reject')) and (`current_candidate_version` = (`previous_candidate_version` + 1))))))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对记忆候选的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

CREATE TABLE IF NOT EXISTS `secretary_memory_candidate_deferred` (
  `account_id` bigint unsigned NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `received_at` datetime(6) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`account_id`,`source_event_id`),
  KEY `fk_secretary_candidate_deferred_event` (`source_event_id`),
  KEY `idx_secretary_candidate_deferred_order` (`account_id`,`received_at`,`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='远程模式被过滤的 local_only 事件；切换本地模型后先于主游标消费';

CREATE TABLE IF NOT EXISTS `secretary_memory_candidate_processing_state` (
  `account_id` bigint unsigned NOT NULL,
  `last_received_at` datetime(6) DEFAULT NULL,
  `last_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`account_id`),
  KEY `idx_secretary_candidate_state_lease` (`lease_token`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='记忆候选提取的持久化游标与租约（崩溃后从游标恢复）';

CREATE TABLE IF NOT EXISTS `secretary_memory_candidate_sources` (
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `actor_platform_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `content_trust_level` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `occurred_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`candidate_id`,`source_event_id`),
  KEY `fk_secretary_candidate_source_account` (`account_id`),
  KEY `idx_secretary_candidate_source_event` (`source_event_id`),
  CONSTRAINT `chk_secretary_candidate_source_trust` CHECK ((`content_trust_level` in (_utf8mb4'normal',_utf8mb4'local_only',_utf8mb4'envelope_only',_utf8mb4'never_long_term')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='记忆候选的精确来源；批准与失效均按此表复验';

CREATE TABLE IF NOT EXISTS `secretary_memory_candidates` (
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `candidate_kind` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `subject_key` varchar(191) COLLATE utf8mb4_unicode_ci NOT NULL,
  `payload_json` json NOT NULL,
  `candidate_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'proposed',
  `candidate_version` bigint unsigned NOT NULL DEFAULT '1',
  `extractor_version` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `deterministic_fingerprint` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`candidate_id`),
  UNIQUE KEY `uk_secretary_memory_candidate_fingerprint` (`account_id`,`deterministic_fingerprint`),
  KEY `idx_secretary_memory_candidate_status` (`account_id`,`candidate_status`,`updated_at`),
  CONSTRAINT `chk_secretary_memory_candidate_kind` CHECK ((`candidate_kind` in (_utf8mb4'person',_utf8mb4'project',_utf8mb4'commitment'))),
  CONSTRAINT `chk_secretary_memory_candidate_status` CHECK ((`candidate_status` in (_utf8mb4'proposed',_utf8mb4'approved',_utf8mb4'rejected',_utf8mb4'invalidated'))),
  CONSTRAINT `chk_secretary_memory_candidate_version` CHECK ((`candidate_version` > 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='结构化记忆候选（proposed 待 Owner 审批；fingerprint 唯一去重）';

CREATE TABLE IF NOT EXISTS `secretary_memory_deletions` (
  `fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `owner_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`fact_id`),
  KEY `idx_secretary_memory_deletion_command` (`command_source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对派生记忆执行删除的不可变审计记录；不隐式删除原始事件';

CREATE TABLE IF NOT EXISTS `secretary_memory_fact_sources` (
  `fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`fact_id`,`source_event_id`),
  KEY `idx_secretary_memory_source_event` (`source_event_id`,`fact_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='长期记忆到无损 SourceEvent 的可回读来源引用';

CREATE TABLE IF NOT EXISTS `secretary_memory_facts` (
  `fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `fact_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `subject_key` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `fact_json` json NOT NULL,
  `fact_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `confidence_bps` smallint unsigned NOT NULL,
  `valid_until_unix_secs` bigint DEFAULT NULL,
  `supersedes_fact_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`fact_id`),
  UNIQUE KEY `uk_secretary_memory_fact_supersedes` (`supersedes_fact_id`),
  KEY `idx_secretary_memory_fact_active` (`account_id`,`fact_kind`,`fact_status`,`valid_until_unix_secs`,`updated_at`),
  KEY `idx_secretary_memory_fact_subject` (`account_id`,`subject_key`,`fact_status`,`updated_at`),
  CONSTRAINT `chk_secretary_memory_fact_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_memory_fact_kind` CHECK ((`fact_kind` in (_utf8mb4'person',_utf8mb4'project',_utf8mb4'commitment'))),
  CONSTRAINT `chk_secretary_memory_fact_not_self` CHECK (((`supersedes_fact_id` is null) or (`supersedes_fact_id` <> `fact_id`))),
  CONSTRAINT `chk_secretary_memory_fact_status` CHECK ((`fact_status` in (_utf8mb4'proposed',_utf8mb4'confirmed',_utf8mb4'superseded',_utf8mb4'expired',_utf8mb4'deleted')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='人物、项目、承诺的类型化版本事实；摘要只导航，原始事件负责证据';

CREATE TABLE IF NOT EXISTS `secretary_message_contents` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `normalized_text` mediumtext COLLATE utf8mb4_unicode_ci NOT NULL,
  `segments` json NOT NULL,
  `mentioned_actor_ids` json NOT NULL,
  `mention_all` tinyint(1) NOT NULL DEFAULT '0',
  `content_mode` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'normal',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  CONSTRAINT `chk_secretary_content_mode` CHECK ((`content_mode` in (_utf8mb4'normal',_utf8mb4'local_only',_utf8mb4'envelope_only',_utf8mb4'never_long_term')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书消息正文、结构化消息段、@目标和内容策略';

CREATE TABLE IF NOT EXISTS `secretary_message_tombstones` (
  `tombstone_id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `account_id` bigint unsigned NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `recall_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `conversation_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `platform_message_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `correlation_key` varchar(500) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `invalidation_reason` varchar(500) COLLATE utf8mb4_unicode_ci NOT NULL,
  `invalidated_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`tombstone_id`),
  UNIQUE KEY `uk_secretary_tombstone_correlation` (`account_id`,`correlation_key`),
  KEY `fk_secretary_tombstone_recall` (`recall_event_id`),
  KEY `idx_secretary_tombstone_source_event` (`source_event_id`,`status`),
  KEY `idx_secretary_tombstone_account_status` (`account_id`,`status`),
  KEY `idx_secretary_tombstone_pending_lookup` (`account_id`,`channel`,`conversation_kind`,`platform_conversation_id`,`platform_message_id`,`status`),
  CONSTRAINT `chk_secretary_tombstone_channel` CHECK ((`channel` in (_utf8mb4'napcat',_utf8mb4'qq_open_platform'))),
  CONSTRAINT `chk_secretary_tombstone_conv_kind` CHECK ((`conversation_kind` in (_utf8mb4'private',_utf8mb4'group',_utf8mb4'owner_control'))),
  CONSTRAINT `chk_secretary_tombstone_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'applied',_utf8mb4'idempotent_reapply')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='被撤回原消息的 tombstone 记录（pending/applied；不物理删除审计历史）';

CREATE TABLE IF NOT EXISTS `secretary_notification_candidates` (
  `notification_candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_kind` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_version` bigint unsigned NOT NULL,
  `match_key_json` json NOT NULL,
  `candidate_status` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`notification_candidate_id`),
  UNIQUE KEY `uk_secretary_notification_candidate_source` (`account_id`,`source_kind`,`source_id`,`source_version`),
  KEY `idx_secretary_notification_candidate_pending` (`account_id`,`candidate_status`,`updated_at`,`notification_candidate_id`),
  CONSTRAINT `chk_secretary_notification_candidate_kind` CHECK ((`source_kind` in (_utf8mb4'agenda',_utf8mb4'follow_up',_utf8mb4'response_expectation'))),
  CONSTRAINT `chk_secretary_notification_candidate_status` CHECK ((`candidate_status` in (_ascii'pending',_ascii'delayed',_ascii'reminded',_ascii'suppressed',_ascii'expired',_ascii'failed_terminal')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Agenda 或 FollowUp 产生的版本化通知候选';

CREATE TABLE IF NOT EXISTS `secretary_notification_decisions` (
  `notification_decision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `evaluation_request_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `notification_candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_decision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `policy_revision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `evaluator_version` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `outcome` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason_code` varchar(256) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `next_allowed_at_unix_secs` bigint DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`notification_decision_id`),
  KEY `fk_secretary_notification_decision_previous` (`previous_decision_id`),
  KEY `fk_secretary_notification_decision_revision` (`policy_revision_id`),
  KEY `idx_secretary_notification_decision_candidate` (`notification_candidate_id`,`created_at`),
  KEY `idx_secretary_notification_decision_request` (`evaluation_request_id`,`created_at`,`notification_decision_id`),
  CONSTRAINT `chk_secretary_notification_decision_outcome` CHECK ((`outcome` in (_ascii'remind',_ascii'delay',_ascii'suppress',_ascii'candidate_expired',_ascii'evaluation_failed_terminal',_ascii'delivery_window_expired',_ascii'schedule_time_ambiguous')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='追加式通知策略决策审计';

CREATE TABLE IF NOT EXISTS `secretary_notification_evaluation_requests` (
  `evaluation_request_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `notification_candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `evaluation_generation` bigint unsigned NOT NULL,
  `trigger_kind` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `request_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at_unix_secs` bigint DEFAULT NULL,
  `attempt` bigint unsigned NOT NULL DEFAULT '0',
  `next_allowed_at_unix_secs` bigint DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`evaluation_request_id`),
  UNIQUE KEY `uk_secretary_notification_evaluation_generation` (`notification_candidate_id`,`evaluation_generation`),
  KEY `idx_secretary_notification_evaluation_claim` (`request_status`,`next_allowed_at_unix_secs`,`lease_expires_at_unix_secs`,`evaluation_request_id`),
  CONSTRAINT `chk_secretary_notification_evaluation_generation` CHECK ((`evaluation_generation` > 0)),
  CONSTRAINT `chk_secretary_notification_evaluation_status` CHECK ((`request_status` in (_utf8mb4'pending',_utf8mb4'claimed',_utf8mb4'completed',_utf8mb4'terminal')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='通知候选的可租约三阶段求值请求';

CREATE TABLE IF NOT EXISTS `secretary_notification_feedback` (
  `feedback_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `notification_candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `important` tinyint(1) NOT NULL,
  `promote_to_rule` tinyint(1) NOT NULL DEFAULT '0',
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `audit_summary` varchar(1024) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`feedback_id`),
  UNIQUE KEY `uk_secretary_notification_feedback_command` (`account_id`,`command_source_event_id`,`important`),
  KEY `fk_secretary_notification_feedback_command` (`command_source_event_id`),
  KEY `idx_secretary_notification_feedback_candidate` (`notification_candidate_id`,`created_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对单条通知的结构化重要性反馈';

CREATE TABLE IF NOT EXISTS `secretary_notification_outbox` (
  `notification_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `command_account_id` bigint unsigned DEFAULT NULL,
  `owner_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `follow_up_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `agenda_item_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `agenda_version` bigint unsigned DEFAULT NULL,
  `notification_candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `notification_decision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `occurrence_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `scheduled_at_unix_secs` bigint NOT NULL,
  `notification_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `payload_json` json NOT NULL,
  `delivery_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `last_error_code` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `platform_message_id` varchar(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `delivered_at` datetime(6) DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`notification_id`),
  UNIQUE KEY `uk_secretary_notification_follow_up` (`follow_up_id`,`notification_kind`),
  UNIQUE KEY `uk_secretary_notification_agenda` (`agenda_item_id`,`agenda_version`,`notification_kind`),
  UNIQUE KEY `uk_secretary_notification_occurrence` (`occurrence_id`),
  KEY `fk_secretary_notification_account` (`account_id`),
  KEY `idx_secretary_notification_claim` (`delivery_status`,`scheduled_at_unix_secs`,`lease_expires_at`,`notification_id`),
  KEY `fk_secretary_notification_outbox_candidate` (`notification_candidate_id`),
  KEY `fk_secretary_notification_outbox_decision` (`notification_decision_id`),
  KEY `idx_secretary_notification_policy_recipient` (`command_account_id`,`owner_actor_id`,`delivery_status`,`notification_id`),
  CONSTRAINT `chk_secretary_notification_kind` CHECK ((`notification_kind` in (_utf8mb4'owner_reminder',_utf8mb4'owner_agenda_reminder',_utf8mb4'owner_policy_reminder'))),
  CONSTRAINT `chk_secretary_notification_source` CHECK ((((`follow_up_id` is not null) and (`agenda_item_id` is null) and (`agenda_version` is null) and (`notification_candidate_id` is null) and (`notification_decision_id` is null) and (`command_account_id` is null) and (`owner_actor_id` is null)) or ((`follow_up_id` is null) and (`agenda_item_id` is not null) and (`agenda_version` is not null) and (`notification_candidate_id` is null) and (`notification_decision_id` is null) and (`command_account_id` is null) and (`owner_actor_id` is null)) or ((`follow_up_id` is null) and (`agenda_item_id` is null) and (`agenda_version` is null) and (`notification_candidate_id` is not null) and (`notification_decision_id` is not null) and (`command_account_id` is not null) and (`owner_actor_id` is not null)))),
  CONSTRAINT `chk_secretary_notification_status` CHECK ((`delivery_status` in (_utf8mb4'pending',_utf8mb4'claimed',_utf8mb4'delivered',_utf8mb4'failed',_utf8mb4'suppressed',_utf8mb4'unknown_commit')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='平台无关通知 Outbox；QQ 开放平台接入前只入队、不发送';

CREATE TABLE IF NOT EXISTS `secretary_notification_policy_families` (
  `policy_family_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `canonical_scope_key` varchar(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `policy_kind` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_revision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `generation` bigint unsigned NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`policy_family_id`),
  UNIQUE KEY `uk_secretary_notification_policy_family` (`account_id`,`canonical_scope_key`,`policy_kind`),
  UNIQUE KEY `uk_secretary_notification_policy_family_head` (`policy_family_id`,`current_revision_id`),
  CONSTRAINT `chk_secretary_notification_policy_family_generation` CHECK ((`generation` > 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='通知策略稳定 Family 与可 CAS 更新的 Head';

CREATE TABLE IF NOT EXISTS `secretary_notification_policy_revisions` (
  `policy_revision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `policy_family_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `revision_number` bigint unsigned NOT NULL,
  `supersedes_revision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `revision_kind` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `rule_json` json DEFAULT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `audit_summary` varchar(1024) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`policy_revision_id`),
  UNIQUE KEY `uk_secretary_notification_policy_revision_number` (`policy_family_id`,`revision_number`),
  UNIQUE KEY `uk_secretary_notification_policy_revision_family_id` (`policy_family_id`,`policy_revision_id`),
  KEY `fk_secretary_notification_policy_revision_supersedes` (`supersedes_revision_id`),
  KEY `fk_secretary_notification_policy_revision_command` (`command_source_event_id`),
  CONSTRAINT `chk_secretary_notification_policy_revision_kind` CHECK ((`revision_kind` in (_utf8mb4'rule',_utf8mb4'tombstone'))),
  CONSTRAINT `chk_secretary_notification_policy_revision_number` CHECK ((`revision_number` > 0)),
  CONSTRAINT `chk_secretary_notification_policy_revision_shape` CHECK ((((`revision_kind` = _utf8mb4'rule') and (`rule_json` is not null)) or ((`revision_kind` = _utf8mb4'tombstone') and (`rule_json` is null))))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='不可变通知策略 revision；停用使用 tombstone';

CREATE TABLE IF NOT EXISTS `secretary_notification_reconciliation_leases` (
  `lease_name` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`lease_name`),
  CONSTRAINT `chk_secretary_notification_reconciliation_lease_name` CHECK ((`lease_name` = _utf8mb4'legacy_owner_outbox_v1'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Legacy Owner Outbox reconciliation singleton lease';

INSERT IGNORE INTO `secretary_notification_reconciliation_leases` (`lease_name`)
VALUES ('legacy_owner_outbox_v1');

CREATE TABLE IF NOT EXISTS `secretary_owner_bindings` (
  `binding_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `managed_account_id` bigint unsigned NOT NULL,
  `command_account_id` bigint unsigned NOT NULL,
  `owner_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'active',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`binding_id`),
  UNIQUE KEY `uk_secretary_owner_binding_managed` (`managed_account_id`,`command_account_id`,`owner_actor_id`),
  KEY `idx_secretary_owner_binding_command` (`command_account_id`,`owner_actor_id`,`status`),
  CONSTRAINT `chk_secretary_owner_binding_status` CHECK ((`status` in (_utf8mb4'active',_utf8mb4'revoked')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='本地配置建立的 Owner 控制身份到被管理账号的显式授权；不从聊天内容推断';

CREATE TABLE IF NOT EXISTS `secretary_participant_conversation_observations` (
  `observation_id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `platform_identity_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `actor_platform_id` varchar(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `group_card` varchar(200) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `group_role` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'unknown',
  `established_by_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `source_event_ids_json` json NOT NULL,
  `invalidated` tinyint(1) NOT NULL DEFAULT '0',
  `invalidation_reason` varchar(200) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `first_seen_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`observation_id`),
  UNIQUE KEY `uk_secretary_observation_scope` (`account_id`,`conversation_id`,`platform_identity_kind`,`actor_platform_id`),
  KEY `fk_secretary_observation_conversation` (`conversation_id`),
  KEY `idx_secretary_observation_actor` (`account_id`,`platform_identity_kind`,`actor_platform_id`),
  KEY `idx_secretary_observation_updated` (`account_id`,`updated_at`),
  CONSTRAINT `chk_secretary_observation_group_role` CHECK ((`group_role` in (_utf8mb4'owner',_utf8mb4'admin',_utf8mb4'member',_utf8mb4'unknown'))),
  CONSTRAINT `chk_secretary_observation_kind` CHECK ((`platform_identity_kind` in (_utf8mb4'owner',_utf8mb4'official_bot',_utf8mb4'external'))),
  CONSTRAINT `chk_secretary_observation_source_count` CHECK ((json_length(`source_event_ids_json`) <= 10))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='会话作用域群名片/群角色观察；只显示不授权，按会话隔离';

CREATE TABLE IF NOT EXISTS `secretary_participant_profiles` (
  `profile_id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `account_id` bigint unsigned NOT NULL,
  `platform_identity_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `actor_platform_id` varchar(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `display_name` varchar(200) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT '',
  `aliases_json` json NOT NULL,
  `trust` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'observed',
  `confirmed` tinyint(1) NOT NULL DEFAULT '0',
  `invalidated` tinyint(1) NOT NULL DEFAULT '0',
  `invalidation_reason` varchar(200) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `source_event_ids_json` json NOT NULL,
  `established_by_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `directory_snapshot_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `current` tinyint(1) NOT NULL DEFAULT '1',
  `current_head` varchar(191) COLLATE utf8mb4_unicode_ci GENERATED ALWAYS AS (if((`current` = 1),`actor_platform_id`,NULL)) STORED,
  `first_seen_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`profile_id`),
  UNIQUE KEY `uk_secretary_participant_current` (`account_id`,`platform_identity_kind`,`current_head`),
  KEY `idx_secretary_participant_actor` (`account_id`,`platform_identity_kind`,`actor_platform_id`,`current`),
  KEY `idx_secretary_participant_updated` (`account_id`,`updated_at`),
  CONSTRAINT `chk_secretary_participant_actor_kind` CHECK ((`platform_identity_kind` in (_utf8mb4'owner',_utf8mb4'official_bot',_utf8mb4'external'))),
  CONSTRAINT `chk_secretary_participant_alias_count` CHECK ((json_length(`aliases_json`) <= 10)),
  CONSTRAINT `chk_secretary_participant_trust` CHECK ((`trust` in (_utf8mb4'verified',_utf8mb4'observed',_utf8mb4'inferred')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='账号作用域参与者稳定档案（昵称/别名）；群名片与群角色见会话观察表';

CREATE TABLE IF NOT EXISTS `secretary_qq_gateway_sessions` (
  `app_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `session_id` varchar(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `last_sequence` bigint unsigned NOT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`app_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='官方 QQ Bot Gateway RESUME 会话；仅在原始消息可靠入库后推进 sequence';

CREATE TABLE IF NOT EXISTS `secretary_qq_raw_events` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `app_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `event_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `envelope_json` json NOT NULL,
  `received_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_qq_raw_app_time` (`app_id`,`received_at`,`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='官方 Gateway 无损原始事件；持久化成功后才推进 Resume sequence';

CREATE TABLE IF NOT EXISTS `secretary_recall_events` (
  `recall_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `recall_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `conversation_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_conversation_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `platform_message_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `correlation_key` varchar(500) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `operator_platform_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `occurred_at_unix_secs` bigint NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`recall_event_id`),
  UNIQUE KEY `uk_secretary_recall_correlation` (`account_id`,`correlation_key`),
  KEY `idx_secretary_recall_account_time` (`account_id`,`occurred_at_unix_secs`),
  KEY `idx_secretary_recall_correlation_lookup` (`account_id`,`channel`,`conversation_kind`,`platform_conversation_id`,`platform_message_id`),
  CONSTRAINT `chk_secretary_recall_channel` CHECK ((`channel` in (_utf8mb4'napcat',_utf8mb4'qq_open_platform'))),
  CONSTRAINT `chk_secretary_recall_conv_kind` CHECK ((`conversation_kind` in (_utf8mb4'private',_utf8mb4'group',_utf8mb4'owner_control'))),
  CONSTRAINT `chk_secretary_recall_kind` CHECK ((`recall_kind` in (_utf8mb4'group',_utf8mb4'friend')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='撤回事件审计记录（撤回本身也是 SourceEvent；关联键禁止单 message_id 跨账号）';

CREATE TABLE IF NOT EXISTS `secretary_recall_inbox` (
  `recall_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `correlation_key` varchar(500) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `event_json` json NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `next_attempt_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `last_error_code` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`recall_event_id`),
  UNIQUE KEY `uk_secretary_recall_inbox_correlation` (`account_id`,`correlation_key`),
  KEY `idx_secretary_recall_inbox_claim` (`status`,`next_attempt_at`,`lease_expires_at`,`created_at`),
  CONSTRAINT `chk_secretary_recall_inbox_status` CHECK ((`status` in (_utf8mb4'pending',_utf8mb4'claimed',_utf8mb4'applied',_utf8mb4'failed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Durable recall inbox with lease, retry and inspectable failure state';

CREATE TABLE IF NOT EXISTS `secretary_realtime_spool_recovery_claims` (
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_expires_at` datetime(6) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`connection_epoch_id`),
  UNIQUE KEY `uk_secretary_realtime_spool_claim_token` (`lease_token`),
  KEY `idx_secretary_realtime_spool_claim_lease` (`lease_expires_at`,`connection_epoch_id`),
  KEY `idx_secretary_realtime_spool_claim_account` (`account_id`,`lease_expires_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Ordinary-message Spool startup recovery lease and fencing token';

CREATE TABLE IF NOT EXISTS `secretary_reply_reconcile_claims` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `next_eligible_at` datetime(6) DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_reply_reconcile_eligible` (`lease_expires_at`,`next_eligible_at`,`source_event_id`),
  KEY `idx_secretary_reply_reconcile_token` (`lease_token`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Reply 修复候选队列；每个 unresolved Reply 子事件一行';

CREATE TABLE IF NOT EXISTS `secretary_response_expectation_owner_controls` (
  `control_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `expectation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_source_version` bigint unsigned NOT NULL,
  `current_source_version` bigint unsigned NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`control_id`),
  UNIQUE KEY `uk_secretary_expectation_control_effect_item` (`effect_id`,`expectation_id`),
  KEY `fk_secretary_expectation_control_run` (`run_id`),
  KEY `fk_secretary_expectation_control_item` (`expectation_id`),
  KEY `fk_secretary_expectation_control_command` (`command_source_event_id`),
  KEY `idx_secretary_expectation_control_item` (`account_id`,`expectation_id`,`created_at`),
  CONSTRAINT `chk_secretary_expectation_control_status` CHECK (((`previous_status` = _utf8mb4'active') and (`current_status` = _utf8mb4'dismissed'))),
  CONSTRAINT `chk_secretary_expectation_control_version` CHECK (((`previous_source_version` > 0) and (`current_source_version` = (`previous_source_version` + 1))))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对 ResponseExpectation 的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

CREATE TABLE IF NOT EXISTS `secretary_response_expectations` (
  `expectation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `source_question_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_version` bigint unsigned NOT NULL DEFAULT '1',
  `due_at_unix_secs` bigint NOT NULL,
  `expectation_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'active',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`expectation_id`),
  UNIQUE KEY `uk_secretary_response_expectation_question` (`source_question_id`),
  KEY `fk_secretary_response_expectation_account` (`account_id`),
  KEY `fk_secretary_response_expectation_thread` (`thread_id`),
  KEY `idx_secretary_response_expectation_due` (`expectation_status`,`due_at_unix_secs`,`expectation_id`),
  CONSTRAINT `chk_secretary_response_expectation_status` CHECK ((`expectation_status` in (_utf8mb4'active',_utf8mb4'resolved',_utf8mb4'dismissed',_utf8mb4'superseded'))),
  CONSTRAINT `chk_secretary_response_expectation_version` CHECK ((`source_version` > 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='外部联系人开放问题的来源化回复期待；本人后续回复或线程终态会自动结束';

CREATE TABLE IF NOT EXISTS `secretary_source_events` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `source_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `platform_event_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `event_type` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `actor_platform_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `actor_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `message_role` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `occurred_at_unix_secs` bigint NOT NULL,
  `reply_to_platform_event_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `reply_to_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `processing_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `received_at` datetime(6) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  UNIQUE KEY `uk_secretary_source_delivery` (`account_id`,`platform_event_id`),
  KEY `fk_secretary_source_reply` (`reply_to_event_id`),
  KEY `idx_secretary_source_conversation_time` (`conversation_id`,`occurred_at_unix_secs`,`source_event_id`),
  KEY `idx_secretary_source_actor_time` (`account_id`,`actor_platform_id`,`occurred_at_unix_secs`),
  KEY `idx_secretary_source_processing` (`processing_status`,`received_at`),
  KEY `idx_secretary_source_reply_platform` (`account_id`,`reply_to_platform_event_id`),
  CONSTRAINT `chk_secretary_source_actor_kind` CHECK ((`actor_kind` in (_utf8mb4'owner',_utf8mb4'official_bot',_utf8mb4'external'))),
  CONSTRAINT `chk_secretary_source_event_type` CHECK ((`event_type` in (_utf8mb4'message',_utf8mb4'recall'))),
  CONSTRAINT `chk_secretary_source_message_role` CHECK ((`message_role` in (_utf8mb4'owner_command',_utf8mb4'owner_observation',_utf8mb4'external_observation',_utf8mb4'assistant_output'))),
  CONSTRAINT `chk_secretary_source_processing` CHECK ((`processing_status` in (_utf8mb4'pending',_utf8mb4'processing',_utf8mb4'processed',_utf8mb4'failed',_utf8mb4'ignored')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='个人秘书不可变入站事件信封和确定性回复关系';

CREATE TABLE IF NOT EXISTS `secretary_thread_claim_sources` (
  `claim_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`claim_id`,`source_event_id`),
  KEY `idx_secretary_thread_claim_source_event` (`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `secretary_thread_claims` (
  `claim_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `claim_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `claimant_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `claimant_account` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `claimant_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `statement` text COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'proposed',
  `confidence_bps` smallint unsigned NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`claim_id`),
  KEY `idx_secretary_thread_claim_thread` (`thread_id`,`claim_kind`,`status`,`created_at`),
  CONSTRAINT `chk_secretary_thread_claim_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_thread_claim_kind` CHECK ((`claim_kind` in (_utf8mb4'request',_utf8mb4'objection',_utf8mb4'confirmation'))),
  CONSTRAINT `chk_secretary_thread_claim_status` CHECK ((`status` in (_utf8mb4'proposed',_utf8mb4'contested',_utf8mb4'confirmed',_utf8mb4'withdrawn')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='谁提出要求、反对或确认的类型化候选；不静默提升为已确认事实';

CREATE TABLE IF NOT EXISTS `secretary_thread_decision_sources` (
  `decision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`decision_id`,`source_event_id`),
  KEY `idx_secretary_thread_decision_source_event` (`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `secretary_thread_decisions` (
  `decision_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `statement` text COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'proposed',
  `confidence_bps` smallint unsigned NOT NULL,
  `supersedes_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`decision_id`),
  UNIQUE KEY `uk_secretary_thread_decision_supersedes` (`supersedes_id`),
  KEY `idx_secretary_thread_decision_thread` (`thread_id`,`created_at`,`decision_id`,`status`),
  CONSTRAINT `chk_secretary_thread_decision_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_thread_decision_status` CHECK ((`status` in (_utf8mb4'proposed',_utf8mb4'confirmed',_utf8mb4'superseded',_utf8mb4'revoked')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程结论及显式修订链；新结论不得静默覆盖旧结论';

CREATE TABLE IF NOT EXISTS `secretary_thread_events` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `added_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_thread_event_thread` (`thread_id`,`added_at`,`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='SourceEvent 到 EventThread 的可审计成员投影；每个事件至多属于一个线程';

CREATE TABLE IF NOT EXISTS `secretary_thread_link_candidate_sources` (
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`candidate_id`,`source_event_id`),
  KEY `idx_secretary_thread_link_source_event` (`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `secretary_thread_link_candidates` (
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `left_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `right_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `left_conversation_id` bigint unsigned NOT NULL,
  `right_conversation_id` bigint unsigned NOT NULL,
  `signal_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `fingerprint_sha256` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'proposed',
  `confidence_bps` smallint unsigned NOT NULL,
  `reason_code` varchar(64) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`candidate_id`),
  UNIQUE KEY `uk_secretary_thread_link_candidate` (`account_id`,`left_thread_id`,`right_thread_id`,`signal_kind`,`fingerprint_sha256`),
  KEY `fk_secretary_thread_link_candidate_left_thread` (`left_thread_id`),
  KEY `fk_secretary_thread_link_candidate_right_thread` (`right_thread_id`),
  KEY `fk_secretary_thread_link_candidate_left_conversation` (`left_conversation_id`),
  KEY `fk_secretary_thread_link_candidate_right_conversation` (`right_conversation_id`),
  KEY `idx_secretary_thread_link_candidate_status` (`account_id`,`status`,`confidence_bps`,`updated_at`),
  CONSTRAINT `chk_secretary_thread_link_candidate_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_thread_link_candidate_kind` CHECK ((`signal_kind` in (_utf8mb4'explicit_project_id',_utf8mb4'exact_file_source_key',_utf8mb4'explicit_file_version',_utf8mb4'exact_forward_source_key',_utf8mb4'exact_rich_content_key'))),
  CONSTRAINT `chk_secretary_thread_link_candidate_status` CHECK ((`status` in (_utf8mb4'proposed',_utf8mb4'accepted',_utf8mb4'rejected',_utf8mb4'expired'))),
  CONSTRAINT `chk_secretary_thread_link_distinct_conversations` CHECK ((`left_conversation_id` <> `right_conversation_id`)),
  CONSTRAINT `chk_secretary_thread_link_distinct_threads` CHECK ((`left_thread_id` < `right_thread_id`))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='跨群/私聊线程关联候选；只有 Owner 控制面可在后续采用，绝不自动合并';

CREATE TABLE IF NOT EXISTS `secretary_thread_link_hints` (
  `hint_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `signal_kind` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `fingerprint_sha256` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`hint_id`),
  UNIQUE KEY `uk_secretary_thread_link_hint` (`source_event_id`,`signal_kind`,`fingerprint_sha256`),
  KEY `fk_secretary_thread_link_hint_conversation` (`conversation_id`),
  KEY `fk_secretary_thread_link_hint_thread` (`thread_id`),
  KEY `idx_secretary_thread_link_hint_match` (`account_id`,`signal_kind`,`fingerprint_sha256`,`thread_id`,`conversation_id`),
  CONSTRAINT `chk_secretary_thread_link_hint_kind` CHECK ((`signal_kind` in (_utf8mb4'explicit_project_id',_utf8mb4'exact_file_source_key',_utf8mb4'explicit_file_version',_utf8mb4'exact_forward_source_key',_utf8mb4'exact_rich_content_key')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='强关联信号的不可逆指纹；不保存项目编号、文件源键或文件名明文';

CREATE TABLE IF NOT EXISTS `secretary_thread_link_reviews` (
  `review_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `candidate_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `review_action` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `owner_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `owner_account` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `owner_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`review_id`),
  UNIQUE KEY `uk_secretary_thread_link_review_candidate` (`candidate_id`),
  UNIQUE KEY `uk_secretary_thread_link_review_command` (`command_source_event_id`),
  KEY `idx_secretary_thread_link_review_owner` (`owner_channel`,`owner_account`,`created_at`),
  CONSTRAINT `chk_secretary_thread_link_review_action` CHECK ((`review_action` in (_utf8mb4'accept',_utf8mb4'reject'))),
  CONSTRAINT `chk_secretary_thread_link_review_channel` CHECK ((`owner_channel` in (_utf8mb4'napcat',_utf8mb4'qq_open_platform')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对关联候选的不可变审核；命令事件、身份和动作均可追溯';

CREATE TABLE IF NOT EXISTS `secretary_thread_link_scan_state` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `completed_at` datetime(6) DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_thread_link_scan_claim` (`completed_at`,`lease_expires_at`,`updated_at`,`source_event_id`),
  KEY `idx_secretary_thread_link_scan_token` (`lease_token`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='跨会话关联候选消费者的独立租约；完成后不重复扫描事件';

CREATE TABLE IF NOT EXISTS `secretary_thread_merge_aliases` (
  `merged_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `canonical_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `active` tinyint(1) NOT NULL DEFAULT '1',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`merged_thread_id`),
  KEY `idx_secretary_thread_merge_canonical` (`canonical_thread_id`,`active`),
  KEY `idx_secretary_thread_merge_proposal` (`proposal_id`),
  CONSTRAINT `chk_secretary_thread_merge_not_self` CHECK ((`merged_thread_id` <> `canonical_thread_id`))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='可撤销线程合并别名；不改写原始 thread_events';

CREATE TABLE IF NOT EXISTS `secretary_thread_mutation_checkpoints` (
  `checkpoint_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `checkpoint_json` json NOT NULL,
  `checkpoint_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'active',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `consumed_at` datetime(6) DEFAULT NULL,
  PRIMARY KEY (`checkpoint_id`),
  UNIQUE KEY `uk_secretary_thread_mutation_checkpoint_proposal` (`proposal_id`),
  KEY `idx_secretary_thread_mutation_checkpoint_status` (`checkpoint_status`,`created_at`),
  CONSTRAINT `chk_secretary_thread_mutation_checkpoint_status` CHECK ((`checkpoint_status` in (_utf8mb4'active',_utf8mb4'consumed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程变更 Graph Checkpoint；恢复时 CAS 单次消费，支持进程重启';

CREATE TABLE IF NOT EXISTS `secretary_thread_mutation_proposals` (
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `mutation_kind` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `proposal_status` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'awaiting_approval',
  `impact_json` json NOT NULL,
  `decision` varchar(16) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `last_error` varchar(1000) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `completed_at` datetime(6) DEFAULT NULL,
  PRIMARY KEY (`proposal_id`),
  UNIQUE KEY `uk_secretary_thread_mutation_effect` (`effect_id`),
  KEY `fk_secretary_thread_mutation_command` (`command_source_event_id`),
  KEY `idx_secretary_thread_mutation_account` (`account_id`,`proposal_status`,`created_at`,`proposal_id`),
  CONSTRAINT `chk_secretary_thread_mutation_decision` CHECK (((`decision` is null) or (`decision` in (_utf8mb4'approve',_utf8mb4'reject')))),
  CONSTRAINT `chk_secretary_thread_mutation_kind` CHECK ((`mutation_kind` in (_utf8mb4'merge',_utf8mb4'split'))),
  CONSTRAINT `chk_secretary_thread_mutation_status` CHECK ((`proposal_status` in (_utf8mb4'awaiting_approval',_utf8mb4'approved',_utf8mb4'rejected',_utf8mb4'applying',_utf8mb4'applied',_utf8mb4'failed',_utf8mb4'unknown_commit')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 审批的线程逻辑变更 Proposal；保存有界影响快照与 Effect 幂等键';

CREATE TABLE IF NOT EXISTS `secretary_thread_mutation_reversions` (
  `reversion_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`reversion_id`),
  UNIQUE KEY `uk_secretary_thread_reversion_proposal` (`proposal_id`),
  UNIQUE KEY `uk_secretary_thread_reversion_command` (`command_source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对已应用线程变更的不可变撤销审计；逻辑覆盖停用但原始成员不变';

CREATE TABLE IF NOT EXISTS `secretary_thread_open_questions` (
  `question_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `raised_by_channel` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `raised_by_account` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `raised_by_actor_id` varchar(191) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  `question` text COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'open',
  `confidence_bps` smallint unsigned NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`question_id`),
  KEY `idx_secretary_thread_question_thread` (`thread_id`,`status`,`created_at`),
  CONSTRAINT `chk_secretary_thread_question_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_thread_question_status` CHECK ((`status` in (_utf8mb4'open',_utf8mb4'answered',_utf8mb4'dismissed')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程未决问题及提出者；开放问题阻止线程关闭';

CREATE TABLE IF NOT EXISTS `secretary_thread_owner_controls` (
  `control_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `target_kind` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `target_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `control_kind` varchar(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `previous_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `current_status` varchar(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`control_id`),
  UNIQUE KEY `uk_secretary_thread_control_effect` (`effect_id`),
  KEY `fk_secretary_thread_control_run` (`run_id`),
  KEY `fk_secretary_thread_control_thread` (`thread_id`),
  KEY `fk_secretary_thread_control_command` (`command_source_event_id`),
  KEY `idx_secretary_thread_control_thread` (`account_id`,`thread_id`,`created_at`),
  CONSTRAINT `chk_secretary_thread_control_kind` CHECK ((`control_kind` in (_utf8mb4'confirm_decision',_utf8mb4'revoke_decision',_utf8mb4'dismiss_question',_utf8mb4'reconfirm_thread_semantics',_utf8mb4'close_thread',_utf8mb4'reopen_thread'))),
  CONSTRAINT `chk_secretary_thread_control_target` CHECK ((`target_kind` in (_ascii'decision',_ascii'question',_ascii'thread')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 对线程结论、未决问题和生命周期的不可变 Effect 审计';

CREATE TABLE IF NOT EXISTS `secretary_thread_projection_claims` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_thread_projection_claim` (`lease_expires_at`,`updated_at`,`source_event_id`),
  KEY `idx_secretary_thread_projection_token` (`lease_token`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='独立线程投影消费者租约；不复用 source_events.processing_status';

CREATE TABLE IF NOT EXISTS `secretary_thread_question_sources` (
  `question_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`question_id`,`source_event_id`),
  KEY `idx_secretary_thread_question_source_event` (`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `secretary_thread_relations` (
  `relation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `from_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `to_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `relation_kind` varchar(64) COLLATE utf8mb4_unicode_ci NOT NULL,
  `confidence_bps` smallint unsigned NOT NULL,
  `reason` varchar(255) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`relation_id`),
  UNIQUE KEY `uk_secretary_thread_relation` (`thread_id`,`from_event_id`,`to_event_id`,`relation_kind`),
  KEY `fk_secretary_thread_relation_from` (`from_event_id`),
  KEY `idx_secretary_thread_relation_to` (`to_event_id`,`relation_kind`),
  CONSTRAINT `chk_secretary_thread_relation_confidence` CHECK ((`confidence_bps` <= 10000)),
  CONSTRAINT `chk_secretary_thread_relation_kind` CHECK ((`relation_kind` in (_utf8mb4'reply',_utf8mb4'same_conversation_window',_utf8mb4'same_actor_within_conversation_window',_utf8mb4'explicit_project_id',_utf8mb4'file_version')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程确定性来源边；reason 只保存证据类型，不保存消息正文';

CREATE TABLE IF NOT EXISTS `secretary_thread_semantic_reconfirmations` (
  `reconfirmation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`reconfirmation_id`),
  UNIQUE KEY `uk_secretary_thread_semantic_reconfirmation_effect` (`effect_id`),
  KEY `idx_secretary_thread_semantic_reconfirmation_thread` (`thread_id`,`created_at`,`reconfirmation_id`),
  KEY `fk_secretary_thread_reconfirmation_command` (`command_source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Owner 重新确认线程语义的不可变审计边界';

CREATE TABLE IF NOT EXISTS `secretary_thread_semantic_invalidations` (
  `invalidation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `invalidation_kind` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`invalidation_id`),
  UNIQUE KEY `uk_secretary_thread_semantic_invalidation` (`proposal_id`,`thread_id`,`invalidation_kind`),
  KEY `idx_secretary_thread_semantic_invalidation_thread` (`thread_id`,`created_at`,`invalidation_id`),
  CONSTRAINT `chk_secretary_thread_semantic_invalidation_kind` CHECK ((`invalidation_kind` in (_utf8mb4'mutation_applied',_utf8mb4'mutation_reverted')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程变更后的语义失效证据；旧派生事实保留审计但不再作为当前状态读取';

CREATE TABLE IF NOT EXISTS `secretary_thread_semantic_state` (
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `last_added_at` datetime(6) DEFAULT NULL,
  `last_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`thread_id`),
  KEY `fk_secretary_thread_semantic_state_event` (`last_source_event_id`),
  KEY `idx_secretary_thread_semantic_claim` (`lease_expires_at`,`updated_at`,`thread_id`),
  KEY `idx_secretary_thread_semantic_token` (`lease_token`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程语义批处理游标与独立租约；失败不推进游标';

CREATE TABLE IF NOT EXISTS `secretary_thread_split_overrides` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `original_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effective_thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `active` tinyint(1) NOT NULL DEFAULT '1',
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `fk_secretary_thread_split_original` (`original_thread_id`),
  KEY `idx_secretary_thread_split_effective` (`effective_thread_id`,`active`),
  KEY `idx_secretary_thread_split_proposal` (`proposal_id`),
  CONSTRAINT `chk_secretary_thread_split_changes_thread` CHECK ((`original_thread_id` <> `effective_thread_id`))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='可撤销线程拆分覆盖；原始成员关系保持不变';

CREATE TABLE IF NOT EXISTS `secretary_thread_status_history` (
  `change_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `from_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `to_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL,
  `authority` varchar(24) COLLATE utf8mb4_unicode_ci NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`change_id`),
  KEY `idx_secretary_thread_history_thread` (`thread_id`,`created_at`,`change_id`),
  CONSTRAINT `chk_secretary_thread_history_authority` CHECK ((`authority` in (_utf8mb4'evidence_derived',_utf8mb4'owner_confirmed',_utf8mb4'system_recovery'))),
  CONSTRAINT `chk_secretary_thread_history_from` CHECK ((`from_status` in (_utf8mb4'open',_utf8mb4'waiting',_utf8mb4'resolved',_utf8mb4'closed',_utf8mb4'reopened'))),
  CONSTRAINT `chk_secretary_thread_history_to` CHECK ((`to_status` in (_utf8mb4'open',_utf8mb4'waiting',_utf8mb4'resolved',_utf8mb4'closed',_utf8mb4'reopened')))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='线程生命周期不可变审计历史';

CREATE TABLE IF NOT EXISTS `secretary_thread_status_sources` (
  `change_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  PRIMARY KEY (`change_id`,`source_event_id`),
  KEY `idx_secretary_thread_status_source_event` (`source_event_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- 所有被引用表均已存在后再添加外键，支持循环引用。
ALTER TABLE `secretary_action_audit` ADD CONSTRAINT `fk_secretary_action_audit_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_action_checkpoints` ADD CONSTRAINT `fk_secretary_action_checkpoint_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_action_effect_receipts` ADD CONSTRAINT `fk_secretary_action_effect_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_action_responses` ADD CONSTRAINT `fk_secretary_action_response_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_action_runs` ADD CONSTRAINT `fk_secretary_action_run_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_action_runs` ADD CONSTRAINT `fk_secretary_action_run_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_artifact_reprocess_audit` ADD CONSTRAINT `fk_secretary_artifact_reprocess_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_artifact_reprocess_audit` ADD CONSTRAINT `fk_secretary_artifact_reprocess_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_artifact_reprocess_audit` ADD CONSTRAINT `fk_secretary_artifact_reprocess_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_agenda_items` ADD CONSTRAINT `fk_secretary_agenda_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_agenda_items` ADD CONSTRAINT `fk_secretary_agenda_created_command` FOREIGN KEY (`created_command_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_agenda_items` ADD CONSTRAINT `fk_secretary_agenda_current_command` FOREIGN KEY (`current_command_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_agenda_mutation_audit` ADD CONSTRAINT `fk_secretary_agenda_audit_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_agenda_mutation_audit` ADD CONSTRAINT `fk_secretary_agenda_audit_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_agenda_mutation_audit` ADD CONSTRAINT `fk_secretary_agenda_audit_item` FOREIGN KEY (`item_id`) REFERENCES `secretary_agenda_items` (`item_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_agenda_mutation_audit` ADD CONSTRAINT `fk_secretary_agenda_audit_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_artifact_derivations` ADD CONSTRAINT `fk_secretary_artifact_derivation_source` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_artifacts` ADD CONSTRAINT `fk_secretary_artifact_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_artifacts` ADD CONSTRAINT `fk_secretary_artifact_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_artifacts` ADD CONSTRAINT `fk_secretary_artifact_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_leases` ADD CONSTRAINT `fk_secretary_backfill_lease_run` FOREIGN KEY (`backfill_run_id`) REFERENCES `secretary_backfill_runs` (`backfill_run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_runs` ADD CONSTRAINT `fk_secretary_backfill_run_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_runs` ADD CONSTRAINT `fk_secretary_backfill_run_connection` FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_runs` ADD CONSTRAINT `fk_secretary_backfill_run_gap` FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_scopes` ADD CONSTRAINT `fk_secretary_backfill_scope_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_scopes` ADD CONSTRAINT `fk_secretary_backfill_scope_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_backfill_scopes` ADD CONSTRAINT `fk_secretary_backfill_scope_run` FOREIGN KEY (`backfill_run_id`) REFERENCES `secretary_backfill_runs` (`backfill_run_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_connection_epochs` ADD CONSTRAINT `fk_secretary_connection_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_conversations` ADD CONSTRAINT `fk_secretary_conversations_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_directory_gap_freeze` ADD CONSTRAINT `fk_secretary_directory_freeze_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_directory_gap_freeze` ADD CONSTRAINT `fk_secretary_directory_freeze_gap` FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_directory_gap_freeze` ADD CONSTRAINT `fk_secretary_directory_freeze_snapshot` FOREIGN KEY (`snapshot_id`) REFERENCES `secretary_directory_snapshots` (`snapshot_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_directory_scopes` ADD CONSTRAINT `fk_secretary_directory_scope_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_directory_scopes` ADD CONSTRAINT `fk_secretary_directory_scope_snapshot` FOREIGN KEY (`snapshot_id`) REFERENCES `secretary_directory_snapshots` (`snapshot_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_directory_snapshots` ADD CONSTRAINT `fk_secretary_directory_snapshot_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_event_ingestion` ADD CONSTRAINT `fk_secretary_event_ingestion_connection` FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_event_ingestion` ADD CONSTRAINT `fk_secretary_event_ingestion_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_event_threads` ADD CONSTRAINT `fk_secretary_event_thread_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_event_threads` ADD CONSTRAINT `fk_secretary_event_thread_latest` FOREIGN KEY (`latest_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_event_threads` ADD CONSTRAINT `fk_secretary_event_thread_root` FOREIGN KEY (`root_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_follow_up_items` ADD CONSTRAINT `fk_secretary_follow_up_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_follow_up_items` ADD CONSTRAINT `fk_secretary_follow_up_memory` FOREIGN KEY (`source_memory_fact_id`) REFERENCES `secretary_memory_facts` (`fact_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_follow_up_owner_controls` ADD CONSTRAINT `fk_secretary_follow_up_control_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_follow_up_owner_controls` ADD CONSTRAINT `fk_secretary_follow_up_control_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_follow_up_owner_controls` ADD CONSTRAINT `fk_secretary_follow_up_control_item` FOREIGN KEY (`follow_up_id`) REFERENCES `secretary_follow_up_items` (`follow_up_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_follow_up_owner_controls` ADD CONSTRAINT `fk_secretary_follow_up_control_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_gap_boundaries` ADD CONSTRAINT `fk_secretary_gap_boundary_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_gap_boundaries` ADD CONSTRAINT `fk_secretary_gap_boundary_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_gap_boundaries` ADD CONSTRAINT `fk_secretary_gap_boundary_gap` FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_gap_reclaim_schedule` ADD CONSTRAINT `fk_secretary_gap_reclaim_gap` FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_gap_signal_scopes` ADD CONSTRAINT `fk_secretary_gap_signal_scope_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_gap_signal_scopes` ADD CONSTRAINT `fk_secretary_gap_signal_scope_gap` FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_ingestion_cursors` ADD CONSTRAINT `fk_secretary_cursor_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_ingestion_cursors` ADD CONSTRAINT `fk_secretary_cursor_connection` FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`) ON DELETE SET NULL;
ALTER TABLE `secretary_ingestion_cursors` ADD CONSTRAINT `fk_secretary_cursor_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_ingestion_cursors` ADD CONSTRAINT `fk_secretary_cursor_event` FOREIGN KEY (`last_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_ingestion_gaps` ADD CONSTRAINT `fk_secretary_gap_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_ingestion_gaps` ADD CONSTRAINT `fk_secretary_gap_connection` FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_controls` ADD CONSTRAINT `fk_secretary_candidate_control_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_controls` ADD CONSTRAINT `fk_secretary_candidate_control_candidate` FOREIGN KEY (`candidate_id`) REFERENCES `secretary_memory_candidates` (`candidate_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_candidate_controls` ADD CONSTRAINT `fk_secretary_candidate_control_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_candidate_controls` ADD CONSTRAINT `fk_secretary_candidate_control_fact` FOREIGN KEY (`fact_id`) REFERENCES `secretary_memory_facts` (`fact_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_candidate_controls` ADD CONSTRAINT `fk_secretary_candidate_control_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_candidate_deferred` ADD CONSTRAINT `fk_secretary_candidate_deferred_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_deferred` ADD CONSTRAINT `fk_secretary_candidate_deferred_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_processing_state` ADD CONSTRAINT `fk_secretary_candidate_state_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_sources` ADD CONSTRAINT `fk_secretary_candidate_source_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_sources` ADD CONSTRAINT `fk_secretary_candidate_source_candidate` FOREIGN KEY (`candidate_id`) REFERENCES `secretary_memory_candidates` (`candidate_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidate_sources` ADD CONSTRAINT `fk_secretary_candidate_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_candidates` ADD CONSTRAINT `fk_secretary_memory_candidate_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_deletions` ADD CONSTRAINT `fk_secretary_memory_deletion_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_deletions` ADD CONSTRAINT `fk_secretary_memory_deletion_fact` FOREIGN KEY (`fact_id`) REFERENCES `secretary_memory_facts` (`fact_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_fact_sources` ADD CONSTRAINT `fk_secretary_memory_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_memory_fact_sources` ADD CONSTRAINT `fk_secretary_memory_source_fact` FOREIGN KEY (`fact_id`) REFERENCES `secretary_memory_facts` (`fact_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_facts` ADD CONSTRAINT `fk_secretary_memory_fact_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_memory_facts` ADD CONSTRAINT `fk_secretary_memory_fact_supersedes` FOREIGN KEY (`supersedes_fact_id`) REFERENCES `secretary_memory_facts` (`fact_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_message_contents` ADD CONSTRAINT `fk_secretary_message_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_message_tombstones` ADD CONSTRAINT `fk_secretary_tombstone_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_message_tombstones` ADD CONSTRAINT `fk_secretary_tombstone_recall` FOREIGN KEY (`recall_event_id`) REFERENCES `secretary_recall_events` (`recall_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_message_tombstones` ADD CONSTRAINT `fk_secretary_tombstone_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE SET NULL;
ALTER TABLE `secretary_notification_candidates` ADD CONSTRAINT `fk_secretary_notification_candidate_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_decisions` ADD CONSTRAINT `fk_secretary_notification_decision_candidate` FOREIGN KEY (`notification_candidate_id`) REFERENCES `secretary_notification_candidates` (`notification_candidate_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_decisions` ADD CONSTRAINT `fk_secretary_notification_decision_previous` FOREIGN KEY (`previous_decision_id`) REFERENCES `secretary_notification_decisions` (`notification_decision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_decisions` ADD CONSTRAINT `fk_secretary_notification_decision_request` FOREIGN KEY (`evaluation_request_id`) REFERENCES `secretary_notification_evaluation_requests` (`evaluation_request_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_decisions` ADD CONSTRAINT `fk_secretary_notification_decision_revision` FOREIGN KEY (`policy_revision_id`) REFERENCES `secretary_notification_policy_revisions` (`policy_revision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_evaluation_requests` ADD CONSTRAINT `fk_secretary_notification_evaluation_candidate` FOREIGN KEY (`notification_candidate_id`) REFERENCES `secretary_notification_candidates` (`notification_candidate_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_feedback` ADD CONSTRAINT `fk_secretary_notification_feedback_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_feedback` ADD CONSTRAINT `fk_secretary_notification_feedback_candidate` FOREIGN KEY (`notification_candidate_id`) REFERENCES `secretary_notification_candidates` (`notification_candidate_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_feedback` ADD CONSTRAINT `fk_secretary_notification_feedback_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_agenda` FOREIGN KEY (`agenda_item_id`) REFERENCES `secretary_agenda_items` (`item_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_follow_up` FOREIGN KEY (`follow_up_id`) REFERENCES `secretary_follow_up_items` (`follow_up_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_outbox_candidate` FOREIGN KEY (`notification_candidate_id`) REFERENCES `secretary_notification_candidates` (`notification_candidate_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_outbox_command_account` FOREIGN KEY (`command_account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_outbox` ADD CONSTRAINT `fk_secretary_notification_outbox_decision` FOREIGN KEY (`notification_decision_id`) REFERENCES `secretary_notification_decisions` (`notification_decision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_policy_families` ADD CONSTRAINT `fk_secretary_notification_policy_family_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_policy_families` ADD CONSTRAINT `fk_secretary_notification_policy_family_head` FOREIGN KEY (`policy_family_id`, `current_revision_id`) REFERENCES `secretary_notification_policy_revisions` (`policy_family_id`, `policy_revision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_policy_revisions` ADD CONSTRAINT `fk_secretary_notification_policy_revision_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_notification_policy_revisions` ADD CONSTRAINT `fk_secretary_notification_policy_revision_family` FOREIGN KEY (`policy_family_id`) REFERENCES `secretary_notification_policy_families` (`policy_family_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_notification_policy_revisions` ADD CONSTRAINT `fk_secretary_notification_policy_revision_supersedes` FOREIGN KEY (`supersedes_revision_id`) REFERENCES `secretary_notification_policy_revisions` (`policy_revision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_owner_bindings` ADD CONSTRAINT `fk_secretary_owner_binding_command_account` FOREIGN KEY (`command_account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_owner_bindings` ADD CONSTRAINT `fk_secretary_owner_binding_managed_account` FOREIGN KEY (`managed_account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_participant_conversation_observations` ADD CONSTRAINT `fk_secretary_observation_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_participant_conversation_observations` ADD CONSTRAINT `fk_secretary_observation_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_participant_profiles` ADD CONSTRAINT `fk_secretary_participant_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_qq_raw_events` ADD CONSTRAINT `fk_secretary_qq_raw_source` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_recall_events` ADD CONSTRAINT `fk_secretary_recall_event_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_recall_inbox` ADD CONSTRAINT `fk_secretary_recall_inbox_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_realtime_spool_recovery_claims` ADD CONSTRAINT `fk_secretary_realtime_spool_claim_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_realtime_spool_recovery_claims` ADD CONSTRAINT `fk_secretary_realtime_spool_claim_epoch` FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_reply_reconcile_claims` ADD CONSTRAINT `fk_secretary_reconcile_claim_source` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_response_expectation_owner_controls` ADD CONSTRAINT `fk_secretary_expectation_control_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_response_expectation_owner_controls` ADD CONSTRAINT `fk_secretary_expectation_control_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_response_expectation_owner_controls` ADD CONSTRAINT `fk_secretary_expectation_control_item` FOREIGN KEY (`expectation_id`) REFERENCES `secretary_response_expectations` (`expectation_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_response_expectation_owner_controls` ADD CONSTRAINT `fk_secretary_expectation_control_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_response_expectations` ADD CONSTRAINT `fk_secretary_response_expectation_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_response_expectations` ADD CONSTRAINT `fk_secretary_response_expectation_question` FOREIGN KEY (`source_question_id`) REFERENCES `secretary_thread_open_questions` (`question_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_response_expectations` ADD CONSTRAINT `fk_secretary_response_expectation_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_source_events` ADD CONSTRAINT `fk_secretary_source_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_source_events` ADD CONSTRAINT `fk_secretary_source_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_source_events` ADD CONSTRAINT `fk_secretary_source_reply` FOREIGN KEY (`reply_to_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE SET NULL;
ALTER TABLE `secretary_thread_claim_sources` ADD CONSTRAINT `fk_secretary_thread_claim_source_claim` FOREIGN KEY (`claim_id`) REFERENCES `secretary_thread_claims` (`claim_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_claim_sources` ADD CONSTRAINT `fk_secretary_thread_claim_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_claims` ADD CONSTRAINT `fk_secretary_thread_claim_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_decision_sources` ADD CONSTRAINT `fk_secretary_thread_decision_source_decision` FOREIGN KEY (`decision_id`) REFERENCES `secretary_thread_decisions` (`decision_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_decision_sources` ADD CONSTRAINT `fk_secretary_thread_decision_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_decisions` ADD CONSTRAINT `fk_secretary_thread_decision_supersedes` FOREIGN KEY (`supersedes_id`) REFERENCES `secretary_thread_decisions` (`decision_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_decisions` ADD CONSTRAINT `fk_secretary_thread_decision_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_events` ADD CONSTRAINT `fk_secretary_thread_event_source` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_events` ADD CONSTRAINT `fk_secretary_thread_event_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidate_sources` ADD CONSTRAINT `fk_secretary_thread_link_source_candidate` FOREIGN KEY (`candidate_id`) REFERENCES `secretary_thread_link_candidates` (`candidate_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidate_sources` ADD CONSTRAINT `fk_secretary_thread_link_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidates` ADD CONSTRAINT `fk_secretary_thread_link_candidate_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidates` ADD CONSTRAINT `fk_secretary_thread_link_candidate_left_conversation` FOREIGN KEY (`left_conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidates` ADD CONSTRAINT `fk_secretary_thread_link_candidate_left_thread` FOREIGN KEY (`left_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidates` ADD CONSTRAINT `fk_secretary_thread_link_candidate_right_conversation` FOREIGN KEY (`right_conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_candidates` ADD CONSTRAINT `fk_secretary_thread_link_candidate_right_thread` FOREIGN KEY (`right_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_hints` ADD CONSTRAINT `fk_secretary_thread_link_hint_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_hints` ADD CONSTRAINT `fk_secretary_thread_link_hint_conversation` FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_hints` ADD CONSTRAINT `fk_secretary_thread_link_hint_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_hints` ADD CONSTRAINT `fk_secretary_thread_link_hint_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_reviews` ADD CONSTRAINT `fk_secretary_thread_link_review_candidate` FOREIGN KEY (`candidate_id`) REFERENCES `secretary_thread_link_candidates` (`candidate_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_link_reviews` ADD CONSTRAINT `fk_secretary_thread_link_review_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_link_scan_state` ADD CONSTRAINT `fk_secretary_thread_link_scan_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_merge_aliases` ADD CONSTRAINT `fk_secretary_thread_merge_canonical` FOREIGN KEY (`canonical_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_merge_aliases` ADD CONSTRAINT `fk_secretary_thread_merge_proposal` FOREIGN KEY (`proposal_id`) REFERENCES `secretary_thread_mutation_proposals` (`proposal_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_merge_aliases` ADD CONSTRAINT `fk_secretary_thread_merge_source` FOREIGN KEY (`merged_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_mutation_checkpoints` ADD CONSTRAINT `fk_secretary_thread_mutation_checkpoint_proposal` FOREIGN KEY (`proposal_id`) REFERENCES `secretary_thread_mutation_proposals` (`proposal_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_mutation_proposals` ADD CONSTRAINT `fk_secretary_thread_mutation_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_mutation_proposals` ADD CONSTRAINT `fk_secretary_thread_mutation_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_mutation_reversions` ADD CONSTRAINT `fk_secretary_thread_reversion_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_mutation_reversions` ADD CONSTRAINT `fk_secretary_thread_reversion_proposal` FOREIGN KEY (`proposal_id`) REFERENCES `secretary_thread_mutation_proposals` (`proposal_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_open_questions` ADD CONSTRAINT `fk_secretary_thread_question_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_owner_controls` ADD CONSTRAINT `fk_secretary_thread_control_account` FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_owner_controls` ADD CONSTRAINT `fk_secretary_thread_control_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_owner_controls` ADD CONSTRAINT `fk_secretary_thread_control_run` FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_owner_controls` ADD CONSTRAINT `fk_secretary_thread_control_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_projection_claims` ADD CONSTRAINT `fk_secretary_thread_projection_source` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_question_sources` ADD CONSTRAINT `fk_secretary_thread_question_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_question_sources` ADD CONSTRAINT `fk_secretary_thread_question_source_question` FOREIGN KEY (`question_id`) REFERENCES `secretary_thread_open_questions` (`question_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_relations` ADD CONSTRAINT `fk_secretary_thread_relation_from` FOREIGN KEY (`from_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_relations` ADD CONSTRAINT `fk_secretary_thread_relation_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_relations` ADD CONSTRAINT `fk_secretary_thread_relation_to` FOREIGN KEY (`to_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_semantic_reconfirmations` ADD CONSTRAINT `fk_secretary_thread_reconfirmation_command` FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_semantic_reconfirmations` ADD CONSTRAINT `fk_secretary_thread_reconfirmation_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE RESTRICT;
ALTER TABLE `secretary_thread_semantic_invalidations` ADD CONSTRAINT `fk_secretary_thread_invalidation_proposal` FOREIGN KEY (`proposal_id`) REFERENCES `secretary_thread_mutation_proposals` (`proposal_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_semantic_invalidations` ADD CONSTRAINT `fk_secretary_thread_invalidation_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_semantic_state` ADD CONSTRAINT `fk_secretary_thread_semantic_state_event` FOREIGN KEY (`last_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_semantic_state` ADD CONSTRAINT `fk_secretary_thread_semantic_state_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_split_overrides` ADD CONSTRAINT `fk_secretary_thread_split_effective` FOREIGN KEY (`effective_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_split_overrides` ADD CONSTRAINT `fk_secretary_thread_split_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_split_overrides` ADD CONSTRAINT `fk_secretary_thread_split_original` FOREIGN KEY (`original_thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_split_overrides` ADD CONSTRAINT `fk_secretary_thread_split_proposal` FOREIGN KEY (`proposal_id`) REFERENCES `secretary_thread_mutation_proposals` (`proposal_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_status_history` ADD CONSTRAINT `fk_secretary_thread_history_thread` FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_status_sources` ADD CONSTRAINT `fk_secretary_thread_status_source_change` FOREIGN KEY (`change_id`) REFERENCES `secretary_thread_status_history` (`change_id`) ON DELETE CASCADE;
ALTER TABLE `secretary_thread_status_sources` ADD CONSTRAINT `fk_secretary_thread_status_source_event` FOREIGN KEY (`source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE CASCADE;

CREATE OR REPLACE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `secretary_effective_thread_events` AS select `te`.`source_event_id` AS `source_event_id`,coalesce(`split`.`effective_thread_id`,`alias`.`canonical_thread_id`,`te`.`thread_id`) AS `thread_id`,`te`.`thread_id` AS `projected_thread_id`,`te`.`added_at` AS `added_at` from ((`secretary_thread_events` `te` left join `secretary_thread_split_overrides` `split` on(((`split`.`source_event_id` = `te`.`source_event_id`) and (`split`.`active` = true)))) left join `secretary_thread_merge_aliases` `alias` on(((`alias`.`merged_thread_id` = `te`.`thread_id`) and (`alias`.`active` = true))));

CREATE OR REPLACE ALGORITHM=UNDEFINED SQL SECURITY DEFINER VIEW `secretary_event_relations` AS select `e`.`account_id` AS `account_id`,`e`.`source_event_id` AS `source_event_id`,`e`.`actor_platform_id` AS `subject_actor_id`,`e`.`actor_kind` AS `subject_actor_kind`,'sent_by' AS `relation_kind`,NULL AS `thread_id`,1 AS `confirmed`,`e`.`occurred_at_unix_secs` AS `occurred_at_unix_secs` from `secretary_source_events` `e` union all select `e`.`account_id` AS `account_id`,`e`.`source_event_id` AS `source_event_id`,`p`.`actor_platform_id` AS `subject_actor_id`,`p`.`actor_kind` AS `subject_actor_kind`,'replies_to' AS `relation_kind`,NULL AS `thread_id`,1 AS `confirmed`,`e`.`occurred_at_unix_secs` AS `occurred_at_unix_secs` from (`secretary_source_events` `e` join `secretary_source_events` `p` on(((`p`.`source_event_id` = `e`.`reply_to_event_id`) and (`p`.`account_id` = `e`.`account_id`)))) union all select `e`.`account_id` AS `account_id`,`e`.`source_event_id` AS `source_event_id`,`e`.`actor_platform_id` AS `subject_actor_id`,`e`.`actor_kind` AS `subject_actor_kind`,'member_of_thread' AS `relation_kind`,`ev`.`thread_id` AS `thread_id`,1 AS `confirmed`,`e`.`occurred_at_unix_secs` AS `occurred_at_unix_secs` from (`secretary_source_events` `e` join `secretary_effective_thread_events` `ev` on((`ev`.`source_event_id` = `e`.`source_event_id`))) union all select `e`.`account_id` AS `account_id`,`e`.`source_event_id` AS `source_event_id`,`e`.`actor_platform_id` AS `subject_actor_id`,`e`.`actor_kind` AS `subject_actor_kind`,'thread_root_by' AS `relation_kind`,`t`.`thread_id` AS `thread_id`,1 AS `confirmed`,`e`.`occurred_at_unix_secs` AS `occurred_at_unix_secs` from ((`secretary_source_events` `e` join `secretary_effective_thread_events` `ev` on((`ev`.`source_event_id` = `e`.`source_event_id`))) join `secretary_event_threads` `t` on(((`t`.`thread_id` = `ev`.`thread_id`) and (`t`.`account_id` = `e`.`account_id`)))) where (`e`.`source_event_id` = `t`.`root_event_id`) union all select `e`.`account_id` AS `account_id`,`e`.`source_event_id` AS `source_event_id`,`jt`.`actor_id` AS `subject_actor_id`,'external' AS `subject_actor_kind`,'mentions' AS `relation_kind`,NULL AS `thread_id`,1 AS `confirmed`,`e`.`occurred_at_unix_secs` AS `occurred_at_unix_secs` from ((`secretary_source_events` `e` join `secretary_message_contents` `m` on((`m`.`source_event_id` = `e`.`source_event_id`))) join json_table(cast(`m`.`mentioned_actor_ids` as char charset utf8mb4), '$[*]' columns (`actor_id` varchar(191) character set utf8mb4 path '$')) `jt`);
