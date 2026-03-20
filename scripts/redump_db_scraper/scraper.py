#!/usr/bin/env python3
"""
Redump.org disc database scraper.

Scrapes edit page fields and change history for disc IDs 2..N from redump.org.
Requires a valid session cookie copied from your browser's dev tools.

Usage:
    python scraper.py 100 --cookie "redump_cookie=BASE64VALUE"
    python scraper.py 132192 --cookie "redump_cookie=BASE64VALUE" --delay 2.0
"""

import argparse
import json
import os
import re
import signal
import sys
import time
from datetime import datetime, timezone

import requests
from bs4 import BeautifulSoup, Tag

BASE_URL = "http://redump.org"

# Stats tracked globally for signal handler access
stats = {"scraped": 0, "skipped_exists": 0, "skipped_inaccessible": 0, "failed": 0}
interrupted = False


def print_summary():
    print("\n--- Summary ---")
    print(f"  Scraped:      {stats['scraped']}")
    print(f"  Already done: {stats['skipped_exists']}")
    print(f"  Inaccessible: {stats['skipped_inaccessible']}")
    print(f"  Failed:       {stats['failed']}")


def signal_handler(_sig, _frame):
    global interrupted
    interrupted = True
    print("\nInterrupted by user.")
    print_summary()
    sys.exit(1)


signal.signal(signal.SIGINT, signal_handler)


# ---------------------------------------------------------------------------
# Edit page parsing
# ---------------------------------------------------------------------------

def parse_edit_page(html: str) -> dict | None:
    """Parse the disc edit form and extract all field values."""
    soup = BeautifulSoup(html, "lxml")

    form = soup.find("form", class_="form")
    if not form:
        return None

    data: dict = {}

    # Static text fields (System, Media, etc.)
    for tr in form.find_all("tr"):
        th = tr.find("th")
        td = tr.find("td", class_="static")
        if th and td:
            label = th.get_text(strip=True)
            value = td.get_text(strip=True)
            key = _label_to_key(label)
            data[key] = value

    # Text inputs
    for inp in form.find_all("input", attrs={"type": "text", "name": True}):
        name = inp["name"]
        data[name] = inp.get("value", "")

    # Hidden inputs
    for inp in form.find_all("input", attrs={"type": "hidden", "name": True}):
        name = inp["name"]
        data[name] = inp.get("value", "")

    # Textareas
    for ta in form.find_all("textarea", attrs={"name": True}):
        name = ta["name"]
        data[name] = ta.get_text()

    # Select dropdowns (single and multiple)
    for sel in form.find_all("select", attrs={"name": True}):
        name = sel["name"]
        selected = []
        for opt in sel.find_all("option", selected=True):
            selected.append({"value": opt.get("value", ""), "text": opt.get_text(strip=True)})
        if sel.get("multiple"):
            data[name] = selected
        else:
            data[name] = selected[0] if selected else None

    # Radio buttons (checked)
    radio_groups: dict[str, dict] = {}
    for inp in form.find_all("input", attrs={"type": "radio", "name": True}):
        name = inp["name"]
        if name not in radio_groups and inp.get("checked") is not None:
            label_el = form.find("label", attrs={"for": inp.get("id", "")})
            label_text = label_el.get_text(strip=True) if label_el else ""
            radio_groups[name] = {"value": inp.get("value", ""), "label": label_text}
    for name, val in radio_groups.items():
        data[name] = val

    # Checkboxes (checked)
    checkbox_groups: dict[str, list] = {}
    for inp in form.find_all("input", attrs={"type": "checkbox", "name": True}):
        name = inp["name"]
        if name not in checkbox_groups:
            checkbox_groups[name] = []
        if inp.get("checked") is not None:
            label_el = form.find("label", attrs={"for": inp.get("id", "")})
            label_text = label_el.get_text(strip=True) if label_el else ""
            checkbox_groups[name].append(
                {"value": inp.get("value", ""), "label": label_text}
            )
    for name, vals in checkbox_groups.items():
        data[name] = vals

    # Extract fieldset legends for context
    sections = []
    for fs in form.find_all("fieldset"):
        legend = fs.find("legend")
        if legend:
            sections.append(legend.get_text(strip=True))
    if sections:
        data["_sections"] = sections

    return _clean_edit_data(data)


