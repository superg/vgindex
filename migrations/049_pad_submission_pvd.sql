-- Pad 81-byte ISO PVD dumps in pending/draft submissions to the 96-byte redumper form.
UPDATE disc_submissions s
SET changes = jsonb_set(
    s.changes,
    '{pvd,add,new}',
    to_jsonb(
        substring(t.txt FROM '^(.*\n)0370 : ') ||
        '0370 : ' || upper(t.b) || ' 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00   ' ||
        CASE WHEN get_byte(decode(t.b, 'hex'), 0) BETWEEN 32 AND 126
             THEN chr(get_byte(decode(t.b, 'hex'), 0)) ELSE ' ' END ||
        repeat(' ', 15)
    )
)
FROM (
    SELECT id,
           changes->'pvd'->'add'->>'new' AS txt,
           substring(changes->'pvd'->'add'->>'new' FROM '\n0370 : ([0-9A-Fa-f]{2}) +[^\n]?$') AS b
    FROM disc_submissions
    WHERE status IN ('Pending', 'Draft')
      AND changes->'pvd'->'add'->>'new' ~ '^0320 : '
      AND changes->'pvd'->'add'->>'new' ~ '\n0370 : [0-9A-Fa-f]{2} +[^\n]?$'
) t
WHERE s.id = t.id;
