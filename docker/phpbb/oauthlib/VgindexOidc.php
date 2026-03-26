<?php

namespace OAuth\OAuth2\Service;

use OAuth\Common\Consumer\CredentialsInterface;
use OAuth\Common\Http\Client\ClientInterface;
use OAuth\Common\Http\Exception\TokenResponseException;
use OAuth\Common\Http\Uri\Uri;
use OAuth\Common\Http\Uri\UriInterface;
use OAuth\Common\Storage\TokenStorageInterface;
use OAuth\OAuth2\Token\StdOAuth2Token;

class VgindexOidc extends AbstractService
{
    const SCOPE_OPENID = 'openid';
    const SCOPE_PROFILE = 'profile';
    const SCOPE_EMAIL = 'email';

    public function __construct(
        CredentialsInterface $credentials,
        ClientInterface $httpClient,
        TokenStorageInterface $storage,
        $scopes = [],
        UriInterface $baseApiUri = null
    ) {
        if (empty($scopes))
        {
            $scopes = [self::SCOPE_OPENID, self::SCOPE_PROFILE, self::SCOPE_EMAIL];
        }

        if (null === $baseApiUri)
        {
            $baseApiUri = new Uri($this->getInternalIssuerBase() . '/');
        }

        parent::__construct($credentials, $httpClient, $storage, $scopes, $baseApiUri, true);
    }

    public function getAuthorizationEndpoint()
    {
        return new Uri($this->getPublicBase() . '/oauth/authorize');
    }

    public function getAccessTokenEndpoint()
    {
        return new Uri($this->getInternalIssuerBase() . '/oauth/token');
    }

    protected function getAuthorizationMethod()
    {
        return static::AUTHORIZATION_METHOD_HEADER_BEARER;
    }

    protected function parseAccessTokenResponse($responseBody)
    {
        $data = json_decode($responseBody, true);

        if (null === $data || !is_array($data))
        {
            throw new TokenResponseException('Unable to parse token response.');
        }
        if (isset($data['error']))
        {
            throw new TokenResponseException('Error retrieving token: ' . $data['error']);
        }
        if (!isset($data['access_token']))
        {
            throw new TokenResponseException('Missing access_token in response.');
        }

        $token = new StdOAuth2Token();
        $token->setAccessToken($data['access_token']);
        $token->setLifetime(isset($data['expires_in']) ? (int) $data['expires_in'] : 3600);

        if (isset($data['refresh_token']))
        {
            $token->setRefreshToken($data['refresh_token']);
            unset($data['refresh_token']);
        }

        unset($data['access_token'], $data['expires_in']);
        $token->setExtraParams($data);

        return $token;
    }

    protected function getPublicBase()
    {
        $explicit = getenv('OIDC_PUBLIC_BASE_URL');
        if ($explicit)
        {
            return rtrim($explicit, '/');
        }

        $domain = getenv('DOMAIN');
        if (!$domain) { throw new \RuntimeException('DOMAIN env var is not set'); }
        $port = getenv('HTTPS_PORT');
        if (!$port) { throw new \RuntimeException('HTTPS_PORT env var is not set'); }
        if ((string) $port === '443')
        {
            return 'https://www.' . $domain;
        }
        return 'https://www.' . $domain . ':' . $port;
    }

    protected function getInternalIssuerBase()
    {
        $issuer = getenv('OIDC_ISSUER_URL');
        if (!$issuer)
        {
            throw new \RuntimeException('OIDC_ISSUER_URL env var is not set');
        }
        return rtrim($issuer, '/');
    }
}
