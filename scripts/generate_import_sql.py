#!/usr/bin/env python3
"""Generate a SQL import file from scraped Redump JSON data.

Usage:
    python scripts/generate_import_sql.py [--data-dir DIR] [--output FILE]

Reads JSON files from data/redump/db/ and produces a single .sql file
that can be imported via: psql -f import.sql
"""

import argparse
import binascii
import hashlib
import json
import os
import re
import sys
from collections import OrderedDict
from datetime import datetime

# ---------------------------------------------------------------------------
# Lookup maps (from 002_seed_data.sql)
# ---------------------------------------------------------------------------

SYSTEM_NAME_TO_CODE = {
    "Sony PlayStation": "PSX",
    "Sony PlayStation 2": "PS2",
    "DVD-Video": "DVD-VIDEO",
    "Sony PlayStation Portable": "PSP",
    "Sega Mega CD & Sega CD": "MCD",
    "Nintendo GameCube": "GC",
    "Sega Dreamcast": "DC",
    "Nintendo Wii": "WII",
    "Sega Saturn": "SS",
    "Panasonic 3DO Interactive Multiplayer": "3DO",
    "IBM PC compatible": "PC",
    "NEC PC Engine CD & TurboGrafx CD": "PCE",
    "Commodore Amiga CDTV": "CDTV",
    "Commodore Amiga CD32": "CD32",
    "Commodore Amiga CD": "ACD",
    "Audio CD": "AUDIO-CD",
    "Bandai Playdia Quick Interactive System": "QIS",
    "Bandai Pippin": "PIPPIN",
    "NEC PC-98 series": "PC-98",
    "Sony PlayStation 3": "PS3",
    "Microsoft Xbox": "XBOX",
    "Microsoft Xbox 360": "XBOX360",
    "Apple Macintosh": "MAC",
    "Fujitsu FM Towns series": "FMT",
    "Mattel HyperScan": "HS",
    "Philips CD-i": "CDI",
    "Video CD": "VCD",
    "Sega Naomi": "NAOMI",
    "Namco · Sega · Nintendo Triforce": "TRF",
    "Sega Chihiro": "CHIHIRO",
    "NEC PC-FX & PC-FXGA": "PC-FX",
    "VTech V.Flash & V.Smile Pro": "VFLASH",
    "Neo Geo CD": "NGCD",
    "BD-Video": "BD-VIDEO",
    "Palm OS": "PALM",
    "Photo CD": "PHOTO-CD",
    "Sega Lindbergh": "LINDBERGH",
    "Sony PlayStation 4": "PS4",
    "NEC PC-88 series": "PC-88",
    "Enhanced CD": "ENHANCED-CD",
    "Nintendo Wii U": "WIIU",
    "Microsoft Xbox One": "XBOXONE",
    "PlayStation GameShark Updates": "PSXGS",
    "Tomy Kiss-Site": "KSITE",
    "ZAPiT Games Game Wave Family Entertainment System": "GAMEWAVE",
    "TAB-Austria Quizard": "QUIZARD",
    "Sega Naomi 2": "NAOMI2",
    "Namco System 246": "NS246",
    "Konami System GV": "KSGV",
    "VM Labs NUON": "NUON",
    "Sega RingEdge 2": "SRE2",
    "Konami e-Amusement": "KEA",
    "Incredible Technologies Eagle": "ITE",
    "Konami FireBeat": "KFB",
    "Konami M2": "KM2",
    "Sega RingEdge": "SRE",
    "Hasbro VideoNow XP": "HVNXP",
    "Panasonic M2": "M2",
    "Hasbro VideoNow Color": "HVNC",
    "Hasbro VideoNow Jr.": "HVNJR",
    "Navisoft Naviken 2.1": "NAVI21",
    "Memorex Visual Information System": "VIS",
    "Mattel Fisher-Price iXL": "IXL",
    "Atari Jaguar CD Interactive Multimedia System": "AJCD",
    "Hasbro VideoNow": "HVN",
    "funworld Photo Play": "FPP",
    "Sega Prologue 21 Multimedia Karaoke System": "SP21",
    "Acorn Archimedes": "ARCH",
    "Pocket PC": "PPC",
    "HD DVD-Video": "HDDVD-VIDEO",
    "Sharp X68000": "X68K",
    "Tao iKTV": "IKTV",
    "Konami System 573": "KS573",
    "Microsoft Xbox Series X": "XBOXSX",
    "Sony PlayStation 5": "PS5",
}

MEDIA_NAME_TO_CODE = {
    "CD": "cd",
    "GD-ROM": "gdrom",
    "DVD-5": "dvd5",
    "Nintendo GameCube Game Disc": "dvd5gc",
    "Wii Optical Disc (SL)": "dvd5wii",
    "DVD-9": "dvd9",
    "Wii Optical Disc (DL)": "dvd9wii",
    "HD DVD (SL)": "hdvd15",
    "HD DVD (DL)": "hdvd30",
    "BD-25": "bd25",
    "Wii U Optical Disc (SL)": "bd25wiiu",
    "BD-50": "bd50",
    "BD-66": "bd66",
    "BD-100": "bd100",
    "UMD (SL)": "umd1",
    "UMD (DL)": "umd2",
}

