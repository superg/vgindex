#!/usr/bin/env python3
"""
Redump.org forum user scraper.

By default, the scraper loads configuration from scraper.cfg located in the
same directory as this script.

Usage:
    python scraper.py
    python scraper.py --config /path/to/scraper.cfg
    python scraper.py --stage2-only
"""

import argparse
import configparser
import csv
import os
import re
import signal
import shutil
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote, urljoin, urlparse

import requests
from bs4 import BeautifulSoup, Tag

DEFAULT_BASE_URL = "http://forum.redump.org"
DEFAULT_OUTPUT_DIR = "data/redump/users"
CONFIG_SECTION = "scraper"
CSV_FILENAME = "users.csv"
SIGNATURE_FILENAME = "signature.txt"

interrupted = False
rate_lock = threading.Lock()


@dataclass(frozen=True)
class ScraperConfig:
    config_path: Path
    base_url: str
    cookie: str
    output_dir: str
    delay_seconds: float
    workers: int


class Interrupted(Exception):
    """Raised when work should stop after SIGINT."""


def signal_handler(_sig, _frame):
    global interrupted
    if interrupted:
        return
    interrupted = True
    print("\nInterrupted - finishing in-flight file writes and stopping...")


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

    output_dir = section.get("output_dir", DEFAULT_OUTPUT_DIR).strip() or DEFAULT_OUTPUT_DIR

    delay_raw = section.get("delay_seconds", "0.1")
    try:
        delay_seconds = float(delay_raw)
    except ValueError as exc:
        raise ValueError("Config key delay_seconds must be a float") from exc
    if delay_seconds < 0:
        raise ValueError("Config key delay_seconds must be >= 0")

    workers_raw = section.get("workers", "4")
    try:
        workers = int(workers_raw)
    except ValueError as exc:
        raise ValueError("Config key workers must be an integer") from exc
    if workers < 1:
        raise ValueError("Config key workers must be >= 1")

    return ScraperConfig(
        config_path=config_path,
        base_url=base_url,
        cookie=cookie,
        output_dir=output_dir,
        delay_seconds=delay_seconds,
        workers=workers,
    )


# ---------------------------------------------------------------------------
# Cookie handling
# ---------------------------------------------------------------------------

def create_session(cookie_str: str | None = None) -> requests.Session:
    """Create a requests session, optionally seeding cookies from a raw header string."""
    session = requests.Session()
    session.headers["User-Agent"] = "vgindex-redump-user-scraper/1.0"

    if cookie_str:
        session.headers["Cookie"] = cookie_str
        for part in cookie_str.split(";"):
            part = part.strip()
            if "=" not in part:
                continue
            name, _, value = part.partition("=")
            name, value = name.strip(), value.strip()
            if name == "redump_cookie":
                session.cookies.set(name, value, domain=".redump.org", path="/")
            elif name == "PHPSESSID":
                session.cookies.set(name, value, domain="forum.redump.org", path="/")

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


# ---------------------------------------------------------------------------
# Stage 1: Public user list
# ---------------------------------------------------------------------------

_TOTAL_RE = re.compile(r"of\s+([\d,]+)")


def parse_public_total(soup: BeautifulSoup) -> int | None:
    """Extract total user count from 'Users: 1 to 50 of 2,441' header."""
    h2 = soup.find("h2", class_="hn")
    if h2:
        m = _TOTAL_RE.search(h2.get_text())
        if m:
            return int(m.group(1).replace(",", ""))
    return None


def parse_public_page(soup: BeautifulSoup) -> list[dict]:
    """Parse one page of the public /users/ table."""
    users = []
    table = soup.find("table")
    if not table:
        return users

    tbody = table.find("tbody")
    rows = tbody.find_all("tr") if tbody else table.find_all("tr")[1:]

    for row in rows:
        tds = row.find_all("td")
        if len(tds) < 4:
            continue
        link = tds[0].find("a")
        if not link or not link.get("href"):
            continue

        user_id = link["href"].rstrip("/").split("/")[-1]
        try:
            user_id = int(user_id)
        except ValueError:
            continue

        users.append({
            "id": user_id,
            "username": link.get_text(strip=True),
            "title": tds[1].get_text(strip=True),
            "registration_date": tds[3].get_text(strip=True),
        })

    return users


