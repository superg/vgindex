<?php

if (!defined('IN_PHPBB'))
{
    exit;
}

if (empty($lang) || !is_array($lang))
{
    $lang = [];
}

$lang = array_merge($lang, [
    'AUTH_PROVIDER_OAUTH_SERVICE_VGINDEX' => 'VGIndex',
]);
