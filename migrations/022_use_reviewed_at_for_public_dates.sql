-- Public disc dates represent when a submission became visible, not when it
-- was originally filed. Historical imports used created_at for synthetic date
-- anchors, so normalize those rows before enforcing the invariant.
UPDATE disc_submissions
SET reviewed_at = created_at
WHERE status IN ('Approved', 'Legacy')
  AND reviewed_at IS NULL;

ALTER TABLE disc_submissions
ADD CONSTRAINT disc_submissions_public_reviewed_at_check
CHECK (status NOT IN ('Approved', 'Legacy') OR reviewed_at IS NOT NULL);

-- Public Added/Modified aggregation and per-disc History ordering now share
-- this reviewed-time index. The global created_at index remains for the queue.
DROP INDEX idx_submissions_target_created;
DROP INDEX idx_submissions_public_history_target_time;

CREATE INDEX idx_submissions_public_history_target_reviewed
ON disc_submissions (target_disc_id, reviewed_at)
WHERE target_disc_id IS NOT NULL
  AND status IN ('Approved', 'Legacy');
