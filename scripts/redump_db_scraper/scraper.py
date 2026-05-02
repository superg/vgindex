#!/usr/bin/env python3
"""
Redump.org disc database scraper.

Scrapes edit page fields and change history for disc IDs from redump.org.
Requires a valid session cookie copied from your browser's dev tools.

Usage:
    python scraper.py --end-id 100 --cookie "redump_cookie=BASE64VALUE"
    python scraper.py --start-id 1 --end-id 132192 --cookie "redump_cookie=BASE64VALUE" --delay 2.0
    python scraper.py --update --cookie "redump_cookie=BASE64VALUE"
"""

import argparse
import json
import os
import re
import signal
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from email.utils import parsedate_to_datetime
from datetime import datetime, timezone

import requests
from bs4 import BeautifulSoup, Tag

BASE_URL = "http://redump.org"

# Stats tracked globally for signal handler access
stats = {"scraped": 0, "skipped_exists": 0, "skipped_inaccessible": 0, "failed": 0}
stats_lock = threading.Lock()
rate_lock = threading.Lock()
print_lock = threading.Lock()
interrupted = False


def print_summary():
    print("\n--- Summary ---")
    print(f"  Scraped:      {stats['scraped']}")
    print(f"  Already done: {stats['skipped_exists']}")
    print(f"  Inaccessible: {stats['skipped_inaccessible']}")
    print(f"  Failed:       {stats['failed']}")


def signal_handler(_sig, _frame):
    global interrupted
    if interrupted:
        return
    interrupted = True
    print("\nInterrupted by user — finishing in-flight requests…")


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
# Same groups sometimes use names without "[]" on the edit form
_CHECKBOX_NAME_TO_BRACKET = {
    "d_editions": "d_editions[]",
    "d_languages": "d_languages[]",
    "d_offset": "d_offset[]",
}


def _checkbox_raw_list_to_values(raw: list) -> list:
    """Turn checkbox group items into plain value strings."""
    return [item["value"] if isinstance(item, dict) else item for item in raw]


def _merge_bracket_checkbox(cleaned: dict, bracket_key: str, raw_list: list) -> None:
    """Merge parsed checkbox values into cleaned[bracket_key]; handles d_x and d_x[] both present."""
    new_vals = _checkbox_raw_list_to_values(raw_list)
    if bracket_key not in cleaned:
        cleaned[bracket_key] = new_vals
        return
    existing = cleaned[bracket_key]
    if not isinstance(existing, list):
        cleaned[bracket_key] = new_vals
        return
    seen = set()
    merged = []
    for x in existing + new_vals:
        if x not in seen:
            seen.add(x)
            merged.append(x)
    cleaned[bracket_key] = merged


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

        # Checkbox fields: value-only lists (may coexist with d_x sans [] — merge)
        if key in _VALUE_ONLY_CHECKBOX and isinstance(value, list):
            _merge_bracket_checkbox(cleaned, key, value)
            continue

        # d_editions / d_languages / d_offset without "[]" → same canonical key as d_x[]
        bracket_key = _CHECKBOX_NAME_TO_BRACKET.get(key)
        if bracket_key and isinstance(value, list):
            _merge_bracket_checkbox(cleaned, bracket_key, value)
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
# RSS update detection
# ---------------------------------------------------------------------------

RSS_URL = f"{BASE_URL}/feeds/recentchanges/rss"
_DISC_ID_RE = re.compile(r"/disc/(\d+)/")


def fetch_changed_ids_from_rss(
    session: requests.Session,
) -> list[tuple[int, datetime]]:
    """Fetch the recent-changes RSS feed and return (disc_id, pub_datetime) pairs.

    Deduplicates by disc ID, keeping the latest pubDate per disc.
    Returns a sorted list of (disc_id, pub_datetime) tuples.
    """
    resp = session.get(RSS_URL, timeout=30)
    resp.raise_for_status()

    soup = BeautifulSoup(resp.content, "xml")
    latest: dict[int, datetime] = {}

    for item in soup.find_all("item"):
        link_tag = item.find("link")
        pub_tag = item.find("pubDate")
        if not link_tag or not pub_tag:
            continue

        link_text = link_tag.get_text(strip=True)
        m = _DISC_ID_RE.search(link_text)
        if not m:
            continue

        disc_id = int(m.group(1))
        pub_dt = parsedate_to_datetime(pub_tag.get_text(strip=True))

        if disc_id not in latest or pub_dt > latest[disc_id]:
            latest[disc_id] = pub_dt

    return sorted(latest.items())


