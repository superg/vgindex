#!/usr/bin/env bash
#
# Import scraped wiki.redump.org data into the local MediaWiki container.
#
# Copies XML files into the running container and runs importDump.php
# on each file directly.
#
# Usage:
#   bash scripts/redump_wiki_scraper/import.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PAGES_DIR="$PROJECT_ROOT/data/redump/wiki/pages"

if [ ! -d "$PAGES_DIR" ]; then
    echo "ERROR: No scraped data found at $PAGES_DIR"
    echo "Run the scraper first: python scripts/redump_wiki_scraper/scrape.py"
    exit 1
fi

CONTAINER="$(docker compose ps -q mediawiki 2>/dev/null || true)"
if [ -z "$CONTAINER" ]; then
    echo "ERROR: mediawiki container is not running. Start it with: docker compose up -d"
    exit 1
fi

TOTAL=$(find "$PAGES_DIR" -name '*.xml' | wc -l)
echo "Copying $TOTAL XML files into container..."
docker cp "$PAGES_DIR" "$CONTAINER:/tmp/wiki_import"

echo "Deleting default Main Page so the imported one takes its place..."
docker exec "$CONTAINER" bash -c 'echo "Main Page" > /tmp/del.txt && php /var/www/html/maintenance/deleteBatch.php /tmp/del.txt 2>/dev/null; rm -f /tmp/del.txt'

echo "Importing $TOTAL pages..."
echo ""

docker exec "$CONTAINER" bash -c '
    COUNT=0
    FAILED=0
    TOTAL=$(find /tmp/wiki_import -name "*.xml" | wc -l)
    for f in /tmp/wiki_import/*.xml; do
        COUNT=$((COUNT + 1))
        if php /var/www/html/maintenance/importDump.php --username-prefix "" "$f" > /dev/null 2>&1; then
            printf "  [%d/%d] %s\n" "$COUNT" "$TOTAL" "$(basename "$f")"
        else
            printf "  [%d/%d] FAILED: %s\n" "$COUNT" "$TOTAL" "$(basename "$f")"
            FAILED=$((FAILED + 1))
        fi
    done
    echo ""
    echo "Import: $((COUNT - FAILED))/$COUNT succeeded."
    [ "$FAILED" -gt 0 ] && echo "$FAILED files failed."
    echo ""
    echo "Cleaning up..."
    rm -rf /tmp/wiki_import
    echo "Rebuilding indexes..."
    php /var/www/html/maintenance/rebuildrecentchanges.php
    php /var/www/html/maintenance/initSiteStats.php --update
    echo "Done."
'
