#!/usr/bin/env python3
"""
Redump.org forum scraper.

By default, the scraper loads configuration from scraper.cfg located in the
same directory as this script.

Usage:
    python scraper.py
    python scraper.py --config /path/to/scraper.cfg
    python scraper.py --metadata-only
"""

import argparse
import configparser
import copy
import hashlib
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
from email.message import Message
from pathlib import Path
from urllib.parse import parse_qs, urljoin, urlparse

import requests
from bs4 import BeautifulSoup, Tag

DEFAULT_BASE_URL = "http://forum.redump.org"
DEFAULT_OUTPUT_DIR = "data/redump/forum"
CONFIG_SECTION = "scraper"

TOPIC_ID_RE = re.compile(r"/topic/(\d+)(?:/|$)|[?&]id=(\d+)(?:&|$)")
FORUM_ID_RE = re.compile(r"/forum/(\d+)(?:/|$)|[?&]id=(\d+)(?:&|$)")
POST_ID_RE = re.compile(r"^p(\d+)$")
ITEMS_TOTAL_RE = re.compile(r"\bof\s+([\d,]+)")
ITEMS_SHORT_TOTAL_RE = re.compile(r"^[^:]+:\s*([\d,]+)$")
COUNT_RE = re.compile(r"([\d,]+)")
TOPIC_INDEX_SAVE_EVERY_PAGES = 25

GENERIC_ATTACHMENT_TEXT = {
    "",
    "attachment",
    "download",
    "download attachment",
    "view",
    "view attachment",
}

interrupted = False
rate_lock = threading.Lock()
print_lock = threading.Lock()
stats_lock = threading.Lock()
stats = {
    "scraped": 0,
    "skipped_exists": 0,
    "missing": 0,
    "moved": 0,
    "failed": 0,
    "forum_pages_failed": 0,
    "attachments": 0,
    "attachment_failed": 0,
}


@dataclass(frozen=True)
class ScraperConfig:
    config_path: Path
    base_url: str
    cookie: str
    max_known_topic_id: int
    missing_stop_after: int
    delay_seconds: float
    workers: int
    output_dir: str


@dataclass(frozen=True)
class ForumInfo:
    forum_id: int
    forum_name: str
    category_name: str
    source_url: str


@dataclass(frozen=True)
class TopicSummary:
    topic_id: int
    category_name: str
    forum_id: int
    forum_name: str
    subject: str
    source_url: str
    flags: dict[str, bool]
    moved_to_topic_id: int | None = None
    view_count: int | None = None


class ScrapeError(Exception):
    """Raised when a page cannot be fetched or parsed safely."""


class Interrupted(Exception):
    """Raised when work should stop after SIGINT."""


def signal_handler(_sig, _frame):
    global interrupted
    if interrupted:
        os._exit(130)
    interrupted = True
    print("\nInterrupted - finishing in-flight file writes and stopping...", flush=True)


@contextmanager
def _no_interrupt():
    old = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, old)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------


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

    base_url = section.get("base_url", DEFAULT_BASE_URL).strip().rstrip("/")
    if not base_url:
        raise ValueError("Config key base_url is required")

    cookie = section.get("cookie", "").strip()
    if not cookie:
        raise ValueError("Config key cookie is required")

    max_known_topic_id_raw = section.get("max_known_topic_id", "0")
    try:
        max_known_topic_id = int(max_known_topic_id_raw)
    except ValueError as exc:
        raise ValueError("Config key max_known_topic_id must be an integer") from exc
    if max_known_topic_id < 0:
        raise ValueError("Config key max_known_topic_id must be >= 0")

    missing_stop_after_raw = section.get("missing_stop_after", "200")
    try:
        missing_stop_after = int(missing_stop_after_raw)
    except ValueError as exc:
        raise ValueError("Config key missing_stop_after must be an integer") from exc
    if missing_stop_after < 1:
        raise ValueError("Config key missing_stop_after must be >= 1")

    delay_raw = section.get("delay_seconds", "0.1")
    try:
        delay_seconds = float(delay_raw)
    except ValueError as exc:
        raise ValueError("Config key delay_seconds must be a float") from exc
    if delay_seconds < 0:
        raise ValueError("Config key delay_seconds must be >= 0")

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
        base_url=base_url,
        cookie=cookie,
        max_known_topic_id=max_known_topic_id,
        missing_stop_after=missing_stop_after,
        delay_seconds=delay_seconds,
        workers=workers,
        output_dir=output_dir,
    )


def update_max_known_topic_id(config: ScraperConfig, new_max_known_topic_id: int) -> None:
    parser = configparser.ConfigParser(interpolation=None)
    parser.read(config.config_path)
    section = parser[CONFIG_SECTION] if parser.has_section(CONFIG_SECTION) else parser["DEFAULT"]
    section["max_known_topic_id"] = str(new_max_known_topic_id)
    with _no_interrupt():
        with open(config.config_path, "w", encoding="utf-8") as f:
            parser.write(f)


# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------


def create_session(cookie_header: str) -> requests.Session:
    session = requests.Session()
    session.headers.update({
        "User-Agent": "vgindex-redump-forum-scraper/1.0",
        "Cookie": cookie_header,
    })
    return session


def _rate_limited_get(
    session: requests.Session,
    url: str,
    delay: float,
    **kwargs,
) -> requests.Response:
    with rate_lock:
        if interrupted:
            raise Interrupted
        now = time.monotonic()
        wait = delay - (now - _rate_limited_get.last_request_time)
        if wait > 0:
            deadline = now + wait
            while time.monotonic() < deadline:
                if interrupted:
                    raise Interrupted
                time.sleep(min(0.25, deadline - time.monotonic()))
        _rate_limited_get.last_request_time = time.monotonic()
    return session.get(url, **kwargs)