REGION_NAME_TO_CODE = {
    "USA": "us", "Japan": "jp", "Europe": "xe", "Asia": "xa",
    "UK": "gb", "France": "fr", "Spain": "es",
    "United Arab Emirates": "ae", "Argentina": "ar", "Austria": "at",
    "Australia": "au", "Belgium": "be", "Bulgaria": "bg", "Brazil": "br",
    "Belarus": "by", "Canada": "ca", "Switzerland": "ch", "China": "cn",
    "Czech": "cz", "Germany": "de", "Denmark": "dk", "Estonia": "ee",
    "Finland": "fi", "Greece": "gr", "Croatia": "hr", "Hungary": "hu",
    "Ireland": "ie", "Israel": "il", "India": "in", "Iceland": "is",
    "Italy": "it", "Korea": "kr", "Lithuania": "lt", "Netherlands": "nl",
    "Norway": "no", "New Zealand": "nz", "Poland": "pl", "Portugal": "pt",
    "Romania": "ro", "Serbia": "rs", "Russia": "ru", "Sweden": "se",
    "Singapore": "sg", "Slovakia": "sk", "Thailand": "th", "Turkey": "tr",
    "Taiwan": "tw", "Ukraine": "ua", "Latin America": "xl",
    "Export": "xp", "Scandinavia": "xs", "World": "xw", "South Africa": "za",
}

LANG3_TO_LANG2 = {
    "afr": "af", "sqi": "sq", "ara": "ar", "hye": "hy", "baq": "eu",
    "bel": "be", "bul": "bg", "cat": "ca", "chi": "zh", "hrv": "hr",
    "cze": "cs", "dan": "da", "dut": "nl", "eng": "en", "est": "et",
    "fin": "fi", "fre": "fr", "gla": "gd", "ger": "de", "gre": "el",
    "heb": "he", "hin": "hi", "hun": "hu", "isl": "is", "ind": "id",
    "ita": "it", "jap": "ja", "kor": "ko", "lat": "la", "lav": "lv",
    "lit": "lt", "mkd": "mk", "nor": "no", "pan": "pa", "pol": "pl",
    "por": "pt", "ron": "ro", "rus": "ru", "srp": "sr", "slk": "sk",
    "slv": "sl", "spa": "es", "swe": "sv", "tam": "ta", "tha": "th",
    "tur": "tr", "ukr": "uk", "vie": "vi",
}

CATEGORY_NAME_TO_ID = {
    "Games": 1, "Demos": 2, "Coverdiscs": 3, "Bonus Discs": 4,
    "Applications": 5, "Multimedia": 6, "Add-Ons": 7, "Educational": 8,
    "Preproduction": 9, "Video": 10, "Audio": 11,
}

REGION_SORT_ORDER = {
    "USA": 1, "Japan": 2, "Europe": 3, "Asia": 4, "UK": 5, "France": 6,
    "Spain": 7, "United Arab Emirates": 8, "Argentina": 9, "Austria": 10,
    "Australia": 11, "Belgium": 12, "Bulgaria": 13, "Brazil": 14,
    "Belarus": 15, "Canada": 16, "Switzerland": 17, "China": 18,
    "Czech": 19, "Germany": 20, "Denmark": 21, "Estonia": 22,
    "Finland": 23, "Greece": 24, "Croatia": 25, "Hungary": 26,
    "Ireland": 27, "Israel": 28, "India": 29, "Iceland": 30,
    "Italy": 31, "Korea": 32, "Lithuania": 33, "Netherlands": 34,
    "Norway": 35, "New Zealand": 36, "Poland": 37, "Portugal": 38,
    "Romania": 39, "Serbia": 40, "Russia": 41, "Sweden": 42,
    "Singapore": 43, "Slovakia": 44, "Thailand": 45, "Turkey": 46,
    "Taiwan": 47, "Ukraine": 48, "Latin America": 49, "Export": 50,
    "Scandinavia": 51, "World": 52, "South Africa": 53,
}

LANG2_SORT_ORDER = {
    "en": 1, "ja": 2, "fr": 3, "de": 4, "es": 5, "it": 6, "nl": 7,
    "pt": 8, "sv": 9, "no": 10, "da": 11, "fi": 12, "zh": 13, "ko": 14,
    "pl": 15, "ru": 16, "uk": 17, "el": 18, "hr": 19, "cs": 20,
    "hu": 21, "sk": 22, "sl": 23, "ar": 24, "th": 25, "tr": 26,
    "eu": 27, "ca": 28, "gd": 29, "hi": 30, "pa": 31, "ta": 32,
    "he": 33, "af": 34, "ro": 35, "is": 36, "la": 37, "mk": 38,
    "id": 39, "lt": 40, "sr": 41, "be": 42, "et": 43, "lv": 44,
    "sq": 45, "hy": 46, "vi": 47, "bg": 48,
}

MEDIA_CODE_TO_ROM_EXT = {"cd": "bin", "gdrom": "bin"}

