<?php
# MediaWiki LocalSettings for vgindex.org wiki integration

$wgSitename = "vgindex.org Wiki";
$wgMetaNamespace = "vgindex";
$wgServer = "http://localhost:8080";
$wgScriptPath = "/wiki";
$wgArticlePath = "/wiki/$1";

# Database settings (PostgreSQL)
$wgDBtype = "postgres";
$wgDBserver = "postgres";
$wgDBname = "mediawiki";
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

# PluggableAuth + OpenID Connect for SSO
# Install these extensions:
#   - PluggableAuth: https://www.mediawiki.org/wiki/Extension:PluggableAuth
#   - OpenID Connect: https://www.mediawiki.org/wiki/Extension:OpenID_Connect
#
# wfLoadExtension('PluggableAuth');
# wfLoadExtension('OpenIDConnect');
#
# $wgPluggableAuth_Config['vgindex'] = [
#     'plugin' => 'OpenIDConnect',
#     'data' => [
#         'providerURL' => 'http://app:3000',
#         'clientID' => 'mediawiki-client',
#         'clientsecret' => 'change-this-secret-mediawiki',
#     ],
# ];
#
# Role mapping (synced from OIDC claims):
# $wgPluggableAuth_Config['vgindex']['data']['preferred_username'] = 'preferred_username';

# File uploads
$wgEnableUploads = true;
$wgFileExtensions = array_merge($wgFileExtensions, ['pdf', 'svg']);
