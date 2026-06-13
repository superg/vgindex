#!/usr/bin/env python3
"""
Redump.org disc database scraper.

By default, the scraper loads configuration from scraper.cfg located in the
same directory as this script.

Usage:
    python scraper.py
    python scraper.py --config /path/to/scraper.cfg
    python scraper.py --check-modified 200
"""

import argparse
import configparser
import json
import os
import re
import signal
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urljoin

import requests
from bs4 import BeautifulSoup, NavigableString, Tag

BASE_URL = "http://redump.org"
DEFAULT_MODIFIED_LIST_URL = f"{BASE_URL}/discs/sort/modified/dir/desc/"
DEFAULT_OUTPUT_DIR = "data/redump/db"
CONFIG_SECTION = "scraper"
NONEXISTENT_DISC_RE = re.compile(r'with ID ".+" doesn\'t exist')


@dataclass
class ScraperConfig:
    config_path: Path
    last_known_disc_id: int
    cookie: str
    delay_seconds: float
    workers: int = 1
    output_dir: str = DEFAULT_OUTPUT_DIR

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
    print("\nInterrupted by user — exiting immediately.", flush=True)
    os._exit(130)


@contextmanager
def _no_interrupt():
    """Block SIGINT delivery for the duration of the with-block.

    SIGINT pressed while inside stays pending in the kernel and fires the
    moment the block exits — so file writes never see a partial flush.
    """
    old = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, old)


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

    for table in tables:
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
# Config and listing helpers
# ---------------------------------------------------------------------------

_DISC_ID_RE = re.compile(r"/disc/(\d+)/")


def default_config_path() -> Path:
    return Path(__file__).resolve().with_name("scraper.cfg")


def _read_config_section(parser: configparser.ConfigParser):
    if parser.has_section(CONFIG_SECTION):
        return parser[CONFIG_SECTION]
    return parser["DEFAULT"]


def load_config(config_path: Path) -> ScraperConfig:
    parser = configparser.ConfigParser(interpolation=None)
    if not parser.read(config_path):
        raise FileNotFoundError(f"Config file not found: {config_path}")

    section = _read_config_section(parser)

    try:
        last_known_disc_id = section.getint("last_known_disc_id")
    except (ValueError, configparser.Error) as exc:
        raise ValueError("Config key last_known_disc_id must be an integer") from exc
    cookie = section.get("cookie", "").strip()
    if not cookie:
        raise ValueError("Config key cookie is required")

    delay_raw = section.get("delay_seconds", "0.1")
    try:
        delay_seconds = float(delay_raw)
    except ValueError as exc:
        raise ValueError("Config key delay_seconds must be a float") from exc
    if delay_seconds < 0:
        raise ValueError("Config key delay_seconds must be >= 0")
    if last_known_disc_id < 1:
        raise ValueError("Config key last_known_disc_id must be >= 1")
    workers_raw = section.get("workers", "1")
    try:
        workers = int(workers_raw)
    except ValueError as exc:
        raise ValueError("Config key workers must be an integer") from exc
    if workers < 1:
        raise ValueError("Config key workers must be >= 1")

    output_dir = section.get("output_dir", DEFAULT_OUTPUT_DIR).strip() or DEFAULT_OUTPUT_DIR
    return ScraperConfig(
        config_path=config_path,
        last_known_disc_id=last_known_disc_id,
        cookie=cookie,
        delay_seconds=delay_seconds,
        workers=workers,
        output_dir=output_dir,
    )


def update_last_known_disc_id(config: ScraperConfig, new_last_known_disc_id: int) -> None:
    parser = configparser.ConfigParser(interpolation=None)
    parser.read(config.config_path)
    if not parser.has_section(CONFIG_SECTION):
        parser.add_section(CONFIG_SECTION)
    parser[CONFIG_SECTION]["last_known_disc_id"] = str(new_last_known_disc_id)
    if "cookie" not in parser[CONFIG_SECTION]:
        parser[CONFIG_SECTION]["cookie"] = config.cookie
    if "delay_seconds" not in parser[CONFIG_SECTION]:
        parser[CONFIG_SECTION]["delay_seconds"] = str(config.delay_seconds)
    if "workers" not in parser[CONFIG_SECTION]:
        parser[CONFIG_SECTION]["workers"] = str(config.workers)
    if "output_dir" not in parser[CONFIG_SECTION] and config.output_dir != DEFAULT_OUTPUT_DIR:
        parser[CONFIG_SECTION]["output_dir"] = config.output_dir
    with _no_interrupt():
        with open(config.config_path, "w", encoding="utf-8") as f:
            parser.write(f)