_rate_limited_get.last_request_time = 0.0


def fetch_html(session: requests.Session, url: str, delay: float, *, max_attempts: int = 3) -> str:
    backoff = 1.0
    for attempt in range(1, max_attempts + 1):
        if interrupted:
            raise Interrupted
        try:
            resp = _rate_limited_get(session, url, delay, timeout=30)
            resp.raise_for_status()
        except Interrupted:
            raise
        except requests.RequestException as exc:
            if attempt == max_attempts:
                raise ScrapeError(f"Request failed for {url}: {exc}") from exc
            _sleep_with_interrupt(min(backoff * (2 ** (attempt - 1)), 30.0))
            continue

        if "</html>" not in resp.text.lower():
            if attempt == max_attempts:
                raise ScrapeError(f"Incomplete HTML response for {url}")
            _sleep_with_interrupt(min(backoff * (2 ** (attempt - 1)), 30.0))
            continue

        return resp.text

    raise ScrapeError(f"Could not fetch {url}")


def _sleep_with_interrupt(seconds: float) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if interrupted:
            raise Interrupted
        time.sleep(min(0.25, deadline - time.monotonic()))


def validate_auth(index_html: str) -> str:
    soup = BeautifulSoup(index_html, "lxml")
    welcome = soup.find("p", id="welcome")
    if not welcome:
        raise ScrapeError("Could not find forum login status")

    text = welcome.get_text(" ", strip=True)
    if "Logged in as" not in text:
        raise ScrapeError("Not authenticated - check the forum cookie in scraper.cfg")

    username = welcome.find("strong")
    return username.get_text(" ", strip=True) if username else "?"


# ---------------------------------------------------------------------------
# URL and text helpers
# ---------------------------------------------------------------------------


def absolute_url(page_url: str, href: str | None) -> str:
    return urljoin(page_url, href or "")


def parse_topic_id(url: str | None) -> int | None:
    if not url:
        return None
    m = TOPIC_ID_RE.search(url)
    if not m:
        return None
    value = m.group(1) or m.group(2)
    return int(value) if value else None


def parse_forum_id(url: str | None) -> int | None:
    if not url:
        return None
    m = FORUM_ID_RE.search(url)
    if not m:
        return None
    value = m.group(1) or m.group(2)
    return int(value) if value else None


def inner_html(tag: Tag) -> str:
    return tag.decode_contents().strip()


def clean_text(tag: Tag | None) -> str:
    return tag.get_text(" ", strip=True) if tag else ""


def parse_count_text(value: str) -> int | None:
    m = COUNT_RE.search(value)
    if not m:
        return None
    return int(m.group(1).replace(",", ""))


def safe_filename(name: str) -> str:
    name = name.strip().replace("\\", "_").replace("/", "_")
    name = re.sub(r"[\x00-\x1f\x7f]+", "", name)
    name = re.sub(r"\s+", " ", name)
    name = name.strip(" .")
    return name or "attachment"


def local_topic_path(output_dir: str, topic_id: int) -> str:
    return os.path.join(output_dir, "topics", f"{topic_id:06d}.json")


def topic_index_path(output_dir: str) -> str:
    return os.path.join(output_dir, "topic_index.json")


def direct_topic_url(base_url: str, topic_id: int) -> str:
    return f"{base_url}/viewtopic.php?id={topic_id}"


def has_topic_output(output_dir: str, topic_id: int) -> bool:
    return os.path.exists(local_topic_path(output_dir, topic_id))


def is_missing_topic_marker(output_dir: str, topic_id: int) -> bool:
    path = local_topic_path(output_dir, topic_id)
    return os.path.exists(path) and os.path.getsize(path) == 0


def local_topic_id(path: Path) -> int | None:
    try:
        return int(path.stem)
    except ValueError:
        return None


def highest_completed_topic_id(output_dir: str) -> int:
    topics_dir = Path(output_dir) / "topics"
    if not topics_dir.exists():
        return 0

    highest = 0
    for path in topics_dir.glob("*.json"):
        topic_id = local_topic_id(path)
        if topic_id is None:
            continue
        try:
            if path.stat().st_size == 0:
                continue
        except OSError:
            continue
        highest = max(highest, topic_id)
    return highest


def write_missing_topic_marker(output_dir: str, topic_id: int) -> None:
    out_path = local_topic_path(output_dir, topic_id)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with _no_interrupt():
        open(out_path, "a", encoding="utf-8").close()


# ---------------------------------------------------------------------------
# Index and forum parsing
# ---------------------------------------------------------------------------


def parse_index_page(html: str, page_url: str) -> list[ForumInfo]:
    soup = BeautifulSoup(html, "lxml")
    forums: list[ForumInfo] = []
    current_category = ""

    for node in soup.select("div.main-head, div.main-content.main-category"):
        classes = set(node.get("class", []))
        if "main-head" in classes:
            heading = node.find(["h1", "h2", "h3"])
            if heading:
                current_category = heading.get_text(" ", strip=True)
            continue

        for item in node.select("div.main-item[id^='forum']"):
            if "redirect" in set(item.get("class", [])):
                continue

            item_id = item.get("id", "")
            m = re.match(r"forum(\d+)$", item_id)
            forum_id = int(m.group(1)) if m else None

            link = item.select_one(".item-subject h3 a[href]")
            if not link:
                continue
            href = link.get("href", "")
            forum_id = forum_id or parse_forum_id(href)
            if forum_id is None:
                continue

            forums.append(ForumInfo(
                forum_id=forum_id,
                forum_name=link.get_text(" ", strip=True),
                category_name=current_category,
                source_url=absolute_url(page_url, href),
            ))

    return forums


