-- Escalate unresolved project blockers through the existing policy-owned FollowUp path.
-- Depends on memory controls/followups and notification policy Task 7.

ALTER TABLE secretary_follow_up_items
    DROP CHECK chk_secretary_follow_up_reason,
    ADD CONSTRAINT chk_secretary_follow_up_reason
        CHECK (reason_code IN ('commitment_due', 'project_blocked'));