def _output_relpath(disc_id: int) -> str:
    return f"{disc_id:06d}.json"


def parse_modified_list_page(html: str, page_url: str) -> tuple[list[int], str | None]:
    soup = BeautifulSoup(html, "lxml")

    disc_ids = []
    seen = set()
    for a in soup.find_all("a", href=True):
        m = _DISC_ID_RE.search(a["href"])
        if not m:
            continue
        disc_id = int(m.group(1))
        if disc_id in seen:
            continue
        seen.add(disc_id)
        disc_ids.append(disc_id)

    next_url = None
    next_link = soup.find("a", rel=lambda v: isinstance(v, list) and "next" in v)
    if not next_link:
        next_link = soup.find("a", rel="next")
    if next_link and next_link.get("href"):
        next_url = urljoin(page_url, next_link["href"])
    else:
        for a in soup.find_all("a", href=True):
            text = a.get_text(" ", strip=True).lower()
            if text in {"next", "next »", ">", ">>", "older"}:
                next_url = urljoin(page_url, a["href"])
                break

    return disc_ids, next_url


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


def _is_complete_redump_html(html: str) -> bool:
    """Best-effort truncation guard for redump HTML pages."""
    lowered = html.lower()
    if "</html>" not in lowered or "</body>" not in lowered:
        return False
    # Footer marker is present on normal disc/edit/changes pages.
    return "redump 0.4" in lowered


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


def _find_row_value_cell(soup: BeautifulSoup, label: str) -> Tag | None:
    needle = label.lower().rstrip(":")
    for tr in soup.find_all("tr"):
        th = tr.find("th")
        if not th:
            continue
        key = th.get_text(" ", strip=True).lower().rstrip(":")
        if key == needle:
            return tr.find("td")
    return None


def _extract_dumpers_from_cell(td: Tag) -> list[str]:
    def _is_control_entry(text: str) -> bool:
        t = text.strip()
        return t == "[+]"

    # Root disc page may fold long dumper lists in UI, but hidden entries are still present in DOM.
    # Collect both linked and plain-text usernames in DOM order so output preserves redump ordering.
    dumpers = []
    seen = set()
    for node in td.descendants:
        name = ""
        if isinstance(node, Tag) and node.name == "a":
            href = node.get("href", "")
            if "/discs/dumper/" not in href:
                continue
            name = node.get_text(" ", strip=True)
        elif isinstance(node, NavigableString):
            if node.parent and getattr(node.parent, "name", "") == "a":
                continue
            name = str(node).strip(" \n\r\t,")
        else:
            continue
        if not name or _is_control_entry(name) or name in seen:
            continue
        seen.add(name)
        dumpers.append(name)
    return dumpers


def parse_disc_root_metadata(html: str) -> dict:
    soup = BeautifulSoup(html, "lxml")
    result = parse_disc_dates_page(html)
    dumpers_td = _find_row_value_cell(soup, "Dumpers")
    if dumpers_td is not None:
        result["d_dumpers[]"] = _extract_dumpers_from_cell(dumpers_td)
    return result


class _Interrupted(Exception):
    """Raised when an operation is interrupted during rate-limited wait."""


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
    session: requests.Session, disc_id: int, delay: float = 0.0, root_metadata: dict | None = None
) -> tuple[dict | str | None, list[str]]:
    """Scrape root, edit, and changes pages for a single disc ID."""
    warnings: list[str] = []
    result: dict = {
        "disc_id": disc_id,
        "scraped_at": datetime.now(timezone.utc).isoformat(),
    }

    if root_metadata is None:
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
        if NONEXISTENT_DISC_RE.search(resp.text):
            return "nonexistent", warnings
        if not _is_complete_redump_html(resp.text):
            warnings.append("[disc] Incomplete/truncated HTML")
            return None, warnings
        root_metadata = parse_disc_root_metadata(resp.text)

    if root_metadata:
        result.update(_strip_empty(root_metadata))

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

    if NONEXISTENT_DISC_RE.search(resp.text):
        return "nonexistent", warnings
    if not _is_complete_redump_html(resp.text):
        warnings.append("[edit] Incomplete/truncated HTML")
        return None, warnings

    edit_data = parse_edit_page(resp.text)
    if edit_data is None:
        if "login" in resp.url:
            return None, warnings
        warnings.append("[edit] No form found in response")
        return None, warnings
    if "d_title" not in edit_data:
        warnings.append("[edit] Missing required title field")
        return None, warnings

    result.update(_strip_empty(edit_data))
    if root_metadata is not None and "d_dumpers[]" in root_metadata:
        # Root page provides merged and correctly ordered dumpers.
        result["d_dumpers[]"] = root_metadata["d_dumpers[]"]
        result.pop("d_dumpers_text", None)

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
        if not _is_complete_redump_html(resp.text):
            warnings.append("[changes] Incomplete/truncated HTML")
            return None, warnings
        changes_data = parse_changes_page(resp.text)
        if changes_data is None:
            warnings.append("[changes] Could not parse changes list")
            return None, warnings
        if changes_data:
            result["changes"] = changes_data
    else:
        warnings.append(f"[changes] HTTP {resp.status_code}")
        return None, warnings

    return result, warnings


