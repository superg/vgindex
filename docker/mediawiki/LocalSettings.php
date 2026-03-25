<?php
# MediaWiki LocalSettings
# Site name and server URL are derived from SITE_DOMAIN and HTTPS_PORT env vars.

$siteDomain = getenv('SITE_DOMAIN') ?: 'localhost';
$httpsPort = getenv('HTTPS_PORT') ?: '8443';
$portSuffix = ($httpsPort === '443') ? '' : ":$httpsPort";

$wgSitename = "$siteDomain Wiki";
$wgMetaNamespace = str_replace('.', '_', $siteDomain);
$wgServer = "https://wiki.{$siteDomain}{$portSuffix}";
$wgScriptPath = "";
$wgArticlePath = "/$1";

# Database settings (PostgreSQL)
$wgDBtype = "postgres";
$wgDBserver = getenv('MEDIAWIKI_DB_HOST') ?: "postgres";
$wgDBport = getenv('MEDIAWIKI_DB_PORT') ?: "5432";
$wgDBname = getenv('MEDIAWIKI_DB_NAME') ?: "mediawiki";
$wgDBmwschema = getenv('MEDIAWIKI_DB_SCHEMA') ?: "public";
$wgDBuser = getenv('MEDIAWIKI_DB_USER') ?: "vgindex";
$wgDBpassword = getenv('MEDIAWIKI_DB_PASSWORD') ?: "changeme";
$wgDBprefix = "";

# Security
$wgSecretKey = getenv('MEDIAWIKI_SECRET_KEY') ?: "change-this-secret-key-in-production";
$wgUpgradeKey = getenv('MEDIAWIKI_UPGRADE_KEY') ?: "change-this-upgrade-key";

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

# editor group: User+ / Moderator / Admin can edit wiki pages
$wgGroupPermissions['editor']['edit'] = true;
$wgGroupPermissions['editor']['createpage'] = true;
$wgGroupPermissions['editor']['createtalk'] = true;
$wgGroupPermissions['editor']['writeapi'] = true;
$wgGroupPermissions['editor']['upload'] = true;

# PluggableAuth + OpenID Connect for SSO
wfLoadExtension('PluggableAuth');
wfLoadExtension('OpenIDConnect');

$wgPluggableAuth_Config['sso'] = [
    'plugin' => 'OpenIDConnect',
    'data' => [
        'providerURL' => getenv('OIDC_PROVIDER_URL') ?: 'http://app:3000',
        'clientID' => getenv('OIDC_CLIENT_ID') ?: 'mediawiki-client',
        'clientsecret' => getenv('OIDC_CLIENT_SECRET') ?: 'change-this-secret-mediawiki',
    ],
];

$wgPluggableAuth_EnableLocalLogin = true;

# Sync OIDC role claim -> MediaWiki groups on every SSO login.
#   User / User+   -> (no extra groups, read-only wiki)
#   User+          -> editor (can edit pages)
#   Moderator      -> editor
#   Admin          -> editor + sysop
$wgHooks['PluggableAuthPopulateGroups'][] = function (
    \MediaWiki\User\User $user,
    array $attributes
) {
    $role = $attributes['role'] ?? null;
    if ($role === null) {
        return;
    }

    $ugm = \MediaWiki\MediaWikiServices::getInstance()->getUserGroupManager();
    $current = $ugm->getUserGroups($user);

    $syncMap = [
        'editor' => in_array($role, ['User+', 'Moderator', 'Admin'], true),
        'sysop'  => ($role === 'Admin'),
    ];

    foreach ($syncMap as $group => $desired) {
        $has = in_array($group, $current, true);
        if ($desired && !$has) {
            $ugm->addUserToGroup($user, $group);
        } elseif (!$desired && $has) {
            $ugm->removeUserFromGroup($user, $group);
        }
    }
};

# Debug logging (disable in production)
$wgShowExceptionDetails = true;
$wgShowDBErrorBacktrace = true;
$wgDebugLogFile = '/tmp/mediawiki-debug.log';

# File uploads
$wgEnableUploads = true;
$wgFileExtensions = array_merge($wgFileExtensions, ['pdf', 'svg']);
