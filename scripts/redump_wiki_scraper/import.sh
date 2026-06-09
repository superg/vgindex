#!/usr/bin/env bash
#
# Import scraped wiki.redump.org data into the local MediaWiki container.
#
# Reads XML files from the read-only /import/redump mount and runs
# importDump.php on one rewritten temporary file at a time.
#
# Usage:
#   bash scripts/redump_wiki_scraper/import.sh [--target-domain localhost]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PAGES_DIR="$PROJECT_ROOT/data/redump/wiki/pages"
CONTAINER_PAGES_DIR="/import/redump/wiki/pages"
TARGET_DOMAIN="localhost"
PHP_MEMORY_LIMIT="${WIKI_IMPORT_MEMORY_LIMIT:-1024M}"

usage() {
    echo "Usage:"
    echo "  bash scripts/redump_wiki_scraper/import.sh [--target-domain DOMAIN[:PORT]] [--pages-dir PATH] [--container-pages-dir PATH] [--php-memory-limit LIMIT]"
}

normalize_target_domain() {
    local domain="$1"
    domain="${domain#http://}"
    domain="${domain#https://}"
    domain="${domain%%/*}"
    if [ -z "$domain" ]; then
        echo "ERROR: --target-domain cannot be empty" >&2
        exit 1
    fi
    printf "%s" "$domain"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target-domain)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --target-domain requires a value" >&2
                exit 1
            fi
            TARGET_DOMAIN="$2"
            shift 2
            ;;
        --pages-dir)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --pages-dir requires a value" >&2
                exit 1
            fi
            PAGES_DIR="$2"
            shift 2
            ;;
        --container-pages-dir)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --container-pages-dir requires a value" >&2
                exit 1
            fi
            CONTAINER_PAGES_DIR="$2"
            shift 2
            ;;
        --php-memory-limit)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --php-memory-limit requires a value" >&2
                exit 1
            fi
            PHP_MEMORY_LIMIT="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

TARGET_DOMAIN="$(normalize_target_domain "$TARGET_DOMAIN")"

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

if ! docker exec "$CONTAINER" test -d "$CONTAINER_PAGES_DIR"; then
    echo "ERROR: MediaWiki container cannot see $CONTAINER_PAGES_DIR"
    echo "Recreate it after the new mount is in place: docker compose up -d --force-recreate mediawiki"
    exit 1
fi

TOTAL=$(find "$PAGES_DIR" -maxdepth 1 -name '*.xml' | wc -l)
echo "Importing $TOTAL pages from mounted Redump wiki XML..."
echo "Rewriting Redump links to target domain: $TARGET_DOMAIN"
echo "Using PHP memory limit: $PHP_MEMORY_LIMIT"

echo "Deleting default Main Page so the imported one takes its place..."
docker exec \
    -e PHP_MEMORY_LIMIT="$PHP_MEMORY_LIMIT" \
    "$CONTAINER" bash -c 'echo "Main Page" > /tmp/del.txt && php -d "memory_limit=$PHP_MEMORY_LIMIT" /var/www/html/maintenance/deleteBatch.php /tmp/del.txt 2>/dev/null; rm -f /tmp/del.txt'

echo ""