_FILENAME_REPLACEMENTS = [
    ("Böse", "Boese"),
    (": ", " - "),
    ('"', ""),
    ("*", "-"),
    (":", "-"),
    ("/", "-"),
    ("?", ""),
    ("°", ""),
    ("Ä", "A"),
    ("å", "a"),
    ("ä", "a"),
    ("É", "E"),
    ("é", "e"),
    ("ё", "e"),
    ("Ö", "O"),
    ("ö", "o"),
    ("Ñ", "N"),
    ("ñ", "n"),
    ("³", " 3"),
    ("α", "Alpha"),
]

# ---------------------------------------------------------------------------
# ROM name generation (mirrors Rust build_rom_base_name / build_rom_name)
# ---------------------------------------------------------------------------

def sanitize_filename(s):
    """Replace filesystem-unsafe and special characters in filenames."""
    for old, new in _FILENAME_REPLACEMENTS:
        s = s.replace(old, new)
    return s


def build_rom_base_name(title, region_names, lang2_codes, disc_number, disc_label, filename_suffix):
    """Build the base ROM name (game name) without track suffix or extension."""
    name = title
    sorted_regions = sorted(region_names, key=lambda r: REGION_SORT_ORDER.get(r, 9999))
    if sorted_regions:
        name += " ({})".format(", ".join(sorted_regions))
    sorted_langs = sorted(lang2_codes, key=lambda c: LANG2_SORT_ORDER.get(c, 9999))
    if len(sorted_langs) > 1:
        capitalized = [c[0].upper() + c[1:] for c in sorted_langs]
        name += " ({})".format(", ".join(capitalized))
    if disc_number:
        name += " (Disc {})".format(disc_number)
    if disc_label:
        name += " ({})".format(disc_label)
    if filename_suffix:
        name += " ({})".format(filename_suffix)
    return sanitize_filename(name)


def build_rom_name(base_name, track_number, total_tracks, extension):
    """Build a full ROM filename with optional track suffix."""
    name = base_name
    if total_tracks > 1 and track_number is not None:
        n = int(track_number)
        if total_tracks >= 10:
            name += " (Track {:02d})".format(n)
        else:
            name += " (Track {})".format(n)
    return name + "." + extension


def finalize_cue(raw_cue, base_name, extension):
    """Rewrite FILE tags in a CUE sheet with proper ROM filenames."""
    lines = raw_cue.split("\n")
    total_tracks = sum(1 for l in lines if l.strip().startswith("TRACK "))

    result = []
    i = 0
    while i < len(lines):
        stripped = lines[i].lstrip()
        if stripped.startswith("FILE "):
            track_num = None
            for ahead in lines[i + 1:]:
                at = ahead.strip()
                if at.startswith("TRACK "):
                    parts = at.split()
                    if len(parts) >= 2:
                        track_num = parts[1].lstrip("0") or "0"
                    break
            rom_name = build_rom_name(base_name, track_num, total_tracks, extension)
            file_type = stripped.rsplit(" ", 1)[-1] if " " in stripped else "BINARY"
            result.append('FILE "{}" {}'.format(rom_name, file_type))
        else:
            result.append(lines[i])
        i += 1
    return "\n".join(result)


def compute_cue_hashes(cue_text):
    """Compute size, CRC32, MD5, SHA1 of CUE text content."""
    data = cue_text.encode("utf-8")
    size = len(data)
    crc = binascii.crc32(data) & 0xFFFFFFFF
    crc32_hex = "{:08x}".format(crc)
    md5_hex = hashlib.md5(data).hexdigest()
    sha1_hex = hashlib.sha1(data).hexdigest()
    return size, crc32_hex, md5_hex, sha1_hex

# ---------------------------------------------------------------------------
# SQL escaping
# ---------------------------------------------------------------------------

def sql_str(val):
    """Escape a string for SQL single-quoted literal. Returns 'NULL' for None."""
    if val is None:
        return "NULL"
    escaped = val.replace("'", "''").replace("\\", "\\\\")
    return f"E'{escaped}'"


def sql_str_or_null(val):
    if val is None or (isinstance(val, str) and not val.strip()):
        return "NULL"
    return sql_str(val)


def sql_int(val):
    if val is None:
        return "NULL"
    return str(int(val))


def sql_bool(val):
    if val is None:
        return "NULL"
    return "TRUE" if val else "FALSE"


def sql_bytea(data_bytes):
    """Convert bytes to PostgreSQL BYTEA hex literal."""
    if data_bytes is None or len(data_bytes) == 0:
        return "NULL"
    return f"'\\x{data_bytes.hex()}'"


def sql_text_array(items):
    """Convert a list of strings to PostgreSQL TEXT[] literal."""
    if not items:
        return "'{}'::TEXT[]"
    inner = ", ".join(sql_str(s) for s in items)
    return f"ARRAY[{inner}]::TEXT[]"


def sql_int_array(items):
    """Convert a list of ints to PostgreSQL INT[] literal."""
    if not items:
        return "NULL"
    inner = ", ".join(str(int(v)) for v in items)
    return f"ARRAY[{inner}]::INT[]"


def sql_int4range_array(ranges):
    """Convert list of (start, end) tuples to INT4RANGE[] literal."""
    if not ranges:
        return "NULL"
    parts = []
    for start, end in ranges:
        parts.append(f"'[{start},{end}]'::INT4RANGE")
    return f"ARRAY[{', '.join(parts)}]"


