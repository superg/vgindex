INSERT INTO media_types
    (code,       name,                layer_count, pic,   rom_extension) VALUES
    ('cd',       'CD',                          1, FALSE, 'bin'),
    ('gdrom',    'GD-ROM',                      1, FALSE, 'bin'),
    ('dvd5',     'DVD-5',                       1, FALSE, 'iso'),
    ('dvd5gc',   'Nintendo GameCube Game Disc', 1, FALSE, 'iso'),
    ('dvd5wii',  'Wii Optical Disc (SL)',       1, FALSE, 'iso'),
    ('dvd9',     'DVD-9',                       2, FALSE, 'iso'),
    ('dvd9wii',  'Wii Optical Disc (DL)',       2, FALSE, 'iso'),
    ('hdvd15',   'HD DVD (SL)',                 1, FALSE, 'iso'),
    ('hdvd30',   'HD DVD (DL)',                 2, FALSE, 'iso'),
    ('bd25',     'BD-25',                       1, TRUE,  'iso'),
    ('bd25wiiu', 'Wii U Optical Disc (SL)',     1, TRUE,  'iso'),
    ('bd50',     'BD-50',                       2, TRUE,  'iso'),
    ('bd66',     'BD-66',                       2, TRUE,  'iso'),
    ('bd100',    'BD-100',                      3, TRUE,  'iso'),
    ('bd128',    'BD-128',                      4, TRUE,  'iso'),
    ('umd1',     'UMD (SL)',                    1, FALSE, 'iso'),
    ('umd2',     'UMD (DL)',                    2, FALSE, 'iso');

INSERT INTO categories
    (name) VALUES
    ('Games'),
    ('Demos'),
    ('Coverdiscs'),
    ('Bonus Discs'),
    ('Applications'),
    ('Multimedia'),
    ('Add-Ons'),
    ('Educational'),
    ('Preproduction'),
    ('Video'),
    ('Audio');

-- Regions: ISO 3166-1 alpha-2 (or X* user-assigned for non-country entries), ordered by dat priority
-- flag_code = ISO 3166-1 alpha-2 country code for the flag SVG (or special code for non-country entries)
INSERT INTO regions
    (code, name,                   flag_code, sort_order) VALUES
    ('us', 'USA',                  'us',       1),
    ('jp', 'Japan',                'jp',       2),
    ('xe', 'Europe',               'eu',       3),
    ('xa', 'Asia',                 'xa',       4),
    ('gb', 'UK',                   'gb',       5),
    ('fr', 'France',               'fr',       6),
    ('es', 'Spain',                'es',       7),
    ('ae', 'United Arab Emirates', 'ae',       8),
    ('ar', 'Argentina',            'ar',       9),
    ('at', 'Austria',              'at',      10),
    ('au', 'Australia',            'au',      11),
    ('be', 'Belgium',              'be',      12),
    ('bg', 'Bulgaria',             'bg',      13),
    ('br', 'Brazil',               'br',      14),
    ('by', 'Belarus',              'by',      15),
    ('ca', 'Canada',               'ca',      16),
    ('ch', 'Switzerland',          'ch',      17),
    ('cn', 'China',                'cn',      18),
    ('cz', 'Czech',                'cz',      19),
    ('de', 'Germany',              'de',      20),
    ('dk', 'Denmark',              'dk',      21),
    ('ee', 'Estonia',              'ee',      22),
    ('fi', 'Finland',              'fi',      23),
    ('gr', 'Greece',               'gr',      24),
    ('hr', 'Croatia',              'hr',      25),
    ('hu', 'Hungary',              'hu',      26),
    ('ie', 'Ireland',              'ie',      27),
    ('il', 'Israel',               'il',      28),
    ('in', 'India',                'in',      29),
    ('is', 'Iceland',              'is',      30),
    ('it', 'Italy',                'it',      31),
    ('kr', 'Korea',                'kr',      32),
    ('lt', 'Lithuania',            'lt',      33),
    ('nl', 'Netherlands',          'nl',      34),
    ('no', 'Norway',               'no',      35),
    ('nz', 'New Zealand',          'nz',      36),
    ('pl', 'Poland',               'pl',      37),
    ('pt', 'Portugal',             'pt',      38),
    ('ro', 'Romania',              'ro',      39),
    ('rs', 'Serbia',               'rs',      40),
    ('ru', 'Russia',               'ru',      41),
    ('se', 'Sweden',               'se',      42),
    ('sg', 'Singapore',            'sg',      43),
    ('sk', 'Slovakia',             'sk',      44),
    ('th', 'Thailand',             'th',      45),
    ('tr', 'Turkey',               'tr',      46),
    ('tw', 'Taiwan',               'tw',      47),
    ('ua', 'Ukraine',              'ua',      48),
    ('xl', 'Latin America',        'xl',      49),
    ('xp', 'Export',               'xp',      50),
    ('xs', 'Scandinavia',          'xs',      51),
    ('xw', 'World',                'un',      52),
    ('za', 'South Africa',         'za',      53);

