UPDATE discs
SET protection = replace(
        replace(protection, 'Title Key: No Title Key', 'Title Key: N/A'),
        'Copyright Protection System Type: No',
        'Copyright Protection System Type: NONE'
    )
WHERE protection LIKE '%Title Key: No Title Key%'
   OR protection LIKE '%Copyright Protection System Type: No%';
