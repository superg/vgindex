<?php
# MediaWiki LocalSettings
# Site name and server URL are derived from SITE_DOMAIN and HTTPS_PORT env vars.

function requireEnv(string $name): string {
    $val = getenv($name);
    if ($val === false || $val === '') {
        throw new RuntimeException("Required environment variable $name is not set");
    }
    return $val;
}

function envBool(string $name, bool $default): bool {
    $val = getenv($name);
    if ($val === false || $val === '') {
        return $default;
    }
    return in_array(strtolower($val), ['1', 'true', 'yes', 'on', 'enabled'], true);
}

function vgiMwUcfirst(string $value): string {
    if ($value === '') {
        return $value;
    }
    return mb_strtoupper(mb_substr($value, 0, 1)) . mb_substr($value, 1);
}

function vgiOidcAllowedCanonicalUsername(string $username): string {
    return vgiMwUcfirst(str_replace('_', ' ', $username));
}

function vgiOidcCrc16Hex(string $value): string {
    $crc = 0xffff;
    $length = strlen($value);
    for ($i = 0; $i < $length; $i++) {
        $crc ^= ord($value[$i]) << 8;
        for ($bit = 0; $bit < 8; $bit++) {
            if (($crc & 0x8000) !== 0) {
                $crc = (($crc << 1) ^ 0x1021) & 0xffff;
            } else {
                $crc = ($crc << 1) & 0xffff;
            }
        }
    }
    return str_pad(strtolower(dechex($crc)), 4, '0', STR_PAD_LEFT);
}

function vgiOidcSafeUsernameBase(string $username): string {
    $base = str_replace('_', ' ', $username);
    $base = preg_replace('/[#<>\[\]|{}]+/u', ' ', $base) ?? '';
    $base = preg_replace('/[\x00-\x1f\x7f]+/u', ' ', $base) ?? '';
    $base = preg_replace('/\s+/u', ' ', trim($base)) ?? '';
    if ($base === '') {
        $base = 'User';
    }

    $title = Title::makeTitleSafe(NS_USER, $base);
    if ($title !== null) {
        return $title->getText();
    }

    $base = preg_replace('/[^\p{L}\p{N} .-]+/u', ' ', $base) ?? '';
    $base = preg_replace('/\s+/u', ' ', trim($base)) ?? '';
    if ($base === '') {
        $base = 'User';
    }

    $title = Title::makeTitleSafe(NS_USER, $base);
    return $title !== null ? $title->getText() : 'User';
}

$siteDomain = requireEnv('SITE_DOMAIN');
$httpsPort = requireEnv('HTTPS_PORT');
$portSuffix = ($httpsPort === '443') ? '' : ":$httpsPort";

$wgSitename = "$siteDomain Wiki";
$wgMetaNamespace = str_replace('.', '_', $siteDomain);
$wgServer = "https://wiki.{$siteDomain}{$portSuffix}";
$wgScriptPath = "";
$wgArticlePath = "/$1";

# Database settings (PostgreSQL)
$wgDBtype = "postgres";
$wgDBserver = requireEnv('MEDIAWIKI_DB_HOST');
$wgDBport = requireEnv('MEDIAWIKI_DB_PORT');
$wgDBname = requireEnv('MEDIAWIKI_DB_NAME');
$wgDBmwschema = requireEnv('MEDIAWIKI_DB_SCHEMA');
$wgDBuser = requireEnv('MEDIAWIKI_DB_USER');
$wgDBpassword = requireEnv('MEDIAWIKI_DB_PASSWORD');
$wgDBprefix = "";

# Security
$wgSecretKey = requireEnv('MEDIAWIKI_SECRET_KEY');
$wgUpgradeKey = requireEnv('MEDIAWIKI_UPGRADE_KEY');

# Default skin
$wgDefaultSkin = "vector-2022";
wfLoadSkin('Vector');

# Disable anonymous editing and account creation
$wgGroupPermissions['*']['edit'] = false;
$wgGroupPermissions['*']['createaccount'] = false;
$wgGroupPermissions['*']['autocreateaccount'] = true;

# Revoke edit from default logged-in users (User and User+ are both
# in the MW "user" group; editing is granted via the "editor" group below)
$wgGroupPermissions['user']['edit'] = false;
$wgGroupPermissions['user']['createpage'] = false;
$wgGroupPermissions['user']['writeapi'] = false;

# editor group: User+ / Moderator / Admin can edit wiki pages
$wgGroupPermissions['editor']['edit'] = true;
$wgGroupPermissions['editor']['createpage'] = true;
$wgGroupPermissions['editor']['createtalk'] = true;
$wgGroupPermissions['editor']['writeapi'] = true;
$wgGroupPermissions['editor']['upload'] = true;

# PluggableAuth + OpenID Connect for SSO
wfLoadExtension('PluggableAuth');
wfLoadExtension('OpenIDConnect');

$wgPluggableAuth_Config['SSO'] = [
    'plugin' => 'OpenIDConnect',
    'data' => [
        'providerURL' => requireEnv('OIDC_PROVIDER_URL'),
        'clientID' => requireEnv('MEDIAWIKI_OIDC_CLIENT_ID'),
        'clientsecret' => requireEnv('MEDIAWIKI_OIDC_CLIENT_SECRET'),
        'scope' => ['openid', 'profile', 'email'],
        'preferred_username' => 'preferred_username',
        'verifyHost' => envBool('MEDIAWIKI_OIDC_VERIFY_TLS', true),
        'verifyPeer' => envBool('MEDIAWIKI_OIDC_VERIFY_TLS', true),
    ],
    'groupsyncs' => [
        'phpbb_roles' => [
            'type' => 'mapped',
            'map' => [
                'editor' => ['groups' => ['User+', 'Moderator', 'Admin']],
                'sysop' => ['groups' => 'Admin'],
            ],
        ],
    ],
];

$wgPluggableAuth_EnableLocalLogin = envBool('MEDIAWIKI_LOCAL_LOGIN', false);
$wgOpenIDConnect_MigrateUsersByUserName = true;
$wgOpenIDConnect_MigrateUsersByEmail = false;
$wgOpenIDConnect_PreferredUsernameProcessor = static function (?string $username, array $attributes): ?string {
    if (!is_string($username) || $username === '') {
        return $username;
    }

    $title = Title::makeTitleSafe(NS_USER, $username);
    if ($title !== null && $title->getText() === vgiOidcAllowedCanonicalUsername($username)) {
        return $username;
    }

    $base = $title !== null ? $title->getText() : vgiOidcSafeUsernameBase($username);
    $checksumSource = isset($attributes['sub']) && is_string($attributes['sub'])
        ? $attributes['sub']
        : $username;
    $candidate = $base . '-' . vgiOidcCrc16Hex($checksumSource);
    $candidateTitle = Title::makeTitleSafe(NS_USER, $candidate);

    return $candidateTitle !== null ? $candidateTitle->getText() : 'User-' . vgiOidcCrc16Hex($checksumSource);
};

# Debug logging (disable in production)
$wgShowExceptionDetails = true;
$wgShowDBErrorBacktrace = true;
$wgDebugLogFile = '/tmp/mediawiki-debug.log';

# File uploads
$wgEnableUploads = true;
$wgFileExtensions = array_merge($wgFileExtensions, ['pdf', 'svg']);