def _output_relpath(disc_id: int, dates_only: bool = False) -> str:
    """Return output filename relative to output_dir for a disc ID."""
    if dates_only:
        return os.path.join("date", f"{disc_id:06d}.date.json")
    return f"{disc_id:06d}.json"


def filter_stale_ids(
    rss_entries: list[tuple[int, datetime]],
    output_dir: str,
    dates_only: bool = False,
) -> list[int]:
    """Return disc IDs whose local JSON is missing or older than the RSS pubDate."""
    stale = []
    for disc_id, pub_dt in rss_entries:
        path = os.path.join(output_dir, _output_relpath(disc_id, dates_only=dates_only))
        if not os.path.isfile(path) or os.path.getsize(path) == 0:
            stale.append(disc_id)
            continue
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            if dates_only:
                if not isinstance(data, dict):
                    stale.append(disc_id)
                    continue
                pub_dt_utc = pub_dt.astimezone(timezone.utc) if pub_dt.tzinfo else pub_dt.replace(tzinfo=timezone.utc)
                file_mtime = datetime.fromtimestamp(os.path.getmtime(path), tz=timezone.utc)
                if file_mtime < pub_dt_utc:
                    stale.append(disc_id)
            else:
                scraped_at = datetime.fromisoformat(data["scraped_at"])
                if scraped_at < pub_dt:
                    stale.append(disc_id)
        except (json.JSONDecodeError, KeyError, ValueError):
            stale.append(disc_id)
    return stale


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


def parse_disc_dates_page(html: str) -> dict:
    """Parse disc page and return any found Added/Last modified fields."""
    soup = BeautifulSoup(html, "lxml")
    result: dict = {}

    # Common case on tabular pages.
    for tr in soup.find_all("tr"):
        th = tr.find("th")
        td = tr.find("td")
        if not th or not td:
            continue
        label = th.get_text(" ", strip=True).lower().rstrip(":")
        value = td.get_text(" ", strip=True)
        if not value:
            continue
        if label == "added":
            result["added"] = value
        elif label == "last modified":
            result["modified"] = value

    # Alternate metadata layouts use <dt>/<dd>.
    if "added" not in result or "modified" not in result:
        dts = soup.find_all("dt")
        dds = soup.find_all("dd")
        for dt, dd in zip(dts, dds):
            label = dt.get_text(" ", strip=True).lower().rstrip(":")
            value = dd.get_text(" ", strip=True)
            if not value:
                continue
            if label == "added" and "added" not in result:
                result["added"] = value
            elif label == "last modified" and "modified" not in result:
                result["modified"] = value

    # Fallback: plain text lines with labels and values.
    if "added" not in result or "modified" not in result:
        text = soup.get_text("\n", strip=True)
        if "added" not in result:
            m = re.search(r"(?im)^Added\s*:?\s*(.+)$", text)
            if m and m.group(1).strip():
                result["added"] = m.group(1).strip()
        if "modified" not in result:
            m = re.search(r"(?im)^Last modified\s*:?\s*(.+)$", text)
            if m and m.group(1).strip():
                result["modified"] = m.group(1).strip()

    return result


class _Interrupted(Exception):
    """Raised when a worker detects the interrupted flag during a wait."""


def _rate_limited_get(
    session: requests.Session, url: str, delay: float, **kwargs
) -> requests.Response:
    """Perform a GET with global rate limiting so all threads share one throttle."""
    with rate_lock:
        if interrupted:
            raise _Interrupted
        now = time.monotonic()
        wait = delay - (now - _rate_limited_get.last_request_time)
        if wait > 0:
            deadline = now + wait
            while time.monotonic() < deadline:
                if interrupted:
                    raise _Interrupted
                time.sleep(min(0.25, deadline - time.monotonic()))
        _rate_limited_get.last_request_time = time.monotonic()
    return session.get(url, **kwargs)

_rate_limited_get.last_request_time = 0.0