def scrape_public_users(session: requests.Session, base_url: str, delay: float) -> tuple[dict[int, dict], int | None]:
    """Scrape all pages of the public user list. Returns (users_by_id, expected_total)."""
    users: dict[int, dict] = {}
    expected_total = None
    page = 1

    while not interrupted:
        url = f"{base_url}/users/?p={page}"
        try:
            resp = session.get(url, timeout=30)
            resp.raise_for_status()
        except requests.RequestException as e:
            print(f"  [!] Page {page}: request failed: {e}")
            break

        soup = BeautifulSoup(resp.text, "lxml")

        if page == 1:
            expected_total = parse_public_total(soup)
            if expected_total:
                total_pages = (expected_total + 49) // 50
                print(f"  Total users reported: {expected_total} ({total_pages} pages)")
            else:
                print("  [!] Could not parse total user count")

        page_users = parse_public_page(soup)
        if not page_users:
            # Pagination glitch: retry once
            if delay > 0:
                time.sleep(delay)
            try:
                resp = session.get(url, timeout=30)
                resp.raise_for_status()
            except requests.RequestException:
                pass
            else:
                page_users = parse_public_page(BeautifulSoup(resp.text, "lxml"))

            if not page_users:
                print(f"  [!] Page {page}: no users found (even after retry), stopping")
                break

        new_count = 0
        for u in page_users:
            if u["id"] not in users:
                new_count += 1
            users[u["id"]] = u

        print(f"  Page {page}: {len(page_users)} users ({new_count} new, {len(users)} total)")

        # Check if we've reached the last page
        paging = soup.find("p", class_="paging")
        has_next = False
        if paging:
            for a in paging.find_all("a"):
                if a.get_text(strip=True) == "Next":
                    has_next = True
                    break

        if not has_next:
            break

        page += 1
        if delay > 0:
            time.sleep(delay)

    return users, expected_total


# ---------------------------------------------------------------------------
# Stage 1: Admin email scrape
# ---------------------------------------------------------------------------

_ADMIN_TOTAL_RE = re.compile(r"Users found \[\s*(\d+)\s*\]")
_USER_ID_RE = re.compile(r"/user/(\d+)/?$")


def parse_admin_page(soup: BeautifulSoup) -> list[dict]:
    """Parse one page of admin search results, extracting ID and email."""
    entries = []
    table = soup.find("table")
    if not table:
        return entries

    tbody = table.find("tbody")
    rows = tbody.find_all("tr") if tbody else table.find_all("tr")[1:]

    for row in rows:
        td0 = row.find("td", class_="tc0")
        if not td0:
            continue

        link = td0.find("a")
        if not link or not link.get("href"):
            continue

        m = _USER_ID_RE.search(link["href"])
        if not m:
            continue
        user_id = int(m.group(1))

        email = ""
        mail_span = td0.find("span", class_="usermail")
        if mail_span:
            mail_link = mail_span.find("a")
            if mail_link:
                email = mail_link.get_text(strip=True)

        entries.append({"id": user_id, "email": email})

    return entries


