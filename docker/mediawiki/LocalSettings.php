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
