-- LS is stored at media_types.layer_count; L0 remains on the data side.
-- Keep source rows and move only the two mould fields, merging existing LS data.
DO $$
BEGIN
    LOCK TABLE disc_ring_code_layers IN SHARE ROW EXCLUSIVE MODE;

    WITH source_layers AS MATERIALIZED (
        SELECT l.id, l.entry_id, l.mould_sids, l.additional_moulds,
               m.layer_count AS label_layer
        FROM disc_ring_code_layers l
        JOIN disc_ring_code_entries e ON e.id = l.entry_id
        JOIN discs d ON d.id = e.disc_id
        JOIN media_types m ON m.code = d.media_type_code
        WHERE l.layer >= 1 AND l.layer < m.layer_count
          AND (l.mould_sids <> '' OR l.additional_moulds <> '')
    ), affected_entries AS (
        SELECT DISTINCT entry_id, label_layer FROM source_layers
    ), mould_values AS (
        SELECT entry_id, label_layer, mould_sids, additional_moulds
        FROM source_layers
        UNION ALL
        SELECT a.entry_id, a.label_layer, l.mould_sids, l.additional_moulds
        FROM affected_entries a
        JOIN disc_ring_code_layers l
          ON l.entry_id = a.entry_id AND l.layer = a.label_layer
    ), normalized_tokens AS (
        SELECT DISTINCT v.entry_id, v.label_layer, f.field,
               -- Match Rust str::trim's Unicode whitespace characters.
               btrim(t.token, U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')
                   COLLATE "C" AS token
        FROM mould_values v
        CROSS JOIN LATERAL (VALUES
            ('mould_sids', v.mould_sids),
            ('additional_moulds', v.additional_moulds)
        ) f(field, value)
        CROSS JOIN LATERAL unnest(string_to_array(f.value, ',')) t(token)
    ), merged AS (
        SELECT a.entry_id, a.label_layer,
               COALESCE(string_agg(t.token, ', ' ORDER BY lower(t.token COLLATE "default") COLLATE "C", t.token)
                   FILTER (WHERE t.field = 'mould_sids' AND t.token <> ''), '') AS mould_sids,
               COALESCE(string_agg(t.token, ', ' ORDER BY lower(t.token COLLATE "default") COLLATE "C", t.token)
                   FILTER (WHERE t.field = 'additional_moulds' AND t.token <> ''), '') AS additional_moulds
        FROM affected_entries a
        LEFT JOIN normalized_tokens t
          ON t.entry_id = a.entry_id AND t.label_layer = a.label_layer
        GROUP BY a.entry_id, a.label_layer
    ), updated_label_layers AS (
        INSERT INTO disc_ring_code_layers (entry_id, layer, mould_sids, additional_moulds)
        SELECT entry_id, label_layer, mould_sids, additional_moulds FROM merged
        ON CONFLICT (entry_id, layer) DO UPDATE
        SET mould_sids = EXCLUDED.mould_sids,
            additional_moulds = EXCLUDED.additional_moulds
        RETURNING entry_id
    )
    UPDATE disc_ring_code_layers l
    SET mould_sids = '', additional_moulds = ''
    FROM source_layers s
    JOIN updated_label_layers u ON u.entry_id = s.entry_id
    WHERE l.id = s.id;
END
$$;
