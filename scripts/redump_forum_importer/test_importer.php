<?php

declare(strict_types=1);

define('REDUMP_IMPORT_NO_MAIN', true);
require getenv('REDUMP_IMPORTER_PATH') ?: __DIR__ . '/import.php';

function assert_contains_text(string $needle, string $haystack, string $label): void
{
    if (!str_contains($haystack, $needle))
    {
        throw new RuntimeException("Assertion failed ({$label}): missing {$needle}");
    }
}

function assert_not_contains_text(string $needle, string $haystack, string $label): void
{
    if (str_contains($haystack, $needle))
    {
        throw new RuntimeException("Assertion failed ({$label}): unexpected {$needle}");
    }
}

function assert_same($expected, $actual, string $label): void
{
    if ($expected !== $actual)
    {
        throw new RuntimeException("Assertion failed ({$label})");
    }
}

$html = '<p>First paragraph.</p>'
    . '<p>Second paragraph with <a href="http://wiki.redump.org/index.php?title=Sony_PlayStation_2">http://wiki.redump.org/index.php?title=Sony_PlayStation_2</a></p>'
    . '<div class="codebox"><pre><code>&lt;b&gt;Logs Link&lt;/b&gt;' . "\n"
    . 'http://redump.org/disc/1/' . "\n"
    . 'http://forum.redump.org/post/127039/</code></pre></div>'
    . '<div class="quotebox"><cite>Alice wrote:</cite><blockquote><p>Hello</p></blockquote></div>'
    . '<script>alert("x")</script>';

$source = punbb_html_to_phpbb_source($html, [72860 => 66035], [127039 => 12345], 'localhost');
assert_contains_text("First paragraph.\n\nSecond paragraph", $source, 'paragraph spacing');
assert_contains_text('[url]https://wiki.localhost/index.php?title=Sony_PlayStation_2[/url]', $source, 'wiki rewrite');
assert_contains_text('<b>Logs Link</b>', $source, 'literal code html');
assert_contains_text('https://localhost/disc/1/', $source, 'root rewrite in code');
assert_contains_text('/viewtopic.php?p=12345#p12345', $source, 'old post rewrite');
assert_contains_text('[quote=Alice]Hello[/quote]', $source, 'quote rewrite');
assert_not_contains_text('<script>', $source, 'unsafe tag stripping');

$formatted = format_imported_html($html, [72860 => 66035], [127039 => 12345], 'localhost');
assert_contains_text('<CODE>', $formatted['text'], 'native code storage');
assert_contains_text('&lt;b&gt;Logs Link&lt;/b&gt;', $formatted['text'], 'escaped literal code html');
assert_contains_text('<QUOTE author="Alice">', $formatted['text'], 'native quote storage');

assert_contains_text(
    '/viewtopic.php?t=66035',
    rewrite_redump_url('http://forum.redump.org/topic/72860/disney', [72860 => 66035], [], 'vgindex.org'),
    'old topic rewrite'
);
assert_contains_text(
    'https://wiki.vgindex.org/index.php?title=X',
    rewrite_redump_url('http://wiki.redump.org/index.php?title=X', [], [], 'vgindex.org'),
    'wiki target rewrite'
);

$signature = format_imported_html(
    '<a href="http://forum.redump.org/topic/17188/region-extractor-a-small-tool/">Region Extractor</a>',
    [17188 => 456],
    [],
    'localhost',
    'sig'
);
assert_contains_text('/viewtopic.php?t=456', $signature['text'], 'signature old topic rewrite');
assert_not_contains_text('/topic/17188/', $signature['text'], 'signature does not keep old topic path');

$missing_test_users = load_test_user_definitions('/tmp/redump-missing-test-users.json');
assert_same([], $missing_test_users, 'missing test users file is optional');

$test_users_file = tempnam(sys_get_temp_dir(), 'redump-test-users-');
file_put_contents($test_users_file, json_encode([
    'phpbb_user' => [
        'password' => 'local-only',
        'role' => 'user',
        'email' => 'phpbb_user@localhost',
    ],
]));
$test_users = load_test_user_definitions($test_users_file);
unlink($test_users_file);
assert_same('local-only', $test_users['phpbb_user']['password'], 'test user password loaded externally');
assert_same('user', $test_users['phpbb_user']['role'], 'test user role loaded externally');

echo "Importer formatter tests passed.\n";
