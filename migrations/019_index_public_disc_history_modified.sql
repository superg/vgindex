-- Cover modified-date aggregation over every approved or legacy history row.
CREATE INDEX idx_submissions_public_history_target_time
ON disc_submissions (target_disc_id)
INCLUDE (reviewed_at, created_at)
WHERE target_disc_id IS NOT NULL
  AND status IN ('Approved', 'Legacy');

DROP INDEX idx_submissions_genuine_change_target_time;
