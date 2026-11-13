CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

-- Add new function for accent-insensitive search
CREATE OR REPLACE FUNCTION public.search_fold(TEXT) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE STRICT
    AS $$
        SELECT LOWER(public.unaccent('public.unaccent'::regdictionary, $1))
    $$;

-- Create new unaccent indices
CREATE INDEX idx_discs_title_fold_trgm
ON discs USING GIN (public.search_fold(title) gin_trgm_ops);

CREATE INDEX idx_discs_title_foreign_fold_trgm
ON discs USING GIN (public.search_fold(title_foreign) gin_trgm_ops);

CREATE INDEX idx_discs_disc_title_fold_trgm
ON discs USING GIN (public.search_fold(disc_title) gin_trgm_ops);

CREATE INDEX idx_discs_disc_title_foreign_fold_trgm
ON discs USING GIN (public.search_fold(disc_title_foreign) gin_trgm_ops);

CREATE INDEX idx_discs_comments_fold_trgm
ON discs USING GIN (public.search_fold(comments) gin_trgm_ops);

CREATE INDEX idx_discs_contents_fold_trgm
ON discs USING GIN (public.search_fold(contents) gin_trgm_ops);

-- Drop old search indices
DROP INDEX idx_discs_title_trgm;
DROP INDEX idx_discs_title_foreign_trgm;
DROP INDEX idx_discs_disc_title_foreign_trgm;
DROP INDEX idx_discs_comments_trgm;
DROP INDEX idx_discs_contents_trgm;