def parse_forum_page(html: str, page_url: str, forum: ForumInfo) -> tuple[list[TopicSummary], str | None]:
    soup = BeautifulSoup(html, "lxml")
    topics: list[TopicSummary] = []

    for item in soup.select("div.main-item[id^='topic']"):
        item_id = item.get("id", "")
        m = re.match(r"topic(\d+)$", item_id)
        if not m:
            continue

        topic_id = int(m.group(1))
        classes = set(item.get("class", []))
        link = item.select_one(".item-subject h3 a[href]")
        if not link:
            continue

        href = link.get("href", "")
        source_url = absolute_url(page_url, href)
        target_topic_id = parse_topic_id(href)
        moved_to_topic_id = None
        if "moved" in classes and target_topic_id and target_topic_id != topic_id:
            moved_to_topic_id = target_topic_id

        flags = {
            "closed": "closed" in classes,
            "sticky": "sticky" in classes,
            "moved": "moved" in classes,
        }

        topics.append(TopicSummary(
            topic_id=topic_id,
            category_name=forum.category_name,
            forum_id=forum.forum_id,
            forum_name=forum.forum_name,
            subject=link.get_text(" ", strip=True),
            source_url=source_url,
            flags=flags,
            moved_to_topic_id=moved_to_topic_id,
            view_count=parse_topic_views(item),
        ))

    return topics, parse_next_url(soup, page_url)


def parse_topic_views(item: Tag) -> int | None:
    for selector in (
        ".item-info .info-views strong",
        ".item-info .info-views",
        "li.info-views strong",
        "li.info-views",
    ):
        node = item.select_one(selector)
        if not node:
            continue
        count = parse_count_text(node.get_text(" ", strip=True))
        if count is not None:
            return count

    for li in item.select(".item-info li"):
        classes = " ".join(li.get("class", [])).lower()
        if "view" not in classes:
            continue
        count = parse_count_text(li.get_text(" ", strip=True))
        if count is not None:
            return count

    return None


def parse_next_url(soup: BeautifulSoup, page_url: str) -> str | None:
    next_link = soup.find("link", rel=lambda value: _rel_contains(value, "next"))
    if next_link and next_link.get("href"):
        return absolute_url(page_url, next_link["href"])

    paging = soup.find("p", class_="paging")
    if paging:
        for link in paging.find_all("a", href=True):
            if link.get_text(" ", strip=True).lower() == "next":
                return absolute_url(page_url, link["href"])

    return None


def _rel_contains(value, needle: str) -> bool:
    if isinstance(value, list):
        return needle in value
    if isinstance(value, str):
        return needle in value.split()
    return False


def parse_items_total(soup: BeautifulSoup) -> int | None:
    h2 = soup.find("h2", class_="hn")
    if not h2:
        return None
    text = h2.get_text(" ", strip=True)
    m = ITEMS_TOTAL_RE.search(text) or ITEMS_SHORT_TOTAL_RE.search(text)
    if not m:
        return None
    return int(m.group(1).replace(",", ""))


def is_missing_topic_page(soup: BeautifulSoup) -> bool:
    if soup.select_one("div.main-content.main-topic"):
        return False
    text = soup.get_text("\n", strip=True)
    missing_markers = (
        "Bad request. The link you followed is incorrect or outdated.",
        "You do not have permission to view these forums.",
        "You do not have permission to access this page.",
    )
    return any(marker in text for marker in missing_markers)


def parse_topic_metadata(
    html: str,
    page_url: str,
    topic_id: int,
    forums_by_id: dict[int, ForumInfo],
) -> dict | None:
    soup = BeautifulSoup(html, "lxml")
    if is_missing_topic_page(soup):
        return None

    main_topic = soup.select_one("div.main-content.main-topic[id^='forum']")
    if not main_topic:
        return None

    forum_id = None
    m = re.match(r"forum(\d+)$", main_topic.get("id", ""))
    if m:
        forum_id = int(m.group(1))

    forum_name = ""
    category_name = ""
    if forum_id is not None and forum_id in forums_by_id:
        forum_name = forums_by_id[forum_id].forum_name
        category_name = forums_by_id[forum_id].category_name

    crumbs = soup.select("#brd-crumbs-top .crumbs p a[href]")
    for link in crumbs:
        href = link.get("href", "")
        crumb_forum_id = parse_forum_id(href)
        if crumb_forum_id is None:
            continue
        forum_id = forum_id or crumb_forum_id
        if not forum_name:
            forum_name = link.get_text(" ", strip=True)
        if forum_id in forums_by_id and not category_name:
            category_name = forums_by_id[forum_id].category_name
        break

    title_link = soup.select_one("h1.main-title a.permalink")
    subject = title_link.get_text(" ", strip=True) if title_link else ""
    if not subject:
        title = soup.select_one("title")
        subject = title.get_text(" ", strip=True).split(" - ")[0] if title else ""

    main_title_text = clean_text(soup.select_one("h1.main-title"))
    flags = {
        "closed": "[ Closed ]" in main_title_text,
    }

    return {
        "topic_id": topic_id,
        "category_name": category_name,
        "forum_id": forum_id,
        "forum_name": forum_name,
        "subject": subject,
        "flags": {k: v for k, v in flags.items() if v},
        "source_url": page_url,
    }


# ---------------------------------------------------------------------------
# Topic and post parsing
# ---------------------------------------------------------------------------


