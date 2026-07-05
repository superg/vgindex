<?php

$expectedVersion = getenv('MEDIAWIKI_EXPECTED_VERSION') ?: '1.46.0';
$context = stream_context_create([
    'http' => [
        'timeout' => 4,
        'ignore_errors' => true,
    ],
]);
$response = @file_get_contents(
    'http://127.0.0.1/api.php?action=query&meta=siteinfo&format=json',
    false,
    $context
);

if ($response === false) {
    exit(1);
}

$payload = json_decode($response, true);
$generator = $payload['query']['general']['generator'] ?? '';
exit($generator === "MediaWiki $expectedVersion" ? 0 : 1);
