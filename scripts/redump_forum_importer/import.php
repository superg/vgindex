#!/usr/bin/env php
<?php

declare(strict_types=1);

/**
 * Import a scraped Redump PunBB forum archive into phpBB 3.3.
 *
 * This script is intended to run inside the repo-managed phpBB container, after
 * phpBB has been installed and config.php exists.
 */

if (PHP_SAPI !== 'cli')
{
    fwrite(STDERR, "This importer must be run from the CLI.\n");
    exit(2);
}

$memory_limit = getenv('REDUMP_IMPORT_MEMORY_LIMIT') ?: '1024M';
if ($memory_limit !== '')
{
    ini_set('memory_limit', $memory_limit);
}

$phpbb_root_path = getenv('PHPBB_ROOT_PATH') ?: '/var/www/html/';
$phpbb_root_path = rtrim($phpbb_root_path, '/') . '/';
$phpEx = 'php';

define('IN_PHPBB', true);
require($phpbb_root_path . 'common.' . $phpEx);
require_once($phpbb_root_path . 'includes/functions_user.' . $phpEx);
require_once($phpbb_root_path . 'includes/functions_content.' . $phpEx);

if (isset($user))
{
    $user->session_begin();
    $auth->acl($user->data);
    $user->setup();
}

const USERS_CSV = 'users.csv';
const USER_DATA_DIR = 'data';
const SIGNATURE_FILE = 'signature.txt';
const USER_PLUS_GROUP = 'User+';
const IMPORTED_POST_IP = '0.0.0.0';
const TOPIC_INDEX_FILE = 'topic_index.json';

/**
 * Tiny table-name shim for phpBB constants that may not exist in all contexts.
 */
function table_name(string $constant, string $suffix): string
{
    global $table_prefix;
    return defined($constant) ? constant($constant) : $table_prefix . $suffix;
}

function usage(): void
{
    echo "Usage:\n";
    echo "  redump-forum-import --forum-data /import/redump/forum --users-dir /import/redump/users [--source-timezone UTC] [--target-domain localhost] [--dry-run]\n";
    echo "  redump-forum-import --finalize-only\n";
}

function parse_args(array $argv): array
{
    $opts = getopt('', [
        'forum-data:',
        'users-dir:',
        'source-timezone:',
        'target-domain:',
        'dry-run',
        'finalize-only',
        'help',
    ]);

    if (isset($opts['help']))
    {
        usage();
        exit(0);
    }

    $forum_data = isset($opts['forum-data']) ? rtrim((string) $opts['forum-data'], '/') : '';
    $users_dir = isset($opts['users-dir']) ? rtrim((string) $opts['users-dir'], '/') : '';
    $source_timezone = isset($opts['source-timezone']) ? (string) $opts['source-timezone'] : 'UTC';
    $target_domain = normalize_target_domain(isset($opts['target-domain']) ? (string) $opts['target-domain'] : 'localhost');
    $finalize_only = isset($opts['finalize-only']);
    $test_users_file = $users_dir !== '' ? $users_dir . '/test_users.json' : '';

    if (!$finalize_only && ($forum_data === '' || $users_dir === ''))
    {
        usage();
        throw new RuntimeException('--forum-data and --users-dir are required.');
    }

    try
    {
        new DateTimeZone($source_timezone);
    }
    catch (Exception $e)
    {
        throw new RuntimeException("Invalid --source-timezone: {$source_timezone}");
    }

    return [
        'forum_data' => $forum_data,
        'users_dir' => $users_dir,
        'source_timezone' => $source_timezone,
        'target_domain' => $target_domain,
        'test_users_file' => $test_users_file,
        'dry_run' => isset($opts['dry-run']),
        'finalize_only' => $finalize_only,
    ];
}

function normalize_target_domain(string $domain): string
{
    $domain = trim($domain);
    $domain = preg_replace('#^[a-z][a-z0-9+.-]*://#i', '', $domain) ?? $domain;
    $domain = preg_replace('#/.*$#', '', $domain) ?? $domain;
    $domain = trim($domain);
    if ($domain === '')
    {
        throw new RuntimeException('--target-domain cannot be empty.');
    }
    return $domain;
}

function require_dir(string $path, string $label): void
{
    if (!is_dir($path))
    {
        throw new RuntimeException("{$label} not found or not a directory: {$path}");
    }
}

function require_file_path(string $path, string $label): void
{
    if (!is_file($path))
    {
        throw new RuntimeException("{$label} not found: {$path}");
    }
}

function sql_value($value): string
{
    global $db;

    if ($value === null)
    {
        return 'NULL';
    }
    if (is_int($value) || is_float($value))
    {
        return (string) $value;
    }
    if (is_bool($value))
    {
        return $value ? '1' : '0';
    }
    return "'" . $db->sql_escape((string) $value) . "'";
}

function sql_insert(string $table, array $row): int
{
    global $db;
    $sql = 'INSERT INTO ' . $table . ' ' . $db->sql_build_array('INSERT', $row);
    $db->sql_query($sql);
    return (int) $db->sql_nextid();
}

function sql_update(string $table, array $row, string $where): void
{
    global $db;
    $sql = 'UPDATE ' . $table . ' SET ' . $db->sql_build_array('UPDATE', $row) . ' WHERE ' . $where;
    $db->sql_query($sql);
}

function sql_fetch_one(string $sql)
{
    global $db;
    $result = $db->sql_query($sql);
    $row = $db->sql_fetchrow($result);
    $db->sql_freeresult($result);
    if (!$row)
    {
        return null;
    }
    return reset($row);
}

function sql_fetch_row(string $sql): ?array
{
    global $db;
    $result = $db->sql_query_limit($sql, 1);
    $row = $db->sql_fetchrow($result);
    $db->sql_freeresult($result);
    return $row ?: null;
}

function sql_fetch_all(string $sql): array
{
    global $db;
    $rows = [];
    $result = $db->sql_query($sql);
    while ($row = $db->sql_fetchrow($result))
    {
        $rows[] = $row;
    }
    $db->sql_freeresult($result);
    return $rows;
}

function count_table(string $table): int
{
    return (int) sql_fetch_one('SELECT COUNT(*) AS c FROM ' . $table);
}

function progress_line(string $message): void
{
    echo $message . "\n";
    @ob_flush();
    @flush();
}

function truncate_text(string $text, int $length): string
{
    if (function_exists('truncate_string'))
    {
        return truncate_string($text, $length);
    }
    return mb_substr($text, 0, $length);
}

function clean_username(string $username): string
{
    return function_exists('utf8_clean_string')
        ? utf8_clean_string($username)
        : mb_strtolower($username);
}

function protected_phpbb_clean_usernames(): array
{
    static $usernames = [
        'Anonymous',
        'admin',
        'AdsBot [Google]',
        'Ahrefs [Bot]',
        'Alexa [Bot]',
        'Alta Vista [Bot]',
        'Amazon [Bot]',
        'Ask Jeeves [Bot]',
        'Baidu [Spider]',
        'Bing [Bot]',
        'DuckDuckGo [Bot]',
        'Exabot [Bot]',
        'FAST Enterprise [Crawler]',
        'FAST WebCrawler [Crawler]',
        'Francis [Bot]',
        'Gigabot [Bot]',
        'Google Adsense [Bot]',
        'Google Desktop',
        'Google Feedfetcher',
        'Google [Bot]',
        'Heise IT-Markt [Crawler]',
        'Heritrix [Crawler]',
        'IBM Research [Bot]',
        'ICCrawler - ICjobs',
        'ichiro [Crawler]',
        'Majestic-12 [Bot]',
        'Metager [Bot]',
        'MSN NewsBlogs',
        'MSN [Bot]',
        'MSNbot Media',
        'NG-Search [Bot]',
        'Nutch [Bot]',
        'Nutch/CVS [Bot]',
        'OmniExplorer [Bot]',
        'Online link [Validator]',
        'psbot [Picsearch]',
        'Seekport [Bot]',
        'Semrush [Bot]',
        'Sensis [Crawler]',
        'SEO Crawler',
        'Seoma [Crawler]',
        'SEOSearch [Crawler]',
        'Snappy [Bot]',
        'Steeler [Crawler]',
        'Synoo [Bot]',
        'Telekom [Bot]',
        'TurnitinBot [Bot]',
        'Voyager [Bot]',
        'W3 [Sitesearch]',
        'W3C [Linkcheck]',
        'W3C [Validator]',
        'WiseNut [Bot]',
        'YaCy [Bot]',
        'Yahoo MMCrawler [Bot]',
        'Yahoo Slurp [Bot]',
        'Yahoo [Bot]',
        'YahooSeeker [Bot]',
    ];
    static $clean = null;
    if ($clean === null)
    {
        $clean = [];
        foreach ($usernames as $username)
        {
            $clean[clean_username($username)] = true;
        }
    }
    return $clean;
}

function phpbb_username_for_source(string $username): string
{
    $username = trim($username);
    if (isset(protected_phpbb_clean_usernames()[clean_username($username)]))
    {
        return truncate_text('redump_' . $username, 255);
    }
    return $username;
}

function is_protected_phpbb_user(?array $user_row): bool
{
    if (!$user_row)
    {
        return false;
    }

    $user_id = (int) ($user_row['user_id'] ?? 0);
    $username_clean = (string) ($user_row['username_clean'] ?? '');
    return $user_id === ANONYMOUS || isset(protected_phpbb_clean_usernames()[$username_clean]);
}

function safe_basename(string $name, string $fallback): string
{
    $name = trim(str_replace(["\\", "/"], '_', $name));
    $name = preg_replace('/[\x00-\x1f\x7f]+/', '', $name);
    $name = preg_replace('/\s+/', ' ', $name);
    $name = trim($name, " .");
    return $name !== '' ? $name : $fallback;
}

function read_json_file(string $path): array
{
    $raw = file_get_contents($path);
    if ($raw === false)
    {
        throw new RuntimeException("Could not read JSON: {$path}");
    }

    $data = json_decode($raw, true);
    if (!is_array($data))
    {
        throw new RuntimeException("Invalid JSON: {$path}");
    }
    return $data;
}

function topic_paths(string $forum_data): array
{
    $paths = glob($forum_data . '/topics/*.json') ?: [];
    $paths = array_values(array_filter($paths, static function (string $path): bool {
        return is_file($path) && filesize($path) > 0;
    }));
    sort($paths, SORT_STRING);
    return $paths;
}

function load_topic_metadata(string $forum_data): array
{
    $path = $forum_data . '/' . TOPIC_INDEX_FILE;
    if (!is_file($path))
    {
        return [];
    }

    $data = read_json_file($path);
    $records = $data['topics'] ?? $data;
    if (!is_array($records))
    {
        return [];
    }

    $metadata = [];
    foreach ($records as $key => $record)
    {
        if (!is_array($record))
        {
            continue;
        }

        $topic_id = is_numeric((string) $key) ? (int) $key : 0;
        if ($topic_id <= 0 && isset($record['topic_id']))
        {
            $topic_id = (int) $record['topic_id'];
        }
        if ($topic_id <= 0)
        {
            continue;
        }

        $metadata[$topic_id] = $record;
    }

    return $metadata;
}

function topic_source_id(array $topic): int
{
    return (int) ($topic['topic_id'] ?? 0);
}

function topic_aux_metadata(array $topic, array $topic_metadata): array
{
    $topic_id = topic_source_id($topic);
    return $topic_id > 0 ? ($topic_metadata[$topic_id] ?? []) : [];
}

function value_missing($value): bool
{
    return $value === null || $value === '' || $value === [];
}