def scrape_disc(
    session: requests.Session, disc_id: int, delay: float = 0.0,
) -> tuple[dict | str | None, list[str]]:
    """Scrape both edit and changes pages for a single disc ID.
    Returns (result_dict_or_None, list_of_warnings)."""
    warnings: list[str] = []
    result: dict = {
        "disc_id": disc_id,
        "scraped_at": datetime.now(timezone.utc).isoformat(),
    }

    # Fetch edit page
    edit_url = f"{BASE_URL}/disc/{disc_id}/edit/"
    try:
        resp = _rate_limited_get(session, edit_url, delay, timeout=30)
    except _Interrupted:
        return None, warnings
    except requests.RequestException as e:
        warnings.append(f"[edit] Request failed: {e}")
        return None, warnings

    if resp.status_code in (403, 404):
        return None, warnings
    if resp.url.rstrip("/") != edit_url.rstrip("/"):
        warnings.append(f"[edit] Redirected to {resp.url}")
        return None, warnings

    if resp.status_code != 200:
        warnings.append(f"[edit] HTTP {resp.status_code}")
        return None, warnings

    if re.search(r'with ID ".+" doesn\'t exist', resp.text):
        return "nonexistent", warnings

    edit_data = parse_edit_page(resp.text)
    if edit_data is None:
        if "login" in resp.url:
            return None, warnings
        warnings.append("[edit] No form found in response")
        return None, warnings

    result.update(_strip_empty(edit_data))

    # Fetch changes page
    changes_url = f"{BASE_URL}/disc/{disc_id}/changes/"
    try:
        resp = _rate_limited_get(session, changes_url, delay, timeout=30)
    except _Interrupted:
        return None, warnings
    except requests.RequestException as e:
        warnings.append(f"[changes] Request failed: {e}")
        return None, warnings

    if resp.status_code == 200:
        changes_data = parse_changes_page(resp.text)
        if changes_data:
            result["changes"] = changes_data
    else:
        warnings.append(f"[changes] HTTP {resp.status_code}")
        return None, warnings

    return result, warnings