-- Languages: IETF BCP 47 two-letter codes, ordered by dat priority
-- flag_code = ISO 3166-1 alpha-2 country code for the flag SVG
INSERT INTO languages
    (code, name,          flag_code, sort_order) VALUES
    ('en', 'English',     'gb',  1),
    ('ja', 'Japanese',    'jp',  2),
    ('fr', 'French',      'fr',  3),
    ('de', 'German',      'de',  4),
    ('es', 'Spanish',     'es',  5),
    ('it', 'Italian',     'it',  6),
    ('nl', 'Dutch',       'nl',  7),
    ('pt', 'Portuguese',  'pt',  8),
    ('sv', 'Swedish',     'se',  9),
    ('no', 'Norwegian',   'no', 10),
    ('da', 'Danish',      'dk', 11),
    ('fi', 'Finnish',     'fi', 12),
    ('zh', 'Chinese',     'cn', 13),
    ('ko', 'Korean',      'kr', 14),
    ('pl', 'Polish',      'pl', 15),
    ('ru', 'Russian',     'ru', 16),
    ('uk', 'Ukrainian',   'ua', 17),
    ('el', 'Greek',       'gr', 18),
    ('hr', 'Croatian',    'hr', 19),
    ('cs', 'Czech',       'cz', 20),
    ('hu', 'Hungarian',   'hu', 21),
    ('sk', 'Slovak',      'sk', 22),
    ('sl', 'Slovenian',   'si', 23),
    ('ar', 'Arabic',      'sa', 24),
    ('th', 'Thai',        'th', 25),
    ('tr', 'Turkish',     'tr', 26),
    ('eu', 'Basque',      'es', 27),
    ('ca', 'Catalan',     'es', 28),
    ('gd', 'Gaelic',      'gb', 29),
    ('hi', 'Hindi',       'in', 30),
    ('pa', 'Punjabi',     'in', 31),
    ('ta', 'Tamil',       'in', 32),
    ('he', 'Hebrew',      'il', 33),
    ('af', 'Afrikaans',   'za', 34),
    ('ro', 'Romanian',    'ro', 35),
    ('is', 'Icelandic',   'is', 36),
    ('la', 'Latin',       'va', 37),
    ('mk', 'Macedonian',  'mk', 38),
    ('id', 'Indonesian',  'id', 39),
    ('lt', 'Lithuanian',  'lt', 40),
    ('sr', 'Serbian',     'rs', 41),
    ('be', 'Belarusian',  'by', 42),
    ('et', 'Estonian',    'ee', 43),
    ('lv', 'Latvian',     'lv', 44),
    ('sq', 'Albanian',    'al', 45),
    ('hy', 'Armenian',    'am', 46),
    ('vi', 'Vietnamese',  'vn', 47),
    ('bg', 'Bulgarian',   'bg', 48);

