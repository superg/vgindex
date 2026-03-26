UPDATE oauth_clients
SET redirect_uri = 'https://wiki.localhost:8443/Special:PluggableAuthLogin'
WHERE client_id = 'mediawiki-client';

UPDATE oauth_clients
SET redirect_uri = 'https://forum.localhost:8443/ucp.php?mode=login'
WHERE client_id = 'phpbb-client';
