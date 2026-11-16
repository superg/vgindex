UPDATE discs
SET volume_label = NULL
WHERE media_type_code = 'xgd'
  AND volume_label IS NOT NULL;

UPDATE discs AS d
SET comments = regexp_replace(
    d.comments,
    '(^|(?<=\n))<b>Universal Hash \(SHA-1\):?</b>:? '
        || encode(d.universal_hash, 'hex')
        || '(\r?\n|$)',
    '',
    'g'
)
WHERE d.system_code = 'AUDIO-CD'
  AND d.universal_hash IS NOT NULL
  AND d.comments ~ (
      '(^|(?<=\n))<b>Universal Hash \(SHA-1\):?</b>:? '
          || encode(d.universal_hash, 'hex')
          || '(\r?\n|$)'
  );
