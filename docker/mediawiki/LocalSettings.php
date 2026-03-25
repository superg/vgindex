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

# Disable anonymous editing
$wgGroupPermissions['*']['edit'] = false;
$wgGroupPermissions['*']['createaccount'] = false;
$wgGroupPermissions['*']['autocreateaccount'] = true;

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

# Debug logging (disable in production)
$wgShowExceptionDetails = true;
$wgShowDBErrorBacktrace = true;
$wgDebugLogFile = '/tmp/mediawiki-debug.log';

# File uploads
$wgEnableUploads = true;
$wgFileExtensions = array_merge($wgFileExtensions, ['pdf', 'svg']);
