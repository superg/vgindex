#!/usr/bin/env python3
"""
Scrape wiki.redump.org with full edit history via the MediaWiki API and Special:Export.

Dumps all namespaces, preserves all revision history. Output is MediaWiki XML
dump files suitable for import via Special:Import or maintenance/importDump.php.

Usage:
    python scripts/redump_wiki_scraper/scrape.py
"""

import json
import re
import time
from pathlib import Path
from xml.etree import ElementTree as ET

import requests

WIKI_BASE = "http://wiki.redump.org"
API_URL = f"{WIKI_BASE}/api.php"
EXPORT_URL = f"{WIKI_BASE}/index.php?title=Special:Export"

OUTPUT_DIR = Path(__file__).resolve().parent.parent.parent / "data" / "redump" / "wiki"

BATCH_SIZE = 5
REQUEST_DELAY = 0.5
REQUEST_TIMEOUT = 120

MW_EXPORT_NS = "http://www.mediawiki.org/xml/export-0.4/"


def log(msg: str):
    print(msg, flush=True)


session = requests.Session()
session.headers.update({"User-Agent": "RedumpWikiScraper/1.0 (vgindex project)"})


def fetch_namespaces() -> dict[int, str]:
    """Fetch all content namespaces from the wiki."""
    resp = session.get(
        API_URL,
        params={"action": "query", "meta": "siteinfo", "siprop": "namespaces", "format": "json"},
        timeout=30,
    )
    resp.raise_for_status()
    namespaces = resp.json()["query"]["namespaces"]
    result = {}
    for ns_id_str, ns_info in namespaces.items():
        ns_id = int(ns_id_str)
        if ns_id < 0:
            continue
        label = ns_info.get("*", "") or "(main)"
        result[ns_id] = label
    return result


def fetch_all_pages(namespace: int) -> list[dict]:
    """Enumerate all pages in the given namespace via the allpages API."""
    pages = []
    params = {
        "action": "query",
        "list": "allpages",
        "aplimit": "500",
        "apnamespace": str(namespace),
        "format": "json",
    }

    while True:
        resp = session.get(API_URL, params=params, timeout=30)
        resp.raise_for_status()
        data = resp.json()

        batch = data.get("query", {}).get("allpages", [])
        pages.extend(batch)

        qc = data.get("query-continue", {}).get("allpages")
        if not qc:
            break
        params["apfrom"] = qc["apfrom"]
        time.sleep(REQUEST_DELAY)

    return pages


def sanitize_filename(title: str) -> str:
    """Convert a page title to a safe filename."""
    safe = re.sub(r'[<>:"/\\|?*]', "_", title)
    safe = safe.replace(" ", "_")
    if len(safe) > 200:
        safe = safe[:200]
    return safe


def export_page_batch(titles: list[str]) -> bytes:
    """Export a batch of pages with full history via Special:Export.

    Omitting 'curonly' tells MediaWiki to include all revisions.
    """
    data = {"pages": "\n".join(titles)}
    resp = session.post(EXPORT_URL, data=data, timeout=REQUEST_TIMEOUT)
    resp.raise_for_status()
    return resp.content


def split_xml_pages(xml_bytes: bytes) -> list[tuple[str, bytes]]:
    """Split a multi-page XML export into individual (title, xml_bytes) pairs."""
    results = []
    try:
        root = ET.fromstring(xml_bytes)
    except ET.ParseError as e:
        log(f"    WARNING: XML parse error: {e}")
        return results

    ns = {"mw": MW_EXPORT_NS}
    siteinfo = root.find("mw:siteinfo", ns)
    siteinfo_bytes = ET.tostring(siteinfo, encoding="unicode") if siteinfo is not None else ""

    for page_el in root.findall("mw:page", ns):
        title_el = page_el.find("mw:title", ns)
        title = title_el.text if title_el is not None else "unknown"
        page_xml = ET.tostring(page_el, encoding="unicode")

        full_xml = (
            f'<mediawiki xmlns="{MW_EXPORT_NS}" '
            f'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" '
            f'xsi:schemaLocation="{MW_EXPORT_NS} {MW_EXPORT_NS.rstrip("/")}.xsd" '
            f'version="0.4" xml:lang="en">\n'
            f"  {siteinfo_bytes}\n"
            f"  {page_xml}\n"
            f"</mediawiki>\n"
        )
        results.append((title, full_xml.encode("utf-8")))

    return results