docker exec \
    -e TARGET_DOMAIN="$TARGET_DOMAIN" \
    -e IMPORT_SOURCE_DIR="$CONTAINER_PAGES_DIR" \
    -e PHP_MEMORY_LIMIT="$PHP_MEMORY_LIMIT" \
    "$CONTAINER" bash -c '
    set -euo pipefail

    rewrite_xml() {
        local source="$1"
        local target="$2"

        php -d "memory_limit=$PHP_MEMORY_LIMIT" -r "
function xml_url_escape(string \$url): string {
    return htmlspecialchars(\$url, ENT_NOQUOTES | ENT_SUBSTITUTE, \"UTF-8\");
}

function local_wiki_path(string \$title, string \$fragment): string {
    \$title = str_replace(\" \", \"_\", \$title);
    \$path = rawurlencode(\$title);
    \$path = str_replace([\"%2F\", \"%3A\"], [\"/\", \":\"], \$path);
    return \"/\" . \$path . \$fragment;
}

function build_url(array \$parts): string {
    \$url = (string) (\$parts[\"scheme\"] ?? \"https\") . \"://\" . (string) (\$parts[\"host\"] ?? \"\");
    if (!empty(\$parts[\"path\"])) {
        \$url .= (string) \$parts[\"path\"];
    }
    if (!empty(\$parts[\"query\"])) {
        \$url .= \"?\" . (string) \$parts[\"query\"];
    }
    if (!empty(\$parts[\"fragment\"])) {
        \$url .= \"#\" . (string) \$parts[\"fragment\"];
    }
    return \$url;
}

function rewrite_redump_url(string \$url, string \$target_domain): string {
    \$trailing = \"\";
    while (\$url !== \"\" && preg_match(\"~[.,;!?)]\\z~\", \$url)) {
        \$trailing = substr(\$url, -1) . \$trailing;
        \$url = substr(\$url, 0, -1);
    }

    \$decoded = html_entity_decode(\$url, ENT_QUOTES | ENT_HTML5, \"UTF-8\");
    \$parts = parse_url(\$decoded);
    if (!is_array(\$parts) || empty(\$parts[\"host\"])) {
        return \$url . \$trailing;
    }

    \$host = strtolower((string) \$parts[\"host\"]);
    if (\$host !== \"redump.org\" && !str_ends_with(\$host, \".redump.org\")) {
        return \$url . \$trailing;
    }

    \$path = (string) (\$parts[\"path\"] ?? \"\");
    if (\$host === \"wiki.redump.org\" && \$path === \"/index.php\" && !empty(\$parts[\"query\"])) {
        parse_str((string) \$parts[\"query\"], \$query);
        if (!empty(\$query[\"title\"])) {
            \$fragment = empty(\$parts[\"fragment\"]) ? \"\" : \"#\" . (string) \$parts[\"fragment\"];
            \$extra_query = \$query;
            unset(\$extra_query[\"title\"]);
            if (empty(\$extra_query)) {
                return xml_url_escape(local_wiki_path((string) \$query[\"title\"], \$fragment)) . \$trailing;
            }
            \$parts[\"scheme\"] = \"\";
            \$parts[\"host\"] = \"\";
            \$parts[\"path\"] = \"/index.php\";
            \$parts[\"query\"] = http_build_query(\$query, \"\", \"&\", PHP_QUERY_RFC3986);
            \$local = \$parts[\"path\"] . \"?\" . \$parts[\"query\"] . \$fragment;
            return xml_url_escape(\$local) . \$trailing;
        }
    }

    \$subdomain = \$host === \"redump.org\" ? \"\" : substr(\$host, 0, -strlen(\".redump.org\"));
    \$parts[\"scheme\"] = \"https\";
    \$parts[\"host\"] = \$subdomain === \"\" ? \$target_domain : \$subdomain . \".\" . \$target_domain;
    unset(\$parts[\"port\"]);
    return xml_url_escape(build_url(\$parts)) . \$trailing;
}

function write_all(\$handle, string \$value): bool {
    \$offset = 0;
    \$length = strlen(\$value);
    while (\$offset < \$length) {
        \$written = fwrite(\$handle, substr(\$value, \$offset));
        if (\$written === false || \$written === 0) {
            return false;
        }
        \$offset += \$written;
    }
    return true;
}

\$source = \$argv[1];
\$target = \$argv[2];
\$target_domain = \$argv[3];
\$pattern = \"~https?://(?:[A-Za-z0-9-]+\\.)*redump\\.org[^\\s<>\\x22\\x27\\[\\]{}|]*~i\";
\$in = fopen(\$source, \"rb\");
if (\$in === false) {
    fwrite(STDERR, \"Could not open \$source\n\");
    exit(1);
}
\$out = fopen(\$target, \"wb\");
if (\$out === false) {
    fclose(\$in);
    fwrite(STDERR, \"Could not open \$target for writing\n\");
    exit(1);
}
while ((\$line = fgets(\$in)) !== false) {
    \$rewritten = preg_replace_callback(
        \$pattern,
        static function (array \$matches) use (\$target_domain): string {
            return rewrite_redump_url(\$matches[0], \$target_domain);
        },
        \$line
    );
    if (\$rewritten === null || !write_all(\$out, \$rewritten)) {
        fclose(\$in);
        fclose(\$out);
        unlink(\$target);
        fwrite(STDERR, \"Could not write rewritten XML to \$target\n\");
        exit(1);
    }
}
if (!feof(\$in)) {
    fclose(\$in);
    fclose(\$out);
    unlink(\$target);
    fwrite(STDERR, \"Could not read \$source\n\");
    exit(1);
}
fclose(\$in);
if (!fclose(\$out)) {
    unlink(\$target);
    fwrite(STDERR, \"Could not finish writing rewritten XML to \$target\n\");
    exit(1);
}
" "$source" "$target" "$TARGET_DOMAIN"
    }

    COUNT=0
    FAILED=0
    TMP_FILE=/tmp/wiki_import_page.xml
    TOTAL=$(find "$IMPORT_SOURCE_DIR" -maxdepth 1 -name "*.xml" | wc -l)
    for f in "$IMPORT_SOURCE_DIR"/*.xml; do
        [ -e "$f" ] || break
        COUNT=$((COUNT + 1))
        if rewrite_xml "$f" "$TMP_FILE" && php -d "memory_limit=$PHP_MEMORY_LIMIT" /var/www/html/maintenance/importDump.php --username-prefix "" "$TMP_FILE" > /dev/null 2>&1; then
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
    rm -f "$TMP_FILE"
    echo "Rebuilding indexes..."
    php -d "memory_limit=$PHP_MEMORY_LIMIT" /var/www/html/maintenance/rebuildrecentchanges.php
    php -d "memory_limit=$PHP_MEMORY_LIMIT" /var/www/html/maintenance/initSiteStats.php --update
    echo "Done."
'