def scrape_admin_emails(session: requests.Session, base_url: str, delay: float) -> tuple[dict[int, str], int | None]:
    """Scrape emails from the admin user search. Returns (emails_by_id, expected_total)."""
    emails: dict[int, str] = {}
    expected_total = None

    # Step 1: fetch admin page for CSRF token
    print("  Fetching CSRF token...")
    try:
        resp = session.get(f"{base_url}/admin/users.php", timeout=30)
        resp.raise_for_status()
    except requests.RequestException as e:
        print(f"  [!] Failed to fetch admin page: {e}")
        return emails, expected_total

    soup = BeautifulSoup(resp.text, "lxml")

    logged_in = soup.find("p", id="welcome")
    if not logged_in or "Logged in as" not in logged_in.get_text():
        print("  [!] Not authenticated - check the cookie in scraper.cfg")
        return emails, expected_total

    username = logged_in.find("strong")
    print(f"  Authenticated as: {username.text if username else '?'}")

    csrf_input = soup.find("input", {"name": "csrf_token"})
    if not csrf_input:
        print("  [!] Could not find CSRF token on admin page")
        return emails, expected_total

    csrf_token = csrf_input["value"]

    # Step 2: initial search with CSRF
    search_params = {
        "csrf_token": csrf_token,
        "form[username]": "*",
        "form[title]": "",
        "form[realname]": "",
        "form[location]": "",
        "form[signature]": "",
        "form[admin_note]": "",
        "form[email]": "",
        "form[url]": "",
        "form[jabber]": "",
        "form[icq]": "",
        "form[msn]": "",
        "form[aim]": "",
        "form[yahoo]": "",
        "order_by": "username",
        "direction": "ASC",
        "user_group": "-1",
        "find_user": "Submit search",
    }

    try:
        resp = session.get(
            f"{base_url}/admin/users.php",
            params=search_params,
            headers={"Referer": f"{base_url}/admin/users.php"},
            timeout=30,
        )
        resp.raise_for_status()
    except requests.RequestException as e:
        print(f"  [!] Search request failed: {e}")
        return emails, expected_total

    soup = BeautifulSoup(resp.text, "lxml")

    if "Bad request" in soup.get_text():
        print("  [!] Search returned 'Bad request' - CSRF token may have expired")
        return emails, expected_total

    # Extract expected total
    h2 = soup.find("h2", class_="hn")
    if h2:
        m = _ADMIN_TOTAL_RE.search(h2.get_text())
        if m:
            expected_total = int(m.group(1))
            total_pages = (expected_total + 29) // 30
            print(f"  Users found: {expected_total} ({total_pages} pages)")

    # Parse page 1
    page_entries = parse_admin_page(soup)
    for entry in page_entries:
        emails[entry["id"]] = entry["email"]

    print(f"  Page 1: {len(page_entries)} users ({len(emails)} total)")

    if delay > 0:
        time.sleep(delay)

    # Step 3: paginate (no CSRF needed)
    page = 2
    while not interrupted:
        url = (
            f"{base_url}/admin/users.php"
            f"?find_user=&order_by=username&direction=ASC&user_group=-1"
            f"&form%5Busername%5D=%2A&p={page}"
        )

        try:
            resp = session.get(url, timeout=30)
            resp.raise_for_status()
        except requests.RequestException as e:
            print(f"  [!] Page {page}: request failed: {e}")
            break

        soup = BeautifulSoup(resp.text, "lxml")
        page_entries = parse_admin_page(soup)

        if not page_entries:
            # Retry once for pagination glitch
            if delay > 0:
                time.sleep(delay)
            try:
                resp = session.get(url, timeout=30)
                resp.raise_for_status()
            except requests.RequestException:
                pass
            else:
                page_entries = parse_admin_page(BeautifulSoup(resp.text, "lxml"))

            if not page_entries:
                print(f"  [!] Page {page}: no users found (even after retry), stopping")
                break

        new_count = 0
        for entry in page_entries:
            if entry["id"] not in emails:
                new_count += 1
            emails[entry["id"]] = entry["email"]

        print(f"  Page {page}: {len(page_entries)} users ({new_count} new, {len(emails)} total)")

        # Check for next page
        paging = soup.find("p", class_="paging")
        has_next = False
        if paging:
            for a in paging.find_all("a"):
                if a.get_text(strip=True) == "Next":
                    has_next = True
                    break

        if not has_next:
            break

        page += 1
        if delay > 0:
            time.sleep(delay)

    return emails, expected_total


# ---------------------------------------------------------------------------
# CSV output
# ---------------------------------------------------------------------------

def users_csv_path(config: ScraperConfig) -> str:
    return os.path.join(config.output_dir, CSV_FILENAME)


