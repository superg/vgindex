#!/usr/bin/env python3
"""Populate Audio CD universal hashes from existing disc comments.

Usage:
    python scripts/fill_universal_hashes.py --user admin --dbname redump --port 15432
    python scripts/fill_universal_hashes.py --user admin --dbname redump --port 15432 --apply

The default mode is a dry run. Pass --apply to update discs.universal_hash.
"""

import argparse
import html
import os
import re
import sys
from dataclasses import dataclass, field


TAG_RE = re.compile(r"<[^>]+>")
SPACE_RE = re.compile(r"\s+")
UNIVERSAL_HASH_RE = re.compile(
    r"\buniversal\s+hash\b\s*(?:\(\s*sha-?1\s*\))?\s*:?\s*([0-9a-f]{40})\b",
    re.IGNORECASE | re.ASCII,
)
UNIVERSAL_HASH_LABEL_RE = re.compile(
    r"\buniversal\s+hash\b\s*(?:\(\s*sha-?1\s*\))?\s*:?",
    re.IGNORECASE | re.ASCII,
)


@dataclass(frozen=True)
class ParsedHash:
    status: str
    hash_hex: str | None = None
    matches: tuple[str, ...] = ()
    normalized_comments: str = ""


@dataclass(frozen=True)
class UpdateAction:
    disc_id: int
    hash_hex: str
    hash_bytes: bytes
    replacing_existing: bool = False


@dataclass(frozen=True)
class SkippedRow:
    disc_id: int
    reason: str
    detail: str


@dataclass
class UpdatePlan:
    scanned: int = 0
    extracted: int = 0
    unchanged: int = 0
    updates: list[UpdateAction] = field(default_factory=list)
    conflicts: list[SkippedRow] = field(default_factory=list)
    malformed: list[SkippedRow] = field(default_factory=list)
    ambiguous: list[SkippedRow] = field(default_factory=list)


def normalize_comments(comments: str | None) -> str:
    if not comments:
        return ""

    text = html.unescape(comments)
    text = TAG_RE.sub(" ", text)
    return SPACE_RE.sub(" ", text).strip()


def parse_universal_hash(comments: str | None) -> ParsedHash:
    normalized = normalize_comments(comments)
    matches = tuple(match.group(1).lower() for match in UNIVERSAL_HASH_RE.finditer(normalized))
    unique_matches = tuple(sorted(set(matches)))

    if len(unique_matches) == 1:
        return ParsedHash(
            status="valid",
            hash_hex=unique_matches[0],
            matches=unique_matches,
            normalized_comments=normalized,
        )

    if len(unique_matches) > 1:
        return ParsedHash(
            status="ambiguous",
            matches=unique_matches,
            normalized_comments=normalized,
        )

    if UNIVERSAL_HASH_LABEL_RE.search(normalized):
        return ParsedHash(status="malformed", normalized_comments=normalized)

    return ParsedHash(status="missing", normalized_comments=normalized)


def _existing_hash_bytes(value) -> bytes | None:
    if value is None:
        return None
    if isinstance(value, memoryview):
        return value.tobytes()
    return bytes(value)


def _snippet(value: str, width: int = 180) -> str:
    if len(value) <= width:
        return value
    return value[: width - 3] + "..."


def build_update_plan(rows, overwrite: bool = False) -> UpdatePlan:
    plan = UpdatePlan()

    for disc_id, comments, existing_hash in rows:
        plan.scanned += 1
        parsed = parse_universal_hash(comments)

        if parsed.status == "missing":
            continue

        if parsed.status == "malformed":
            plan.malformed.append(
                SkippedRow(
                    disc_id=disc_id,
                    reason="malformed",
                    detail=_snippet(parsed.normalized_comments),
                )
            )
            continue

        if parsed.status == "ambiguous":
            plan.ambiguous.append(
                SkippedRow(
                    disc_id=disc_id,
                    reason="ambiguous",
                    detail=", ".join(parsed.matches),
                )
            )
            continue

        plan.extracted += 1
        parsed_bytes = bytes.fromhex(parsed.hash_hex)
        existing_bytes = _existing_hash_bytes(existing_hash)

        if existing_bytes is None:
            plan.updates.append(
                UpdateAction(disc_id=disc_id, hash_hex=parsed.hash_hex, hash_bytes=parsed_bytes)
            )
        elif existing_bytes == parsed_bytes:
            plan.unchanged += 1
        elif overwrite:
            plan.updates.append(
                UpdateAction(
                    disc_id=disc_id,
                    hash_hex=parsed.hash_hex,
                    hash_bytes=parsed_bytes,
                    replacing_existing=True,
                )
            )
        else:
            plan.conflicts.append(
                SkippedRow(
                    disc_id=disc_id,
                    reason="conflict",
                    detail=f"existing={existing_bytes.hex()} parsed={parsed.hash_hex}",
                )
            )

    return plan


