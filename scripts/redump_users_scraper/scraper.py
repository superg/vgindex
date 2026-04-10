#!/usr/bin/env python3
"""
Redump.org forum user scraper.

Scrapes user data from the public user list and admin panel for migration.
Phase 1 (public): ID, Username, Title, Registration Date from /users/
Phase 2 (admin):  Email from /admin/users.php (requires moderator cookie)

Usage:
    python scraper.py --cookie "PHPSESSID=abc; redump_cookie=xyz"
    python scraper.py --cookie "PHPSESSID=abc; redump_cookie=xyz" --output users.csv
    python scraper.py --skip-emails
"""

import argparse
import csv
import os
import re
import signal
import sys
import time

import requests
from bs4 import BeautifulSoup

FORUM_URL = "http://forum.redump.org"

interrupted = False


def signal_handler(_sig, _frame):
    global interrupted
    if interrupted:
        return
    interrupted = True
    print("\nInterrupted — writing partial results…")


# ---------------------------------------------------------------------------
# Cookie handling
# ---------------------------------------------------------------------------

def create_session(cookie_str: str | None = None) -> requests.Session:
    """Create a requests session, optionally seeding cookies from a raw header string."""
    session = requests.Session()
    session.headers["User-Agent"] = "vgindex-redump-user-scraper/1.0"

    if cookie_str:
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


# ---------------------------------------------------------------------------
# Phase 1: Public user list
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


def scrape_public_users(session: requests.Session, delay: float) -> tuple[dict[int, dict], int | None]:
    """Scrape all pages of the public user list. Returns (users_by_id, expected_total)."""
    users: dict[int, dict] = {}
    expected_total = None
    page = 1

    while not interrupted:
        url = f"{FORUM_URL}/users/?p={page}"
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
# Phase 2: Admin email scrape
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


def scrape_admin_emails(session: requests.Session, delay: float) -> tuple[dict[int, str], int | None]:
    """Scrape emails from the admin user search. Returns (emails_by_id, expected_total)."""
    emails: dict[int, str] = {}
    expected_total = None

    # Step 1: fetch admin page for CSRF token
    print("  Fetching CSRF token…")
    try:
        resp = session.get(f"{FORUM_URL}/admin/users.php", timeout=30)
        resp.raise_for_status()
    except requests.RequestException as e:
        print(f"  [!] Failed to fetch admin page: {e}")
        return emails, expected_total

    soup = BeautifulSoup(resp.text, "lxml")

    logged_in = soup.find("p", id="welcome")
    if not logged_in or "Logged in as" not in logged_in.get_text():
        print("  [!] Not authenticated — check your --cookie value")
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
            f"{FORUM_URL}/admin/users.php",
            params=search_params,
            headers={"Referer": f"{FORUM_URL}/admin/users.php"},
            timeout=30,
        )
        resp.raise_for_status()
    except requests.RequestException as e:
        print(f"  [!] Search request failed: {e}")
        return emails, expected_total

    soup = BeautifulSoup(resp.text, "lxml")

    if "Bad request" in soup.get_text():
        print("  [!] Search returned 'Bad request' — CSRF token may have expired")
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
            f"{FORUM_URL}/admin/users.php"
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

def write_csv(users: dict[int, dict], output_path: str) -> None:
    """Write user data to CSV, sorted by ID."""
    os.makedirs(os.path.dirname(output_path) or ".", exist_ok=True)

    sorted_users = sorted(users.values(), key=lambda u: u["id"])

    with open(output_path, "w", newline="", encoding="utf-8") as f:
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


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Scrape user data from redump.org forum for migration."
    )
    parser.add_argument(
        "--cookie",
        default=None,
        help=(
            'Full Cookie header value from browser dev tools '
            '(e.g. "PHPSESSID=abc; redump_cookie=xyz"). '
            "Required unless --skip-emails is used."
        ),
    )
    parser.add_argument(
        "--output",
        default="data/redump/users.csv",
        help="Output CSV path (default: data/redump/users.csv).",
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=0.1,
        help="Delay in seconds between requests (default: 0.1).",
    )
    parser.add_argument(
        "--skip-emails",
        action="store_true",
        help="Skip admin email scrape (Phase 2). No cookie needed.",
    )
    args = parser.parse_args()

    if not args.skip_emails and not args.cookie:
        parser.error("--cookie is required unless --skip-emails is used")

    if args.delay < 0:
        parser.error("delay must be >= 0")

    # Phase 1: public user list
    print("Phase 1: Scraping public user list…")
    session = create_session()
    users, public_total = scrape_public_users(session, args.delay)

    print()
    print(f"  Distinct users scraped: {len(users)}")
    if public_total is not None:
        if len(users) == public_total:
            print(f"  ✓ Matches expected count ({public_total})")
        else:
            diff = public_total - len(users)
            print(f"  ✗ Expected {public_total}, got {len(users)} (difference: {diff})")
    print()

    # Phase 2: admin email scrape
    admin_total = None
    if not args.skip_emails:
        print("Phase 2: Scraping admin panel for emails…")
        admin_session = create_session(args.cookie)
        emails, admin_total = scrape_admin_emails(admin_session, args.delay)

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
                print(f"  ✓ Admin count matches expected ({admin_total})")
            else:
                print(f"  ✗ Admin expected {admin_total}, got {len(emails)}")

        if admin_only_ids:
            print(f"  ✗ {len(admin_only_ids)} user(s) in admin but NOT in public list: {admin_only_ids[:10]}{'…' if len(admin_only_ids) > 10 else ''}")

        if users_without_email:
            print(f"  ✗ {len(users_without_email)} user(s) in public list but NOT in admin: {users_without_email[:10]}{'…' if len(users_without_email) > 10 else ''}")

        # Cross-validate totals
        if public_total is not None and admin_total is not None and public_total != admin_total:
            print(f"  ⚠ Public total ({public_total}) ≠ Admin total ({admin_total})")

        print()
    else:
        print("Phase 2: Skipped (--skip-emails)")
        print()

    # Phase 3: write CSV
    write_csv(users, args.output)
    abs_path = os.path.abspath(args.output)
    print(f"Wrote {len(users)} users to {abs_path}")

    # Final summary
    with_email = sum(1 for u in users.values() if u.get("email"))
    without_email = len(users) - with_email
    print()
    print("--- Summary ---")
    print(f"  Total users:      {len(users)}")
    if public_total is not None:
        print(f"  Expected (public): {public_total}")
    if admin_total is not None:
        print(f"  Expected (admin):  {admin_total}")
    print(f"  With email:       {with_email}")
    print(f"  Without email:    {without_email}")


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal_handler)
    main()