def scrape_disc_dates(
    session: requests.Session, disc_id: int, delay: float = 0.0,
) -> tuple[dict | str | None, list[str]]:
    """Scrape only Added/Last modified metadata for a disc ID."""
    warnings: list[str] = []
    disc_url = f"{BASE_URL}/disc/{disc_id}/"
    try:
        resp = _rate_limited_get(session, disc_url, delay, timeout=30)
    except _Interrupted:
        return None, warnings
    except requests.RequestException as e:
        warnings.append(f"[disc] Request failed: {e}")
        return None, warnings

    if resp.status_code in (403, 404):
        return None, warnings
    if resp.url.rstrip("/") != disc_url.rstrip("/"):
        warnings.append(f"[disc] Redirected to {resp.url}")
        return None, warnings
    if resp.status_code != 200:
        warnings.append(f"[disc] HTTP {resp.status_code}")
        return None, warnings

    if re.search(r'with ID ".+" doesn\'t exist', resp.text):
        return "nonexistent", warnings

    return parse_disc_dates_page(resp.text), warnings


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
        default=None,
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
        "--workers",
        type=int,
        default=1,
        help="Number of parallel workers (default: 1).",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Re-scrape even if JSON file already exists.",
    )
    parser.add_argument(
        "--update",
        action="store_true",
        help="Fetch RSS recent-changes feed and re-scrape only stale entries. Overrides --start-id / --end-id.",
    )
    parser.add_argument(
        "--dates-only",
        action="store_true",
        help="Scrape only Added/Last modified into date/{disc_id}.date.json files.",
    )

    args = parser.parse_args()

    if not args.update and args.end_id is None:
        parser.error("--end-id is required unless --update is used")

    if args.delay < 0:
        parser.error("delay must be >= 0")
    if args.workers < 1:
        parser.error("workers must be >= 1")
    if not args.update:
        if args.start_id < 1:
            parser.error("start_id must be >= 1")
        if args.end_id < args.start_id:
            parser.error("end_id must be >= start_id")

    os.makedirs(args.output_dir, exist_ok=True)
    if args.dates_only:
        os.makedirs(os.path.join(args.output_dir, "date"), exist_ok=True)
    session = create_session(args.cookie)

    # Per-thread sessions for thread safety (requests.Session is not thread-safe)
    _thread_local = threading.local()

    def _get_session() -> requests.Session:
        if not hasattr(_thread_local, "session"):
            _thread_local.session = create_session(args.cookie)
        return _thread_local.session

    def worker_scrape(disc_id: int) -> tuple[int, dict | str | None, list[str]]:
        if interrupted:
            return disc_id, None, []
        s = session if args.workers == 1 else _get_session()
        if args.dates_only:
            result, warnings = scrape_disc_dates(s, disc_id, delay=args.delay)
        else:
            result, warnings = scrape_disc(s, disc_id, delay=args.delay)
        return disc_id, result, warnings

    # Build list of disc IDs to process
    if args.update:
        print("Fetching recent-changes RSS feed...")
        rss_entries = fetch_changed_ids_from_rss(session)
        print(f"RSS feed contains {len(rss_entries)} unique disc(s)")
        disc_ids = filter_stale_ids(rss_entries, args.output_dir, dates_only=args.dates_only)
        print(f"{len(disc_ids)} disc(s) need updating")
    elif args.force:
        disc_ids = list(range(args.start_id, args.end_id + 1))
    else:
        disc_ids = []
        if args.dates_only:
            existing_dir = os.path.join(args.output_dir, "date")
            existing = set(os.listdir(existing_dir))
        else:
            existing = set(os.listdir(args.output_dir))
        for disc_id in range(args.start_id, args.end_id + 1):
            relpath = _output_relpath(disc_id, dates_only=args.dates_only)
            filename = os.path.basename(relpath)
            if filename in existing:
                stats["skipped_exists"] += 1
            else:
                disc_ids.append(disc_id)

    total = len(disc_ids)

    if not args.update:
        if stats["skipped_exists"] > 0:
            print(f"Skipping {stats['skipped_exists']} already scraped (use --force to re-scrape)")
            print()
        print(f"Scraping discs {args.start_id}..{args.end_id} ({total} entries)")

    print(f"Output: {os.path.abspath(args.output_dir)}")
    print(f"Delay: {args.delay}s, Workers: {args.workers}")
    print()

    completed = 0

    if args.workers == 1:
        for disc_id in disc_ids:
            if interrupted:
                break
            completed += 1
            _, result, warnings = worker_scrape(disc_id)
            _save_result(
                disc_id,
                completed,
                total,
                result,
                warnings,
                args.output_dir,
                dates_only=args.dates_only,
            )
    else:
        with ThreadPoolExecutor(max_workers=args.workers) as executor:
            futures = {executor.submit(worker_scrape, did): did for did in disc_ids}
            for future in as_completed(futures):
                if interrupted:
                    executor.shutdown(wait=False, cancel_futures=True)
                    break
                disc_id, result, warnings = future.result()
                completed += 1
                _save_result(
                    disc_id,
                    completed,
                    total,
                    result,
                    warnings,
                    args.output_dir,
                    dates_only=args.dates_only,
                )

    print_summary()
    if interrupted:
        sys.exit(1)


def _save_result(disc_id: int, progress: int, total: int,
                 result: dict | str | None, warnings: list[str], output_dir: str,
                 dates_only: bool = False):
    """Save scrape result to JSON and update stats. Output is atomic per disc."""
    lines = []
    out_path = os.path.join(output_dir, _output_relpath(disc_id, dates_only=dates_only))
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    if result == "nonexistent":
        status = "skipped (doesn't exist)"
        try:
            open(out_path, "w").close()  # create empty file
        except OSError:
            pass
        with stats_lock:
            stats["skipped_inaccessible"] += 1
    elif result is None:
        status = "FAILED"
        with stats_lock:
            stats["failed"] += 1
    else:
        try:
            with open(out_path, "w", encoding="utf-8") as f:
                json.dump(result, f, ensure_ascii=False, indent=2)
            if dates_only:
                status = "OK"
            else:
                title = result.get("d_title", "")
                status = f"OK{f' - {title}' if title else ''}"
            with stats_lock:
                stats["scraped"] += 1
        except OSError as e:
            status = f"FAILED (write error: {e})"
            with stats_lock:
                stats["failed"] += 1

    pct = round(100 * progress / total) if total > 0 else 100
    lines.append(f"[{pct:3d}%] Disc {disc_id}: {status}")
    for w in warnings:
        lines.append(f"  ^ {w}")

    with print_lock:
        print("\n".join(lines), flush=True)


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal_handler)
    main()