def get_exported_titles(pages_dir: Path) -> set[str]:
    """Return the set of page titles already exported to disk."""
    exported = set()
    if not pages_dir.exists():
        return exported
    for f in pages_dir.iterdir():
        if f.suffix == ".xml":
            try:
                tree = ET.parse(f)
                ns = {"mw": MW_EXPORT_NS}
                title_el = tree.find(".//mw:page/mw:title", ns)
                if title_el is not None and title_el.text:
                    exported.add(title_el.text)
            except ET.ParseError:
                pass
    return exported


def export_namespace(namespace: int, ns_label: str, pages_dir: Path, already_exported: set[str]):
    """Export all pages in a namespace, skipping already-exported titles."""
    page_list_file = OUTPUT_DIR / f"page_list_ns{namespace}.json"

    if page_list_file.exists():
        with open(page_list_file) as f:
            pages = json.load(f)
    else:
        pages = fetch_all_pages(namespace)
        with open(page_list_file, "w") as f:
            json.dump(pages, f, indent=2)

    all_titles = [p["title"] for p in pages]
    titles_to_export = [t for t in all_titles if t not in already_exported]

    if not all_titles:
        log(f"  [{ns_label}] No pages found.")
        return 0

    if not titles_to_export:
        log(f"  [{ns_label}] All {len(all_titles)} pages already exported.")
        return 0

    log(f"  [{ns_label}] {len(titles_to_export)} pages to export ({len(all_titles) - len(titles_to_export)} already done)...")
    total_batches = (len(titles_to_export) + BATCH_SIZE - 1) // BATCH_SIZE
    exported_count = 0

    for batch_idx in range(0, len(titles_to_export), BATCH_SIZE):
        batch_num = batch_idx // BATCH_SIZE + 1
        batch = titles_to_export[batch_idx : batch_idx + BATCH_SIZE]

        log(f"    Batch {batch_num}/{total_batches}")

        xml_bytes = None
        for attempt in range(3):
            try:
                xml_bytes = export_page_batch(batch)
                break
            except requests.RequestException as e:
                if attempt < 2:
                    wait = 5 * (attempt + 1)
                    log(f"      ERROR: {e}. Retrying in {wait}s...")
                    time.sleep(wait)
                else:
                    log(f"      FAILED after 3 attempts: {e}")
                    log(f"      Skipped: {batch}")

        if xml_bytes is None:
            continue

        page_pairs = split_xml_pages(xml_bytes)

        for title, page_xml in page_pairs:
            filename = sanitize_filename(title) + ".xml"
            out_path = pages_dir / filename
            with open(out_path, "wb") as f:
                f.write(page_xml)
            already_exported.add(title)
            exported_count += 1

        time.sleep(REQUEST_DELAY)

    log(f"  [{ns_label}] Exported {exported_count} pages.")
    return exported_count


def main():
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    pages_dir = OUTPUT_DIR / "pages"
    pages_dir.mkdir(exist_ok=True)

    log("Fetching wiki namespace list...")
    namespaces = fetch_namespaces()
    log(f"  Found {len(namespaces)} namespaces: {', '.join(f'{k}={v}' for k, v in sorted(namespaces.items()))}")

    log("\nScanning already-exported pages...")
    already_exported = get_exported_titles(pages_dir)
    if already_exported:
        log(f"  {len(already_exported)} pages already on disk.")
    else:
        log("  Starting fresh.")

    log("\nExporting all namespaces with full history...\n")
    total_exported = 0
    for ns_id in sorted(namespaces.keys()):
        ns_label = namespaces[ns_id]
        total_exported += export_namespace(ns_id, ns_label or "(main)", pages_dir, already_exported)

    final_count = len(list(pages_dir.glob("*.xml")))
    total_size = sum(f.stat().st_size for f in pages_dir.glob("*.xml"))
    log(f"\nDone. {final_count} pages total in {pages_dir}")
    log(f"Total size: {total_size / 1024 / 1024:.1f} MB")


if __name__ == "__main__":
    main()
