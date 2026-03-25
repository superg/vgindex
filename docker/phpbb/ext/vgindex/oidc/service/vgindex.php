<?php

namespace vgindex\oidc\service;

class vgindex extends \phpbb\auth\provider\oauth\service\base
{
    /** @var \phpbb\config\config */
    protected $config;

    /** @var \phpbb\request\request_interface */
    protected $request;

    /**
     * Stash the latest userinfo claims so the auto-provision listener
     * can read preferred_username / email without a second HTTP call.
     *
     * @var array|null
     */
    protected static $last_userinfo = null;

    public function __construct(\phpbb\config\config $config, \phpbb\request\request_interface $request)
    {
        $this->config = $config;
        $this->request = $request;
    }

    public function get_auth_scope()
    {
        return ['openid', 'profile', 'email'];
    }

    public function get_service_credentials()
    {
        return [
            'key' => $this->config['auth_oauth_vgindex_key'],
            'secret' => $this->config['auth_oauth_vgindex_secret'],
        ];
    }

    public function get_external_service_class()
    {
        return '\OAuth\OAuth2\Service\VgindexOidc';
    }

    public function perform_auth_login()
    {
        if (!($this->service_provider instanceof \OAuth\OAuth2\Service\VgindexOidc))
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_INVALID_SERVICE_TYPE');
        }

        try
        {
            $this->service_provider->requestAccessToken(
                $this->request->variable('code', '')
            );
        }
        catch (\OAuth\Common\Http\Exception\TokenResponseException $e)
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_REQUEST');
        }
        catch (\OAuth\OAuth2\Service\Exception\InvalidAuthorizationStateException $e)
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_REQUEST');
        }

        return $this->read_subject_from_userinfo();
    }

    public function perform_token_auth()
    {
        if (!($this->service_provider instanceof \OAuth\OAuth2\Service\VgindexOidc))
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_INVALID_SERVICE_TYPE');
        }

        return $this->read_subject_from_userinfo();
    }

    /**
     * Read userinfo from IdP; stash full claims for auto-provisioning.
     */
    protected function read_subject_from_userinfo()
    {
        try
        {
            $result = json_decode($this->service_provider->request($this->get_userinfo_endpoint()), true);
        }
        catch (\OAuth\Common\Exception\Exception $e)
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_REQUEST');
        }

        if (!is_array($result) || !isset($result['sub']))
        {
            throw new \phpbb\auth\provider\oauth\service\exception('AUTH_PROVIDER_OAUTH_ERROR_REQUEST');
        }

        self::$last_userinfo = $result;

        return (string) $result['sub'];
    }

    /**
     * @return array|null  The latest userinfo claims from the IdP.
     */
    public static function get_last_userinfo()
    {
        return self::$last_userinfo;
    }

    protected function get_userinfo_endpoint()
    {
        $issuer = getenv('OIDC_ISSUER_URL');
        if (!$issuer)
        {
            $issuer = 'http://app:3000';
        }
        return rtrim($issuer, '/') . '/oauth/userinfo';
    }
}
