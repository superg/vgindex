CREATE INDEX IF NOT EXISTS idx_discs_active_id_desc
ON discs (id DESC)
WHERE status <> 'Disabled';

CREATE INDEX IF NOT EXISTS idx_submissions_target_created
ON disc_submissions (target_disc_id, created_at);

CREATE INDEX IF NOT EXISTS idx_submissions_public_target_id
ON disc_submissions (target_disc_id, id)
WHERE status IN ('Approved', 'Legacy');

CREATE INDEX IF NOT EXISTS idx_submissions_public_target_created_desc
ON disc_submissions (target_disc_id, created_at DESC, id DESC)
WHERE status IN ('Approved', 'Legacy');