# Regex patterns for fields to discard entirely
_DISCARD_PATTERNS = re.compile(
    r"^$"                        # empty key
    r"|^d_\w+_status$"           # d_*_status fields (but not d_status itself)
    r"|^d_ring_\d+_id$"          # d_ring_X_id
    r"|^d_ring_\d+_offsets$"     # d_ring_X_offsets
    r"|^d_ring_\d+_\d+_status$"  # d_ring_X_Y_status
    r"|^d_ring_count$"
    r"|^d_id$"
    r"|^d_is_regional_parent$"
    r"|^_sections$"
)

# Matches any d_ring_N or d_ring_N_* field
_RING_RE = re.compile(r"^d_ring_(\d+)(?:_(.+))?$")

# Fields where radio/checkbox should store only the label
_LABEL_ONLY = {"d_region", "d_edc", "d_protection_a", "d_protection_l"}
# Fields where radio should store only the value
_VALUE_ONLY_RADIO = {"d_status"}
# Checkbox fields where only the value string matters
_VALUE_ONLY_CHECKBOX = {"d_languages[]", "d_editions[]", "d_offset[]"}


def _clean_edit_data(data: dict) -> dict:
    """Post-process parsed edit data to normalize and slim down the output."""
    cleaned: dict = {}

    # Collect ring fields into a "rings" subnode
    rings: dict[int, dict] = {}

    for key, value in data.items():
        # Skip discarded fields
        if _DISCARD_PATTERNS.search(key):
            continue

        # Ring fields → group into rings subnode
        m = _RING_RE.match(key)
        if m:
            ring_idx = int(m.group(1))
            sub_key = m.group(2)  # everything after d_ring_N_
            if ring_idx not in rings:
                rings[ring_idx] = {}
            if sub_key is not None:
                rings[ring_idx][sub_key] = value
            else:
                rings[ring_idx]["ring"] = value
            continue

        # d_category → just text
        if key == "d_category" and isinstance(value, dict):
            cleaned[key] = value.get("text", value)
            continue

        # d_dumpers[] → list of text strings
        if key == "d_dumpers[]" and isinstance(value, list):
            cleaned[key] = [item["text"] if isinstance(item, dict) else item for item in value]
            continue

        # Radio fields: label-only or value-only
        if key in _LABEL_ONLY and isinstance(value, dict):
            cleaned[key] = value.get("label", value)
            continue
        if key in _VALUE_ONLY_RADIO and isinstance(value, dict):
            cleaned[key] = value.get("value", value)
            continue

        # Checkbox fields: value-only lists
        if key in _VALUE_ONLY_CHECKBOX and isinstance(value, list):
            cleaned[key] = [item["value"] if isinstance(item, dict) else item for item in value]
            continue

        cleaned[key] = value

    # Add rings, dropping the last one if all its values are empty/zero
    if rings:
        max_idx = max(rings)
        last_ring = rings[max_idx]
        all_empty = all(
            v == "" or v == "0" or v is None
            for v in (last_ring.values() if isinstance(last_ring, dict) else [last_ring])
        )
        if all_empty:
            del rings[max_idx]
        if rings:
            # Strip empty values and internal meta keys within each ring
            _ring_skip = {"id", "offsets"}
            cleaned["rings"] = [
                {k: v for k, v in rings[i].items()
                 if v != "" and k not in _ring_skip and not k.endswith("_id") and not k.endswith("_offsets")}
                for i in sorted(rings)
            ]

    return cleaned


def _label_to_key(label: str) -> str:
    """Convert a human-readable label to a snake_case key."""
    key = label.lower().strip()
    key = re.sub(r"[^a-z0-9]+", "_", key)
    return key.strip("_")


# ---------------------------------------------------------------------------
# Changes page parsing
# ---------------------------------------------------------------------------

def parse_changes_page(html: str) -> list[dict] | None:
    """Parse the disc changes page and extract structured change entries."""
    soup = BeautifulSoup(html, "lxml")

    h1 = soup.find("h1")
    if h1 and "No changes" in h1.get_text():
        return []

    changes_ul = soup.find("ul", class_="changes")
    if not changes_ul:
        return None

    entries = []
    for li in changes_ul.find_all("li", recursive=False):
        entry = _parse_change_entry(li)
        if entry:
            entries.append(entry)

    return entries