def _positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a positive integer") from exc
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Scrape disc data from redump.org using scraper.cfg settings."
    )
    parser.add_argument(
        "--config",
        help="Path to scraper config file. Defaults to scraper.cfg next to this script.",
    )
    parser.add_argument(
        "--check-modified",
        type=_positive_int,
        metavar="N",
        help="Delete local files for the N latest modified disc IDs, so backfill re-scrapes them.",
    )
    return parser


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = build_arg_parser()
    args = parser.parse_args()
    config_path = Path(args.config).expanduser().resolve() if args.config else default_config_path()

    try:
        config = load_config(config_path)
    except (FileNotFoundError, ValueError) as exc:
        parser.error(str(exc))

    os.makedirs(config.output_dir, exist_ok=True)
    session = create_session(config.cookie)

    print(f"Config: {config.config_path}")
    print(f"Output: {os.path.abspath(config.output_dir)}")
    print(f"Delay: {config.delay_seconds}s, Workers: {config.workers}")
    print()

    if args.check_modified is not None:
        run_modified_detection_phase(session, config, args.check_modified)
    else:
        print("Skipping modified-disc replacement (pass --check-modified N to enable).\n")
    if not interrupted:
        run_backfill_phase(session, config)
    if not interrupted:
        run_discovery_phase(session, config)

    print_summary()
    if interrupted:
        sys.exit(1)


def _has_nonexistent_marker(output_dir: str, disc_id: int) -> bool:
    path = os.path.join(output_dir, _output_relpath(disc_id))
    return os.path.isfile(path) and os.path.getsize(path) == 0


def _collect_missing_historical_ids(output_dir: str, last_known_disc_id: int) -> list[int]:
    missing = []
    for disc_id in range(1, last_known_disc_id + 1):
        path = os.path.join(output_dir, _output_relpath(disc_id))
        if os.path.isfile(path):
            with stats_lock:
                stats["skipped_exists"] += 1
            continue
        missing.append(disc_id)
    return missing


def run_backfill_phase(session: requests.Session, config: ScraperConfig) -> None:
    disc_ids = _collect_missing_historical_ids(config.output_dir, config.last_known_disc_id)
    if stats["skipped_exists"] > 0:
        print(f"Skipping {stats['skipped_exists']} already scraped")
    print(f"Backfill phase: {len(disc_ids)} missing disc(s) in 1..{config.last_known_disc_id}")
    if config.workers == 1:
        for idx, disc_id in enumerate(disc_ids, start=1):
            if interrupted:
                return
            result, warnings = scrape_disc(session, disc_id, delay=config.delay_seconds)
            _save_result(disc_id, idx, len(disc_ids), result, warnings, config.output_dir)
    else:
        thread_local = threading.local()

        def _get_worker_session() -> requests.Session:
            if not hasattr(thread_local, "session"):
                thread_local.session = create_session(config.cookie)
            return thread_local.session

        def worker_scrape(disc_id: int):
            if interrupted:
                return disc_id, None, []
            s = _get_worker_session()
            result, warnings = scrape_disc(s, disc_id, delay=config.delay_seconds)
            return disc_id, result, warnings

        completed = 0
        with ThreadPoolExecutor(max_workers=config.workers) as executor:
            futures = {executor.submit(worker_scrape, did): did for did in disc_ids}
            for future in as_completed(futures):
                if interrupted:
                    executor.shutdown(wait=False, cancel_futures=True)
                    return
                disc_id, result, warnings = future.result()
                completed += 1
                _save_result(disc_id, completed, len(disc_ids), result, warnings, config.output_dir)
    print()


