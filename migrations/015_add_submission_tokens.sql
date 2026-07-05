ALTER TABLE disc_submissions
ADD COLUMN submission_token VARCHAR(64),
ADD COLUMN submission_fingerprint VARCHAR(64);

CREATE UNIQUE INDEX idx_submissions_submission_token_unique
ON disc_submissions (submission_token)
WHERE submission_token IS NOT NULL;