def sql_timestamp(ts_str):
    """Convert a timestamp string to SQL timestamp literal."""
    if not ts_str:
        return "NULL"
    return sql_str(ts_str)


def sql_jsonb(obj):
    """Convert a Python object to a SQL JSONB literal."""
    return sql_str(json.dumps(obj, ensure_ascii=False))

# ---------------------------------------------------------------------------
# Hex dump parsing
# ---------------------------------------------------------------------------

def parse_hex_dump_with_address(text):
    """Parse hex dump format: '0320 : XX XX XX ... ASCII_TEXT\\n'.
    Returns raw bytes."""
    result = bytearray()
    for line in text.split("\n"):
        line = line.strip()
        if not line:
            continue
        colon_pos = line.find(":")
        if colon_pos < 0:
            continue
        hex_and_ascii = line[colon_pos + 1:].strip()
        # The ASCII column follows after 3+ spaces; the byte-group gap is only 2.
        triple_space = hex_and_ascii.find("   ")
        if triple_space >= 0:
            hex_part = hex_and_ascii[:triple_space]
        else:
            hex_part = hex_and_ascii
        for token in hex_part.split():
            if len(token) == 2:
                try:
                    result.append(int(token, 16))
                except ValueError:
                    pass
    return bytes(result)


def parse_hex_raw_spaced(text):
    """Parse raw hex: 'XX XX XX XX ... \\nXX XX ...' (no address, no ASCII).
    Used for d_pic_data."""
    result = bytearray()
    for token in text.split():
        if len(token) == 2:
            try:
                result.append(int(token, 16))
            except ValueError:
                pass
    return bytes(result)


def parse_hex_bca(text):
    """Parse BCA hex: 'XXXX XXXX XXXX XXXX ...' (groups of 4 hex chars = 2 bytes)."""
    result = bytearray()
    for token in text.split():
        token = token.strip()
        if not token:
            continue
        for i in range(0, len(token), 2):
            pair = token[i:i+2]
            if len(pair) == 2:
                try:
                    result.append(int(pair, 16))
                except ValueError:
                    pass
    return bytes(result)

# ---------------------------------------------------------------------------
# Track / cue parsing
# ---------------------------------------------------------------------------

def parse_track_numbers_from_cue(cue_text):
    """Extract track numbers from cue sheet in order."""
    tracks = []
    for line in cue_text.split("\n"):
        m = re.match(r'\s*TRACK\s+(\d+)', line)
        if m:
            tracks.append(m.group(1).lstrip("0") or "0")
    return tracks


def parse_track_hashes(tracks_text):
    """Parse d_tracks: newline-delimited rows of size/crc/md5/sha1."""
    rows = []
    for line in tracks_text.strip().split("\n"):
        line = line.strip()
        if not line:
            continue
        size = _extract_attr(line, "size") or "0"
        crc = _extract_attr(line, "crc") or ""
        md5 = _extract_attr(line, "md5") or ""
        sha1 = _extract_attr(line, "sha1") or ""
        rows.append((int(size), crc, md5, sha1))
    return rows


def _extract_attr(text, name):
    m = re.search(rf'{name}="([^"]*)"', text)
    return m.group(1) if m else None

# ---------------------------------------------------------------------------
# Protection ranges parsing
# ---------------------------------------------------------------------------

def parse_ss_ranges(text):
    """Parse '108976-113071\\n3719856-3723951' into list of (start, end) tuples."""
    ranges = []
    for line in text.strip().split("\n"):
        line = line.strip()
        if not line:
            continue
        parts = line.split("-")
        if len(parts) == 2:
            try:
                ranges.append((int(parts[0]), int(parts[1])))
            except ValueError:
                pass
    return ranges

# ---------------------------------------------------------------------------
# Ring code parsing
# ---------------------------------------------------------------------------

def parse_ring_entry(ring_obj):
    """Parse a ring object into entry-level and layer-level data.

    Returns: (offset, offset_extra, layers_dict)
        offset: int offset from 0_value (or None)
        offset_extra: int offset from 1_value (or None)
        layers_dict: {layer_num: {mastering_code, mastering_sid, mould_sids, toolstamps, additional_moulds}}
    """
    offset = None
    offset_extra = None
    layers = {}

    for key, val in ring_obj.items():
        if key == "0_value":
            try:
                offset = int(val)
            except (ValueError, TypeError):
                pass
            continue
        if key == "1_value":
            try:
                offset_extra = int(val)
            except (ValueError, TypeError):
                pass
            continue
        if key == "1_density":
            continue

        m = re.match(r'^(ma|ts|mo)(\d+)(_sid)?$', key)
        if not m:
            continue
        prefix, layer_str, suffix = m.group(1), m.group(2), m.group(3)
        raw_layer_num = int(layer_str)
        # Redump ring keys are typically 1-based (ma1, mo1_sid, ...).
        # Store layers zero-based in DB for consistency with app editing logic.
        layer_num = raw_layer_num - 1 if raw_layer_num > 0 else raw_layer_num
        if layer_num not in layers:
            layers[layer_num] = {
                "mastering_code": None,
                "mastering_sid": None,
                "mould_sids": [],
                "toolstamps": [],
                "additional_moulds": [],
            }
        layer = layers[layer_num]

        val_str = str(val).strip() if val else ""
        if prefix == "ma" and suffix is None:
            layer["mastering_code"] = val_str
        elif prefix == "ma" and suffix == "_sid":
            layer["mastering_sid"] = val_str
        elif prefix == "ts":
            layer["toolstamps"] = [s.strip() for s in val_str.split(",") if s.strip()]
        elif prefix == "mo" and suffix == "_sid":
            layer["mould_sids"] = [s.strip() for s in val_str.split(",") if s.strip()]
        elif prefix == "mo" and suffix is None:
            layer["additional_moulds"] = [s.strip() for s in val_str.split(",") if s.strip()]

    return offset, offset_extra, layers

