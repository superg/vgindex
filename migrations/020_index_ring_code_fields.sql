CREATE OR REPLACE FUNCTION public.ringcode_field_search_text(TEXT) RETURNS TEXT
    LANGUAGE SQL IMMUTABLE PARALLEL SAFE
    AS $$
        SELECT LOWER(
            REGEXP_REPLACE(COALESCE($1, ''), '[[:blank:]]{2,}', CHR(9), 'g')
        )
    $$;

CREATE INDEX idx_ring_layers_mastering_code_trgm
ON disc_ring_code_layers USING GIN (
    public.ringcode_field_search_text(mastering_code) gin_trgm_ops
);

CREATE INDEX idx_ring_layers_mastering_sid_trgm
ON disc_ring_code_layers USING GIN (
    public.ringcode_field_search_text(mastering_sid) gin_trgm_ops
);

CREATE INDEX idx_ring_layers_toolstamps_trgm
ON disc_ring_code_layers USING GIN (
    public.ringcode_field_search_text(toolstamps) gin_trgm_ops
);

CREATE INDEX idx_ring_layers_mould_sids_trgm
ON disc_ring_code_layers USING GIN (
    public.ringcode_field_search_text(mould_sids) gin_trgm_ops
);

CREATE INDEX idx_ring_layers_additional_moulds_trgm
ON disc_ring_code_layers USING GIN (
    public.ringcode_field_search_text(additional_moulds) gin_trgm_ops
);

DROP INDEX idx_ring_layers_search_trgm;
DROP FUNCTION public.ringcode_layer_search_text(TEXT, TEXT);