function merge_topic_aux_metadata(array $topic, array $metadata): array
{
    if (!$metadata)
    {
        return $topic;
    }

    $merged = $topic;
    foreach (['category_name', 'forum_name', 'subject', 'source_url', 'moved_to_topic_id'] as $key)
    {
        if ((!array_key_exists($key, $merged) || value_missing($merged[$key])) && !value_missing($metadata[$key] ?? null))
        {
            $merged[$key] = $metadata[$key];
        }
    }

    $forum_id = (int) ($merged['forum_id'] ?? 0);
    $metadata_forum_id = (int) ($metadata['forum_id'] ?? 0);
    if ($forum_id <= 0 && $metadata_forum_id > 0)
    {
        $merged['forum_id'] = $metadata_forum_id;
    }

    $flags = is_array($merged['flags'] ?? null) ? $merged['flags'] : [];
    $metadata_flags = is_array($metadata['flags'] ?? null) ? $metadata['flags'] : [];
    foreach ($metadata_flags as $key => $value)
    {
        if (!array_key_exists($key, $flags) && $value)
        {
            $flags[$key] = $value;
        }
    }
    $merged['flags'] = array_filter($flags);

    return $merged;
}

function read_users_csv(string $users_dir): array
{
    $path = $users_dir . '/' . USERS_CSV;
    require_file_path($path, 'users.csv');

    $handle = fopen($path, 'r');
    if (!$handle)
    {
        throw new RuntimeException("Could not open users.csv: {$path}");
    }

    $headers = fgetcsv($handle);
    if (!$headers)
    {
        throw new RuntimeException("users.csv has no header row: {$path}");
    }

    $headers[0] = preg_replace('/^\xEF\xBB\xBF/', '', (string) $headers[0]);
    $users_by_name = [];
    $users_by_id = [];

    while (($row = fgetcsv($handle)) !== false)
    {
        $record = [];
        foreach ($headers as $idx => $name)
        {
            $record[$name] = isset($row[$idx]) ? trim((string) $row[$idx]) : '';
        }
        if (($record['ID'] ?? '') === '' || ($record['Username'] ?? '') === '')
        {
            continue;
        }

        $id = (int) $record['ID'];
        $username = (string) $record['Username'];
        $email = (string) ($record['Email'] ?? '');
        if ($email === '')
        {
            $email = imported_email($username, $id);
        }

        $user = [
            'source_id' => $id,
            'username' => $username,
            'email' => $email,
            'title' => (string) ($record['Title'] ?? ''),
            'registration_date' => (string) ($record['Registration Date'] ?? ''),
            'stub' => false,
        ];
        $users_by_id[$id] = $user;
        $users_by_name[$username] = $user;
    }
    fclose($handle);

    return [$users_by_id, $users_by_name];
}

function imported_email(string $username, int $source_id = 0): string
{
    $seed = $source_id > 0 ? (string) $source_id : $username;
    return 'redump-' . substr(sha1($seed . ':' . $username), 0, 16) . '@imported.invalid';
}

function parse_registration_date(string $value, string $timezone): int
{
    if ($value === '')
    {
        return time();
    }

    $dt = DateTimeImmutable::createFromFormat('!Y-m-d', $value, timezone_for_name($timezone));
    return $dt ? $dt->getTimestamp() : time();
}

function timezone_for_name(string $timezone): DateTimeZone
{
    static $cache = [];
    if (!isset($cache[$timezone]))
    {
        $cache[$timezone] = new DateTimeZone($timezone);
    }
    return $cache[$timezone];
}

function parse_source_time(?string $value, int $file_mtime, string $timezone): int
{
    $value = trim((string) $value);
    if ($value === '')
    {
        return $file_mtime;
    }

    $tz = timezone_for_name($timezone);
    if (preg_match('/^(\d{2}-\d{2}-\d{4}\s+\d{1,2}:\d{2}\s+[ap]m)$/i', $value, $m))
    {
        $dt = DateTimeImmutable::createFromFormat('!m-d-Y g:i a', strtolower($m[1]), $tz);
        if ($dt)
        {
            return $dt->getTimestamp();
        }
    }

    if (preg_match('/^(Today|Yesterday)\s+(\d{1,2}:\d{2}\s+[ap]m)$/i', $value, $m))
    {
        $base = (new DateTimeImmutable('@' . $file_mtime))->setTimezone($tz);
        if (strcasecmp($m[1], 'Yesterday') === 0)
        {
            $base = $base->modify('-1 day');
        }
        $date = $base->format('m-d-Y') . ' ' . strtolower($m[2]);
        $dt = DateTimeImmutable::createFromFormat('!m-d-Y g:i a', $date, $tz);
        if ($dt)
        {
            return $dt->getTimestamp();
        }
    }

    return $file_mtime;
}

function parse_edit_info(array $post, int $file_mtime, string $timezone): array
{
    $edited_by = trim((string) ($post['edited_by'] ?? ''));
    $edited_at = trim((string) ($post['edited_at'] ?? ''));
    $edited_text = trim((string) ($post['edited_text'] ?? ''));

    if ($edited_text !== '' && ($edited_by === '' || $edited_at === ''))
    {
        $date_pattern = '(\d{2}-\d{2}-\d{4}\s+\d{1,2}:\d{2}\s+[ap]m|Today\s+\d{1,2}:\d{2}\s+[ap]m|Yesterday\s+\d{1,2}:\d{2}\s+[ap]m)';
        if (preg_match('/^\(edited by\s+(.+)\s+' . $date_pattern . '\)$/i', $edited_text, $m))
        {
            $edited_by = trim($m[1]);
            $edited_at = trim($m[2]);
        }
    }

    if ($edited_by === '' && $edited_at === '')
    {
        return ['time' => 0, 'editor' => '', 'text' => $edited_text];
    }

    return [
        'time' => parse_source_time($edited_at, $file_mtime, $timezone),
        'editor' => $edited_by,
        'text' => $edited_text,
    ];
}

function format_imported_html(string $html, array $source_topic_id_map, array $source_post_id_map, string $target_domain, string $mode = 'post'): array
{
    $source = punbb_html_to_phpbb_source($html, $source_topic_id_map, $source_post_id_map, $target_domain);
    return format_phpbb_source($source, $mode);
}

function format_phpbb_source(string $source, string $mode = 'post'): array
{
    $source = normalize_phpbb_source($source);
    if ($source === '')
    {
        return [
            'text' => '',
            'bbcode_uid' => '',
            'bbcode_bitfield' => '',
            'enable_bbcode' => 1,
            'enable_smilies' => 1,
            'enable_magic_url' => 1,
        ];
    }

    $uid = '';
    $bitfield = '';
    $flags = 0;
    generate_text_for_storage($source, $uid, $bitfield, $flags, true, true, true, true, true, true, true, $mode);

    return [
        'text' => $source,
        'bbcode_uid' => $uid,
        'bbcode_bitfield' => $bitfield,
        'enable_bbcode' => 1,
        'enable_smilies' => 1,
        'enable_magic_url' => 1,
    ];
}

function normalize_phpbb_source(string $source): string
{
    $source = str_replace(["\r\n", "\r"], "\n", $source);
    return trim($source);
}