# ---------------------------------------------------------------------------
# Change date parsing
# ---------------------------------------------------------------------------

def parse_change_date(date_str):
    """Parse 'Mar 21 2026, 11:27' -> ISO timestamp string."""
    if not date_str:
        return None
    try:
        dt = datetime.strptime(date_str.strip(), "%b %d %Y, %H:%M")
        return dt.strftime("%Y-%m-%d %H:%M:00")
    except ValueError:
        return None

# ---------------------------------------------------------------------------
# Merge helpers
# ---------------------------------------------------------------------------

def merge_dumpers(data):
    """Merge d_dumpers[] and d_dumpers_text into one list of usernames."""
    dumpers = list(data.get("d_dumpers[]", []))
    text = data.get("d_dumpers_text", "")
    if text:
        for name in text.split(","):
            name = name.strip()
            if name and name not in dumpers:
                dumpers.append(name)
    return dumpers


def merge_editions(data):
    """Merge d_editions[] and d_editions_text into a list of edition strings."""
    editions = list(data.get("d_editions[]", []))
    text = data.get("d_editions_text", "")
    if text:
        for e in text.split(","):
            e = e.strip()
            if e and e not in editions:
                editions.append(e)
    return editions


def split_csv(value):
    """Split a comma-separated string into a list of trimmed, non-empty values."""
    if not value or not isinstance(value, str):
        return []
    return [s.strip() for s in value.split(",") if s.strip()]


def merge_offsets(data):
    """Merge d_offset[] and d_offset_text into a list of int offset values."""
    offsets = []
    for val in data.get("d_offset[]", []):
        try:
            offsets.append(int(val))
        except (ValueError, TypeError):
            pass
    text = data.get("d_offset_text", "")
    if text:
        for val in text.split(","):
            val = val.strip()
            if val:
                try:
                    offsets.append(int(val))
                except (ValueError, TypeError):
                    pass
    return offsets


def build_protection(data):
    """Combine d_protection, d_protection_a, d_protection_l into protection string."""
    parts = []
    prot = data.get("d_protection", "")
    if prot:
        parts.append(prot.strip())
    if data.get("d_protection_a", "").lower() == "yes":
        parts.append("AntiModchip")
    if data.get("d_protection_l", "").lower() == "yes":
        parts.append("libcrypt")
    return ", ".join(parts) if parts else None


def build_keys(data):
    """Collect d_d1_key and d_d2_key into a text array."""
    keys = []
    d1 = data.get("d_d1_key", "")
    d2 = data.get("d_d2_key", "")
    if d1:
        keys.append(d1.strip().replace(" ", ""))
    if d2:
        keys.append(d2.strip().replace(" ", ""))
    return keys if keys else None

# ---------------------------------------------------------------------------
# Change history -> submission data payload
# ---------------------------------------------------------------------------

def build_submission_data(fields_list):
    """Build a JSON-serializable data payload from change fields."""
    result_fields = []
    for f in fields_list:
        field_name = f.get("field", "")
        field_type = f.get("type", "")
        new_val = f.get("new_value", "")
        old_val = f.get("old_value", "")
        if new_val == old_val:
            continue
        entry = {"field": field_name, "type": field_type}
        if new_val:
            entry["new_value"] = new_val
        if old_val:
            entry["old_value"] = old_val
        result_fields.append(entry)
    return {"fields": result_fields}

# ---------------------------------------------------------------------------
# Main import generation
# ---------------------------------------------------------------------------

def disc_id_from_filename(fname):
    """Extract disc ID from filename like '000042.json' -> 42."""
    base = os.path.splitext(fname)[0]
    return int(base.lstrip("0") or "0")


