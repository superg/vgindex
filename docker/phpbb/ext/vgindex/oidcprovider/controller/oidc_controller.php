<?php

namespace vgindex\oidcprovider\controller;

use phpbb\config\config;
use phpbb\request\request_interface;
use phpbb\user;
use Symfony\Component\HttpFoundation\JsonResponse;
use Symfony\Component\HttpFoundation\RedirectResponse;
use Symfony\Component\HttpFoundation\Response;
use vgindex\oidcprovider\service\claims;
use vgindex\oidcprovider\service\crypto;
use vgindex\oidcprovider\service\repository;

class oidc_controller
{
    private const AUTH_CODE_TTL = 300;
    private const ACCESS_TOKEN_TTL = 3600;
    private const ALLOWED_SCOPES = ['openid', 'profile', 'email'];

    /** @var config */
    private $config;

    /** @var request_interface */
    private $request;

    /** @var user */
    private $user;

    /** @var repository */
    private $repository;

    /** @var crypto */
    private $crypto;

    /** @var claims */
    private $claims;

    public function __construct(
        config $config,
        request_interface $request,
        user $user,
        repository $repository,
        crypto $crypto,
        claims $claims
    ) {
        $this->config = $config;
        $this->request = $request;
        $this->user = $user;
        $this->repository = $repository;
        $this->crypto = $crypto;
        $this->claims = $claims;
    }

    public function discovery(): JsonResponse
    {
        $issuer = $this->issuer();
        $authorization_endpoint = trim((string) $this->config['vgindex_oidc_authorize_url']);
        if ($authorization_endpoint === '') {
            $authorization_endpoint = $issuer . '/authorize';
        }

        return $this->json([
            'issuer' => $issuer,
            'authorization_endpoint' => $authorization_endpoint,
            'token_endpoint' => $issuer . '/token',
            'userinfo_endpoint' => $issuer . '/userinfo',
            'jwks_uri' => $issuer . '/jwks',
            'response_types_supported' => ['code'],
            'grant_types_supported' => ['authorization_code'],
            'subject_types_supported' => ['public'],
            'id_token_signing_alg_values_supported' => ['RS256'],
            'scopes_supported' => self::ALLOWED_SCOPES,
            'claims_supported' => [
                'sub',
                'preferred_username',
                'email',
                'email_verified',
                'groups',
                'role',
                'picture',
            ],
            'token_endpoint_auth_methods_supported' => ['client_secret_basic', 'client_secret_post'],
            'code_challenge_methods_supported' => ['S256'],
        ]);
    }

    public function jwks(): JsonResponse
    {
        return $this->json(['keys' => [$this->crypto->jwk()]]);
    }

    public function authorize(): Response
    {
        $client_id = $this->request->variable('client_id', '');
        $redirect_uri = $this->request->variable('redirect_uri', '');
        $response_type = $this->request->variable('response_type', '');
        $scope = $this->request->variable('scope', 'openid');
        $state = $this->request->variable('state', '');
        $nonce = $this->request->variable('nonce', '');
        $code_challenge = $this->request->variable('code_challenge', '');
        $code_challenge_method = $this->request->variable('code_challenge_method', '');

        $client = $this->repository->find_client($client_id);
        if (!$client || !(bool) $client['active']) {
            return $this->plain_error('invalid_client', 400);
        }
        if (!(bool) $client['first_party']) {
            return $this->plain_error('consent_required', 403);
        }
        if (!in_array($redirect_uri, $this->repository->client_redirect_uris($client), true)) {
            return $this->plain_error('redirect_uri mismatch', 400);
        }
        if ($response_type !== 'code') {
            return $this->redirect_error($redirect_uri, $state, 'unsupported_response_type');
        }

        $normalized_scope = $this->normalize_scope($scope);
        if ($normalized_scope === null) {
            return $this->redirect_error($redirect_uri, $state, 'invalid_scope');
        }
        if (($code_challenge === '' && $code_challenge_method !== '') || ($code_challenge !== '' && $code_challenge_method !== 'S256')) {
            return $this->redirect_error($redirect_uri, $state, 'invalid_request', 'Only S256 PKCE is supported.');
        }

        if (!$this->is_registered_user()) {
            return new RedirectResponse($this->login_url());
        }

        $user_id = (int) $this->user->data['user_id'];
        $code = $this->crypto->random_token();
        $this->repository->store_authorization_code(
            $this->crypto->hash_token($code),
            $client_id,
            $user_id,
            $redirect_uri,
            $normalized_scope,
            $nonce,
            $code_challenge,
            $code_challenge_method,
            time() + self::AUTH_CODE_TTL
        );

        return new RedirectResponse($this->with_query($redirect_uri, [
            'code' => $code,
            'state' => $state,
        ]));
    }