def _parse_change_entry(li: Tag) -> dict | None:
    """Parse a single <li> change entry from the changes list."""
    dl = li.find("dl")
    if not dl:
        return None

    entry: dict = {"date": None, "user": None, "fields": []}

    dts = dl.find_all("dt")
    dds = dl.find_all("dd")

    for dt, dd in zip(dts, dds):
        dt_text = dt.get_text(strip=True).rstrip(":")
        if dt_text == "Date":
            entry["date"] = dd.get_text(strip=True)
        elif dt_text == "User":
            entry["user"] = dd.get_text(strip=True)
        elif dt_text == "Changes":
            entry["fields"] = _parse_change_tables(dd)

    return entry


def _parse_change_tables(dd: Tag) -> list[dict]:
    """
    Parse the change content from a <dd> element.

    The content is one or more <table> elements (separated by <br/> from Rss::blankrow).
    Each table contains rows describing field changes with color-coded inline styles:
      - no color / black     → unchanged (just displayed for context)
      - color: #0000aa (blue)  → field was modified (new value)
      - color: #00aa00 (green) → field was added
      - color: #aa0000 (red)   → field was removed
      - color: #777777 (gray) on <tr> with "was" td → old value for previous field
    """
    fields = []
    tables = dd.find_all("table")

    def _td_text(td: Tag) -> str:
        """Extract text from a <td>, converting <br> tags to newlines."""
        for br in td.find_all("br"):
            br.replace_with("\n")
        return td.get_text().strip()

    for table_idx, table in enumerate(tables):
        rows = table.find_all("tr")
        i = 0
        while i < len(rows):
            row = rows[i]
            tds = row.find_all("td")
            if len(tds) < 2:
                i += 1
                continue

            first_td = tds[0]
            second_td = tds[1]

            # Check if this is a "was" row (old value) — skip, handled with previous field
            first_text = first_td.get_text(strip=True)
            if first_text == "was":
                i += 1
                continue

            # Extract field name from the bold tag
            bold = first_td.find("b")
            field_name = bold.get_text(strip=True) if bold else first_text
            new_value = _td_text(second_td)

            # Determine change type from inline style colors
            style = first_td.get("style", "")
            tr_style = row.get("style", "")
            change_type = _detect_change_type(style, tr_style)

            # Skip unchanged fields (just displayed for context)
            if change_type == "unchanged":
                i += 1
                continue

            old_value = None
            # Look ahead for a "was" row
            if i + 1 < len(rows):
                next_row = rows[i + 1]
                next_tds = next_row.find_all("td")
                if len(next_tds) >= 2:
                    next_first = next_tds[0].get_text(strip=True)
                    next_tr_style = next_row.get("style", "")
                    if next_first == "was" and "777777" in next_tr_style:
                        old_value = _td_text(next_tds[1])
                        i += 1  # skip the "was" row

            field_entry = {
                "field": field_name,
                "type": change_type,
                "new_value": new_value if change_type != "removed" else None,
            }

            # For removed fields, the "(removed)" text is in new_value position
            if change_type == "removed":
                field_entry["new_value"] = None

            if old_value is not None:
                field_entry["old_value"] = old_value

            fields.append(field_entry)
            i += 1

    return fields


def _detect_change_type(td_style: str, tr_style: str) -> str:
    """Detect the change type from inline CSS color styles."""
    if "777777" in tr_style:
        return "old_value"
    if "0000aa" in td_style:
        return "modified"
    if "00aa00" in td_style:
        return "added"
    if "aa0000" in td_style:
        return "removed"
    return "unchanged"


# ---------------------------------------------------------------------------
# Scraping logic
# ---------------------------------------------------------------------------

def create_session(cookie_str: str) -> requests.Session:
    """Create a requests session with the provided cookie header."""
    session = requests.Session()
    session.headers.update({
        "User-Agent": "vgindex-redump-scraper/1.0",
        "Cookie": cookie_str,
    })
    return session


def _strip_empty(d: dict) -> dict:
    """Remove keys with empty string values from a dict."""
    return {k: v for k, v in d.items() if v != ""}