def write_csv(users: dict[int, dict], output_path: str) -> None:
    """Write user data to CSV, sorted by ID."""
    os.makedirs(os.path.dirname(output_path) or ".", exist_ok=True)

    sorted_users = sorted(users.values(), key=lambda u: u["id"])

    tmp_path = f"{output_path}.tmp"
    with _no_interrupt():
        with open(tmp_path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["ID", "Username", "Email", "Title", "Registration Date"])
            for u in sorted_users:
                writer.writerow([
                    u["id"],
                    u["username"],
                    u.get("email", ""),
                    u["title"],
                    u["registration_date"],
                ])
        os.replace(tmp_path, output_path)


def read_users_csv(input_path: str) -> list[dict]:
    users = []
    with open(input_path, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        for row in reader:
            raw_id = (row.get("ID") or "").strip()
            try:
                user_id = int(raw_id)
            except ValueError:
                continue
            users.append({
                "id": user_id,
                "username": row.get("Username", ""),
            })
    return users


# ---------------------------------------------------------------------------
# Stage 2: Profile assets
# ---------------------------------------------------------------------------

def inner_html(tag: Tag) -> str:
    return tag.decode_contents().strip()


def safe_filename(name: str) -> str:
    name = name.strip().replace("\\", "_").replace("/", "_")
    name = re.sub(r"[\x00-\x1f\x7f]+", "", name)
    name = re.sub(r"\s+", " ", name)
    name = name.strip(" .")
    return name or "avatar"


def filename_from_url(url: str, fallback: str) -> str:
    parsed = urlparse(url)
    filename = os.path.basename(unquote(parsed.path))
    return safe_filename(filename or fallback)


def profile_url(config: ScraperConfig, user_id: int) -> str:
    return f"{config.base_url}/user/{user_id}/"


def assets_root(config: ScraperConfig) -> str:
    return os.path.join(config.output_dir, "data")


def user_assets_dir(config: ScraperConfig, user_id: int) -> str:
    return os.path.join(assets_root(config), str(user_id))


def user_signature_path(config: ScraperConfig, user_id: int) -> str:
    return os.path.join(user_assets_dir(config, user_id), SIGNATURE_FILENAME)


def user_assets_complete(config: ScraperConfig, user_id: int) -> bool:
    return os.path.exists(user_signature_path(config, user_id))


def parse_profile_assets(html: str, page_url: str) -> dict:
    soup = BeautifulSoup(html, "lxml")
    avatar_url = ""
    avatar = soup.select_one("li.useravatar img")
    if avatar and avatar.get("src"):
        avatar_url = urljoin(page_url, avatar["src"])

    signature = soup.select_one(".sig-demo")
    signature_html = inner_html(signature) if signature else ""

    return {
        "avatar_url": avatar_url,
        "signature_html": signature_html,
    }


def write_text_atomic(path: str, text: str) -> None:
    tmp_path = f"{path}.tmp"
    with _no_interrupt():
        with open(tmp_path, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp_path, path)


def download_avatar(
    session: requests.Session,
    avatar_url: str,
    staging_dir: str,
    config: ScraperConfig,
    user_id: int,
) -> str:
    filename = filename_from_url(avatar_url, f"avatar_{user_id}")
    if filename == SIGNATURE_FILENAME:
        filename = f"avatar_{filename}"
    out_path = os.path.join(staging_dir, filename)
    tmp_path = f"{out_path}.tmp"

    resp = _rate_limited_get(
        session,
        avatar_url,
        config.delay_seconds,
        timeout=60,
        stream=True,
    )
    resp.raise_for_status()

    with _no_interrupt():
        with open(tmp_path, "wb") as f:
            for chunk in resp.iter_content(chunk_size=1024 * 64):
                if chunk:
                    f.write(chunk)
        os.replace(tmp_path, out_path)

    return filename


def replace_user_assets(staging_dir: str, final_dir: str) -> None:
    parent = os.path.dirname(final_dir)
    os.makedirs(parent, exist_ok=True)
    with _no_interrupt():
        if os.path.isdir(final_dir):
            shutil.rmtree(final_dir)
        elif os.path.exists(final_dir):
            os.remove(final_dir)
        os.replace(staging_dir, final_dir)


def scrape_user_assets(user: dict, config: ScraperConfig) -> dict:
    user_id = int(user["id"])
    username = user.get("username", "")
    final_dir = user_assets_dir(config, user_id)

    if user_assets_complete(config, user_id):
        return {"id": user_id, "username": username, "status": "skipped", "message": "already complete"}

    session = create_session(config.cookie)
    url = profile_url(config, user_id)

    try:
        resp = _rate_limited_get(session, url, config.delay_seconds, timeout=30)
        resp.raise_for_status()
    except Interrupted:
        raise
    except requests.RequestException as exc:
        return {"id": user_id, "username": username, "status": "profile_failed", "message": str(exc)}

    assets = parse_profile_assets(resp.text, url)
    root = assets_root(config)
    os.makedirs(root, exist_ok=True)
    staging_dir = tempfile.mkdtemp(prefix=f".{user_id}.", dir=root)

    try:
        avatar_filename = ""
        if assets["avatar_url"]:
            avatar_filename = download_avatar(
                session,
                assets["avatar_url"],
                staging_dir,
                config,
                user_id,
            )

        write_text_atomic(
            os.path.join(staging_dir, SIGNATURE_FILENAME),
            assets["signature_html"],
        )
        replace_user_assets(staging_dir, final_dir)
    except Interrupted:
        shutil.rmtree(staging_dir, ignore_errors=True)
        raise
    except (OSError, requests.RequestException) as exc:
        shutil.rmtree(staging_dir, ignore_errors=True)
        status = "avatar_failed" if assets["avatar_url"] else "asset_failed"
        return {"id": user_id, "username": username, "status": status, "message": str(exc)}

    suffix = f"avatar {avatar_filename}, signature" if avatar_filename else "signature"
    return {"id": user_id, "username": username, "status": "ok", "message": suffix}


def run_stage2(config: ScraperConfig) -> dict:
    csv_path = users_csv_path(config)
    if not os.path.exists(csv_path):
        raise FileNotFoundError(f"Stage 2 requires existing CSV: {csv_path}")

    users = read_users_csv(csv_path)
    stats = {
        "ok": 0,
        "skipped": 0,
        "profile_failed": 0,
        "avatar_failed": 0,
        "asset_failed": 0,
    }

    print("Stage 2: Scraping profile assets...")
    print(f"  Users from CSV: {len(users)}")
    print(f"  Workers: {config.workers}")

    if not users:
        print("  [!] No users found in CSV")
        return stats

    with ThreadPoolExecutor(max_workers=config.workers) as executor:
        futures = {executor.submit(scrape_user_assets, user, config): user for user in users}
        completed = 0
        for future in as_completed(futures):
            if interrupted:
                break
            completed += 1
            try:
                result = future.result()
            except Interrupted:
                break
            except Exception as exc:
                user = futures[future]
                result = {
                    "id": user.get("id", "?"),
                    "username": user.get("username", ""),
                    "status": "asset_failed",
                    "message": str(exc),
                }

            status = result["status"]
            if status not in stats:
                stats[status] = 0
            stats[status] += 1

            label = f"{result['id']}"
            if result.get("username"):
                label += f" ({result['username']})"
            if status == "ok":
                print(f"  [{completed}/{len(users)}] User {label}: OK - {result['message']}")
            elif status == "skipped":
                print(f"  [{completed}/{len(users)}] User {label}: skipped ({result['message']})")
            else:
                print(f"  [{completed}/{len(users)}] User {label}: WARNING {status}: {result['message']}")

    print()
    print("--- Stage 2 Summary ---")
    print(f"  Saved:           {stats['ok']}")
    print(f"  Already done:    {stats['skipped']}")
    print(f"  Profile failed:  {stats['profile_failed']}")
    print(f"  Avatar failed:   {stats['avatar_failed']}")
    print(f"  Other failed:    {stats['asset_failed']}")
    print()
    return stats


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def run_stage1(config: ScraperConfig) -> None:
    print("Stage 1: Scraping public user list...")
    session = create_session(config.cookie)
    users, public_total = scrape_public_users(session, config.base_url, config.delay_seconds)

    print()
    print(f"  Distinct users scraped: {len(users)}")
    if public_total is not None:
        if len(users) == public_total:
            print(f"  Matches expected count ({public_total})")
        else:
            diff = public_total - len(users)
            print(f"  [!] Expected {public_total}, got {len(users)} (difference: {diff})")
    print()

    admin_total = None
    if not interrupted:
        print("Stage 1: Scraping admin panel for emails...")
        admin_session = create_session(config.cookie)
        emails, admin_total = scrape_admin_emails(
            admin_session,
            config.base_url,
            config.delay_seconds,
        )

        # Merge emails into user records
        merged = 0
        admin_only_ids = []
        for uid, email in emails.items():
            if uid in users:
                users[uid]["email"] = email
                merged += 1
            else:
                admin_only_ids.append(uid)

        users_without_email = [uid for uid, u in users.items() if "email" not in u]

        print()
        print(f"  Emails scraped: {len(emails)}")
        print(f"  Merged into user records: {merged}")

        if admin_total is not None:
            if len(emails) == admin_total:
                print(f"  Admin count matches expected ({admin_total})")
            else:
                print(f"  [!] Admin expected {admin_total}, got {len(emails)}")

        if admin_only_ids:
            suffix = "..." if len(admin_only_ids) > 10 else ""
            print(f"  [!] {len(admin_only_ids)} user(s) in admin but NOT in public list: {admin_only_ids[:10]}{suffix}")

        if users_without_email:
            suffix = "..." if len(users_without_email) > 10 else ""
            print(
                f"  [!] {len(users_without_email)} user(s) in public list but NOT in admin: "
                f"{users_without_email[:10]}{suffix}"
            )

        # Cross-validate totals
        if public_total is not None and admin_total is not None and public_total != admin_total:
            print(f"  [!] Public total ({public_total}) != Admin total ({admin_total})")

        print()

    output_path = users_csv_path(config)
    write_csv(users, output_path)
    abs_path = os.path.abspath(output_path)
    print(f"Wrote {len(users)} users to {abs_path}")

    with_email = sum(1 for u in users.values() if u.get("email"))
    without_email = len(users) - with_email
    print()
    print("--- Stage 1 Summary ---")
    print(f"  Total users:      {len(users)}")
    if public_total is not None:
        print(f"  Expected (public): {public_total}")
    if admin_total is not None:
        print(f"  Expected (admin):  {admin_total}")
    print(f"  With email:       {with_email}")
    print(f"  Without email:    {without_email}")
    print()


def main():
    parser = argparse.ArgumentParser(
        description="Scrape Redump forum users and profile assets using scraper.cfg settings."
    )
    parser.add_argument(
        "--config",
        help="Path to scraper config file. Defaults to scraper.cfg next to this script.",
    )
    parser.add_argument(
        "--stage2-only",
        action="store_true",
        help="Skip users.csv generation and only retry profile asset scraping.",
    )
    args = parser.parse_args()

    config_path = Path(args.config).expanduser().resolve() if args.config else default_config_path()
    try:
        config = load_config(config_path)
    except (FileNotFoundError, ValueError) as exc:
        parser.error(str(exc))

    os.makedirs(config.output_dir, exist_ok=True)

    print(f"Config: {config.config_path}")
    print(f"Base URL: {config.base_url}")
    print(f"Output: {os.path.abspath(config.output_dir)}")
    print(f"Delay: {config.delay_seconds}s, Workers: {config.workers}")
    print()

    if not args.stage2_only:
        run_stage1(config)
    elif not os.path.exists(users_csv_path(config)):
        parser.error(f"--stage2-only requires existing CSV: {users_csv_path(config)}")

    if not interrupted:
        try:
            run_stage2(config)
        except FileNotFoundError as exc:
            parser.error(str(exc))

    if interrupted:
        sys.exit(1)


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal_handler)
    main()