def run_discovery_phase(session: requests.Session, config: ScraperConfig) -> None:
    print("Discovery phase: probing for new disc IDs...")
    disc_id = config.last_known_disc_id + 1
    scraped_new = 0
    new_last_known = config.last_known_disc_id

    while not interrupted:
        if _has_nonexistent_marker(config.output_dir, disc_id):
            # Nonexistent markers can become stale when new discs are added later.
            print(f"Discovery probe at disc {disc_id}: local nonexistent marker exists, re-checking remotely.")

        result, warnings = scrape_disc(session, disc_id, delay=config.delay_seconds)
        scraped_new += 1
        _save_result(disc_id, scraped_new, 0, result, warnings, config.output_dir)

        if result == "nonexistent":
            break
        if result is None:
            print("Discovery stopped due to request failure before reaching nonexistent disc.")
            return

        new_last_known = disc_id
        disc_id += 1

    if new_last_known != config.last_known_disc_id:
        update_last_known_disc_id(config, new_last_known)
        config.last_known_disc_id = new_last_known
        print(f"Updated last_known_disc_id in config to {new_last_known}")
    else:
        print("No new discs discovered beyond configured last_known_disc_id")
    print()


def iter_modified_disc_ids(session: requests.Session, delay: float):
    url = DEFAULT_MODIFIED_LIST_URL
    seen_pages = set()
    max_attempts = 5
    backoff_seconds = 1.0
    while url and not interrupted:
        if url in seen_pages:
            break
        seen_pages.add(url)
        resp = None
        for attempt in range(1, max_attempts + 1):
            if interrupted:
                return
            try:
                # Listing pages are heavier and can take longer to generate server-side.
                resp = _rate_limited_get(session, url, delay, timeout=(10, 300))
                resp.raise_for_status()
                break
            except requests.RequestException as exc:
                if attempt == max_attempts:
                    print(f"Failed to fetch modified listing page {url}: {exc}")
                    return
                wait_for = min(backoff_seconds * (2 ** (attempt - 1)), 30.0)
                print(
                    f"Modified listing fetch failed (attempt {attempt}/{max_attempts}) for {url}: {exc}; "
                    f"retrying in {wait_for:.1f}s..."
                )
                deadline = time.monotonic() + wait_for
                while time.monotonic() < deadline:
                    if interrupted:
                        return
                    time.sleep(min(0.25, deadline - time.monotonic()))
        disc_ids, next_url = parse_modified_list_page(resp.text, url)
        for disc_id in disc_ids:
            yield disc_id
        url = next_url


def _create_backup_dir(output_dir: str) -> str:
    base_name = datetime.now(timezone.utc).strftime("backup-%Y%m%d-%H%M%SZ")
    for idx in range(100):
        name = base_name if idx == 0 else f"{base_name}-{idx:02d}"
        path = os.path.join(output_dir, name)
        try:
            os.makedirs(path)
            return path
        except FileExistsError:
            continue
    raise OSError(f"Could not create unique backup directory under {output_dir}")


def run_modified_detection_phase(
    session: requests.Session, config: ScraperConfig, limit: int
) -> None:
    print(
        f"Modified replacement phase: collecting the {limit} latest modified disc IDs "
        "(no scraping yet)..."
    )
    to_delete: list[int] = []
    seen: set[int] = set()

    for disc_id in iter_modified_disc_ids(session, config.delay_seconds):
        if interrupted:
            return
        if disc_id in seen:
            continue
        seen.add(disc_id)
        to_delete.append(disc_id)
        if len(to_delete) >= limit:
            break

    if interrupted:
        return

    if len(to_delete) < limit:
        print(f"Modified listing ended after {len(to_delete)} unique disc ID(s).")
    print(
        f"Moving local files for {len(to_delete)} latest modified disc ID(s) "
        "so backfill repopulates them."
    )

    backup_dir: str | None = None
    for disc_id in to_delete:
        if interrupted:
            return
        path = os.path.join(config.output_dir, _output_relpath(disc_id))
        if not os.path.isfile(path):
            continue
        if backup_dir is None:
            backup_dir = _create_backup_dir(config.output_dir)
        try:
            os.rename(path, os.path.join(backup_dir, os.path.basename(path)))
        except OSError as exc:
            print(f"Failed to move {path} to {backup_dir}: {exc}")
    if backup_dir is not None:
        print(f"Backup directory: {backup_dir}")
    print()


def _save_result(disc_id: int, progress: int, total: int,
                 result: dict | str | None, warnings: list[str], output_dir: str):
    """Save scrape result to JSON and update stats. Output is atomic per disc."""
    lines = []
    out_path = os.path.join(output_dir, _output_relpath(disc_id))
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
            with _no_interrupt():
                with open(out_path, "w", encoding="utf-8") as f:
                    json.dump(result, f, ensure_ascii=False, indent=2)
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