def process_all(data_dir, output_path, max_disc_id=None):
    # Pass 1: collect all usernames and load all disc data
    print(f"Scanning {data_dir} ...", file=sys.stderr)
    all_usernames = set()
    disc_files = []

    for fname in sorted(os.listdir(data_dir)):
        if not fname.endswith(".json"):
            continue
        disc_id = disc_id_from_filename(fname)
        if max_disc_id is not None and disc_id > max_disc_id:
            continue
        path = os.path.join(data_dir, fname)
        file_size = os.path.getsize(path)
        if file_size == 0:
            disc_files.append((fname, None))
            continue
        with open(path) as f:
            data = json.load(f)
        disc_files.append((fname, data))

        for name in merge_dumpers(data):
            all_usernames.add(name)
        for change in data.get("changes", []):
            user = change.get("user", "")
            if user:
                all_usernames.add(user)

    # Assign user IDs
    user_id_map = {}
    for uid, username in enumerate(sorted(all_usernames), start=1):
        user_id_map[username] = uid

    print(f"Found {len(disc_files)} disc files, {len(all_usernames)} unique users", file=sys.stderr)

    # Pass 2: generate SQL
    with open(output_path, "w") as out:
        out.write("-- Auto-generated import from Redump scrape data\n")
        out.write("-- Generated: {}\n\n".format(datetime.now().astimezone().isoformat()))
        out.write("BEGIN;\n\n")

        # Users
        _write_users(out, user_id_map)

        # Track ring entry ID counter
        ring_entry_id = 0

        # Per-disc data
        disc_inserts = []
        region_inserts = []
        language_inserts = []
        file_inserts = []
        ring_entry_inserts = []
        ring_layer_inserts = []
        dumper_inserts = []
        submission_inserts = []

        for fname, data in disc_files:
            disc_id = disc_id_from_filename(fname)

            if data is None:
                # Empty file: disabled stub
                disc_inserts.append(_build_empty_disc_insert(disc_id))
                continue

            # Regions (collect names before disc insert for CUE finalization)
            region_str = data.get("d_region", "")
            region_names = [r.strip() for r in region_str.split(", ")
                           if r.strip() and r.strip() in REGION_NAME_TO_CODE]
            for region_name in region_names:
                region_inserts.append(
                    f"({disc_id}, '{REGION_NAME_TO_CODE[region_name]}')"
                )

            # Languages (collect codes before disc insert for CUE finalization)
            lang2_codes = []
            for lang3 in data.get("d_languages[]", []):
                lang2 = LANG3_TO_LANG2.get(lang3)
                if lang2:
                    lang2_codes.append(lang2)
                    language_inserts.append(f"({disc_id}, '{lang2}')")

            # Finalize CUE before building disc insert
            d_cue = data.get("d_cue", "")
            media_code = MEDIA_NAME_TO_CODE.get(data.get("media", ""), "cd")
            rom_ext = MEDIA_CODE_TO_ROM_EXT.get(media_code, "iso")
            if d_cue.strip():
                title = data.get("d_title", str(disc_id))
                if not title:
                    title = str(disc_id)
                base_name = build_rom_base_name(
                    title, region_names, lang2_codes,
                    data.get("d_number"), data.get("d_label"),
                    data.get("d_version_datfile"),
                )
                data["d_cue"] = finalize_cue(d_cue, base_name, rom_ext)

            # Build disc INSERT (picks up finalized d_cue)
            disc_sql = _build_disc_insert(disc_id, data)
            disc_inserts.append(disc_sql)

            # Files: whole-disc hashes (track 0)
            d_size = data.get("d_size")
            d_crc32 = data.get("d_crc32")
            d_md5 = data.get("d_md5")
            d_sha1 = data.get("d_sha1")
            if d_size and d_crc32 and d_md5 and d_sha1:
                file_inserts.append(
                    f"({disc_id}, '0', {int(d_size)}, "
                    f"{sql_str(d_crc32)}, {sql_str(d_md5)}, {sql_str(d_sha1)})"
                )

            # Files: per-track from d_tracks + d_cue
            d_tracks = data.get("d_tracks", "")
            if d_tracks.strip():
                track_hashes = parse_track_hashes(d_tracks)
                if d_cue.strip():
                    track_numbers = parse_track_numbers_from_cue(d_cue)
                else:
                    track_numbers = [str(i + 1) for i in range(len(track_hashes))]

                for i, (size, crc, md5, sha1) in enumerate(track_hashes):
                    tn = track_numbers[i] if i < len(track_numbers) else str(i + 1)
                    file_inserts.append(
                        f"({disc_id}, {sql_str(tn)}, {size}, "
                        f"{sql_str(crc)}, {sql_str(md5)}, {sql_str(sha1)})"
                    )

            # CUE file entry (track_number = NULL)
            if d_cue.strip():
                cue_size, cue_crc, cue_md5, cue_sha1 = compute_cue_hashes(data["d_cue"])
                file_inserts.append(
                    f"({disc_id}, NULL, {cue_size}, "
                    f"{sql_str(cue_crc)}, {sql_str(cue_md5)}, {sql_str(cue_sha1)})"
                )

            # Ring codes
            offsets = merge_offsets(data)
            rings = data.get("rings", [])

            if rings:
                for ring_obj in rings:
                    ring_entry_id += 1
                    entry_offset, entry_offset_extra, layers_dict = parse_ring_entry(ring_obj)
                    comment = None

                    # Apply offset inheritance
                    if entry_offset is None and len(offsets) == 1:
                        entry_offset = offsets[0]
                        comment = "offset inherited"

                    ring_entry_inserts.append(
                        f"({ring_entry_id}, {disc_id}, "
                        f"{sql_int(entry_offset)}, "
                        f"{sql_int(entry_offset_extra)}, "
                        f"NULL, "  # sample_data_start
                        f"{sql_str_or_null(comment)})"
                    )

                    for layer_num in sorted(layers_dict.keys()):
                        layer = layers_dict[layer_num]
                        ring_layer_inserts.append(
                            f"({ring_entry_id}, {layer_num}, "
                            f"{sql_str_or_null(layer['mastering_code'])}, "
                            f"{sql_str_or_null(layer['mastering_sid'])}, "
                            f"{sql_text_array(layer['mould_sids'])}, "
                            f"{sql_text_array(layer['toolstamps'])}, "
                            f"{sql_text_array(layer['additional_moulds'])})"
                        )
            elif offsets:
                # No rings but have offsets: create entry-only rows
                for off_val in offsets:
                    ring_entry_id += 1
                    ring_entry_inserts.append(
                        f"({ring_entry_id}, {disc_id}, "
                        f"{sql_int(off_val)}, "
                        f"NULL, "
                        f"NULL, "
                        f"{sql_str('offset inherited')})"
                    )

            # Dumpers
            for dumper_name in merge_dumpers(data):
                uid = user_id_map.get(dumper_name)
                if uid:
                    dumper_inserts.append(f"({disc_id}, {uid})")

            # Submissions (changes)
            for change in data.get("changes", []):
                user = change.get("user", "")
                uid = user_id_map.get(user)
                if not uid:
                    continue
                ts = parse_change_date(change.get("date", ""))
                fields = change.get("fields", [])
                payload = build_submission_data(fields)
                submission_inserts.append(
                    f"('Edit', {uid}, {disc_id}, "
                    f"{sql_jsonb(payload)}, "
                    f"'Approved', {uid}, {sql_str('inherited')}, "
                    f"{sql_timestamp(ts)}, {sql_timestamp(ts)})"
                )

        # Write disc inserts
        _write_batched(out, "discs",
            "(id, enabled, media_type_code, category_id, system_code, title, "
            "filename_suffix, comments, contents, title_foreign, disc_title, "
            "disc_number, serial, version, edition, barcode, error_count, "
            "exe_date, edc, layerbreaks, pvd, pic, bca, header, protection, "
            "sector_ranges, sbi, keys, cue, "
            "questionable) OVERRIDING SYSTEM VALUE",
            disc_inserts,
        )

        _write_batched(out, "disc_regions",
            "(disc_id, region_code)", region_inserts)

        _write_batched(out, "disc_languages",
            "(disc_id, language_code)", language_inserts)

        _write_batched(out, "files",
            "(disc_id, track_number, size, crc32, md5, sha1)", file_inserts)

        _write_batched(out, "disc_ring_code_entries",
            "(id, disc_id, offset_value, offset_extra_value, sample_data_start, comment) OVERRIDING SYSTEM VALUE",
            ring_entry_inserts)

        _write_batched(out, "disc_ring_code_layers",
            "(entry_id, layer, mastering_code, mastering_sid, mould_sids, toolstamps, additional_moulds)",
            ring_layer_inserts)

        _write_batched(out, "disc_dumpers",
            "(disc_id, user_id)", dumper_inserts)

        _write_batched(out, "disc_submissions",
            "(submission_type, submitter_id, target_disc_id, changes, status, "
            "reviewer_id, review_comment, created_at, reviewed_at)",
            submission_inserts)

        # Reset sequences
        out.write("\n-- Reset sequences\n")
        out.write("SELECT setval('discs_id_seq', (SELECT COALESCE(MAX(id), 1) FROM discs));\n")
        out.write("SELECT setval('users_id_seq', (SELECT COALESCE(MAX(id), 1) FROM users));\n")
        out.write("SELECT setval('disc_ring_code_entries_id_seq', (SELECT COALESCE(MAX(id), 1) FROM disc_ring_code_entries));\n")
        out.write("SELECT setval('files_id_seq', (SELECT COALESCE(MAX(id), 1) FROM files));\n")
        out.write("SELECT setval('disc_submissions_id_seq', (SELECT COALESCE(MAX(id), 1) FROM disc_submissions));\n")

        out.write("\nCOMMIT;\n")

    print(f"Wrote {output_path}", file=sys.stderr)