-- Systems: Redump `code`; `ecd` vs `audio-cd` for PK.
-- has_* = OR over scraped discs: has_exe_date=d_date, has_sbi=d_libcrypt|d_securom, has_pvd=d_pvd, has_edc=d_edc (non-empty).
-- media_types is ordered by preference; the first code is the default selected when changing systems.
INSERT INTO systems
    (code,          type,     manufacturer,              name,                                      media_types) VALUES
    ('PSX',         '',       'Sony',                    'PlayStation',                             '{cd}'),
    ('PS2',         '',       'Sony',                    'PlayStation 2',                           '{dvd5,dvd9,cd}'),
    ('DVD-VIDEO',   '',       '',                        'DVD-Video',                               '{dvd5,dvd9}'),
    ('PSP',         '',       'Sony',                    'PlayStation Portable',                    '{umd1,umd2,dvd5,dvd9}'),
    ('MCD',         '',       'Sega',                    'Mega CD & Sega CD',                       '{cd}'),
    ('GC',          '',       'Nintendo',                'GameCube',                                '{dvd5gc}'),
    ('DC',          '',       'Sega',                    'Dreamcast',                               '{gdrom,cd}'),
    ('WII',         '',       'Nintendo',                'Wii',                                     '{dvd5wii,dvd9wii}'),
    ('SS',          '',       'Sega',                    'Saturn',                                  '{cd}'),
    ('3DO',         '',       '',                        '3DO Interactive Multiplayer',             '{cd}'),
    ('PC',          '',       'IBM',                     'PC compatible',                           '{cd,dvd5,dvd9,bd25,bd50,bd100,bd128}'),
    ('PCE',         '',       'NEC',                     'PC Engine CD & TurboGrafx CD',            '{cd}'),
    ('CDTV',        '',       'Commodore',               'Amiga CDTV',                              '{cd}'),
    ('CD32',        '',       'Commodore',               'Amiga CD32',                              '{cd}'),
    ('ACD',         '',       'Commodore',               'Amiga CD',                                '{cd,dvd5,dvd9}'),
    ('AUDIO-CD',    '',       '',                        'Audio CD',                                '{cd}'),
    ('QIS',         '',       'Bandai',                  'Playdia Quick Interactive System',        '{cd}'),
    ('PIPPIN',      '',       'Apple',                   'Pippin',                                  '{cd}'),
    ('PC-98',       '',       'NEC',                     'PC-98 series',                            '{cd}'),
    ('PS3',         '',       'Sony',                    'PlayStation 3',                           '{bd25,bd50,dvd5,dvd9,cd}'),
    ('XBOX',        '',       'Microsoft',               'Xbox',                                    '{dvd5,dvd9,cd}'),
    ('XBOX360',     '',       'Microsoft',               'Xbox 360',                                '{dvd5,dvd9,cd}'),
    ('MAC',         '',       'Apple',                   'Macintosh',                               '{cd,dvd5,dvd9}'),
    ('FMT',         '',       'Fujitsu',                 'FM Towns series',                         '{cd}'),
    ('HS',          '',       'Mattel',                  'HyperScan',                               '{cd}'),
    ('CDI',         '',       'Philips',                 'CD-i',                                    '{cd}'),
    ('VCD',         '',       '',                        'Video CD',                                '{cd}'),
    ('NAOMI',       'Arcade', 'Sega',                    'Naomi',                                   '{gdrom}'),
    ('TRF',         'Arcade', 'Namco · Sega · Nintendo', 'Triforce',                                '{gdrom}'),
    ('CHIHIRO',     'Arcade', 'Sega',                    'Chihiro',                                 '{gdrom}'),
    ('PC-FX',       '',       'NEC',                     'PC-FX & PC-FXGA',                         '{cd}'),
    ('VFLASH',      '',       'VTech',                   'V.Flash & V.Smile Pro',                   '{cd}'),
    ('NGCD',        '',       'SNK',                     'Neo Geo CD',                              '{cd}'),
    ('BD-VIDEO',    '',       '',                        'BD-Video',                                '{bd25,bd50}'),
    ('PALM',        '',       '',                        'Palm OS',                                 '{cd,dvd5,dvd9}'),
    ('PHOTO-CD',    '',       '',                        'Photo CD',                                '{cd}'),
    ('LINDBERGH',   'Arcade', 'Sega',                    'Lindbergh',                               '{dvd5,dvd9}'),
    ('PS4',         '',       'Sony',                    'PlayStation 4',                           '{bd25,bd50}'),
    ('PC-88',       '',       'NEC',                     'PC-88 series',                            '{cd}'),
    ('ENHANCED-CD', '',       '',                        'Enhanced CD',                             '{cd}'),
    ('WIIU',        '',       'Nintendo',                'Wii U',                                   '{bd25wiiu}'),
    ('XBOXONE',     '',       'Microsoft',               'Xbox One',                                '{bd25,bd50}'),
    ('PSXGS',       '',       'Datel',                   'PlayStation Cheat Device Updates',        '{cd}'),
    ('KSITE',       '',       'Tomy',                    'Kiss-Site',                               '{cd}'),
    ('GAMEWAVE',    '',       'ZAPiT Games',             'Game Wave Family Entertainment System',   '{dvd5,cd}'),
    ('QUIZARD',     '',       'TAB-Austria',             'Quizard',                                 '{cd}'),
    ('NAOMI2',      'Arcade', 'Sega',                    'Naomi 2',                                 '{gdrom}'),
    ('NS246',       'Arcade', 'Namco',                   'System 246',                              '{dvd5,cd}'),
    ('KSGV',        'Arcade', 'Konami',                  'System GV',                               '{cd}'),
    ('NUON',        '',       'VM Labs',                 'NUON',                                    '{dvd5}'),
    ('SRE2',        'Arcade', 'Sega',                    'RingEdge 2',                              '{dvd5,dvd9}'),
    ('KEA',         'Arcade', 'Konami',                  'e-Amusement',                             '{cd,dvd5}'),
    ('ITE',         '',       'Incredible Technologies', 'Eagle',                                   '{cd}'),
    ('KFB',         'Arcade', 'Konami',                  'FireBeat',                                '{cd,dvd5}'),
    ('KM2',         'Arcade', 'Konami',                  'M2',                                      '{cd}'),
    ('SRE',         'Arcade', 'Sega',                    'RingEdge',                                '{dvd5,dvd9}'),
    ('HVNXP',       '',       'Hasbro',                  'VideoNow XP',                             '{cd}'),
    ('M2',          '',       'Panasonic',               'M2',                                      '{cd}'),
    ('HVNC',        '',       'Hasbro',                  'VideoNow Color',                          '{cd}'),
    ('HVNJR',       '',       'Hasbro',                  'VideoNow Jr.',                            '{cd}'),
    ('NAVI',        '',       'Navisoft',                'Naviken',                                 '{cd}'),
    ('VIS',         '',       'Memorex',                 'Visual Information System',               '{cd}'),
    ('IXL',         '',       'Mattel',                  'Fisher-Price iXL',                        '{cd}'),
    ('AJCD',        '',       'Atari',                   'Jaguar CD Interactive Multimedia System', '{cd}'),
    ('HVN',         '',       'Hasbro',                  'VideoNow',                                '{cd}'),
    ('FPP',         'Arcade', 'Funworld',                'Photo Play',                              '{cd,dvd5,dvd9}'),
    ('SP21',        '',       'Sega',                    'Prologue 21 Multimedia Karaoke System',   '{cd}'),
    ('ARCH',        '',       'Acorn',                   'Archimedes & Risc PC',                    '{cd}'),
    ('PPC',         '',       'Microsoft',               'Pocket PC',                               '{cd,dvd5,dvd9}'),
    ('HDDVD-VIDEO', '',       '',                        'HD DVD-Video',                            '{hdvd15,hdvd30}'),
    ('X68K',        '',       'Sharp',                   'X68000',                                  '{cd,dvd5,dvd9}'),
    ('IKTV',        '',       'Tao',                     'iKTV',                                    '{cd}'),
    ('KS573',       'Arcade', 'Konami',                  'System 573',                              '{cd}'),
    ('XBOXSX',      '',       'Microsoft',               'Xbox Series X',                           '{bd25,bd50}'),
    ('PS5',         '',       'Sony',                    'PlayStation 5',                           '{bd66,bd100}');

