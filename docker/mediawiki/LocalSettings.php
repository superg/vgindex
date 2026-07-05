<?php
# MediaWiki LocalSettings
# Site name and server URL are derived from MEDIAWIKI_PUBLIC_URL.

use MediaWiki\Title\Title;

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

$mediawikiPublicUrl = rtrim(requireEnv('MEDIAWIKI_PUBLIC_URL'), '/');
$mediawikiHost = parse_url($mediawikiPublicUrl, PHP_URL_HOST) ?: 'wiki';
$mediawikiSiteName = getenv('MEDIAWIKI_SITE_NAME');

$wgSitename = ($mediawikiSiteName === false || $mediawikiSiteName === '') ? "$mediawikiHost Wiki" : $mediawikiSiteName;
$wgMetaNamespace = str_replace('.', '_', $mediawikiHost);
$wgServer = $mediawikiPublicUrl;
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

$mediawikiReadOnlyReason = getenv('MEDIAWIKI_READ_ONLY_REASON');
if (PHP_SAPI !== 'cli' && $mediawikiReadOnlyReason !== false && $mediawikiReadOnlyReason !== '') {
    $wgReadOnly = $mediawikiReadOnlyReason;
}

# Email
$mediawikiEmailEnabled = envBool('MEDIAWIKI_EMAIL_ENABLE', false);
$wgEnableEmail = $mediawikiEmailEnabled;
$wgEnableUserEmail = $mediawikiEmailEnabled;

if ($mediawikiEmailEnabled) {
    $mediawikiEmailFrom = requireEnv('MEDIAWIKI_EMAIL_FROM');
    $mediawikiEmailFromName = getenv('MEDIAWIKI_EMAIL_FROM_NAME');
    $mediawikiSmtpHost = requireEnv('MEDIAWIKI_SMTP_HOST');
    $mediawikiSmtpPort = (int) requireEnv('MEDIAWIKI_SMTP_PORT');
    $mediawikiSmtpUser = getenv('MEDIAWIKI_SMTP_USER');
    $mediawikiSmtpPassword = getenv('MEDIAWIKI_SMTP_PASSWORD');
    $mediawikiSmtpUser = $mediawikiSmtpUser === false ? '' : $mediawikiSmtpUser;
    $mediawikiSmtpPassword = $mediawikiSmtpPassword === false ? '' : $mediawikiSmtpPassword;
    $mediawikiSmtpStarttls = envBool('MEDIAWIKI_SMTP_STARTTLS', strpos($mediawikiSmtpHost, '://') === false);

    if (($mediawikiSmtpUser === '') !== ($mediawikiSmtpPassword === '')) {
        throw new RuntimeException('MEDIAWIKI_SMTP_USER and MEDIAWIKI_SMTP_PASSWORD must both be set, or both be blank');
    }

    $wgPasswordSender = $mediawikiEmailFrom;
    $wgEmergencyContact = $mediawikiEmailFrom;

    if ($mediawikiEmailFromName !== false && $mediawikiEmailFromName !== '') {
        $wgPasswordSenderName = $mediawikiEmailFromName;
    }

    $wgSMTP = [
        'host' => $mediawikiSmtpHost,
        'IDHost' => $mediawikiHost,
        'localhost' => $mediawikiHost,
        'port' => $mediawikiSmtpPort,
        'auth' => false,
        'starttls' => $mediawikiSmtpStarttls,
    ];

    if ($mediawikiSmtpUser !== '') {
        $wgSMTP['auth'] = true;
        $wgSMTP['username'] = $mediawikiSmtpUser;
        $wgSMTP['password'] = $mediawikiSmtpPassword;
    }
}

# Default skin
$wgDefaultSkin = "vector-2022";
wfLoadSkin('Vector');
$wgDefaultUserOptions['vector-appearance-pinned'] = 1;

# Disable anonymous editing and account creation
$wgGroupPermissions['*']['edit'] = false;
$wgGroupPermissions['*']['createaccount'] = false;
$wgGroupPermissions['*']['autocreateaccount'] = true;

# Revoke edit from default logged-in users (User and User+ are both
# in the MW "user" group; editing is granted via the "editor" group below)
$wgGroupPermissions['user']['edit'] = false;
$wgGroupPermissions['user']['createpage'] = false;
$wgGroupPermissions['user']['writeapi'] = false;

# editor group: trusted users can edit wiki pages
$wgGroupPermissions['editor']['edit'] = true;
$wgGroupPermissions['editor']['createpage'] = true;
$wgGroupPermissions['editor']['createtalk'] = true;
$wgGroupPermissions['editor']['writeapi'] = true;
$wgGroupPermissions['editor']['upload'] = true;

# sysop group: User+ / Moderator / Admin can edit wiki pages
$wgGroupPermissions['sysop']['edit'] = true;
$wgGroupPermissions['sysop']['createpage'] = true;
$wgGroupPermissions['sysop']['createtalk'] = true;
$wgGroupPermissions['sysop']['writeapi'] = true;
$wgGroupPermissions['sysop']['upload'] = true;

# Allow sysop to move users to editor and back
$wgAddGroups['sysop'] = [ 'editor' ];
$wgRemoveGroups['sysop'] = [ 'editor' ];

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
                'sysop' => ['groups' => ['User+', 'Moderator', 'Admin']],
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

# File uploads
$wgEnableUploads = true;
$wgFileExtensions = array_merge($wgFileExtensions, ['pdf', 'svg']);

# Wiki logos
$wgLogos = [
    '1x'   => '/images/6/6e/Logo_sq_135.png',
    '2x'   => '/images/3/39/Logo_sq_270.png',
    'icon' => '/images/4/4f/Logo_sq_50.png'
];

# Cookie expiry 30 days
$wgRememberMe = 'always';

# Enable dark mode
$wgVectorNightMode['logged_out'] = true;
$wgVectorNightMode['logged_in'] = true;
$wgVectorNightMode['beta'] = true;
