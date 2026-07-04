-- Cover the exact public-change predicate used by the home page and /discs
-- modification sort.  The narrow target key preserves grouping order while
-- the included values avoid reading the wide submission rows.
CREATE INDEX idx_submissions_genuine_change_target_time
ON disc_submissions (target_disc_id)
INCLUDE (reviewed_at, created_at, id, submission_type)
WHERE target_disc_id IS NOT NULL
  AND status IN ('Approved', 'Legacy')
  AND (
    (
      submission_type = 'Edit'
      AND (
        changes <> '{}'::jsonb
        OR COALESCE(review_comment, '') <> ALL (
            ARRAY['added-backfill', 'no-added-sentinel']::TEXT[]
        )
      )
    )
    OR submission_type = 'Disc'
  );

-- These split timestamp indexes were not selected by the shared genuine-change
-- predicate.  The covering index above replaces both while using less space.
DROP INDEX idx_submissions_public_target_created_desc;
DROP INDEX idx_submissions_public_target_reviewed_desc;