def parse_topic_page(html: str, page_url: str) -> tuple[list[dict], list[dict], str | None, int | None]:
    soup = BeautifulSoup(html, "lxml")
    posts: list[dict] = []
    attachments: list[dict] = []
    expected_total = parse_items_total(soup)

    for posthead in soup.select("div.posthead[id^='p']"):
        post_id_match = POST_ID_RE.match(posthead.get("id", ""))
        if not post_id_match:
            continue
        post_id = int(post_id_match.group(1))
        post_container = posthead.parent if isinstance(posthead.parent, Tag) else posthead

        post_attachments = parse_attachments(post_container, page_url, post_id)
        attachments.extend(post_attachments)

        post = {
            "post_id": post_id,
            "author_name": parse_author_name(post_container),
            "posted_at": parse_posted_at(post_container),
            "message_html": parse_message_html(post_container),
        }

        edited = parse_edited(post_container)
        post.update(edited)

        if post_attachments:
            post["attachment_ids"] = [a["attachment_id"] for a in post_attachments]

        posts.append({k: v for k, v in post.items() if v not in ("", None, [])})

    return posts, attachments, parse_next_url(soup, page_url), expected_total


def parse_author_name(post_container: Tag) -> str:
    author = post_container.select_one(".post-author .author-ident .username")
    if author:
        return author.get_text(" ", strip=True)

    byline = post_container.select_one(".post-byline")
    if byline:
        link = byline.find(["a", "strong"])
        if link:
            return link.get_text(" ", strip=True)

    return ""


def parse_posted_at(post_container: Tag) -> str:
    link = post_container.select_one(".post-link a.permalink")
    return link.get_text(" ", strip=True) if link else ""


def parse_edited(post_container: Tag) -> dict:
    edited = post_container.select_one(".post-edit")
    if not edited:
        return {}

    text = edited.get_text(" ", strip=True).strip()
    normalized = text.strip("()")
    m = re.match(r"edited by\s+(.+?)\s+((?:\d{4}-\d{2}-\d{2}|Today|Yesterday)\b.*)$", normalized)
    if not m:
        return {"edited_text": text}

    return {
        "edited_by": m.group(1).strip(),
        "edited_at": m.group(2).strip(),
    }


def parse_message_html(post_container: Tag) -> str:
    entry = post_container.select_one(".post-entry .entry-content")
    if not entry:
        return ""

    cloned = copy.deepcopy(entry)

    for sig in cloned.select(".sig-content"):
        sig.decompose()

    for link in list(cloned.find_all("a", href=True)):
        if link.attrs is None:
            continue
        if not is_attachment_href(link.get("href", "")):
            continue
        container = attachment_container_for(link, cloned)
        container.decompose()

    return inner_html(cloned)


def parse_attachments(post_container: Tag, page_url: str, post_id: int) -> list[dict]:
    attachments = []
    seen = set()

    for link in post_container.find_all("a", href=True):
        href = link.get("href", "")
        if not is_attachment_href(href):
            continue

        source_url = absolute_url(page_url, href)
        attachment_id = attachment_id_from_url(source_url)
        if attachment_id in seen:
            continue
        seen.add(attachment_id)

        container = attachment_container_for(link, post_container)
        filename = attachment_filename(link, container, attachment_id)

        attachments.append({
            "attachment_id": attachment_id,
            "post_id": post_id,
            "filename": filename,
            "source_url": source_url,
            "source_text": clean_text(container),
        })

    return attachments


def is_attachment_href(href: str) -> bool:
    if "pun_attachment" in href:
        return True
    parsed = urlparse(href)
    query = parse_qs(parsed.query)
    return query.get("action", [""])[0] == "pun_attachment"


def attachment_id_from_url(url: str) -> str:
    parsed = urlparse(url)
    query = parse_qs(parsed.query)
    for key in ("item", "id", "aid", "attach_id"):
        values = query.get(key)
        if values and values[0]:
            return safe_filename(values[0])
    return hashlib.sha1(url.encode("utf-8")).hexdigest()[:16]


def attachment_container_for(link: Tag, stop_at: Tag) -> Tag:
    current = link
    while isinstance(current.parent, Tag) and current.parent is not stop_at:
        parent = current.parent
        classes = " ".join(parent.get("class", []))
        ident = parent.get("id", "")
        marker = f"{classes} {ident}".lower()
        if "attach" in marker:
            return parent
        text = parent.get_text(" ", strip=True).lower()
        if "post's attachments" in text or "attachments" in text:
            return parent
        current = parent
    return link


def attachment_filename(link: Tag, container: Tag, attachment_id: str) -> str:
    candidates = [
        link.get("download", ""),
        link.get("title", ""),
        link.get_text(" ", strip=True),
    ]
    for value in candidates:
        value = value.strip()
        if value.lower() in GENERIC_ATTACHMENT_TEXT:
            continue
        filename = filename_from_text(value)
        return safe_filename(filename or value)

    text = container.get_text(" ", strip=True)
    filename = filename_from_text(text)
    if filename:
        return safe_filename(filename)

    return f"attachment_{attachment_id}"


def filename_from_text(text: str) -> str:
    m = re.search(r"([\w .,\-()[\]{}@!#$%^&+=~']+\.[A-Za-z0-9]{1,12})", text)
    if m:
        return m.group(1).strip()
    return ""


# ---------------------------------------------------------------------------
# Attachment downloading and JSON output
# ---------------------------------------------------------------------------


