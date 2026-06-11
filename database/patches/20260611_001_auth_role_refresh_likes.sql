-- 20260611_001_auth_role_refresh_likes.sql
-- Purpose:
--   1. Add users.role for role-based authorization.
--   2. Add refresh_tokens for persistent refresh token rotation/revocation.
--   3. Add content_likes for user-level like persistence.
--
-- Execution policy:
--   Manually executed database patch. NOT auto-executed by the Rust service.

ALTER TABLE `users`
  ADD COLUMN `role` VARCHAR(32) NOT NULL DEFAULT 'USER' COMMENT 'USER/ADMIN/SUPER_ADMIN' AFTER `status`;

CREATE INDEX `idx_users_role` ON `users` (`role`);

CREATE TABLE IF NOT EXISTS `refresh_tokens` (
  `refresh_token_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `token_id` VARCHAR(64) NOT NULL COMMENT 'JWT jti',
  `user_id` BIGINT UNSIGNED NOT NULL,
  `token_hash` CHAR(64) NOT NULL COMMENT 'SHA-256 hex of refresh token',
  `expires_at` BIGINT UNSIGNED NOT NULL COMMENT 'Unix timestamp seconds',
  `revoked_at` DATETIME(6) NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`refresh_token_id`),
  UNIQUE KEY `uk_refresh_tokens_token_id` (`token_id`),
  UNIQUE KEY `uk_refresh_tokens_token_hash` (`token_hash`),
  KEY `idx_refresh_tokens_user_id` (`user_id`),
  KEY `idx_refresh_tokens_expires_at` (`expires_at`),
  CONSTRAINT `fk_refresh_tokens_user_id`
    FOREIGN KEY (`user_id`) REFERENCES `users` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `content_likes` (
  `like_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `user_id` BIGINT UNSIGNED NOT NULL,
  `content_type` VARCHAR(64) NOT NULL,
  `content_id` BIGINT UNSIGNED NOT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`like_id`),
  UNIQUE KEY `uk_content_likes_user_content` (`user_id`, `content_type`, `content_id`),
  KEY `idx_content_likes_content` (`content_type`, `content_id`),
  KEY `idx_content_likes_user_id` (`user_id`),
  CONSTRAINT `fk_content_likes_user_id`
    FOREIGN KEY (`user_id`) REFERENCES `users` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
