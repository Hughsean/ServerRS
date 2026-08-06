-- Restore the singleton row created by the pre-v1 migration chain but omitted
-- from the original compressed baseline. Existing and fresh schemas converge
-- through the same idempotent seed.
INSERT IGNORE INTO `secretary_notification_reconciliation_leases` (`lease_name`)
VALUES ('legacy_owner_outbox_v1');

-- Fail closed unless exactly one authoritative singleton row exists.
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
    FROM `secretary_notification_reconciliation_leases`
    WHERE `lease_name` = 'legacy_owner_outbox_v1'
  ), 0) <> 1
) AS notification_reconciliation_lease_seed_verified;