def download_attachment(
    session: requests.Session,
    attachment: dict,
    config: ScraperConfig,
) -> dict:
    metadata = dict(attachment)
    attachment_id = str(metadata["attachment_id"])
    filename = safe_filename(str(metadata.get("filename", f"attachment_{attachment_id}")))

    try:
        resp = _rate_limited_get(session, metadata["source_url"], config.delay_seconds, timeout=60, stream=True)
        resp.raise_for_status()
    except Interrupted:
        raise
    except requests.RequestException as exc:
        metadata["download_error"] = str(exc)
        with stats_lock:
            stats["attachment_failed"] += 1
        return metadata

    disposition_name = filename_from_content_disposition(resp.headers.get("Content-Disposition", ""))
    if disposition_name:
        filename = safe_filename(disposition_name)
        metadata["filename"] = filename

    rel_path = f"attachments/{attachment_id}/{filename}"
    out_path = os.path.join(config.output_dir, rel_path)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)

    try:
        with _no_interrupt():
            tmp_path = f"{out_path}.tmp"
            with open(tmp_path, "wb") as f:
                for chunk in resp.iter_content(chunk_size=1024 * 64):
                    if chunk:
                        f.write(chunk)
            os.replace(tmp_path, out_path)
    except OSError as exc:
        metadata["download_error"] = str(exc)
        with stats_lock:
            stats["attachment_failed"] += 1
        return metadata

    metadata["local_path"] = rel_path
    metadata["content_type"] = resp.headers.get("Content-Type", "")
    try:
        metadata["size_bytes"] = os.path.getsize(out_path)
    except OSError:
        pass

    with stats_lock:
        stats["attachments"] += 1

    return metadata


def filename_from_content_disposition(value: str) -> str:
    if not value:
        return ""
    msg = Message()
    msg["Content-Disposition"] = value
    filename = msg.get_filename()
    return filename or ""


def write_topic_json(topic: dict, config: ScraperConfig) -> None:
    out_path = local_topic_path(config.output_dir, int(topic["topic_id"]))
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    tmp_path = f"{out_path}.tmp"
    topic = topic_json_record(topic)
    with _no_interrupt():
        with open(tmp_path, "w", encoding="utf-8") as f:
            json.dump(topic, f, ensure_ascii=False, indent=2)
            f.write("\n")
        os.replace(tmp_path, out_path)


def load_topic_index(output_dir: str) -> dict[int, dict]:
    path = topic_index_path(output_dir)
    if not os.path.exists(path):
        return {}

    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)

    records = data.get("topics", data)
    if not isinstance(records, dict):
        return {}

    result: dict[int, dict] = {}
    for key, value in records.items():
        if not isinstance(value, dict):
            continue
        try:
            topic_id = int(key)
        except ValueError:
            continue
        result[topic_id] = value
    return result


