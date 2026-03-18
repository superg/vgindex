-- System regions (NTSC/PAL/etc.)
INSERT INTO system_regions (name, flag_code, display_order) VALUES
    ('NTSC-U/C', 'us', 1),
    ('NTSC-J', 'jp', 2),
    ('NTSC-K', 'kr', 3),
    ('PAL', 'eu', 4);

-- Release regions / countries
INSERT INTO release_regions (code, name, flag_code, display_order) VALUES
    ('A', 'Asia', 'un', 1),
    ('Ar', 'Argentina', 'ar', 2),
    ('At', 'Austria', 'at', 3),
    ('Au', 'Australia', 'au', 4),
    ('B', 'Brazil', 'br', 5),
    ('C', 'China', 'cn', 6),
    ('Ca', 'Canada', 'ca', 7),
    ('Cz', 'Czech Republic', 'cz', 8),
    ('E', 'Europe', 'eu', 9),
    ('EJ', 'Europe, Japan', 'eu', 10),
    ('F', 'France', 'fr', 11),
    ('Fi', 'Finland', 'fi', 12),
    ('G', 'Germany', 'de', 13),
    ('Gr', 'Greece', 'gr', 14),
    ('H', 'Hungary', 'hu', 15),
    ('I', 'Italy', 'it', 16),
    ('J', 'Japan', 'jp', 17),
    ('K', 'Korea', 'kr', 18),
    ('N', 'Netherlands', 'nl', 19),
    ('P', 'Poland', 'pl', 20),
    ('Pt', 'Portugal', 'pt', 21),
    ('R', 'Russia', 'ru', 22),
    ('S', 'Spain', 'es', 23),
    ('Sk', 'Slovakia', 'sk', 24),
    ('Sw', 'Sweden', 'se', 25),
    ('UK', 'United Kingdom', 'gb', 26),
    ('U', 'USA', 'us', 27),
    ('UE', 'USA, Europe', 'us', 28),
    ('UJ', 'USA, Japan', 'us', 29),
    ('Ua', 'Ukraine', 'ua', 30),
    ('W', 'World', 'un', 31);

-- Languages
INSERT INTO languages (code, name, flag_code, display_order) VALUES
    ('ara', 'Arabic', 'sa', 1),
    ('cat', 'Catalan', 'es', 2),
    ('chi', 'Chinese', 'cn', 3),
    ('cze', 'Czech', 'cz', 4),
    ('dan', 'Danish', 'dk', 5),
    ('dut', 'Dutch', 'nl', 6),
    ('eng', 'English', 'us', 7),
    ('fin', 'Finnish', 'fi', 8),
    ('fre', 'French', 'fr', 9),
    ('ger', 'German', 'de', 10),
    ('gre', 'Greek', 'gr', 11),
    ('hrv', 'Croatian', 'hr', 12),
    ('hun', 'Hungarian', 'hu', 13),
    ('ita', 'Italian', 'it', 14),
    ('jap', 'Japanese', 'jp', 15),
    ('kor', 'Korean', 'kr', 16),
    ('nor', 'Norwegian', 'no', 17),
    ('pol', 'Polish', 'pl', 18),
    ('por', 'Portuguese', 'pt', 19),
    ('rus', 'Russian', 'ru', 20),
    ('slk', 'Slovak', 'sk', 21),
    ('spa', 'Spanish', 'es', 22),
    ('swe', 'Swedish', 'se', 23),
    ('tha', 'Thai', 'th', 24);

-- Title types
INSERT INTO title_types (name, display_order) VALUES
    ('Disc Title', 1),
    ('Disc Number', 2),
    ('Spine', 3),
    ('In-game', 4),
    ('Native', 5),
    ('Internal', 6),
    ('Disc Label', 7);

-- Serial types
INSERT INTO serial_types (name, display_order) VALUES
    ('Disc', 1),
    ('Case', 2),
    ('Internal', 3);

