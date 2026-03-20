# phpBB OAuth2/OIDC Integration

## Setup Steps

1. Install phpBB OAuth2 authentication extension (e.g. `phpbb/phpbb-ext-oauth`)
2. In phpBB Admin Control Panel:
   - Go to General > Authentication
   - Enable OAuth2 provider
   - Configure with these settings:
     - **Provider URL**: `http://app:3000`
     - **Client ID**: `phpbb-client`
     - **Client Secret**: `change-this-secret-phpbb`
     - **Authorization Endpoint**: `http://app:3000/oauth/authorize`
     - **Token Endpoint**: `http://app:3000/oauth/token`
     - **Userinfo Endpoint**: `http://app:3000/oauth/userinfo`
     - **Scopes**: `openid profile email`
3. Map roles from OIDC claims to phpBB groups:
   - `User` -> Registered Users
   - `UserPlus` -> Registered Users + "Verified Dumpers" group
   - `Moderator` -> Global Moderators
   - `Admin` -> Administrators

## Creating the "News" forum category

Create a forum category called "News" in phpBB. Posts from this category
will appear on the vgindex.org homepage via direct database query.