-- Short display names (used for compact UI labels; falls back to `code` when empty).
UPDATE systems SET short_name = CASE code
    WHEN 'AUDIO-CD'    THEN 'Audio CD'
    WHEN 'BD-VIDEO'    THEN 'BD-Video'
    WHEN 'CDI'         THEN 'CD-i'
    WHEN 'CHIHIRO'     THEN 'Chihiro'
    WHEN 'DVD-VIDEO'   THEN 'DVD-Video'
    WHEN 'ENHANCED-CD' THEN 'Enhanced CD'
    WHEN 'GAMEWAVE'    THEN 'Game Wave'
    WHEN 'HDDVD-VIDEO' THEN 'HD DVD-Video'
    WHEN 'IXL'         THEN 'iXL'
    WHEN 'LINDBERGH'   THEN 'Lindbergh'
    WHEN 'NAOMI'       THEN 'Naomi'
    WHEN 'NAOMI2'      THEN 'Naomi 2'
    WHEN 'PALM'        THEN 'Palm OS'
    WHEN 'PHOTO-CD'    THEN 'Photo CD'
    WHEN 'PIPPIN'      THEN 'Pippin'
    WHEN 'QUIZARD'     THEN 'Quizard'
    WHEN 'VFLASH'      THEN 'V.Flash'
    WHEN 'WII'         THEN 'Wii'
    WHEN 'WIIU'        THEN 'Wii U'
    WHEN 'XBOX'        THEN 'Xbox'
    WHEN 'XBOX360'     THEN 'Xbox 360'
    WHEN 'XBOXONE'     THEN 'Xbox One'
    WHEN 'XBOXSX'      THEN 'Xbox SX'
    ELSE short_name