def scrape_disc(session: requests.Session, disc_id: int) -> dict | None:
    """Scrape both edit and changes pages for a single disc ID."""
    result: dict = {
        "disc_id": disc_id,
        "scraped_at": datetime.now(timezone.utc).isoformat(),
    }

    # Fetch edit page
    edit_url = f"{BASE_URL}/disc/{disc_id}/edit/"
    try:
        resp = session.get(edit_url, timeout=30)
    except requests.RequestException as e:
        print(f"  [edit] Request failed: {e}")
        return None

    if resp.status_code in (403, 404) or resp.url.rstrip("/") != edit_url.rstrip("/"):
        # Redirected away or forbidden — disc is inaccessible
        return None

    if resp.status_code != 200:
        print(f"  [edit] HTTP {resp.status_code}")
        return None

    edit_data = parse_edit_page(resp.text)
    if edit_data is None:
        # Page loaded but no form found — might be an error page or redirect
        # Check if we got redirected to login or disc view
        if "login" in resp.url or "/disc/" in resp.text and "edit" not in resp.url:
            return None
        print(f"  [edit] No form found in response")
        return None

    result.update(_strip_empty(edit_data))

    # Fetch changes page
    changes_url = f"{BASE_URL}/disc/{disc_id}/changes/"
    try:
        resp = session.get(changes_url, timeout=30)
    except requests.RequestException as e:
        print(f"  [changes] Request failed: {e}")
        return result

    if resp.status_code == 200:
        changes_data = parse_changes_page(resp.text)
        if changes_data:
            result["changes"] = changes_data
    else:
        print(f"  [changes] HTTP {resp.status_code}")

    return result


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Scrape disc data from redump.org (edit pages and change history)."
    )
    parser.add_argument(
        "--start-id",
        type=int,
        default=1,
        help="First disc ID to scrape (default: 1).",
    )
    parser.add_argument(
        "--end-id",
        type=int,
        required=True,
        help="Last disc ID to scrape (range is start_id..end_id inclusive).",
    )
    parser.add_argument(
        "--cookie",
        required=True,
        help='Full Cookie header value from browser dev tools (e.g. "redump_cookie=BASE64VALUE").',
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=1.0,
        help="Delay in seconds between requests (default: 1.0).",
    )
    parser.add_argument(
        "--output-dir",
        default="data/redump/db",
        help="Output directory for JSON files (default: data/redump/db).",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Re-scrape even if JSON file already exists.",
    )

    args = parser.parse_args()

    if args.start_id < 1:
        parser.error("start_id must be >= 1")
    if args.end_id < args.start_id:
        parser.error("end_id must be >= start_id")
    if args.delay < 0:
        parser.error("delay must be >= 0")

    total = args.end_id - args.start_id + 1
    os.makedirs(args.output_dir, exist_ok=True)
    session = create_session(args.cookie)

    print(f"Scraping discs {args.start_id}..{args.end_id} ({total} entries)")
    print(f"Output: {os.path.abspath(args.output_dir)}")
    print(f"Delay: {args.delay}s")
    print()

    for disc_id in range(args.start_id, args.end_id + 1):
        if interrupted:
            break

        progress = disc_id - args.start_id + 1
        out_path = os.path.join(args.output_dir, f"{disc_id:06d}.json")

        # Skip if already scraped
        if not args.force and os.path.exists(out_path):
            stats["skipped_exists"] += 1
            continue

        print(f"[{progress}/{total}] Disc {disc_id}: ", end="", flush=True)

        result = scrape_disc(session, disc_id)

        if result is None:
            stats["skipped_inaccessible"] += 1
            print("skipped (inaccessible)")
        else:
            try:
                with open(out_path, "w", encoding="utf-8") as f:
                    json.dump(result, f, ensure_ascii=False, indent=2)
                title = result.get("d_title", "")
                print(f"OK{f' - {title}' if title else ''}")
                stats["scraped"] += 1
            except OSError as e:
                print(f"FAILED (write error: {e})")
                stats["failed"] += 1

        # Delay between discs (two requests per disc: edit + changes)
        if disc_id < args.end_id:
            time.sleep(args.delay)

    print_summary()


if __name__ == "__main__":
    main()