def fetch_candidate_rows(conn):
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT id, comments, universal_hash
            FROM discs
            WHERE system_code = %s
              AND comments ILIKE %s
            ORDER BY id
            """,
            ("AUDIO-CD", "%universal hash%"),
        )
        return cur.fetchall()


def apply_updates(conn, updates: list[UpdateAction]) -> None:
    if not updates:
        return

    with conn.cursor() as cur:
        cur.executemany(
            """
            UPDATE discs
            SET universal_hash = %s
            WHERE id = %s
              AND system_code = 'AUDIO-CD'
            """,
            [(update.hash_bytes, update.disc_id) for update in updates],
        )


def print_report(plan: UpdatePlan, applied: bool, report_limit: int) -> None:
    replacing = sum(1 for update in plan.updates if update.replacing_existing)
    new_updates = len(plan.updates) - replacing

    print(f"Audio-CD candidate rows scanned: {plan.scanned}")
    print(f"Rows with extractable universal hash: {plan.extracted}")
    print(f"Already populated with same hash: {plan.unchanged}")
    if applied:
        print(f"Rows updated: {len(plan.updates)}")
    else:
        print(f"Rows that would be updated: {len(plan.updates)}")
    print(f"  new values: {new_updates}")
    print(f"  replacements: {replacing}")
    print(f"Conflicts skipped: {len(plan.conflicts)}")
    print(f"Malformed mentions skipped: {len(plan.malformed)}")
    print(f"Ambiguous mentions skipped: {len(plan.ambiguous)}")

    for title, rows in (
        ("Conflicts", plan.conflicts),
        ("Malformed mentions", plan.malformed),
        ("Ambiguous mentions", plan.ambiguous),
    ):
        if not rows:
            continue

        print(f"\n{title} (first {min(report_limit, len(rows))}):")
        for row in rows[:report_limit]:
            print(f"  disc_id={row.disc_id} {row.reason}: {row.detail}")

    if not applied:
        print("\nDry run only. Pass --apply to update discs.universal_hash.")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Populate AUDIO-CD discs.universal_hash from existing comments."
    )
    parser.add_argument("--host", default=os.environ.get("PGHOST", "localhost"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("PGPORT", "5432")))
    parser.add_argument("--user", default=os.environ.get("PGUSER"))
    parser.add_argument("--password", default=os.environ.get("PGPASSWORD"))
    parser.add_argument("--dbname", default=os.environ.get("PGDATABASE"))
    parser.add_argument("--apply", action="store_true", help="update the database")
    parser.add_argument(
        "--overwrite",
        action="store_true",
        help="replace existing different universal_hash values",
    )
    parser.add_argument(
        "--report-limit",
        type=int,
        default=20,
        help="maximum skipped rows to print per category",
    )

    args = parser.parse_args(argv)
    missing = [name for name in ("user", "dbname") if getattr(args, name) is None]
    if missing:
        parser.error(
            "missing required connection option(s): "
            + ", ".join(f"--{name}" for name in missing)
        )
    return args


def connect(args: argparse.Namespace):
    try:
        import psycopg
    except ImportError as exc:
        raise SystemExit(
            "Missing dependency: psycopg. Install script dependencies with "
            "`python -m pip install -r scripts/requirements.txt`."
        ) from exc

    return psycopg.connect(
        host=args.host,
        port=args.port,
        user=args.user,
        password=args.password,
        dbname=args.dbname,
    )


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    conn = connect(args)

    try:
        rows = fetch_candidate_rows(conn)
        plan = build_update_plan(rows, overwrite=args.overwrite)
        if args.apply:
            apply_updates(conn, plan.updates)
            conn.commit()
        else:
            conn.rollback()
        print_report(plan, applied=args.apply, report_limit=args.report_limit)
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