-- Systems: CD-based
INSERT INTO systems (short_code, full_name, allowed_media, allowed_system_regions, has_date_field, has_sbi, has_pvd, has_edc_field, display_order) VALUES
    ('psx', 'Sony PlayStation', '{CD}', '{1,2,3,4}', true, true, true, true, 1),
    ('ps2', 'Sony PlayStation 2', '{CD,DVD-5,DVD-9}', '{1,2,3,4}', true, false, true, false, 2),
    ('ps3', 'Sony PlayStation 3', '{BD-25,BD-50}', '{1,2,3,4}', false, false, false, false, 3),
    ('psp', 'Sony PlayStation Portable', '{UMD}', '{1,2,3,4}', false, false, false, false, 4),
    ('ss', 'Sega Saturn', '{CD}', '{1,2,4}', true, false, false, false, 5),
    ('dc', 'Sega Dreamcast', '{GD-ROM}', '{1,2,4}', true, false, false, false, 6),
    ('scd', 'Sega Mega-CD / Sega CD', '{CD}', '{1,2,4}', false, false, false, false, 7),
    ('pce', 'NEC PC Engine CD / TurboGrafx-CD', '{CD}', '{1,2}', false, false, false, false, 8),
    ('gc', 'Nintendo GameCube', '{DVD-5}', '{1,2,3,4}', false, false, false, false, 9),
    ('wii', 'Nintendo Wii', '{DVD-5,DVD-9}', '{1,2,3,4}', false, false, false, false, 10),
    ('xbox', 'Microsoft Xbox', '{DVD-5,DVD-9}', '{1,2,3,4}', false, false, false, false, 11),
    ('xbox360', 'Microsoft Xbox 360', '{DVD-5,DVD-9}', '{1,2,3,4}', false, false, false, false, 12),
    ('3do', 'Panasonic 3DO Interactive Multiplayer', '{CD}', '{}', false, false, false, false, 13),
    ('pc', 'IBM PC compatible', '{CD,DVD-5,DVD-9}', '{}', false, false, true, false, 14),
    ('mac', 'Apple Macintosh', '{CD,DVD-5,DVD-9}', '{}', false, false, false, false, 15),
    ('audio-cd', 'Audio CD', '{CD}', '{}', false, false, false, false, 16),
    ('dvd-video', 'DVD-Video', '{DVD-5,DVD-9}', '{}', false, false, false, false, 17),
    ('playdia', 'Bandai Playdia', '{CD}', '{2}', false, false, false, false, 18),
    ('pippin', 'Bandai / Apple Pippin', '{CD}', '{1,2}', false, false, false, false, 19),
    ('acd', 'Commodore Amiga CD', '{CD}', '{}', false, false, false, false, 20),
    ('cd32', 'Commodore Amiga CD32', '{CD}', '{}', false, false, false, false, 21),
    ('cdtv', 'Commodore Amiga CDTV', '{CD}', '{}', false, false, false, false, 22),
    ('dvdpg', 'DVDPG', '{DVD-5,DVD-9}', '{}', false, false, false, false, 23),
    ('fmt', 'Fujitsu FM Towns series', '{CD}', '{2}', false, false, false, false, 24),
    ('pc-98', 'NEC PC-98 series', '{CD}', '{2}', false, false, false, false, 25);

-- Systems: additional systems from redump.org (current site has many more)
INSERT INTO systems (short_code, full_name, allowed_media, allowed_system_regions, has_date_field, has_sbi, has_pvd, has_edc_field, display_order) VALUES
    ('ps4', 'Sony PlayStation 4', '{BD-25,BD-50,BD-66,BD-100}', '{1,2,3,4}', false, false, false, false, 26),
    ('ps5', 'Sony PlayStation 5', '{BD-25,BD-50,BD-66,BD-100}', '{1,2,3,4}', false, false, false, false, 27),
    ('vita', 'Sony PlayStation Vita', '{CD}', '{1,2,3,4}', false, false, false, false, 28),
    ('wiiu', 'Nintendo Wii U', '{DVD-5,DVD-9,BD-25}', '{1,2,3,4}', false, false, false, false, 29),
    ('switch', 'Nintendo Switch', '{BD-25}', '{1,2,3,4}', false, false, false, false, 30),
    ('xone', 'Microsoft Xbox One', '{BD-25,BD-50}', '{1,2,3,4}', false, false, false, false, 31),
    ('xsx', 'Microsoft Xbox Series X/S', '{BD-25,BD-50,BD-100}', '{1,2,3,4}', false, false, false, false, 32),
    ('hddvd', 'HD-DVD', '{HD-DVD}', '{}', false, false, false, false, 33),
    ('bd-video', 'Blu-ray Video', '{BD-25,BD-50}', '{}', false, false, false, false, 34),
    ('cdi', 'Philips CD-i', '{CD}', '{}', false, false, false, false, 35),
    ('neo-geo', 'SNK Neo Geo CD', '{CD}', '{1,2}', false, false, false, false, 36),
    ('pcfx', 'NEC PC-FX', '{CD}', '{2}', false, false, false, false, 37),
    ('tg16', 'NEC TurboGrafx-16 / PC Engine', '{CD}', '{1,2}', false, false, false, false, 38),
    ('vb', 'VCD', '{CD}', '{}', false, false, false, false, 39),
    ('palm-os', 'Palm OS', '{CD}', '{}', false, false, false, false, 40),
    ('photo-cd', 'Photo CD', '{CD}', '{}', false, false, false, false, 41),
    ('nuon', 'VM Labs NUON', '{DVD-5}', '{}', false, false, false, false, 42),
    ('n64dd', 'Nintendo 64DD', '{CD}', '{2}', false, false, false, false, 43),
    ('atari-jag', 'Atari Jaguar CD', '{CD}', '{1}', false, false, false, false, 44),
    ('3ds', 'Nintendo 3DS', '{CD}', '{1,2,3,4}', false, false, false, false, 45),
    ('nds', 'Nintendo DS', '{CD}', '{1,2,3,4}', false, false, false, false, 46),
    ('wii-u', 'Nintendo Wii U', '{DVD-9,BD-25}', '{1,2,3,4}', false, false, false, false, 47),
    ('psx-bios', 'Sony PlayStation - BIOS Images', '{CD}', '{}', false, false, false, false, 90),
    ('ps2-bios', 'Sony PlayStation 2 - BIOS Images', '{CD}', '{}', false, false, false, false, 91),
    ('xbox-bios', 'Microsoft Xbox - BIOS Images', '{CD}', '{}', false, false, false, false, 92);

-- OIDC clients seed (for phpBB and MediaWiki)
INSERT INTO oauth_clients (client_id, client_secret, redirect_uri, name) VALUES
    ('phpbb-client', 'change-this-secret-phpbb', 'http://localhost:8080/forum/ucp.php?mode=login', 'phpBB Forum'),
    ('mediawiki-client', 'change-this-secret-mediawiki', 'http://localhost:8080/wiki/Special:PluggableAuthLogin', 'MediaWiki');