END;


UPDATE systems
SET has_title_foreign = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'CD32', 'ACD', 'AUDIO-CD', 'QIS', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'CDI', 'VCD', 'NAOMI', 'TRF', 'CHIHIRO', 'PC-FX', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'LINDBERGH', 'PS4', 'PC-88', 'ENHANCED-CD', 'WIIU', 'XBOXONE', 'PSXGS', 'KSITE', 'QUIZARD', 'NAOMI2', 'NS246', 'KSGV', 'NUON', 'SRE2', 'KEA', 'KFB', 'KM2', 'SRE', 'NAVI', 'AJCD', 'FPP', 'SP21', 'ARCH', 'PPC', 'HDDVD-VIDEO', 'X68K', 'KS573', 'XBOXSX', 'PS5');

UPDATE systems
SET has_disc_number = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'ACD', 'AUDIO-CD', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'CDI', 'VCD', 'PC-FX', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'PS4', 'ENHANCED-CD', 'XBOXONE', 'GAMEWAVE', 'KFB', 'HVNXP', 'M2', 'HVNC', 'HVNJR', 'HVN', 'FPP', 'ARCH', 'PPC', 'XBOXSX');

UPDATE systems
SET has_disc_title = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'CDTV', 'CD32', 'ACD', 'AUDIO-CD', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'CDI', 'VCD', 'PC-FX', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'PS4', 'ENHANCED-CD', 'XBOXONE', 'GAMEWAVE', 'SRE2', 'KEA', 'KFB', 'SRE', 'HVNXP', 'HVNC', 'HVNJR', 'NAVI', 'HVN', 'SP21', 'PPC', 'HDDVD-VIDEO', 'X68K', 'KS573', 'XBOXSX', 'PS5');

UPDATE systems
SET has_serial = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'CD32', 'ACD', 'AUDIO-CD', 'QIS', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'HS', 'CDI', 'VCD', 'NAOMI', 'TRF', 'CHIHIRO', 'PC-FX', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'LINDBERGH', 'PS4', 'PC-88', 'ENHANCED-CD', 'WIIU', 'XBOXONE', 'PSXGS', 'KSITE', 'GAMEWAVE', 'QUIZARD', 'NAOMI2', 'NS246', 'KSGV', 'NUON', 'SRE2', 'KEA', 'KFB', 'KM2', 'SRE', 'HVNXP', 'HVNC', 'HVNJR', 'NAVI', 'VIS', 'IXL', 'AJCD', 'HVN', 'FPP', 'SP21', 'ARCH', 'PPC', 'HDDVD-VIDEO', 'X68K', 'IKTV', 'KS573', 'XBOXSX', 'PS5');

UPDATE systems
SET has_edition = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'CD32', 'ACD', 'AUDIO-CD', 'QIS', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'HS', 'CDI', 'VCD', 'NAOMI', 'TRF', 'CHIHIRO', 'PC-FX', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'LINDBERGH', 'PS4', 'PC-88', 'ENHANCED-CD', 'WIIU', 'XBOXONE', 'PSXGS', 'KSITE', 'GAMEWAVE', 'QUIZARD', 'NAOMI2', 'NS246', 'NUON', 'ITE', 'SRE', 'HVNXP', 'HVNC', 'HVNJR', 'NAVI', 'VIS', 'IXL', 'AJCD', 'HVN', 'ARCH', 'PPC', 'HDDVD-VIDEO', 'X68K', 'IKTV', 'XBOXSX', 'PS5');

