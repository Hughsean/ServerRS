-- Hardening for directory Gap first-write freezing.
-- Older installations created snapshot_id as NOT NULL, which cannot represent
-- "no directory snapshot existed when the Gap was created".

ALTER TABLE secretary_directory_gap_freeze
    MODIFY snapshot_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL;