def write_topic_index(topic_index: dict[int, dict], output_dir: str) -> None:
    os.makedirs(output_dir, exist_ok=True)
    path = topic_index_path(output_dir)
    tmp_path = f"{path}.tmp"
    data = {
        "topics": {str(topic_id): topic_index[topic_id] for topic_id in sorted(topic_index)},
    }
    with _no_interrupt():
        with open(tmp_path, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
            f.write("\n")
        os.replace(tmp_path, path)


def max_topic_index_id(topic_index: dict[int, dict]) -> int:
    return max(topic_index.keys(), default=0)


def known_topic_high_water(config: ScraperConfig, highest_disk_topic_id: int) -> int:
    return max(config.max_known_topic_id, highest_disk_topic_id)


def topic_json_record(topic: dict) -> dict:
    record = dict(topic)
    for key in ("view_count", "views", "topic_views"):
        record.pop(key, None)
    return record


# ---------------------------------------------------------------------------
# Crawling
# ---------------------------------------------------------------------------


def collect_forum_topic_metadata(
    session: requests.Session,
    config: ScraperConfig,
    forums: list[ForumInfo],
) -> dict[int, dict]:
    topic_index = load_topic_index(config.output_dir)
    pages_since_save = 0

    print(f"Forum metadata phase: starting with {len(topic_index)} indexed topic(s)")
    for forum in forums:
        url = forum.source_url
        seen_pages = set()
        page = 1

        while url and not interrupted:
            if url in seen_pages:
                break
            seen_pages.add(url)

            try:
                html = fetch_html(session, url, config.delay_seconds)
            except ScrapeError as exc:
                with stats_lock:
                    stats["forum_pages_failed"] += 1
                print(f"  [!] Forum {forum.forum_id} metadata page {page}: {exc}")
                break

            page_topics, next_url = parse_forum_page(html, url, forum)
            for topic in page_topics:
                topic_index[topic.topic_id] = topic_summary_record(topic)

            print(f"  Forum {forum.forum_id} metadata page {page}: {len(page_topics)} topic(s)")
            pages_since_save += 1
            if pages_since_save >= TOPIC_INDEX_SAVE_EVERY_PAGES:
                write_topic_index(topic_index, config.output_dir)
                pages_since_save = 0

            url = next_url
            page += 1

    write_topic_index(topic_index, config.output_dir)
    print(f"Forum metadata phase: indexed {len(topic_index)} topic(s)")
    print()
    return topic_index


def crawl_forum(
    session: requests.Session,
    config: ScraperConfig,
    forum: ForumInfo,
    seen_topic_ids: set[int],
) -> None:
    url = forum.source_url
    seen_pages = set()
    page = 1

    while url and not interrupted:
        if url in seen_pages:
            break
        seen_pages.add(url)

        try:
            html = fetch_html(session, url, config.delay_seconds)
        except ScrapeError as exc:
            with stats_lock:
                stats["forum_pages_failed"] += 1
            print(f"  [!] Forum {forum.forum_id} page {page}: {exc}")
            return

        page_topics, next_url = parse_forum_page(html, url, forum)
        new_topics = []
        for topic in page_topics:
            if topic.topic_id in seen_topic_ids:
                continue
            seen_topic_ids.add(topic.topic_id)
            new_topics.append(topic)

        print(f"  Forum {forum.forum_id} page {page}: {len(page_topics)} topic(s), {len(new_topics)} new")
        process_topic_batch(new_topics, config)

        url = next_url
        page += 1


def scrape_topic(session: requests.Session, config: ScraperConfig, summary: TopicSummary) -> tuple[dict | None, list[str]]:
    warnings: list[str] = []

    if summary.flags.get("moved"):
        topic = base_topic_record(summary)
        if summary.moved_to_topic_id is not None:
            topic["moved_to_topic_id"] = summary.moved_to_topic_id
        topic["posts"] = []
        topic["attachments"] = []
        return topic, warnings

    posts_by_id: dict[int, dict] = {}
    attachments: list[dict] = []
    expected_total = None
    url = summary.source_url
    seen_pages = set()

    while url and not interrupted:
        if url in seen_pages:
            warnings.append(f"Repeated topic page URL, stopping pagination: {url}")
            break
        seen_pages.add(url)

        html = fetch_html(session, url, config.delay_seconds)
        page_posts, page_attachments, next_url, page_expected_total = parse_topic_page(html, url)
        if expected_total is None and page_expected_total is not None:
            expected_total = page_expected_total
        for post in page_posts:
            posts_by_id[post["post_id"]] = post
        attachments.extend(page_attachments)
        url = next_url

    if expected_total is not None and len(posts_by_id) != expected_total:
        warnings.append(f"Expected {expected_total} post(s), scraped {len(posts_by_id)}")

    deduped_attachments = dedupe_attachments(attachments)
    downloaded = []
    for attachment in deduped_attachments:
        if interrupted:
            raise Interrupted
        downloaded.append(download_attachment(session, attachment, config))

    topic = base_topic_record(summary)
    topic["posts"] = [posts_by_id[post_id] for post_id in sorted(posts_by_id)]
    topic["attachments"] = downloaded
    return topic, warnings


def scrape_topic_id(
    session: requests.Session,
    config: ScraperConfig,
    topic_id: int,
    forums_by_id: dict[int, ForumInfo],
) -> tuple[dict | None, bool, list[str]]:
    warnings: list[str] = []
    posts_by_id: dict[int, dict] = {}
    attachments: list[dict] = []
    expected_total = None
    first_url = direct_topic_url(config.base_url, topic_id)
    url: str | None = first_url
    seen_pages = set()
    topic_record = None

    while url and not interrupted:
        if url in seen_pages:
            warnings.append(f"Repeated topic page URL, stopping pagination: {url}")
            break
        seen_pages.add(url)

        html = fetch_html(session, url, config.delay_seconds)
        if topic_record is None:
            topic_record = parse_topic_metadata(html, first_url, topic_id, forums_by_id)
            if topic_record is None:
                return None, True, warnings

        page_posts, page_attachments, next_url, page_expected_total = parse_topic_page(html, url)
        if expected_total is None and page_expected_total is not None:
            expected_total = page_expected_total
        for post in page_posts:
            posts_by_id[post["post_id"]] = post
        attachments.extend(page_attachments)
        url = next_url

    if topic_record is None:
        return None, True, warnings

    if expected_total is not None and len(posts_by_id) != expected_total:
        warnings.append(f"Expected {expected_total} post(s), scraped {len(posts_by_id)}")

    deduped_attachments = dedupe_attachments(attachments)
    downloaded = []
    for attachment in deduped_attachments:
        if interrupted:
            raise Interrupted
        downloaded.append(download_attachment(session, attachment, config))

    topic_record["posts"] = [posts_by_id[post_id] for post_id in sorted(posts_by_id)]
    topic_record["attachments"] = downloaded
    return topic_record, False, warnings


def base_topic_record(summary: TopicSummary) -> dict:
    return {
        "topic_id": summary.topic_id,
        "category_name": summary.category_name,
        "forum_id": summary.forum_id,
        "forum_name": summary.forum_name,
        "subject": summary.subject,
        "flags": {k: v for k, v in summary.flags.items() if v},
        "source_url": summary.source_url,
    }


def topic_summary_record(summary: TopicSummary) -> dict:
    record = base_topic_record(summary)
    if summary.view_count is not None:
        record["view_count"] = summary.view_count
    if summary.moved_to_topic_id is not None:
        record["moved_to_topic_id"] = summary.moved_to_topic_id
    return record


def dedupe_attachments(attachments: list[dict]) -> list[dict]:
    seen = set()
    result = []
    for attachment in attachments:
        key = (attachment.get("attachment_id"), attachment.get("post_id"))
        if key in seen:
            continue
        seen.add(key)
        result.append(attachment)
    return result


def process_topic(topic: TopicSummary, config: ScraperConfig) -> tuple[int, str, list[str]]:
    if os.path.exists(local_topic_path(config.output_dir, topic.topic_id)):
        with stats_lock:
            stats["skipped_exists"] += 1
        return topic.topic_id, "skipped (exists)", []

    session = create_session(config.cookie)
    try:
        result, warnings = scrape_topic(session, config, topic)
        if result is None:
            with stats_lock:
                stats["failed"] += 1
            return topic.topic_id, "FAILED", warnings
        if any(a.get("download_error") for a in result.get("attachments", [])):
            with stats_lock:
                stats["failed"] += 1
            warnings.append("One or more attachments failed to download; topic JSON was not written")
            return topic.topic_id, "FAILED (attachment download)", warnings
        write_topic_json(result, config)
        with stats_lock:
            if topic.flags.get("moved"):
                stats["moved"] += 1
            else:
                stats["scraped"] += 1
        post_count = len(result.get("posts", []))
        attachment_count = len(result.get("attachments", []))
        if topic.flags.get("moved"):
            return topic.topic_id, "OK (moved topic stub)", warnings
        return topic.topic_id, f"OK ({post_count} post(s), {attachment_count} attachment(s))", warnings
    except Interrupted:
        raise
    except Exception as exc:
        with stats_lock:
            stats["failed"] += 1
        return topic.topic_id, f"FAILED ({exc})", []


def process_topic_id(
    topic_id: int,
    config: ScraperConfig,
    forums_by_id: dict[int, ForumInfo],
    *,
    write_missing_marker: bool = True,
    ignore_missing_marker: bool = False,
) -> tuple[int, str, list[str], str]:
    if has_topic_output(config.output_dir, topic_id) and not (
        ignore_missing_marker and is_missing_topic_marker(config.output_dir, topic_id)
    ):
        with stats_lock:
            stats["skipped_exists"] += 1
        return topic_id, "skipped (exists)", [], "skipped"

    session = create_session(config.cookie)
    try:
        result, missing, warnings = scrape_topic_id(session, config, topic_id, forums_by_id)
        if missing:
            if write_missing_marker:
                write_missing_topic_marker(config.output_dir, topic_id)
            with stats_lock:
                stats["missing"] += 1
            marker_status = "missing/inaccessible"
            if not write_missing_marker:
                marker_status += " (not marked)"
            return topic_id, marker_status, warnings, "missing"
        if result is None:
            with stats_lock:
                stats["failed"] += 1
            return topic_id, "FAILED", warnings, "failed"
        if any(a.get("download_error") for a in result.get("attachments", [])):
            with stats_lock:
                stats["failed"] += 1
            warnings.append("One or more attachments failed to download; topic JSON was not written")
            return topic_id, "FAILED (attachment download)", warnings, "failed"

        write_topic_json(result, config)
        with stats_lock:
            stats["scraped"] += 1
        post_count = len(result.get("posts", []))
        attachment_count = len(result.get("attachments", []))
        return topic_id, f"OK ({post_count} post(s), {attachment_count} attachment(s))", warnings, "ok"
    except Interrupted:
        raise
    except ScrapeError as exc:
        with stats_lock:
            stats["failed"] += 1
        return topic_id, f"FAILED ({exc})", [], "failed"
    except Exception as exc:
        with stats_lock:
            stats["failed"] += 1
        return topic_id, f"FAILED ({exc})", [], "failed"


def process_topic_batch(topics: list[TopicSummary], config: ScraperConfig) -> None:
    if not topics:
        return

    if config.workers == 1:
        for idx, topic in enumerate(topics, start=1):
            if interrupted:
                return
            topic_id, status, warnings = process_topic(topic, config)
            print_topic_result(idx, len(topics), topic_id, status, warnings)
        return

    completed = 0
    with ThreadPoolExecutor(max_workers=config.workers) as executor:
        futures = {executor.submit(process_topic, topic, config): topic for topic in topics}
        for future in as_completed(futures):
            if interrupted:
                executor.shutdown(wait=False, cancel_futures=True)
                return
            completed += 1
            topic_id, status, warnings = future.result()
            print_topic_result(completed, len(topics), topic_id, status, warnings)


def process_topic_id_batch(
    topic_ids: list[int],
    config: ScraperConfig,
    forums_by_id: dict[int, ForumInfo],
) -> list[tuple[int, str]]:
    if not topic_ids:
        return []

    results: list[tuple[int, str]] = []
    if config.workers == 1:
        for idx, topic_id in enumerate(topic_ids, start=1):
            if interrupted:
                return results
            tid, status, warnings, result_type = process_topic_id(topic_id, config, forums_by_id)
            results.append((tid, result_type))
            print_topic_result(idx, len(topic_ids), tid, status, warnings)
        return results

    completed = 0
    with ThreadPoolExecutor(max_workers=config.workers) as executor:
        futures = {
            executor.submit(process_topic_id, topic_id, config, forums_by_id): topic_id
            for topic_id in topic_ids
        }
        for future in as_completed(futures):
            if interrupted:
                executor.shutdown(wait=False, cancel_futures=True)
                return results
            completed += 1
            tid, status, warnings, result_type = future.result()
            results.append((tid, result_type))
            print_topic_result(completed, len(topic_ids), tid, status, warnings)
    return results


def collect_missing_historical_ids(config: ScraperConfig, high_water_topic_id: int) -> list[int]:
    missing = []
    skipped = 0
    for topic_id in range(1, high_water_topic_id + 1):
        if has_topic_output(config.output_dir, topic_id):
            skipped += 1
            continue
        missing.append(topic_id)
    if skipped:
        with stats_lock:
            stats["skipped_exists"] += skipped
        print(f"Skipping {skipped} already completed topic ID(s)")
    return missing


def run_backfill_phase(
    config: ScraperConfig,
    forums_by_id: dict[int, ForumInfo],
    high_water_topic_id: int,
) -> None:
    topic_ids = collect_missing_historical_ids(config, high_water_topic_id)
    print(f"Backfill phase: {len(topic_ids)} missing topic ID(s) in 1..{high_water_topic_id}")
    process_topic_id_batch(topic_ids, config, forums_by_id)
    print()


def mark_pending_missing_topics(config: ScraperConfig, topic_ids: list[int]) -> None:
    if not topic_ids:
        return

    written = 0
    for topic_id in topic_ids:
        if not has_topic_output(config.output_dir, topic_id):
            write_missing_topic_marker(config.output_dir, topic_id)
            written += 1

    if written:
        print(f"Marked {written} confirmed historical missing topic ID(s)")


def run_discovery_phase(
    config: ScraperConfig,
    forums_by_id: dict[int, ForumInfo],
    start_topic_id: int,
) -> int:
    print(f"Discovery phase: probing from topic ID {start_topic_id}...")
    topic_id = start_topic_id
    discovered = 0
    consecutive_missing = 0
    max_confirmed_topic_id = start_topic_id - 1
    pending_missing: list[int] = []

    while not interrupted and consecutive_missing < config.missing_stop_after:
        has_output = has_topic_output(config.output_dir, topic_id)
        has_missing_marker = has_output and is_missing_topic_marker(config.output_dir, topic_id)
        if has_output and not has_missing_marker:
            mark_pending_missing_topics(config, pending_missing)
            pending_missing = []
            consecutive_missing = 0
            print(f"Topic {topic_id}: already completed")
            max_confirmed_topic_id = max(max_confirmed_topic_id, topic_id)
            topic_id += 1
            continue

        tid, status, warnings, result_type = process_topic_id(
            topic_id,
            config,
            forums_by_id,
            write_missing_marker=False,
            ignore_missing_marker=has_missing_marker,
        )
        print_topic_result(consecutive_missing + 1, config.missing_stop_after, tid, status, warnings)

        if result_type == "ok":
            mark_pending_missing_topics(config, pending_missing)
            pending_missing = []
            consecutive_missing = 0
            discovered += 1
            max_confirmed_topic_id = max(max_confirmed_topic_id, topic_id)
        elif result_type == "missing":
            if not has_missing_marker:
                pending_missing.append(topic_id)
            consecutive_missing += 1
        elif result_type == "failed":
            print("Discovery stopped due to request failure before reaching the missing-ID threshold.")
            break

        topic_id += 1

    if consecutive_missing >= config.missing_stop_after:
        print(f"Discovery stopped after {config.missing_stop_after} consecutive missing topic ID(s).")
    if discovered == 0:
        print("No new topics discovered")
    print()
    return max_confirmed_topic_id


def print_topic_result(progress: int, total: int, topic_id: int, status: str, warnings: list[str]) -> None:
    pct = round(100 * progress / total) if total else 100
    lines = [f"[{pct:3d}%] Topic {topic_id}: {status}"]
    for warning in warnings:
        lines.append(f"  ^ {warning}")
    with print_lock:
        print("\n".join(lines), flush=True)


def print_summary() -> None:
    print("\n--- Summary ---")
    print(f"  Scraped topics:       {stats['scraped']}")
    print(f"  Moved topic stubs:    {stats['moved']}")
    print(f"  Already done:         {stats['skipped_exists']}")
    print(f"  Failed topics:        {stats['failed']}")
    print(f"  Failed forum pages:   {stats['forum_pages_failed']}")
    print(f"  Attachments saved:    {stats['attachments']}")
    print(f"  Attachment failures:  {stats['attachment_failed']}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Scrape visible Redump forum topics using scraper.cfg settings."
    )
    parser.add_argument(
        "--config",
        help="Path to scraper config file. Defaults to scraper.cfg next to this script.",
    )
    parser.add_argument(
        "--metadata-only",
        action="store_true",
        help="Only refresh topic_index.json from forum topic lists; do not scrape topic JSONs or attachments.",
    )
    args = parser.parse_args()
    config_path = Path(args.config).expanduser().resolve() if args.config else default_config_path()

    try:
        config = load_config(config_path)
    except (FileNotFoundError, ValueError) as exc:
        parser.error(str(exc))

    os.makedirs(os.path.join(config.output_dir, "topics"), exist_ok=True)
    os.makedirs(os.path.join(config.output_dir, "attachments"), exist_ok=True)

    highest_disk_topic_id = highest_completed_topic_id(config.output_dir)
    session = create_session(config.cookie)

    print(f"Config: {config.config_path}")
    print(f"Base URL: {config.base_url}")
    print(f"Output: {os.path.abspath(config.output_dir)}")
    print(f"Max known topic ID in config: {config.max_known_topic_id}")
    print(f"Highest completed topic ID on disk: {highest_disk_topic_id}")
    print(f"Missing stop after: {config.missing_stop_after}")
    print(f"Delay: {config.delay_seconds}s, Workers: {config.workers}")
    print()

    try:
        index_html = fetch_html(session, f"{config.base_url}/", config.delay_seconds)
        username = validate_auth(index_html)
        print(f"Authenticated as: {username}")
        forums = parse_index_page(index_html, f"{config.base_url}/")
        forums_by_id = {forum.forum_id: forum for forum in forums}
        print(f"Visible forums: {len(forums)}")
        print()

        if args.metadata_only:
            topic_index = collect_forum_topic_metadata(session, config, forums)
            high_water_topic_id = max(
                config.max_known_topic_id,
                highest_disk_topic_id,
                max_topic_index_id(topic_index),
            )
            if high_water_topic_id > config.max_known_topic_id:
                update_max_known_topic_id(config, high_water_topic_id)
                print(f"Updated max_known_topic_id in config to {high_water_topic_id}")
                print()

            print("Metadata-only mode complete; topic scraping was skipped.")
            print_summary()
            if interrupted or stats["forum_pages_failed"] > 0:
                sys.exit(1)
            return

        high_water_topic_id = known_topic_high_water(config, highest_disk_topic_id)
        if high_water_topic_id > config.max_known_topic_id:
            update_max_known_topic_id(config, high_water_topic_id)
            print(f"Updated max_known_topic_id in config to {high_water_topic_id}")
            print()

        if not interrupted:
            run_backfill_phase(config, forums_by_id, high_water_topic_id)
        if not interrupted:
            discovered_high_water_topic_id = run_discovery_phase(
                config,
                forums_by_id,
                high_water_topic_id + 1,
            )
            if discovered_high_water_topic_id > high_water_topic_id:
                update_max_known_topic_id(config, discovered_high_water_topic_id)
                print(f"Updated max_known_topic_id in config to {discovered_high_water_topic_id}")

    except Interrupted:
        pass
    except ScrapeError as exc:
        print(f"[!] {exc}")
        print_summary()
        sys.exit(1)

    print_summary()
    if (
        interrupted
        or stats["failed"] > 0
        or stats["forum_pages_failed"] > 0
        or stats["attachment_failed"] > 0
    ):
        sys.exit(1)


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal_handler)
    main()