UPDATE systems
SET has_barcode = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'CD32', 'ACD', 'AUDIO-CD', 'QIS', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'HS', 'CDI', 'VCD', 'PC-FX', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'PS4', 'PC-88', 'ENHANCED-CD', 'WIIU', 'XBOXONE', 'PSXGS', 'KSITE', 'GAMEWAVE', 'NUON', 'HVNXP', 'HVNC', 'HVNJR', 'NAVI', 'VIS', 'IXL', 'AJCD', 'HVN', 'ARCH', 'PPC', 'HDDVD-VIDEO', 'X68K', 'IKTV', 'XBOXSX', 'PS5');

UPDATE systems
SET has_version = TRUE
WHERE code IN ('PSX', 'PS2', 'PSP', 'MCD', 'GC', 'DC', 'WII', 'SS', '3DO', 'PC', 'PCE', 'CDTV', 'CD32', 'ACD', 'PIPPIN', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'HS', 'CDI', 'VCD', 'NAOMI', 'TRF', 'CHIHIRO', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'LINDBERGH', 'PS4', 'ENHANCED-CD', 'WIIU', 'XBOXONE', 'PSXGS', 'GAMEWAVE', 'QUIZARD', 'NAOMI2', 'NS246', 'KSGV', 'NUON', 'SRE2', 'KEA', 'ITE', 'KFB', 'KM2', 'SRE', 'M2', 'VIS', 'IXL', 'AJCD', 'SP21', 'ARCH', 'PPC', 'KS573', 'PS5');

UPDATE systems
SET has_exe_date = TRUE
WHERE code IN ('PSX', 'PS2', 'MCD', 'DC', 'SS', 'CDTV', 'CD32', 'ACD', 'QIS', 'PC-98', 'FMT', 'HS', 'NAOMI', 'TRF', 'CHIHIRO', 'NGCD', 'NAOMI2', 'KSGV', 'KEA', 'KFB', 'NAVI', 'KS573');

UPDATE systems
SET has_edc = TRUE
WHERE code IN ('PSX');

UPDATE systems
SET has_disc_id = TRUE
WHERE code IN ('PS3');

UPDATE systems
SET has_key = TRUE
WHERE code IN ('PS3', 'WIIU');

UPDATE systems
SET has_universal_hash = TRUE
WHERE code IN ('AUDIO-CD');

UPDATE systems
SET has_protection = TRUE
WHERE code IN ('PSX', 'DVD-VIDEO', 'PC', 'MAC', 'BD-VIDEO', 'GAMEWAVE');

UPDATE systems
SET has_sector_ranges = TRUE
WHERE code IN ('XBOX', 'XBOX360');

UPDATE systems
SET has_sbi = TRUE
WHERE code IN ('PSX', 'PC', 'MAC');

UPDATE systems
SET has_pvd = TRUE
WHERE code IN ('PSX', 'PS2', 'DVD-VIDEO', 'PSP', 'MCD', 'DC', 'SS', 'PC', 'CDTV', 'CD32', 'ACD', 'QIS', 'PC-98', 'PS3', 'XBOX', 'XBOX360', 'MAC', 'FMT', 'HS', 'CDI', 'VCD', 'NAOMI', 'TRF', 'CHIHIRO', 'VFLASH', 'NGCD', 'BD-VIDEO', 'PALM', 'PHOTO-CD', 'PS4', 'ENHANCED-CD', 'XBOXONE', 'KSITE', 'GAMEWAVE', 'QUIZARD', 'NAOMI2', 'NS246', 'KSGV', 'NUON', 'KEA', 'ITE', 'KFB', 'VIS', 'IXL', 'SP21', 'ARCH', 'PPC', 'KS573', 'XBOXSX', 'PS5');

UPDATE systems
SET has_header = TRUE
WHERE code IN ('MCD', 'DC', 'SS', 'NAOMI', 'TRF', 'CHIHIRO', 'NAOMI2');

UPDATE systems
SET has_bca = TRUE
WHERE code IN ('GC', 'WII');

UPDATE systems
SET has_sample_start = TRUE
WHERE code IN ('AUDIO-CD');

UPDATE systems
SET has_offset_extra = TRUE
WHERE code IN ('DC', 'NAOMI', 'CHIHIRO', 'NAOMI2', 'TRF');