function punbb_html_to_phpbb_source(string $html, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $html = trim($html);
    if ($html === '')
    {
        return '';
    }

    $doc = new DOMDocument('1.0', 'UTF-8');
    $previous = libxml_use_internal_errors(true);
    $doc->loadHTML('<?xml encoding="UTF-8"><div id="redump-import-root">' . $html . '</div>', LIBXML_HTML_NOIMPLIED | LIBXML_HTML_NODEFDTD);
    libxml_clear_errors();
    libxml_use_internal_errors($previous);

    $root = $doc->getElementById('redump-import-root');
    if (!$root)
    {
        return rewrite_redump_urls_in_text(html_entity_decode(strip_tags($html), ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8'), $source_topic_id_map, $source_post_id_map, $target_domain);
    }

    return normalize_phpbb_source(nodes_to_phpbb_source($root->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
}

function nodes_to_phpbb_source(DOMNodeList $nodes, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $source = '';
    foreach ($nodes as $node)
    {
        $source .= node_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain);
    }
    return $source;
}

function node_to_phpbb_source(DOMNode $node, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    if ($node instanceof DOMText)
    {
        return rewrite_redump_urls_in_text($node->nodeValue ?? '', $source_topic_id_map, $source_post_id_map, $target_domain);
    }
    if ($node instanceof DOMComment || !($node instanceof DOMElement))
    {
        return '';
    }

    $tag = strtolower($node->tagName);
    if (in_array($tag, ['script', 'style', 'iframe', 'object', 'embed', 'input', 'button', 'select', 'textarea'], true))
    {
        return '';
    }

    if (element_has_class($node, 'codebox'))
    {
        $code = rewrite_redump_urls_in_text(code_text_from_element($node), $source_topic_id_map, $source_post_id_map, $target_domain);
        return "[code]" . trim($code, "\r\n") . "[/code]\n\n";
    }

    if (element_has_class($node, 'quotebox'))
    {
        return quote_box_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain);
    }

    return match ($tag) {
        'p' => block_source(nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'br' => "\n",
        'strong', 'b' => wrap_bbcode('b', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'em', 'i' => wrap_bbcode('i', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'u' => wrap_bbcode('u', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        's', 'strike' => wrap_bbcode('s', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'span' => span_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain),
        'a' => link_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain),
        'img' => image_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain),
        'ul', 'ol' => list_to_phpbb_source($node, $source_topic_id_map, $source_post_id_map, $target_domain),
        'blockquote' => quote_to_phpbb_source('', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'div', 'dl' => block_source(nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'dt', 'dd', 'li' => trim(nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain)) . "\n",
        'h5' => block_source(wrap_bbcode('b', nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain))),
        'pre', 'code' => "[code]" . trim(rewrite_redump_urls_in_text($node->textContent ?? '', $source_topic_id_map, $source_post_id_map, $target_domain), "\r\n") . "[/code]\n\n",
        'table' => block_source(table_to_text($node, $source_topic_id_map, $source_post_id_map, $target_domain)),
        'tbody', 'thead', 'tr', 'td', 'th', 'cite' => nodes_to_phpbb_source($node->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain),
        default => rewrite_redump_urls_in_text($node->textContent ?? '', $source_topic_id_map, $source_post_id_map, $target_domain),
    };
}

function element_has_class(DOMElement $element, string $class): bool
{
    $classes = preg_split('/\s+/', $element->getAttribute('class')) ?: [];
    return in_array($class, $classes, true);
}

function block_source(string $source): string
{
    $source = trim($source);
    return $source === '' ? '' : $source . "\n\n";
}

function wrap_bbcode(string $tag, string $source): string
{
    return '[' . $tag . ']' . trim($source) . '[/' . $tag . ']';
}

function span_to_phpbb_source(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $source = nodes_to_phpbb_source($element->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain);
    if (element_has_class($element, 'bbu'))
    {
        return wrap_bbcode('u', $source);
    }
    return $source;
}

function link_to_phpbb_source(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $text = trim(nodes_to_phpbb_source($element->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
    $href = trim($element->getAttribute('href'));
    if ($href === '')
    {
        return $text;
    }

    $href = rewrite_redump_url($href, $source_topic_id_map, $source_post_id_map, $target_domain);
    if ($text === '' || $text === $href)
    {
        return '[url]' . $href . '[/url]';
    }
    return '[url=' . bbcode_attr($href) . ']' . $text . '[/url]';
}

function image_to_phpbb_source(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $src = trim($element->getAttribute('src'));
    if ($src === '')
    {
        return trim($element->getAttribute('alt'));
    }
    return '[img]' . rewrite_redump_url($src, $source_topic_id_map, $source_post_id_map, $target_domain) . '[/img]';
}

function list_to_phpbb_source(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $items = [];
    foreach ($element->childNodes as $child)
    {
        if ($child instanceof DOMElement && strtolower($child->tagName) === 'li')
        {
            $items[] = '[*]' . trim(nodes_to_phpbb_source($child->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
        }
    }

    if (!$items)
    {
        return block_source(nodes_to_phpbb_source($element->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
    }

    $list_tag = strtolower($element->tagName) === 'ol' ? 'list=1' : 'list';
    return "[" . $list_tag . "]\n" . implode("\n", $items) . "\n[/list]\n\n";
}

function quote_box_to_phpbb_source(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $author = '';
    foreach ($element->getElementsByTagName('cite') as $cite)
    {
        $author = trim(preg_replace('/\s+wrote:\s*$/i', '', $cite->textContent ?? '') ?? '');
        break;
    }

    foreach ($element->getElementsByTagName('blockquote') as $blockquote)
    {
        $body = nodes_to_phpbb_source($blockquote->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain);
        return quote_to_phpbb_source($author, $body);
    }

    return quote_to_phpbb_source($author, nodes_to_phpbb_source($element->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
}

function quote_to_phpbb_source(string $author, string $body): string
{
    $body = trim($body);
    if ($body === '')
    {
        return '';
    }
    $open = $author !== '' ? '[quote=' . bbcode_attr($author) . ']' : '[quote]';
    return $open . $body . '[/quote]' . "\n\n";
}

function table_to_text(DOMElement $element, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $rows = [];
    foreach ($element->getElementsByTagName('tr') as $tr)
    {
        $cells = [];
        foreach ($tr->childNodes as $cell)
        {
            if ($cell instanceof DOMElement && in_array(strtolower($cell->tagName), ['td', 'th'], true))
            {
                $cells[] = trim(nodes_to_phpbb_source($cell->childNodes, $source_topic_id_map, $source_post_id_map, $target_domain));
            }
        }
        if ($cells)
        {
            $rows[] = implode("\t", $cells);
        }
    }
    return implode("\n", $rows);
}

function code_text_from_element(DOMElement $element): string
{
    foreach ($element->getElementsByTagName('code') as $code)
    {
        return $code->textContent ?? '';
    }
    foreach ($element->getElementsByTagName('pre') as $pre)
    {
        return $pre->textContent ?? '';
    }
    return $element->textContent ?? '';
}

function bbcode_attr(string $value): string
{
    return str_replace(["\n", "\r", ']'], [' ', ' ', '&#93;'], $value);
}

function rewrite_redump_urls_in_text(string $text, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    return preg_replace_callback(
        '~https?://(?:[a-z0-9-]+\.)*redump\.org(?::[0-9]+)?(?:/[^\s<>"\']*)?~i',
        static function (array $matches) use ($source_topic_id_map, $source_post_id_map, $target_domain): string {
            $url = $matches[0];
            $trailing = '';
            while ($url !== '' && preg_match('/[.,;!?)]$/', $url))
            {
                $trailing = substr($url, -1) . $trailing;
                $url = substr($url, 0, -1);
            }
            return rewrite_redump_url($url, $source_topic_id_map, $source_post_id_map, $target_domain) . $trailing;
        },
        $text
    ) ?? $text;
}

function rewrite_redump_url(string $url, array $source_topic_id_map, array $source_post_id_map, string $target_domain): string
{
    $parts = parse_url(html_entity_decode($url, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8'));
    if (!is_array($parts) || empty($parts['host']))
    {
        return $url;
    }

    $host = strtolower((string) $parts['host']);
    if (!redump_host($host))
    {
        return $url;
    }

    if ($host === 'forum.redump.org')
    {
        $mapped = rewrite_forum_redump_url($parts, $source_topic_id_map, $source_post_id_map);
        if ($mapped !== null)
        {
            return $mapped;
        }
    }

    $subdomain = redump_subdomain($host);
    $parts['scheme'] = 'https';
    $parts['host'] = $subdomain === '' ? $target_domain : $subdomain . '.' . $target_domain;
    unset($parts['port']);
    return build_rewritten_url($parts);
}

function rewrite_forum_redump_url(array $parts, array $source_topic_id_map, array $source_post_id_map): ?string
{
    $path = (string) ($parts['path'] ?? '');
    $query = [];
    if (!empty($parts['query']))
    {
        parse_str((string) $parts['query'], $query);
    }

    $source_post_id = 0;
    if (preg_match('#^/post/(\d+)/?$#', $path, $m) || preg_match('#^/post(\d+)(?:\.html)?$#', $path, $m))
    {
        $source_post_id = (int) $m[1];
    }
    else if (isset($query['pid']))
    {
        $source_post_id = (int) $query['pid'];
    }

    if ($source_post_id > 0 && isset($source_post_id_map[$source_post_id]))
    {
        $post_id = (int) $source_post_id_map[$source_post_id];
        return '/viewtopic.php?p=' . $post_id . '#p' . $post_id;
    }

    $source_topic_id = 0;
    if (preg_match('#^/topic/(\d+)(?:/.*)?$#', $path, $m) || preg_match('#^/topic(\d+)(?:[-_].*)?(?:\.html)?$#', $path, $m))
    {
        $source_topic_id = (int) $m[1];
    }
    else if (isset($query['id']))
    {
        $source_topic_id = (int) $query['id'];
    }

    if ($source_topic_id > 0 && isset($source_topic_id_map[$source_topic_id]))
    {
        return '/viewtopic.php?t=' . (int) $source_topic_id_map[$source_topic_id];
    }

    return null;
}

function redump_host(string $host): bool
{
    return $host === 'redump.org' || str_ends_with($host, '.redump.org');
}

function redump_subdomain(string $host): string
{
    if ($host === 'redump.org')
    {
        return '';
    }
    return substr($host, 0, -strlen('.redump.org'));
}

function build_rewritten_url(array $parts): string
{
    $url = (string) ($parts['scheme'] ?? 'https') . '://' . (string) ($parts['host'] ?? '');
    if (!empty($parts['path']))
    {
        $url .= (string) $parts['path'];
    }
    if (!empty($parts['query']))
    {
        $url .= '?' . (string) $parts['query'];
    }
    if (!empty($parts['fragment']))
    {
        $url .= '#' . (string) $parts['fragment'];
    }
    return $url;
}

function text_for_search(string $html): string
{
    return trim(html_entity_decode(strip_tags($html), ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8'));
}

function scan_archive(string $forum_data, string $users_dir): array
{
    echo "Scanning import inputs...\n";
    require_dir($forum_data, 'Forum data directory');
    require_dir($forum_data . '/topics', 'Forum topics directory');
    require_dir($users_dir, 'Users directory');
    require_dir($users_dir . '/' . USER_DATA_DIR, 'Users data directory');

    echo "  Reading users.csv...\n";
    [$users_by_id, $users_by_name] = read_users_csv($users_dir);
    echo "  Loading topic metadata...\n";
    $topic_metadata = load_topic_metadata($forum_data);
    echo "  Finding topic JSON files...\n";
    $paths = topic_paths($forum_data);
    if (!$paths)
    {
        throw new RuntimeException("No non-empty topic JSON files found in {$forum_data}/topics");
    }
    echo "  Found " . count($paths) . " non-empty topic JSON file(s).\n";

    $forums = [];
    $authors = [];
    $stats = [
        'topics' => 0,
        'posts' => 0,
        'attachments' => 0,
        'skipped_empty_topics' => 0,
        'relative_dates' => 0,
        'missing_attachment_files' => [],
        'ambiguous_avatar_dirs' => [],
        'missing_signature_files' => [],
    ];

    $scanned = 0;
    echo "  Scanning topic JSON files...\n";
    foreach ($paths as $path)
    {
        $scanned++;
        $topic = read_json_file($path);
        $topic = merge_topic_aux_metadata($topic, topic_aux_metadata($topic, $topic_metadata));
        $posts = $topic['posts'] ?? [];
        if (!is_array($posts) || count($posts) === 0)
        {
            $stats['skipped_empty_topics']++;
            continue;
        }

        $stats['topics']++;
        $stats['posts'] += count($posts);

        $forum_id = (int) ($topic['forum_id'] ?? 0);
        $forum_key = forum_key((string) ($topic['category_name'] ?? ''), (string) ($topic['forum_name'] ?? ''), $forum_id);
        if (!isset($forums[$forum_key]))
        {
            $forums[$forum_key] = [
                'source_forum_id' => $forum_id,
                'category_name' => (string) ($topic['category_name'] ?? ''),
                'forum_name' => (string) ($topic['forum_name'] ?? ''),
            ];
        }

        foreach ($posts as $post)
        {
            $author = trim((string) ($post['author_name'] ?? ''));
            if ($author !== '')
            {
                $authors[$author] = true;
            }
            foreach (['posted_at', 'edited_at', 'edited_text'] as $date_field)
            {
                $value = (string) ($post[$date_field] ?? '');
                if (str_starts_with($value, 'Today') || str_starts_with($value, 'Yesterday') || str_contains($value, ' Today ') || str_contains($value, ' Yesterday '))
                {
                    $stats['relative_dates']++;
                }
            }
        }

        $attachments = $topic['attachments'] ?? [];
        if (is_array($attachments))
        {
            foreach ($attachments as $attachment)
            {
                $stats['attachments']++;
                $local = (string) ($attachment['local_path'] ?? '');
                if ($local === '' || !is_file($forum_data . '/' . $local))
                {
                    $stats['missing_attachment_files'][] = [
                        'topic' => (int) ($topic['topic_id'] ?? 0),
                        'attachment' => (string) ($attachment['attachment_id'] ?? ''),
                        'path' => $local,
                    ];
                }
            }
        }

        if ($scanned % 5000 === 0)
        {
            echo "  Scanned {$scanned}/" . count($paths) . " topic file(s)...\n";
        }
    }
    echo "  Scanned {$scanned}/" . count($paths) . " topic file(s).\n";

    $scanned_users = 0;
    echo "  Checking user assets...\n";
    foreach ($users_by_id as $user)
    {
        $scanned_users++;
        $asset_dir = user_asset_dir($users_dir, (int) $user['source_id']);
        $signature_path = $asset_dir . '/' . SIGNATURE_FILE;
        if (!is_file($signature_path))
        {
            $stats['missing_signature_files'][] = (int) $user['source_id'];
        }
        $avatars = avatar_files($asset_dir);
        if (count($avatars) > 1)
        {
            $stats['ambiguous_avatar_dirs'][] = $asset_dir;
        }

        if ($scanned_users % 500 === 0)
        {
            echo "  Checked {$scanned_users}/" . count($users_by_id) . " user asset dir(s)...\n";
        }
    }
    echo "  Checked {$scanned_users}/" . count($users_by_id) . " user asset dir(s).\n";

    echo "  Resolving missing post authors...\n";
    foreach (array_keys($authors) as $author)
    {
        $author = (string) $author;
        if (!isset($users_by_name[$author]))
        {
            $users_by_name[$author] = [
                'source_id' => 0,
                'username' => $author,
                'email' => imported_email($author),
                'title' => '',
                'registration_date' => '',
                'stub' => true,
            ];
        }
    }

    $stats['users_csv'] = count($users_by_id);
    $stats['users_total'] = count($users_by_name);
    $stats['stub_users'] = $stats['users_total'] - $stats['users_csv'];
    $stats['forums'] = count($forums);
    $stats['topic_metadata'] = count($topic_metadata);

    return [
        'paths' => $paths,
        'forums' => $forums,
        'topic_metadata' => $topic_metadata,
        'users_by_id' => $users_by_id,
        'users_by_name' => $users_by_name,
        'stats' => $stats,
    ];
}

function user_asset_dir(string $users_dir, int $user_id): string
{
    return $users_dir . '/' . USER_DATA_DIR . '/' . $user_id;
}

function avatar_files(string $asset_dir): array
{
    if (!is_dir($asset_dir))
    {
        return [];
    }
    $files = [];
    foreach (scandir($asset_dir) ?: [] as $name)
    {
        if ($name === '.' || $name === '..' || $name === SIGNATURE_FILE)
        {
            continue;
        }
        $path = $asset_dir . '/' . $name;
        if (is_file($path))
        {
            $files[] = $path;
        }
    }
    sort($files, SORT_STRING);
    return $files;
}

function forum_key(string $category, string $forum, int $forum_id): string
{
    return $category . "\0" . $forum . "\0" . $forum_id;
}

function print_preflight(array $scan): void
{
    $stats = $scan['stats'];
    echo "Preflight:\n";
    echo "  Topics:              {$stats['topics']}\n";
    echo "  Posts:               {$stats['posts']}\n";
    echo "  Attachments:         {$stats['attachments']}\n";
    echo "  Forums:              {$stats['forums']}\n";
    echo "  Users from CSV:      {$stats['users_csv']}\n";
    echo "  Stub users:          {$stats['stub_users']}\n";
    echo "  Topic metadata:      {$stats['topic_metadata']}\n";
    echo "  Relative date refs:  {$stats['relative_dates']}\n";
    echo "  Empty topics skipped: {$stats['skipped_empty_topics']}\n";
}

function assert_preflight_ok(array $scan): void
{
    $stats = $scan['stats'];
    if ($stats['missing_attachment_files'])
    {
        $sample = array_slice($stats['missing_attachment_files'], 0, 5);
        $lines = array_map(static function (array $item): string {
            return "topic {$item['topic']} attachment {$item['attachment']} ({$item['path']})";
        }, $sample);
        throw new RuntimeException("Missing attachment files:\n  " . implode("\n  ", $lines));
    }
    if ($stats['ambiguous_avatar_dirs'])
    {
        throw new RuntimeException("Multiple avatar candidates found in: " . implode(', ', array_slice($stats['ambiguous_avatar_dirs'], 0, 5)));
    }
}

function ensure_sample_or_empty_board(): void
{
    $forums_table = table_name('FORUMS_TABLE', 'forums');
    $topics_table = table_name('TOPICS_TABLE', 'topics');
    $posts_table = table_name('POSTS_TABLE', 'posts');

    $forum_count = count_table($forums_table);
    $topic_count = count_table($topics_table);
    $post_count = count_table($posts_table);

    if ($forum_count === 0 && $topic_count === 0 && $post_count === 0)
    {
        return;
    }

    if ($topic_count === 0 && $post_count === 0)
    {
        return;
    }

    if ($topic_count > 1 || $post_count > 1 || $forum_count > 2)
    {
        throw new RuntimeException("phpBB already contains forum content ({$forum_count} forums, {$topic_count} topics, {$post_count} posts). Refusing to import.");
    }

    $forum_names = sql_fetch_all('SELECT forum_name FROM ' . $forums_table . ' ORDER BY forum_id');
    foreach ($forum_names as $row)
    {
        if (!in_array($row['forum_name'], ['Your first category', 'Your first forum'], true))
        {
            throw new RuntimeException('phpBB forums are not the installer sample content. Refusing to import.');
        }
    }
}

function clear_sample_board(): void
{
    global $db;

    $tables = [
        table_name('ACL_GROUPS_TABLE', 'acl_groups') => 'forum_id > 0',
        table_name('ATTACHMENTS_TABLE', 'attachments') => '1=1',
        table_name('BOOKMARKS_TABLE', 'bookmarks') => '1=1',
        table_name('FORUMS_ACCESS_TABLE', 'forums_access') => '1=1',
        table_name('FORUMS_TRACK_TABLE', 'forums_track') => '1=1',
        table_name('FORUMS_WATCH_TABLE', 'forums_watch') => '1=1',
        table_name('POLL_OPTIONS_TABLE', 'poll_options') => '1=1',
        table_name('POLL_VOTES_TABLE', 'poll_votes') => '1=1',
        table_name('POSTS_TABLE', 'posts') => '1=1',
        table_name('REPORTS_TABLE', 'reports') => '1=1',
        table_name('SEARCH_RESULTS_TABLE', 'search_results') => '1=1',
        table_name('SEARCH_WORDMATCH_TABLE', 'search_wordmatch') => '1=1',
        table_name('SEARCH_WORDLIST_TABLE', 'search_wordlist') => '1=1',
        table_name('TOPICS_POSTED_TABLE', 'topics_posted') => '1=1',
        table_name('TOPICS_TRACK_TABLE', 'topics_track') => '1=1',
        table_name('TOPICS_WATCH_TABLE', 'topics_watch') => '1=1',
        table_name('TOPICS_TABLE', 'topics') => '1=1',
        table_name('FORUMS_TABLE', 'forums') => '1=1',
    ];

    foreach ($tables as $table => $where)
    {
        $db->sql_query('DELETE FROM ' . $table . ' WHERE ' . $where);
    }
}

function ensure_user_plus_group(): int
{
    $group_ids = get_group_ids(USER_PLUS_GROUP);
    if ($group_ids)
    {
        $keep_id = $group_ids[0];
        foreach (array_slice($group_ids, 1) as $duplicate_id)
        {
            delete_duplicate_group($duplicate_id);
        }
        return $keep_id;
    }

    return sql_insert(table_name('GROUPS_TABLE', 'groups'), [
        'group_type' => GROUP_CLOSED,
        'group_founder_manage' => 0,
        'group_skip_auth' => 0,
        'group_name' => USER_PLUS_GROUP,
        'group_desc' => 'VGIndex User+ role',
        'group_desc_bitfield' => '',
        'group_desc_options' => 7,
        'group_desc_uid' => '',
        'group_display' => 0,
        'group_receive_pm' => 0,
        'group_message_limit' => 0,
        'group_max_recipients' => 0,
    ]);
}

function get_group_ids(string $group_name): array
{
    global $db;
    static $cache = [];
    if (array_key_exists($group_name, $cache))
    {
        return $cache[$group_name];
    }

    $sql = 'SELECT group_id FROM ' . table_name('GROUPS_TABLE', 'groups')
        . " WHERE group_name = '" . $db->sql_escape($group_name) . "'"
        . ' ORDER BY group_id';
    $group_ids = array_map(static fn (array $row): int => (int) $row['group_id'], sql_fetch_all($sql));
    if ($group_ids)
    {
        $cache[$group_name] = $group_ids;
    }
    return $group_ids;
}

function delete_duplicate_group(int $group_id): void
{
    global $db;
    $db->sql_query('DELETE FROM ' . table_name('ACL_GROUPS_TABLE', 'acl_groups') . ' WHERE group_id = ' . $group_id);
    $db->sql_query('DELETE FROM ' . table_name('USER_GROUP_TABLE', 'user_group') . ' WHERE group_id = ' . $group_id);
    $db->sql_query('DELETE FROM ' . table_name('GROUPS_TABLE', 'groups') . ' WHERE group_id = ' . $group_id);
}

function get_group_id(string $group_name): int
{
    $group_ids = get_group_ids($group_name);
    return $group_ids[0] ?? 0;
}

function get_role_id(string $role_name): int
{
    global $db;
    static $cache = [];
    if (isset($cache[$role_name]))
    {
        return $cache[$role_name];
    }

    $sql = 'SELECT role_id FROM ' . table_name('ACL_ROLES_TABLE', 'acl_roles')
        . " WHERE role_name = '" . $db->sql_escape($role_name) . "'";
    $role_id = (int) sql_fetch_one($sql);
    if (!$role_id)
    {
        throw new RuntimeException("Could not find phpBB ACL role: {$role_name}");
    }
    $cache[$role_name] = $role_id;
    return $role_id;
}

function get_auth_option_id(string $auth_option): int
{
    global $db;
    static $cache = [];
    if (isset($cache[$auth_option]))
    {
        return $cache[$auth_option];
    }

    $sql = 'SELECT auth_option_id FROM ' . table_name('ACL_OPTIONS_TABLE', 'acl_options')
        . " WHERE auth_option = '" . $db->sql_escape($auth_option) . "'";
    $auth_option_id = (int) sql_fetch_one($sql);
    if (!$auth_option_id)
    {
        throw new RuntimeException("Could not find phpBB ACL option: {$auth_option}");
    }
    $cache[$auth_option] = $auth_option_id;
    return $auth_option_id;
}

function assign_forum_role(int $forum_id, string $group_name, string $role_name): void
{
    $group_id = get_group_id($group_name);
    if (!$group_id)
    {
        throw new RuntimeException("Could not find phpBB group: {$group_name}");
    }

    sql_insert(table_name('ACL_GROUPS_TABLE', 'acl_groups'), [
        'group_id' => $group_id,
        'forum_id' => $forum_id,
        'auth_option_id' => 0,
        'auth_role_id' => get_role_id($role_name),
        'auth_setting' => 0,
    ]);
}

function assign_forum_permission(int $forum_id, string $group_name, string $auth_option): void
{
    global $db;

    $group_id = get_group_id($group_name);
    if (!$group_id)
    {
        throw new RuntimeException("Could not find phpBB group: {$group_name}");
    }
    $auth_option_id = get_auth_option_id($auth_option);

    $db->sql_query(
        'DELETE FROM ' . table_name('ACL_GROUPS_TABLE', 'acl_groups')
        . ' WHERE group_id = ' . $group_id
        . ' AND forum_id = ' . $forum_id
        . ' AND auth_option_id = ' . $auth_option_id
    );
    sql_insert(table_name('ACL_GROUPS_TABLE', 'acl_groups'), [
        'group_id' => $group_id,
        'forum_id' => $forum_id,
        'auth_option_id' => $auth_option_id,
        'auth_role_id' => 0,
        'auth_setting' => 1,
    ]);
}

function forum_permission_profile(array $forum): string
{
    $category = strtolower((string) $forum['category_name']);
    $forum_name = strtolower((string) $forum['forum_name']);

    if (str_contains($forum_name, 'staff'))
    {
        return 'staff';
    }
    if (guest_topic_forum($forum_name))
    {
        return 'guest_topic';
    }
    if ($category === 'private' || guest_hidden_forum($forum_name))
    {
        return 'registered';
    }
    return 'public';
}

function guest_topic_forum(string $forum_name): bool
{
    return strtolower($forum_name) === 'guests & account requests';
}

function guest_hidden_forum(string $forum_name): bool
{
    static $forums = [
        'history for dumps' => true,
        'history for fixes' => true,
        'patches' => true,
        'discarded dumps' => true,
    ];
    return isset($forums[strtolower($forum_name)]);
}

function assign_permissions_for_profile(int $forum_id, string $profile, bool $category): void
{
    if ($profile === 'public' || $profile === 'guest_topic')
    {
        assign_forum_role($forum_id, 'GUESTS', 'ROLE_FORUM_READONLY');
        assign_forum_role($forum_id, 'BOTS', 'ROLE_FORUM_BOT');
        assign_forum_role($forum_id, 'REGISTERED', $category ? 'ROLE_FORUM_READONLY' : 'ROLE_FORUM_STANDARD');
        assign_forum_role($forum_id, USER_PLUS_GROUP, $category ? 'ROLE_FORUM_READONLY' : 'ROLE_FORUM_STANDARD');
        assign_forum_role($forum_id, 'GLOBAL_MODERATORS', 'ROLE_FORUM_FULL');
        assign_forum_role($forum_id, 'ADMINISTRATORS', 'ROLE_FORUM_FULL');
        if ($profile === 'guest_topic' && !$category)
        {
            assign_forum_permission($forum_id, 'GUESTS', 'f_post');
            assign_forum_permission($forum_id, 'GUESTS', 'f_noapprove');
        }
        return;
    }

    if ($profile === 'registered')
    {
        assign_forum_role($forum_id, 'REGISTERED', $category ? 'ROLE_FORUM_READONLY' : 'ROLE_FORUM_STANDARD');
        assign_forum_role($forum_id, USER_PLUS_GROUP, $category ? 'ROLE_FORUM_READONLY' : 'ROLE_FORUM_STANDARD');
        assign_forum_role($forum_id, 'GLOBAL_MODERATORS', 'ROLE_FORUM_FULL');
        assign_forum_role($forum_id, 'ADMINISTRATORS', 'ROLE_FORUM_FULL');
        return;
    }

    assign_forum_role($forum_id, 'GLOBAL_MODERATORS', 'ROLE_FORUM_FULL');
    assign_forum_role($forum_id, 'ADMINISTRATORS', 'ROLE_FORUM_FULL');
}

function category_order(string $category): int
{
    static $order = [
        'Private' => 0,
        'Redump Forum' => 1,
        'Others' => 2,
    ];
    return $order[$category] ?? 100;
}

function forum_order(string $category, string $forum_name): int
{
    static $order = [
        'Private' => [
            'Staff' => 0,
            'Dumpers' => 1,
        ],
        'Redump Forum' => [
            'News' => 0,
            'New Dumps' => 1,
            'Verifications' => 2,
            'Fixes & additions' => 3,
            'History for dumps' => 4,
            'History for fixes' => 5,
            'General discussion' => 6,
            'Guests & account requests' => 7,
        ],
        'Others' => [
            'Discarded dumps' => 0,
            'Patches' => 1,
        ],
    ];
    return $order[$category][$forum_name] ?? 100;
}

function category_permission_profile(array $forums): string
{
    $profile = 'staff';
    foreach ($forums as $forum)
    {
        $forum_profile = forum_permission_profile($forum);
        if ($forum_profile === 'public' || $forum_profile === 'guest_topic')
        {
            return 'public';
        }
        if ($forum_profile === 'registered')
        {
            $profile = 'registered';
        }
    }
    return $profile;
}

function import_forums(array $forums): array
{
    $categories = [];
    foreach ($forums as $forum)
    {
        $category = $forum['category_name'] !== '' ? $forum['category_name'] : 'Redump Forum';
        if (!isset($categories[$category]))
        {
            $categories[$category] = [];
        }
        $categories[$category][] = $forum;
    }

    uksort($categories, static function (string $a, string $b): int {
        return [category_order($a), $a] <=> [category_order($b), $b];
    });
    foreach ($categories as $category => &$category_forums)
    {
        usort($category_forums, static function (array $a, array $b) use ($category): int {
            return [
                forum_order($category, (string) $a['forum_name']),
                (int) $a['source_forum_id'],
                (string) $a['forum_name'],
            ] <=> [
                forum_order($category, (string) $b['forum_name']),
                (int) $b['source_forum_id'],
                (string) $b['forum_name'],
            ];
        });
    }
    unset($category_forums);

    $forum_ids = [];
    $left = 1;
    foreach ($categories as $category_name => $category_forums)
    {
        $category_left = $left++;
        $category_id = sql_insert(table_name('FORUMS_TABLE', 'forums'), [
            'parent_id' => 0,
            'left_id' => $category_left,
            'right_id' => 0,
            'forum_parents' => '',
            'forum_name' => truncate_text($category_name, 255),
            'forum_desc' => '',
            'forum_desc_bitfield' => '',
            'forum_desc_options' => 7,
            'forum_desc_uid' => '',
            'forum_type' => FORUM_CAT,
            'forum_status' => ITEM_UNLOCKED,
            'forum_flags' => FORUM_FLAG_POST_REVIEW,
            'display_on_index' => 1,
            'enable_indexing' => 1,
            'enable_icons' => 1,
            'display_subforum_list' => 1,
        ]);

        $category_profile = category_permission_profile($category_forums);
        foreach ($category_forums as $forum)
        {
            $profile = forum_permission_profile($forum);

            $forum_id = sql_insert(table_name('FORUMS_TABLE', 'forums'), [
                'parent_id' => $category_id,
                'left_id' => $left++,
                'right_id' => $left++,
                'forum_parents' => '',
                'forum_name' => truncate_text($forum['forum_name'], 255),
                'forum_desc' => '',
                'forum_desc_bitfield' => '',
                'forum_desc_options' => 7,
                'forum_desc_uid' => '',
                'forum_type' => FORUM_POST,
                'forum_status' => ITEM_UNLOCKED,
                'forum_flags' => FORUM_FLAG_POST_REVIEW,
                'display_on_index' => 1,
                'enable_indexing' => 1,
                'enable_icons' => 1,
                'display_subforum_list' => 1,
            ]);
            $forum_ids[forum_key($forum['category_name'], $forum['forum_name'], (int) $forum['source_forum_id'])] = $forum_id;
            assign_permissions_for_profile($forum_id, $profile, false);
        }

        sql_update(table_name('FORUMS_TABLE', 'forums'), ['right_id' => $left++], 'forum_id = ' . $category_id);
        assign_permissions_for_profile($category_id, $category_profile, true);
    }

    return $forum_ids;
}

function find_phpbb_user_by_clean(string $username): ?array
{
    global $db;
    $clean = clean_username($username);
    $sql = 'SELECT * FROM ' . table_name('USERS_TABLE', 'users')
        . " WHERE username_clean = '" . $db->sql_escape($clean) . "'";
    $rows = sql_fetch_all($sql);
    return $rows[0] ?? null;
}

function find_phpbb_user_by_email(string $email): ?array
{
    global $db;
    if ($email === '')
    {
        return null;
    }
    $sql = 'SELECT * FROM ' . table_name('USERS_TABLE', 'users')
        . " WHERE LOWER(user_email) = '" . $db->sql_escape(mb_strtolower($email)) . "'";
    $rows = sql_fetch_all($sql);
    return $rows[0] ?? null;
}

function inactive_reason_manual(): int
{
    return defined('INACTIVE_MANUAL') ? (int) constant('INACTIVE_MANUAL') : 3;
}

function source_user_role(array $source_user): string
{
    if (!empty($source_user['stub']))
    {
        return 'stub';
    }

    $title = trim(preg_replace('/\s+/', ' ', (string) ($source_user['title'] ?? '')) ?? '');
    $title = mb_strtolower($title);

    return match ($title) {
        'administrator', 't-11305h' => 'admin',
        'moderator', 'moderator (retired)', 'moderators (on break)' => 'moderator',
        'banned' => 'banned',
        default => 'user',
    };
}

function user_type_for_role(string $role): int
{
    return in_array($role, ['banned', 'stub'], true) ? USER_INACTIVE : USER_NORMAL;
}

function inactive_reason_for_role(string $role): int
{
    return in_array($role, ['banned', 'stub'], true) ? inactive_reason_manual() : 0;
}

function managed_role_groups(): array
{
    return ['GLOBAL_MODERATORS', 'ADMINISTRATORS', USER_PLUS_GROUP];
}

function add_user_to_group(int $user_id, string $group_name, bool $default = false): void
{
    $group_id = get_group_id($group_name);
    if (!$group_id)
    {
        throw new RuntimeException("Could not find phpBB group: {$group_name}");
    }

    group_user_add($group_id, [$user_id], false, false, $default);
    if ($default)
    {
        sql_update(table_name('USERS_TABLE', 'users'), ['group_id' => $group_id], 'user_id = ' . $user_id);
    }
}

function reset_managed_user_groups(int $user_id): void
{
    global $db;

    $group_ids = [];
    foreach (managed_role_groups() as $group_name)
    {
        $group_id = get_group_id($group_name);
        if ($group_id)
        {
            $group_ids[] = $group_id;
        }
    }

    if (!$group_ids)
    {
        return;
    }

    $db->sql_query(
        'DELETE FROM ' . table_name('USER_GROUP_TABLE', 'user_group')
        . ' WHERE user_id = ' . $user_id
        . ' AND ' . $db->sql_in_set('group_id', $group_ids)
    );
}

function apply_phpbb_role(int $user_id, string $role): void
{
    reset_managed_user_groups($user_id);

    $default_group = 'REGISTERED';
    add_user_to_group($user_id, 'REGISTERED', false);

    if ($role === 'admin')
    {
        add_user_to_group($user_id, 'GLOBAL_MODERATORS');
        add_user_to_group($user_id, 'ADMINISTRATORS');
        $default_group = 'ADMINISTRATORS';
    }
    else if ($role === 'moderator')
    {
        add_user_to_group($user_id, 'GLOBAL_MODERATORS');
        $default_group = 'GLOBAL_MODERATORS';
    }
    else if ($role === 'userplus')
    {
        add_user_to_group($user_id, USER_PLUS_GROUP);
        $default_group = USER_PLUS_GROUP;
    }

    add_user_to_group($user_id, $default_group, true);
}

function upsert_user_bans(int $user_id, string $email): void
{
    $banlist_table = table_name('BANLIST_TABLE', 'banlist');
    $now = time();

    $existing_user_ban = (int) sql_fetch_one('SELECT COUNT(*) FROM ' . $banlist_table . ' WHERE ban_userid = ' . $user_id);
    if ($existing_user_ban === 0)
    {
        sql_insert($banlist_table, [
            'ban_userid' => $user_id,
            'ban_ip' => '',
            'ban_email' => '',
            'ban_start' => $now,
            'ban_end' => 0,
            'ban_exclude' => 0,
            'ban_reason' => 'Imported Redump banned user',
            'ban_give_reason' => 'Imported Redump banned user',
        ]);
    }

    if ($email !== '')
    {
        global $db;
        $existing_email_ban = (int) sql_fetch_one(
            'SELECT COUNT(*) FROM ' . $banlist_table
            . " WHERE LOWER(ban_email) = '" . $db->sql_escape(mb_strtolower($email)) . "'"
        );
        if ($existing_email_ban === 0)
        {
            sql_insert($banlist_table, [
                'ban_userid' => 0,
                'ban_ip' => '',
                'ban_email' => $email,
                'ban_start' => $now,
                'ban_end' => 0,
                'ban_exclude' => 0,
                'ban_reason' => 'Imported Redump banned email',
                'ban_give_reason' => 'Imported Redump banned email',
            ]);
        }
    }
}

function upsert_phpbb_user(array $source_user, string $role, string $timezone, ?string $plain_password = null): int
{
    global $config, $phpbb_container;

    $passwords = $phpbb_container->get('passwords.manager');
    $registered_group_id = get_group_id('REGISTERED') ?: 2;

    $source_username = trim((string) ($source_user['username'] ?? ''));
    if ($source_username === '')
    {
        throw new RuntimeException('Cannot import a phpBB user with an empty username.');
    }

    $source_id = (int) ($source_user['source_id'] ?? 0);
    $username = phpbb_username_for_source($source_username);
    $email = trim((string) ($source_user['email'] ?? ''));
    if ($email === '')
    {
        $email = imported_email($source_username, $source_id);
    }

    $existing = find_phpbb_user_by_clean($username);
    if (is_protected_phpbb_user($existing))
    {
        $existing = null;
    }
    if (!$existing && $email !== '')
    {
        $existing = find_phpbb_user_by_email($email);
        if (is_protected_phpbb_user($existing))
        {
            $existing = null;
        }
    }

    $user_type = user_type_for_role($role);
    $inactive_reason = inactive_reason_for_role($role);
    $inactive_time = $inactive_reason ? time() : 0;
    $password = $plain_password ?? bin2hex(random_bytes(32));

    if ($existing)
    {
        $user_id = (int) $existing['user_id'];
        $update = [
            'user_email' => $email,
            'user_type' => $user_type,
            'user_inactive_reason' => $inactive_reason,
            'user_inactive_time' => $inactive_time,
            'user_new' => 0,
        ];
        if ($plain_password !== null || in_array($role, ['banned', 'stub'], true))
        {
            $update['user_password'] = $passwords->hash($password);
        }
        sql_update(table_name('USERS_TABLE', 'users'), $update, 'user_id = ' . $user_id);
    }
    else
    {
        $regdate = parse_registration_date((string) ($source_user['registration_date'] ?? ''), $timezone);
        $row = [
            'username' => $username,
            'user_password' => $passwords->hash($password),
            'user_email' => $email,
            'group_id' => $registered_group_id,
            'user_type' => $user_type,
            'user_inactive_reason' => $inactive_reason,
            'user_inactive_time' => $inactive_time,
            'user_regdate' => $regdate,
            'user_ip' => IMPORTED_POST_IP,
            'user_lang' => (string) $config['default_lang'],
            'user_style' => (int) $config['default_style'],
            'user_timezone' => (string) $config['board_timezone'],
            'user_new' => 0,
        ];

        $user_id = (int) user_add($row);
        if (!$user_id)
        {
            throw new RuntimeException("phpBB refused to create user: {$username}");
        }
    }

    apply_phpbb_role($user_id, $role);
    if ($role === 'banned')
    {
        upsert_user_bans($user_id, $email);
    }

    return $user_id;
}

function create_or_update_users(array $users_by_name, array $users_by_id, string $users_dir, string $timezone, string $target_domain): array
{
    $user_ids = [];
    $total_users = count($users_by_name);
    $done_users = 0;
    progress_line("  Importing {$total_users} user account(s)...");

    foreach ($users_by_name as $username => $source_user)
    {
        $done_users++;
        $username = (string) ($source_user['username'] ?? $username);
        if ($done_users === 1 || $done_users % 100 === 0)
        {
            progress_line("  Importing user {$done_users}/{$total_users}: {$username}");
        }

        $role = source_user_role($source_user);
        $user_id = upsert_phpbb_user($source_user, $role, $timezone);
        $user_ids[$username] = $user_id;
    }
    progress_line("  Imported {$done_users}/{$total_users} user account(s).");

    $total_assets = count($users_by_id);
    $done_assets = 0;
    progress_line("  Importing {$total_assets} user asset set(s)...");
    foreach ($users_by_id as $source_id => $source_user)
    {
        $done_assets++;
        $username = (string) ($source_user['username'] ?? '');
        if ($done_assets === 1 || $done_assets % 250 === 0)
        {
            progress_line("  Importing user assets {$done_assets}/{$total_assets}: {$username}");
        }

        if (!isset($user_ids[$username]))
        {
            continue;
        }
        import_user_assets($user_ids[$username], (int) $source_id, $users_dir, $target_domain);
    }
    progress_line("  Imported {$done_assets}/{$total_assets} user asset set(s).");

    return $user_ids;
}

function seed_test_users(string $test_users_file, string $timezone): array
{
    $definitions = load_test_user_definitions($test_users_file);

    $user_ids = [];
    foreach ($definitions as $username => $definition)
    {
        $existing = find_phpbb_user_by_clean($username);
        $email = $existing ? (string) $existing['user_email'] : (string) $definition['email'];
        $source_user = [
            'source_id' => 0,
            'username' => $username,
            'email' => $email,
            'registration_date' => '',
            'title' => '',
            'stub' => false,
        ];
        $user_ids[$username] = upsert_phpbb_user($source_user, (string) $definition['role'], $timezone, (string) $definition['password']);
    }

    return $user_ids;
}

function load_test_user_definitions(string $test_users_file): array
{
    if ($test_users_file === '' || !is_file($test_users_file))
    {
        return [];
    }

    $decoded = json_decode((string) file_get_contents($test_users_file), true);
    if (!is_array($decoded))
    {
        throw new RuntimeException("Invalid test users JSON: {$test_users_file}");
    }

    $definitions = [];
    foreach ($decoded as $username => $definition)
    {
        if (!is_string($username) || trim($username) === '' || !is_array($definition))
        {
            throw new RuntimeException("Invalid test user entry in {$test_users_file}");
        }

        $password = (string) ($definition['password'] ?? '');
        $role = (string) ($definition['role'] ?? '');
        $email = (string) ($definition['email'] ?? '');

        if ($password === '')
        {
            throw new RuntimeException("Test user {$username} is missing a password.");
        }
        if (!in_array($role, ['user', 'userplus', 'moderator', 'admin'], true))
        {
            throw new RuntimeException("Test user {$username} has invalid role: {$role}");
        }
        if ($email === '' || !preg_match('/^[^@\s]+@[^@\s]+$/', $email))
        {
            throw new RuntimeException("Test user {$username} has invalid email: {$email}");
        }

        $definitions[$username] = [
            'password' => $password,
            'role' => $role,
            'email' => $email,
        ];
    }

    return $definitions;
}

function import_user_assets(int $user_id, int $source_id, string $users_dir, string $target_domain): void
{
    $asset_dir = user_asset_dir($users_dir, $source_id);
    $update = signature_update_for_user($source_id, $users_dir, [], [], $target_domain);

    $avatars = avatar_files($asset_dir);
    if (count($avatars) === 1)
    {
        $avatar = import_avatar_file($user_id, $avatars[0]);
        $update = array_merge($update, $avatar);
    }

    sql_update(table_name('USERS_TABLE', 'users'), $update, 'user_id = ' . $user_id);
}

function signature_update_for_user(int $source_id, string $users_dir, array $source_topic_id_map, array $source_post_id_map, string $target_domain): array
{
    $signature_path = user_asset_dir($users_dir, $source_id) . '/' . SIGNATURE_FILE;
    $signature = format_phpbb_source('', 'sig');
    if (is_file($signature_path) && filesize($signature_path) > 0)
    {
        $signature = format_imported_html((string) file_get_contents($signature_path), $source_topic_id_map, $source_post_id_map, $target_domain, 'sig');
    }

    return [
        'user_sig' => $signature['text'],
        'user_sig_bbcode_uid' => $signature['bbcode_uid'],
        'user_sig_bbcode_bitfield' => $signature['bbcode_bitfield'],
    ];
}

function rewrite_imported_user_signatures(array $users_by_id, array $user_ids, string $users_dir, array $source_topic_id_map, array $source_post_id_map, string $target_domain): int
{
    $updated = 0;
    foreach ($users_by_id as $source_id => $source_user)
    {
        $username = (string) ($source_user['username'] ?? '');
        if ($username === '' || !isset($user_ids[$username]))
        {
            continue;
        }

        $update = signature_update_for_user((int) $source_id, $users_dir, $source_topic_id_map, $source_post_id_map, $target_domain);
        sql_update(table_name('USERS_TABLE', 'users'), $update, 'user_id = ' . (int) $user_ids[$username]);
        $updated++;
    }
    return $updated;
}

function import_avatar_file(int $user_id, string $source_path): array
{
    global $config, $phpbb_root_path;

    $info = @getimagesize($source_path);
    if (!$info)
    {
        throw new RuntimeException("Avatar is not a readable image: {$source_path}");
    }

    $ext = match ($info[2]) {
        IMAGETYPE_GIF => 'gif',
        IMAGETYPE_JPEG => 'jpg',
        IMAGETYPE_PNG => 'png',
        default => '',
    };
    if ($ext === '')
    {
        return [
            'user_avatar' => '',
            'user_avatar_type' => '',
            'user_avatar_width' => 0,
            'user_avatar_height' => 0,
        ];
    }

    $avatar_dir = $phpbb_root_path . trim((string) $config['avatar_path'], '/');
    if (!is_dir($avatar_dir) && !mkdir($avatar_dir, 0775, true) && !is_dir($avatar_dir))
    {
        throw new RuntimeException("Could not create phpBB avatar directory: {$avatar_dir}");
    }

    $filename = $user_id . '_' . time() . '.' . $ext;
    $physical = (string) $config['avatar_salt'] . '_' . $user_id . '.' . $ext;
    $dest = $avatar_dir . '/' . $physical;
    if (!copy($source_path, $dest))
    {
        throw new RuntimeException("Could not copy avatar {$source_path} to {$dest}");
    }

    return [
        'user_avatar' => $filename,
        'user_avatar_type' => 'avatar.driver.upload',
        'user_avatar_width' => (int) $info[0],
        'user_avatar_height' => (int) $info[1],
    ];
}

function import_topics(array $paths, string $forum_data, array $forum_ids, array $user_ids, array $topic_metadata, string $timezone, string $target_domain): array
{
    global $db;

    $stats = [
        'topics' => 0,
        'posts' => 0,
        'attachments' => 0,
        'skipped_empty_topics' => 0,
        'rewritten_posts' => 0,
        'unmapped_forum_links' => 0,
        'source_topic_id_map' => [],
        'source_post_id_map' => [],
    ];
    $total = count($paths);
    $done = 0;
    $source_topic_id_map = [];
    $source_post_id_map = [];
    $rewrite_candidates = [];
    $transaction_open = false;

    try
    {
        $db->sql_transaction('begin');
        $transaction_open = true;

        foreach ($paths as $path)
        {
            $done++;
            $topic = read_json_file($path);
            $source_metadata = topic_aux_metadata($topic, $topic_metadata);
            $topic = merge_topic_aux_metadata($topic, $source_metadata);
            $posts = $topic['posts'] ?? [];
            if (!is_array($posts) || count($posts) === 0)
            {
                $stats['skipped_empty_topics']++;
                continue;
            }

            $forum_key = forum_key(
                (string) ($topic['category_name'] ?? ''),
                (string) ($topic['forum_name'] ?? ''),
                (int) ($topic['forum_id'] ?? 0)
            );
            if (!isset($forum_ids[$forum_key]))
            {
                throw new RuntimeException("No imported phpBB forum for source topic {$topic['topic_id']}");
            }
            $forum_id = $forum_ids[$forum_key];
            $file_mtime = filemtime($path) ?: time();

            $attachments_by_post = [];
            foreach (($topic['attachments'] ?? []) as $attachment)
            {
                $source_post_id = (int) ($attachment['post_id'] ?? 0);
                $attachments_by_post[$source_post_id][] = $attachment;
            }

            $topic_id = import_topic(
                $topic,
                $posts,
                $attachments_by_post,
                $forum_data,
                $forum_id,
                $user_ids,
                $source_metadata,
                $file_mtime,
                $timezone,
                $target_domain,
                $source_topic_id_map,
                $source_post_id_map,
                $rewrite_candidates
            );
            $stats['topics']++;
            $stats['posts'] += count($posts);
            $stats['attachments'] += count($topic['attachments'] ?? []);

            if ($done % 500 === 0)
            {
                $db->sql_transaction('commit');
                $transaction_open = false;
                echo "  Imported {$done}/{$total} topic file(s); latest phpBB topic {$topic_id}\n";
                $db->sql_transaction('begin');
                $transaction_open = true;
            }
        }

        $db->sql_transaction('commit');
        $transaction_open = false;
    }
    catch (Throwable $e)
    {
        if ($transaction_open)
        {
            $db->sql_transaction('rollback');
        }
        throw $e;
    }

    echo "  Rewriting mapped forum links in imported post bodies...\n";
    $rewrite_stats = rewrite_imported_post_bodies($rewrite_candidates, $source_topic_id_map, $source_post_id_map, $target_domain);
    $stats['rewritten_posts'] = $rewrite_stats['rewritten_posts'];
    $stats['unmapped_forum_links'] = $rewrite_stats['unmapped_forum_links'];
    $stats['source_topic_id_map'] = $source_topic_id_map;
    $stats['source_post_id_map'] = $source_post_id_map;

    return $stats;
}

function import_topic(
    array $topic,
    array $posts,
    array $attachments_by_post,
    string $forum_data,
    int $forum_id,
    array $user_ids,
    array $source_metadata,
    int $file_mtime,
    string $timezone,
    string $target_domain,
    array &$source_topic_id_map,
    array &$source_post_id_map,
    array &$rewrite_candidates
): int {
    $subject = truncate_text((string) ($topic['subject'] ?? '(no subject)'), 120);
    $first_post = $posts[0];
    $first_author = (string) ($first_post['author_name'] ?? '');
    $first_user_id = $user_ids[$first_author] ?? ANONYMOUS;
    $first_time = parse_source_time((string) ($first_post['posted_at'] ?? ''), $file_mtime, $timezone);
    $flags = is_array($topic['flags'] ?? null) ? $topic['flags'] : [];
    $topic_type = !empty($flags['sticky']) ? POST_STICKY : POST_NORMAL;
    $topic_status = !empty($flags['closed']) ? ITEM_LOCKED : ITEM_UNLOCKED;
    $view_count = source_view_count($source_metadata);

    $topic_id = sql_insert(table_name('TOPICS_TABLE', 'topics'), [
        'forum_id' => $forum_id,
        'icon_id' => 0,
        'topic_attachment' => !empty($topic['attachments']) ? 1 : 0,
        'topic_reported' => 0,
        'topic_title' => $subject,
        'topic_poster' => $first_user_id,
        'topic_time' => $first_time,
        'topic_time_limit' => 0,
        'topic_views' => $view_count,
        'topic_status' => $topic_status,
        'topic_type' => $topic_type,
        'topic_first_post_id' => 0,
        'topic_first_poster_name' => $first_author,
        'topic_first_poster_colour' => '',
        'topic_last_post_id' => 0,
        'topic_last_poster_id' => $first_user_id,
        'topic_last_poster_name' => $first_author,
        'topic_last_poster_colour' => '',
        'topic_last_post_subject' => $subject,
        'topic_last_post_time' => $first_time,
        'topic_last_view_time' => $first_time,
        'topic_moved_id' => 0,
        'topic_bumped' => 0,
        'topic_bumper' => 0,
        'topic_visibility' => ITEM_APPROVED,
        'topic_posts_approved' => count($posts),
        'topic_posts_unapproved' => 0,
        'topic_posts_softdeleted' => 0,
    ]);
    $source_topic_id = (int) ($topic['topic_id'] ?? 0);
    if ($source_topic_id > 0)
    {
        $source_topic_id_map[$source_topic_id] = $topic_id;
    }

    $first_post_id = 0;
    $last_post_id = 0;
    $last_post_time = 0;
    $last_poster_id = $first_user_id;
    $last_poster_name = $first_author;
    $has_attachment = false;

    foreach ($posts as $idx => $post)
    {
        $source_post_id = (int) ($post['post_id'] ?? 0);
        $post_author = (string) ($post['author_name'] ?? '');
        $poster_id = $user_ids[$post_author] ?? ANONYMOUS;
        $post_time = parse_source_time((string) ($post['posted_at'] ?? ''), $file_mtime, $timezone);
        $message_html = (string) ($post['message_html'] ?? '');
        $message = format_imported_html($message_html, $source_topic_id_map, $source_post_id_map, $target_domain);
        $post_subject = $idx === 0 ? $subject : truncate_text('Re: ' . $subject, 120);
        $edit = parse_edit_info($post, $file_mtime, $timezone);
        $edit_user = $edit['editor'] !== '' && isset($user_ids[$edit['editor']]) ? $user_ids[$edit['editor']] : 0;

        $post_id = sql_insert(table_name('POSTS_TABLE', 'posts'), [
            'topic_id' => $topic_id,
            'forum_id' => $forum_id,
            'poster_id' => $poster_id,
            'icon_id' => 0,
            'poster_ip' => IMPORTED_POST_IP,
            'post_time' => $post_time,
            'post_reported' => 0,
            'enable_bbcode' => $message['enable_bbcode'],
            'enable_smilies' => $message['enable_smilies'],
            'enable_magic_url' => $message['enable_magic_url'],
            'enable_sig' => 1,
            'post_username' => $poster_id === ANONYMOUS ? $post_author : '',
            'post_subject' => $post_subject,
            'post_text' => $message['text'],
            'post_checksum' => md5(text_for_search($message['text'])),
            'post_attachment' => !empty($attachments_by_post[$source_post_id]) ? 1 : 0,
            'bbcode_bitfield' => $message['bbcode_bitfield'],
            'bbcode_uid' => $message['bbcode_uid'],
            'post_postcount' => 1,
            'post_edit_time' => (int) $edit['time'],
            'post_edit_reason' => '',
            'post_edit_user' => $edit_user,
            'post_edit_count' => $edit['time'] ? 1 : 0,
            'post_edit_locked' => 0,
            'post_visibility' => ITEM_APPROVED,
        ]);
        if ($source_post_id > 0)
        {
            $source_post_id_map[$source_post_id] = $post_id;
        }
        if (message_may_contain_old_forum_link($message_html))
        {
            $rewrite_candidates[] = [
                'post_id' => $post_id,
                'message_html' => $message_html,
            ];
        }

        if ($first_post_id === 0)
        {
            $first_post_id = $post_id;
        }
        if ($post_time >= $last_post_time)
        {
            $last_post_id = $post_id;
            $last_post_time = $post_time;
            $last_poster_id = $poster_id;
            $last_poster_name = $post_author;
        }

        foreach ($attachments_by_post[$source_post_id] ?? [] as $attachment)
        {
            import_attachment($attachment, $forum_data, $topic_id, $post_id, $poster_id, $post_time);
            $has_attachment = true;
        }
    }

    sql_update(table_name('TOPICS_TABLE', 'topics'), [
        'topic_first_post_id' => $first_post_id,
        'topic_last_post_id' => $last_post_id,
        'topic_last_poster_id' => $last_poster_id,
        'topic_last_poster_name' => $last_poster_name,
        'topic_last_post_subject' => $subject,
        'topic_last_post_time' => $last_post_time,
        'topic_last_view_time' => $last_post_time,
        'topic_attachment' => $has_attachment ? 1 : 0,
    ], 'topic_id = ' . $topic_id);

    return $topic_id;
}

function rewrite_imported_post_bodies(array $candidates, array $source_topic_id_map, array $source_post_id_map, string $target_domain): array
{
    global $db;

    $stats = ['rewritten_posts' => 0, 'unmapped_forum_links' => 0];
    if (!$candidates)
    {
        return $stats;
    }

    $transaction_open = false;
    try
    {
        $db->sql_transaction('begin');
        $transaction_open = true;

        foreach ($candidates as $candidate)
        {
            $post_id = (int) ($candidate['post_id'] ?? 0);
            if ($post_id <= 0)
            {
                continue;
            }

            $message_html = (string) ($candidate['message_html'] ?? '');
            $stats['unmapped_forum_links'] += count_unmapped_forum_links($message_html, $source_topic_id_map, $source_post_id_map);
            $message = format_imported_html($message_html, $source_topic_id_map, $source_post_id_map, $target_domain);
            sql_update(table_name('POSTS_TABLE', 'posts'), [
                'post_text' => $message['text'],
                'post_checksum' => md5(text_for_search($message['text'])),
                'bbcode_bitfield' => $message['bbcode_bitfield'],
                'bbcode_uid' => $message['bbcode_uid'],
                'enable_bbcode' => $message['enable_bbcode'],
                'enable_smilies' => $message['enable_smilies'],
                'enable_magic_url' => $message['enable_magic_url'],
            ], 'post_id = ' . $post_id);

            $stats['rewritten_posts']++;
            if ($stats['rewritten_posts'] % 5000 === 0)
            {
                $db->sql_transaction('commit');
                $transaction_open = false;
                echo "  Rewrote {$stats['rewritten_posts']} post body/bodies with forum links\n";
                $db->sql_transaction('begin');
                $transaction_open = true;
            }
        }

        $db->sql_transaction('commit');
        $transaction_open = false;
    }
    catch (Throwable $e)
    {
        if ($transaction_open)
        {
            $db->sql_transaction('rollback');
        }
        throw $e;
    }

    if ($stats['unmapped_forum_links'] > 0)
    {
        echo "  Warning: {$stats['unmapped_forum_links']} old forum link(s) could not be mapped to imported phpBB IDs\n";
    }

    return $stats;
}

function message_may_contain_old_forum_link(string $html): bool
{
    return stripos($html, 'forum.redump.org') !== false;
}

function count_unmapped_forum_links(string $html, array $source_topic_id_map, array $source_post_id_map): int
{
    $count = 0;
    if (!preg_match_all('~https?://forum\.redump\.org(?::[0-9]+)?(?:/[^\s<>"\']*)?~i', $html, $matches))
    {
        return 0;
    }

    foreach ($matches[0] as $url)
    {
        $parts = parse_url(html_entity_decode($url, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8'));
        if (!is_array($parts))
        {
            continue;
        }

        $path = (string) ($parts['path'] ?? '');
        $query = [];
        if (!empty($parts['query']))
        {
            parse_str((string) $parts['query'], $query);
        }

        if ((preg_match('#^/post/(\d+)/?$#', $path, $m) || preg_match('#^/post(\d+)(?:\.html)?$#', $path, $m)) && !isset($source_post_id_map[(int) $m[1]]))
        {
            $count++;
        }
        else if (isset($query['pid']) && !isset($source_post_id_map[(int) $query['pid']]))
        {
            $count++;
        }
        else if ((preg_match('#^/topic/(\d+)(?:/.*)?$#', $path, $m) || preg_match('#^/topic(\d+)(?:[-_].*)?(?:\.html)?$#', $path, $m)) && !isset($source_topic_id_map[(int) $m[1]]))
        {
            $count++;
        }
        else if (isset($query['id']) && !isset($source_topic_id_map[(int) $query['id']]))
        {
            $count++;
        }
    }

    return $count;
}

function source_view_count(array $topic_metadata): int
{
    foreach (['view_count', 'views', 'topic_views'] as $key)
    {
        if (!isset($topic_metadata[$key]))
        {
            continue;
        }
        if (is_int($topic_metadata[$key]))
        {
            return max(0, $topic_metadata[$key]);
        }
        if (is_string($topic_metadata[$key]) && preg_match('/\d/', $topic_metadata[$key]))
        {
            return max(0, (int) str_replace(',', '', $topic_metadata[$key]));
        }
    }
    return 0;
}

function import_attachment(array $attachment, string $forum_data, int $topic_id, int $post_id, int $poster_id, int $filetime): int
{
    global $config, $phpbb_root_path;

    $local = (string) ($attachment['local_path'] ?? '');
    $source = $forum_data . '/' . $local;
    if ($local === '' || !is_file($source))
    {
        throw new RuntimeException("Missing attachment file for phpBB post {$post_id}: {$local}");
    }

    $real_filename = safe_basename((string) ($attachment['filename'] ?? basename($source)), 'attachment');
    $ext = strtolower(pathinfo($real_filename, PATHINFO_EXTENSION));
    $physical = substr(sha1($topic_id . ':' . $post_id . ':' . ($attachment['attachment_id'] ?? '') . ':' . $real_filename), 0, 32);
    if ($ext !== '')
    {
        $physical .= '.' . $ext;
    }

    $upload_dir = $phpbb_root_path . trim((string) $config['upload_path'], '/');
    if (!is_dir($upload_dir) && !mkdir($upload_dir, 0775, true) && !is_dir($upload_dir))
    {
        throw new RuntimeException("Could not create phpBB upload directory: {$upload_dir}");
    }
    if (!copy($source, $upload_dir . '/' . $physical))
    {
        throw new RuntimeException("Could not copy attachment {$source}");
    }

    return sql_insert(table_name('ATTACHMENTS_TABLE', 'attachments'), [
        'post_msg_id' => $post_id,
        'topic_id' => $topic_id,
        'in_message' => 0,
        'poster_id' => $poster_id,
        'is_orphan' => 0,
        'physical_filename' => $physical,
        'real_filename' => $real_filename,
        'download_count' => 0,
        'attach_comment' => 'Imported Redump attachment ' . (string) ($attachment['attachment_id'] ?? ''),
        'extension' => $ext,
        'mimetype' => (string) ($attachment['content_type'] ?? mime_content_type($source) ?: 'application/octet-stream'),
        'filesize' => filesize($source) ?: 0,
        'filetime' => $filetime,
        'thumbnail' => 0,
    ]);
}

function finalize_import(): void
{
    global $db;

    echo "  Recomputing topic attachment flags...\n";
    recompute_topic_flags();
    echo "  Recomputing forum counters...\n";
    recompute_forums();
    echo "  Recomputing user counters...\n";
    recompute_users();
    echo "  Recomputing board counters...\n";
    recompute_config();
    echo "  Rebuilding search index...\n";
    rebuild_search_index();
    echo "  Repairing database sequences...\n";
    repair_sequences();
    echo "  Clearing permission cache...\n";
    clear_permission_prefetch();

    global $cache;
    if (isset($cache))
    {
        $cache->purge();
    }

    echo "  Repairing writable directory permissions...\n";
    repair_phpbb_writable_permissions();
}

function repair_phpbb_writable_permissions(): void
{
    global $config, $phpbb_root_path;

    $paths = [
        $phpbb_root_path . 'cache',
        $phpbb_root_path . 'store',
        $phpbb_root_path . trim((string) ($config['upload_path'] ?? 'files'), '/'),
        $phpbb_root_path . trim((string) ($config['avatar_path'] ?? 'images/avatars/upload'), '/'),
    ];

    foreach (array_unique($paths) as $path)
    {
        repair_path_permissions($path);
    }
}

function repair_path_permissions(string $path): void
{
    if (!file_exists($path))
    {
        return;
    }

    repair_single_path_permissions($path);
    if (!is_dir($path) || is_link($path))
    {
        return;
    }

    $iterator = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator($path, FilesystemIterator::SKIP_DOTS),
        RecursiveIteratorIterator::SELF_FIRST
    );

    foreach ($iterator as $item)
    {
        repair_single_path_permissions($item->getPathname());
    }
}

function repair_single_path_permissions(string $path): void
{
    if (is_link($path))
    {
        return;
    }

    @chown($path, 'www-data');
    @chgrp($path, 'www-data');
    @chmod($path, is_dir($path) ? 0775 : 0664);
}

function clear_permission_prefetch(): void
{
    global $db;
    $db->sql_query('UPDATE ' . table_name('USERS_TABLE', 'users') . " SET user_permissions = ''");
}

function recompute_topic_flags(): void
{
    global $db;

    $topics = table_name('TOPICS_TABLE', 'topics');
    $attachments = table_name('ATTACHMENTS_TABLE', 'attachments');
    $db->sql_query(
        'UPDATE ' . $topics . '
         SET topic_attachment = CASE
             WHEN EXISTS (SELECT 1 FROM ' . $attachments . ' a WHERE a.topic_id = ' . $topics . '.topic_id) THEN 1
             ELSE 0
         END'
    );
}

function recompute_forums(): void
{
    $forums = sql_fetch_all('SELECT forum_id, forum_type FROM ' . table_name('FORUMS_TABLE', 'forums'));
    foreach ($forums as $forum)
    {
        $forum_id = (int) $forum['forum_id'];
        if ((int) $forum['forum_type'] !== FORUM_POST)
        {
            sql_update(table_name('FORUMS_TABLE', 'forums'), [
                'forum_posts_approved' => 0,
                'forum_topics_approved' => 0,
                'forum_last_post_id' => 0,
                'forum_last_poster_id' => 0,
                'forum_last_post_subject' => '',
                'forum_last_post_time' => 0,
                'forum_last_poster_name' => '',
                'forum_last_poster_colour' => '',
            ], 'forum_id = ' . $forum_id);
            continue;
        }

        $post_count = (int) sql_fetch_one('SELECT COUNT(*) FROM ' . table_name('POSTS_TABLE', 'posts') . ' WHERE forum_id = ' . $forum_id . ' AND post_visibility = ' . ITEM_APPROVED);
        $topic_count = (int) sql_fetch_one('SELECT COUNT(*) FROM ' . table_name('TOPICS_TABLE', 'topics') . ' WHERE forum_id = ' . $forum_id . ' AND topic_visibility = ' . ITEM_APPROVED);
        $row = sql_fetch_row(
            'SELECT p.post_id, p.poster_id, p.post_subject, p.post_time, u.username, u.user_colour
             FROM ' . table_name('POSTS_TABLE', 'posts') . ' p
             LEFT JOIN ' . table_name('USERS_TABLE', 'users') . ' u ON u.user_id = p.poster_id
             WHERE p.forum_id = ' . $forum_id . ' AND p.post_visibility = ' . ITEM_APPROVED . '
             ORDER BY p.post_time DESC, p.post_id DESC'
        );
        sql_update(table_name('FORUMS_TABLE', 'forums'), [
            'forum_posts_approved' => $post_count,
            'forum_posts_unapproved' => 0,
            'forum_posts_softdeleted' => 0,
            'forum_topics_approved' => $topic_count,
            'forum_topics_unapproved' => 0,
            'forum_topics_softdeleted' => 0,
            'forum_last_post_id' => $row ? (int) $row['post_id'] : 0,
            'forum_last_poster_id' => $row ? (int) $row['poster_id'] : 0,
            'forum_last_post_subject' => $row ? (string) $row['post_subject'] : '',
            'forum_last_post_time' => $row ? (int) $row['post_time'] : 0,
            'forum_last_poster_name' => $row ? (string) $row['username'] : '',
            'forum_last_poster_colour' => $row ? (string) $row['user_colour'] : '',
        ], 'forum_id = ' . $forum_id);
    }
}

function recompute_users(): void
{
    global $db;
    $users_table = table_name('USERS_TABLE', 'users');
    $posts_table = table_name('POSTS_TABLE', 'posts');
    $normal_types = USER_NORMAL . ', ' . USER_FOUNDER;

    $db->sql_query(
        'UPDATE ' . $users_table . '
         SET user_posts = 0, user_lastpost_time = 0
         WHERE user_type IN (' . $normal_types . ')'
    );
    $db->sql_query(
        'UPDATE ' . $users_table . ' u
         SET user_posts = p.post_count, user_lastpost_time = p.last_post_time
         FROM (
             SELECT poster_id, COUNT(*) AS post_count, COALESCE(MAX(post_time), 0) AS last_post_time
             FROM ' . $posts_table . '
             WHERE post_visibility = ' . ITEM_APPROVED . '
             GROUP BY poster_id
         ) p
         WHERE u.user_id = p.poster_id
         AND u.user_type IN (' . $normal_types . ')'
    );
}

function recompute_config(): void
{
    set_config_value('num_posts', (string) count_table(table_name('POSTS_TABLE', 'posts')));
    set_config_value('num_topics', (string) count_table(table_name('TOPICS_TABLE', 'topics')));
    set_config_value('num_files', (string) count_table(table_name('ATTACHMENTS_TABLE', 'attachments')));
    $upload_size = (int) sql_fetch_one('SELECT COALESCE(SUM(filesize), 0) FROM ' . table_name('ATTACHMENTS_TABLE', 'attachments'));
    set_config_value('upload_dir_size', (string) $upload_size);
}

function set_config_value(string $name, string $value): void
{
    global $db, $config;
    $exists = sql_fetch_one(
        'SELECT config_name FROM ' . table_name('CONFIG_TABLE', 'config')
        . " WHERE config_name = '" . $db->sql_escape($name) . "'"
    );
    if ($exists)
    {
        sql_update(table_name('CONFIG_TABLE', 'config'), ['config_value' => $value], "config_name = '" . $db->sql_escape($name) . "'");
    }
    else
    {
        sql_insert(table_name('CONFIG_TABLE', 'config'), [
            'config_name' => $name,
            'config_value' => $value,
            'is_dynamic' => 0,
        ]);
    }
    $config[$name] = $value;
}

function rebuild_search_index(): void
{
    global $auth, $config, $db, $phpbb_dispatcher, $phpbb_root_path, $phpEx, $user;

    $search_results = table_name('SEARCH_RESULTS_TABLE', 'search_results');
    $search_wordmatch = table_name('SEARCH_WORDMATCH_TABLE', 'search_wordmatch');
    $search_wordlist = table_name('SEARCH_WORDLIST_TABLE', 'search_wordlist');

    $search_type = (string) $config['search_type'];
    if (!class_exists($search_type))
    {
        return;
    }

    $error = false;
    $search = new $search_type($error, $phpbb_root_path, $phpEx, $auth, $config, $db, $user, $phpbb_dispatcher);
    if ($error)
    {
        echo "  Search index skipped: {$error}\n";
        return;
    }

    $posts_table = table_name('POSTS_TABLE', 'posts');
    $count = 0;
    $last_post_id = 0;
    $batch_size = 1000;
    $next_report = 1000;
    $transaction_open = false;

    try
    {
        $db->sql_transaction('begin');
        $transaction_open = true;
        $db->sql_query('DELETE FROM ' . $search_results);
        $db->sql_query('DELETE FROM ' . $search_wordmatch);
        $db->sql_query('DELETE FROM ' . $search_wordlist);

        while (true)
        {
            $result = $db->sql_query_limit(
                'SELECT post_id, post_text, post_subject, poster_id, forum_id
                 FROM ' . $posts_table . '
                 WHERE post_id > ' . $last_post_id . '
                 ORDER BY post_id',
                $batch_size
            );

            $batch_count = 0;
            while ($row = $db->sql_fetchrow($result))
            {
                $last_post_id = (int) $row['post_id'];
                $message = text_for_search((string) $row['post_text']);
                $subject = (string) $row['post_subject'];
                $search->index('post', $last_post_id, $message, $subject, (int) $row['poster_id'], (int) $row['forum_id']);
                $count++;
                $batch_count++;
            }
            $db->sql_freeresult($result);

            if ($count >= $next_report)
            {
                $db->sql_transaction('commit');
                $transaction_open = false;
                echo "  Search indexed {$count} post(s)\n";
                $db->sql_transaction('begin');
                $transaction_open = true;
                while ($next_report <= $count)
                {
                    $next_report += 1000;
                }
            }

            if ($batch_count === 0)
            {
                break;
            }
        }

        $db->sql_transaction('commit');
        $transaction_open = false;
    }
    catch (Throwable $e)
    {
        if ($transaction_open)
        {
            $db->sql_transaction('rollback');
        }
        throw $e;
    }
}

function repair_sequences(): void
{
    global $db, $table_prefix;

    $pairs = [
        ['attachments', 'attach_id'],
        ['forums', 'forum_id'],
        ['groups', 'group_id'],
        ['posts', 'post_id'],
        ['topics', 'topic_id'],
        ['users', 'user_id'],
    ];

    foreach ($pairs as [$suffix, $column])
    {
        $table = $table_prefix . $suffix;
        $sequence = $table . '_seq';
        $sql = "SELECT setval('" . $db->sql_escape($sequence) . "', GREATEST((SELECT COALESCE(MAX({$column}), 1) FROM {$table}), 1), true)";
        $db->sql_query($sql);
    }
}

function main(array $argv): int
{
    $args = parse_args($argv);
    if ($args['finalize_only'])
    {
        echo "Finalizing phpBB counters, search index, sequences, and cache...\n";
        finalize_import();
        echo "Finalization complete:\n";
        echo "  Topics:      " . count_table(table_name('TOPICS_TABLE', 'topics')) . "\n";
        echo "  Posts:       " . count_table(table_name('POSTS_TABLE', 'posts')) . "\n";
        echo "  Attachments: " . count_table(table_name('ATTACHMENTS_TABLE', 'attachments')) . "\n";
        return 0;
    }

    $scan = scan_archive($args['forum_data'], $args['users_dir']);
    print_preflight($scan);
    assert_preflight_ok($scan);

    ensure_sample_or_empty_board();

    if ($args['dry_run'])
    {
        echo "Dry run complete. No phpBB data was changed.\n";
        return 0;
    }

    echo "Clearing installer sample forum content...\n";
    clear_sample_board();

    echo "Ensuring phpBB groups...\n";
    ensure_user_plus_group();

    echo "Importing users...\n";
    $user_ids = create_or_update_users($scan['users_by_name'], $scan['users_by_id'], $args['users_dir'], $args['source_timezone'], $args['target_domain']);
    $test_user_ids = [];
    if (is_file($args['test_users_file']))
    {
        echo "Seeding test users from {$args['test_users_file']}...\n";
        $test_user_ids = seed_test_users($args['test_users_file'], $args['source_timezone']);
        foreach ($test_user_ids as $username => $user_id)
        {
            $user_ids[$username] = $user_id;
        }
    }
    else
    {
        echo "No test users file found at {$args['test_users_file']}; skipping test users.\n";
    }

    echo "Importing forums...\n";
    $forum_ids = import_forums($scan['forums']);

    echo "Importing topics, posts, and attachments...\n";
    $import_stats = import_topics($scan['paths'], $args['forum_data'], $forum_ids, $user_ids, $scan['topic_metadata'], $args['source_timezone'], $args['target_domain']);

    echo "Rewriting user signatures with imported forum link maps...\n";
    $signature_updates = rewrite_imported_user_signatures(
        $scan['users_by_id'],
        $user_ids,
        $args['users_dir'],
        $import_stats['source_topic_id_map'],
        $import_stats['source_post_id_map'],
        $args['target_domain']
    );

    echo "Finalizing phpBB counters, search index, sequences, and cache...\n";
    finalize_import();

    echo "Import complete:\n";
    echo "  Topics:      {$import_stats['topics']}\n";
    echo "  Posts:       {$import_stats['posts']}\n";
    echo "  Attachments: {$import_stats['attachments']}\n";
    echo "  Rewritten:   {$import_stats['rewritten_posts']}\n";
    echo "  Signatures:  {$signature_updates}\n";
    echo "  Users:       " . count($user_ids) . "\n";
    echo "  Test users:  " . count($test_user_ids) . "\n";
    echo "  Forums:      " . count($forum_ids) . "\n";

    return 0;
}

if (!defined('REDUMP_IMPORT_NO_MAIN'))
{
    try
    {
        exit(main($argv));
    }
    catch (Throwable $e)
    {
        fwrite(STDERR, "redump-forum-import: ERROR: " . $e->getMessage() . "\n");
        exit(1);
    }
}