def _write_users(out, user_id_map):
    out.write("-- Users (imported, non-loginable)\n")
    batch = []
    for username, uid in sorted(user_id_map.items(), key=lambda x: x[1]):
        email = f"{username}@imported.invalid"
        batch.append(
            f"({uid}, {sql_str(username)}, {sql_str(email)}, '!', "
            f"'User', FALSE, FALSE)"
        )
    _write_batched(out, "users",
        "(id, username, email, password_hash, role, is_active, email_verified) "
        "OVERRIDING SYSTEM VALUE",
        batch)


def _build_empty_disc_insert(disc_id):
    """Build INSERT values for an empty/nonexistent disc."""
    return (
        f"({disc_id}, FALSE, 'cd', 1, 'PSX', {sql_str(str(disc_id).zfill(6))}, "
        f"NULL, NULL, NULL, NULL, NULL, NULL, "
        f"'{{}}'::TEXT[], NULL, '{{}}'::TEXT[], '{{}}'::TEXT[], "
        f"NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, "
        f"NULL, NULL, NULL, FALSE)"
    )


def _build_disc_insert(disc_id, data):
    """Build INSERT values for a disc from JSON data."""
    system_code = SYSTEM_NAME_TO_CODE.get(data.get("system", ""), "PSX")
    media_code = MEDIA_NAME_TO_CODE.get(data.get("media", ""), "cd")
    category_id = CATEGORY_NAME_TO_ID.get(data.get("d_category", ""), 1)

    title = data.get("d_title", str(disc_id))
    if not title:
        title = str(disc_id)

    # Status
    status = data.get("d_status", "")
    enabled = status != "2"
    questionable = status == "3"

    # Edition
    edition = merge_editions(data)

    # Error count
    error_count = None
    if data.get("d_errors"):
        try:
            error_count = int(data["d_errors"])
        except ValueError:
            pass

    # EDC
    edc = data.get("d_edc", "")
    edc_value = None
    if edc.lower() == "yes":
        edc_value = True
    elif edc.lower() == "no":
        edc_value = False

    # Layerbreaks
    layerbreaks = None
    lb = data.get("d_layerbreak")
    if lb:
        try:
            layerbreaks = [int(lb)]
        except (ValueError, TypeError):
            pass

    # Hex fields
    pvd_bytes = None
    if data.get("d_pvd"):
        raw = parse_hex_dump_with_address(data["d_pvd"])
        pvd_bytes = raw[:82] if len(raw) > 82 else raw

    pic_bytes = None
    if data.get("d_pic_data"):
        pic_bytes = parse_hex_raw_spaced(data["d_pic_data"])

    bca_bytes = None
    if data.get("d_bca"):
        bca_bytes = parse_hex_bca(data["d_bca"])

    header_bytes = None
    if data.get("d_header"):
        header_bytes = parse_hex_dump_with_address(data["d_header"])

    # Protection
    protection = build_protection(data)

    # Sector ranges
    sector_ranges = None
    if data.get("d_ssranges"):
        sector_ranges = parse_ss_ranges(data["d_ssranges"])

    # SBI
    sbi = data.get("d_libcrypt") or data.get("d_securom") or None

    # Keys
    keys = build_keys(data)

    # Cue
    cue = data.get("d_cue") or None

    return (
        f"({disc_id}, {sql_bool(enabled)}, "
        f"{sql_str(media_code)}, {category_id}, {sql_str(system_code)}, "
        f"{sql_str(title)}, "
        f"{sql_str_or_null(data.get('d_version_datfile'))}, "
        f"{sql_str_or_null(data.get('d_comments'))}, "
        f"{sql_str_or_null(data.get('d_contents'))}, "
        f"{sql_str_or_null(data.get('d_title_foreign'))}, "
        f"{sql_str_or_null(data.get('d_label'))}, "
        f"{sql_str_or_null(data.get('d_number'))}, "
        f"{sql_text_array(split_csv(data.get('d_serial')))}, "
        f"{sql_str_or_null(data.get('d_version'))}, "
        f"{sql_text_array(edition)}, "
        f"{sql_text_array(split_csv(data.get('d_barcode')))}, "
        f"{sql_int(error_count)}, "
        f"{sql_str_or_null(data.get('d_date'))}, "
        f"{sql_bool(edc_value)}, "
        f"{sql_int_array(layerbreaks) if layerbreaks else 'NULL'}, "
        f"{sql_bytea(pvd_bytes)}, "
        f"{sql_bytea(pic_bytes)}, "
        f"{sql_bytea(bca_bytes)}, "
        f"{sql_bytea(header_bytes)}, "
        f"{sql_str_or_null(protection)}, "
        f"{sql_int4range_array(sector_ranges) if sector_ranges else 'NULL'}, "
        f"{sql_str_or_null(sbi)}, "
        f"{sql_text_array(keys) if keys else 'NULL'}, "
        f"{sql_str_or_null(cue)}, "
        f"{sql_bool(questionable)})"
    )


BATCH_SIZE = 500

def _write_batched(out, table, columns, values):
    """Write INSERT statements in batches."""
    if not values:
        return
    out.write(f"\n-- {table} ({len(values)} rows)\n")
    for i in range(0, len(values), BATCH_SIZE):
        batch = values[i:i + BATCH_SIZE]
        out.write(f"INSERT INTO {table} {columns} VALUES\n")
        out.write(",\n".join(f"  {v}" for v in batch))
        out.write(";\n")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Generate SQL import from Redump scrape data")
    parser.add_argument("--data-dir", default="data/redump/db",
                        help="Path to scraped JSON directory")
    parser.add_argument("--output", "-o", default="import.sql",
                        help="Output SQL file path")
    parser.add_argument("--max-id", type=int, default=None,
                        help="Only import discs up to this ID (for faster iteration)")
    args = parser.parse_args()

    if not os.path.isdir(args.data_dir):
        print(f"Error: {args.data_dir} is not a directory", file=sys.stderr)
        sys.exit(1)

    process_all(args.data_dir, args.output, max_disc_id=args.max_id)


if __name__ == "__main__":
    main()