    public function token(): JsonResponse
    {
        $grant_type = $this->request->variable('grant_type', '');
        if ($grant_type !== 'authorization_code') {
            return $this->oauth_error('unsupported_grant_type', 400);
        }

        [$client_id, $client_secret] = $this->client_credentials();
        if ($client_id === '' || $client_secret === '') {
            return $this->oauth_error('invalid_client', 401);
        }

        $client = $this->repository->find_client($client_id);
        if (!$client || !(bool) $client['active'] || !password_verify($client_secret, (string) $client['client_secret_hash'])) {
            return $this->oauth_error('invalid_client', 401);
        }

        $code = $this->request->variable('code', '');
        $redirect_uri = $this->request->variable('redirect_uri', '');
        $auth_code = $this->repository->consume_authorization_code($this->crypto->hash_token($code));
        if (!$auth_code || (string) $auth_code['client_id'] !== $client_id || (string) $auth_code['redirect_uri'] !== $redirect_uri) {
            return $this->oauth_error('invalid_grant', 400);
        }

        if ((string) $auth_code['code_challenge'] !== '') {
            $verifier = $this->request->variable('code_verifier', '');
            if ($verifier === '' || !hash_equals((string) $auth_code['code_challenge'], $this->crypto->pkce_s256($verifier))) {
                return $this->oauth_error('invalid_grant', 400);
            }
        }

        $user = $this->repository->find_user((int) $auth_code['user_id']);
        if (!$user || !$this->is_authenticatable_user($user)) {
            return $this->oauth_error('invalid_grant', 400);
        }

        $access_token = $this->crypto->random_token();
        $this->repository->store_access_token(
            $this->crypto->hash_token($access_token),
            $client_id,
            (int) $user['user_id'],
            (string) $auth_code['scope'],
            time() + self::ACCESS_TOKEN_TTL
        );

        $role = $this->claims->role_for_user((int) $user['user_id']);
        $now = time();
        $id_claims = array_merge($this->claims->user_claims($user, $role), [
            'iss' => $this->issuer(),
            'aud' => $client_id,
            'iat' => $now,
            'exp' => $now + self::ACCESS_TOKEN_TTL,
        ]);
        if ((string) $auth_code['nonce'] !== '') {
            $id_claims['nonce'] = (string) $auth_code['nonce'];
        }

        return $this->json([
            'access_token' => $access_token,
            'token_type' => 'Bearer',
            'expires_in' => self::ACCESS_TOKEN_TTL,
            'scope' => (string) $auth_code['scope'],
            'id_token' => $this->crypto->sign_jwt($id_claims),
        ]);
    }

    public function userinfo(): JsonResponse
    {
        $token = $this->bearer_token();
        if ($token === '') {
            return $this->oauth_error('invalid_token', 401);
        }

        $row = $this->repository->find_access_token($this->crypto->hash_token($token));
        if (!$row) {
            return $this->oauth_error('invalid_token', 401);
        }

        $user = $this->repository->find_user((int) $row['user_id']);
        if (!$user || !$this->is_authenticatable_user($user)) {
            return $this->oauth_error('invalid_token', 401);
        }

        $role = $this->claims->role_for_user((int) $user['user_id']);
        return $this->json($this->claims->user_claims($user, $role));
    }

    private function issuer(): string
    {
        $issuer = trim((string) $this->config['vgindex_oidc_issuer_url']);
        if ($issuer !== '') {
            return rtrim($issuer, '/');
        }
        return rtrim(generate_board_url(true), '/') . '/app.php/oidc';
    }

