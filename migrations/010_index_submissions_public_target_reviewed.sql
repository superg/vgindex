CREATE INDEX IF NOT EXISTS idx_submissions_public_target_reviewed_desc
ON disc_submissions (target_disc_id, reviewed_at DESC, id DESC)
WHERE status IN ('Approved', 'Legacy');