    private function normalize_scope(string $scope): ?string
    {
        $parts = preg_split('/\s+/', trim($scope));
        $parts = array_values(array_unique(array_filter($parts ?: [])));
        if (!$parts) {
            $parts = ['openid'];
        }
        if (!in_array('openid', $parts, true)) {
            return null;
        }
        foreach ($parts as $part) {
            if (!in_array($part, self::ALLOWED_SCOPES, true)) {
                return null;
            }
        }
        return implode(' ', $parts);
    }

    private function client_credentials(): array
    {
        $php_auth_user = (string) $this->request->server('PHP_AUTH_USER', '');
        $php_auth_pw = (string) $this->request->server('PHP_AUTH_PW', '');
        if ($php_auth_user !== '' || $php_auth_pw !== '') {
            return [rawurldecode($php_auth_user), rawurldecode($php_auth_pw)];
        }

        $auth = $this->request->header('Authorization', '');
        if ($auth === '') {
            $auth = (string) $this->request->server('HTTP_AUTHORIZATION', '');
        }
        if ($auth === '') {
            $auth = (string) $this->request->server('REDIRECT_HTTP_AUTHORIZATION', '');
        }
        if (str_starts_with($auth, 'Basic ')) {
            $decoded = base64_decode(substr($auth, 6), true);
            if (is_string($decoded) && str_contains($decoded, ':')) {
                [$client_id, $client_secret] = explode(':', $decoded, 2);
                return [rawurldecode($client_id), rawurldecode($client_secret)];
            }
        }

        return [
            $this->request->variable('client_id', ''),
            $this->request->variable('client_secret', ''),
        ];
    }

    private function bearer_token(): string
    {
        $auth = $this->request->header('Authorization', '');
        if ($auth === '') {
            $auth = (string) $this->request->server('HTTP_AUTHORIZATION', '');
        }
        if ($auth === '') {
            $auth = (string) $this->request->server('REDIRECT_HTTP_AUTHORIZATION', '');
        }
        return str_starts_with($auth, 'Bearer ') ? substr($auth, 7) : '';
    }

    private function is_registered_user(): bool
    {
        return isset($this->user->data['user_id'])
            && $this->is_authenticatable_user($this->user->data);
    }

    private function is_authenticatable_user(array $user): bool
    {
        $user_id = (int) ($user['user_id'] ?? 0);
        $user_type = (int) ($user['user_type'] ?? -1);
        return $user_id !== (defined('ANONYMOUS') ? ANONYMOUS : 1)
            && in_array($user_type, [
                defined('USER_NORMAL') ? USER_NORMAL : 0,
                defined('USER_FOUNDER') ? USER_FOUNDER : 3,
            ], true)
            && !$this->repository->is_user_banned($user_id, (string) ($user['user_email'] ?? ''));
    }

    private function login_url(): string
    {
        $return_to = htmlspecialchars_decode((string) $this->request->server('REQUEST_URI'), ENT_QUOTES);
        $return_to = ltrim($return_to, '/');
        return rtrim(generate_board_url(true), '/') . '/ucp.php?mode=login&redirect=' . rawurlencode($return_to);
    }

    private function with_query(string $uri, array $params): string
    {
        $params = array_filter($params, static fn ($value): bool => $value !== '');
        return $uri . (str_contains($uri, '?') ? '&' : '?') . http_build_query($params, '', '&', PHP_QUERY_RFC3986);
    }

    private function redirect_error(string $redirect_uri, string $state, string $error, string $description = ''): RedirectResponse
    {
        $params = ['error' => $error, 'state' => $state];
        if ($description !== '') {
            $params['error_description'] = $description;
        }
        return new RedirectResponse($this->with_query($redirect_uri, $params));
    }

    private function plain_error(string $message, int $status): Response
    {
        return new Response($message, $status, ['Content-Type' => 'text/plain; charset=UTF-8']);
    }

    private function oauth_error(string $error, int $status): JsonResponse
    {
        return $this->json(['error' => $error], $status);
    }

    private function json(array $payload, int $status = 200): JsonResponse
    {
        $response = new JsonResponse($payload, $status);
        $response->headers->set('Cache-Control', 'no-store');
        $response->headers->set('Pragma', 'no-cache');
        return $response;
    }
}
