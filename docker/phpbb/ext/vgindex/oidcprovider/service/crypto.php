<?php

namespace vgindex\oidcprovider\service;

class crypto
{
    /** @var string */
    private $key_path;

    public function __construct(string $root_path)
    {
        $this->key_path = rtrim($root_path, '/') . '/store/vgindex_oidc_private_key.pem';
    }

    public function random_token(int $bytes = 32): string
    {
        return $this->base64url(random_bytes($bytes));
    }

    public function hash_token(string $token): string
    {
        return hash('sha256', $token);
    }

    public function pkce_s256(string $verifier): string
    {
        return $this->base64url(hash('sha256', $verifier, true));
    }

    public function jwk(): array
    {
        $details = openssl_pkey_get_details($this->private_key());
        if (!is_array($details) || empty($details['rsa']['n']) || empty($details['rsa']['e'])) {
            throw new \RuntimeException('Could not inspect OIDC signing key.');
        }

        return [
            'kty' => 'RSA',
            'use' => 'sig',
            'alg' => 'RS256',
            'kid' => $this->kid(),
            'n' => $this->base64url($details['rsa']['n']),
            'e' => $this->base64url($details['rsa']['e']),
        ];
    }

    public function sign_jwt(array $claims): string
    {
        $header = [
            'typ' => 'JWT',
            'alg' => 'RS256',
            'kid' => $this->kid(),
        ];

        $segments = [
            $this->base64url(json_encode($header, JSON_UNESCAPED_SLASHES)),
            $this->base64url(json_encode($claims, JSON_UNESCAPED_SLASHES)),
        ];
        $payload = implode('.', $segments);

        $signature = '';
        if (!openssl_sign($payload, $signature, $this->private_key(), OPENSSL_ALGO_SHA256)) {
            throw new \RuntimeException('Could not sign OIDC ID token.');
        }

        $segments[] = $this->base64url($signature);
        return implode('.', $segments);
    }

    public function kid(): string
    {
        $details = openssl_pkey_get_details($this->private_key());
        if (!is_array($details) || empty($details['key'])) {
            throw new \RuntimeException('Could not inspect OIDC signing key.');
        }
        return substr(hash('sha256', $details['key']), 0, 16);
    }

    private function private_key()
    {
        $this->ensure_key_exists();
        $key = openssl_pkey_get_private((string) file_get_contents($this->key_path));
        if ($key === false) {
            throw new \RuntimeException('Could not load OIDC signing key.');
        }
        return $key;
    }

    private function ensure_key_exists(): void
    {
        if (is_file($this->key_path) && filesize($this->key_path) > 0) {
            return;
        }

        $dir = dirname($this->key_path);
        if (!is_dir($dir)) {
            mkdir($dir, 0700, true);
        }

        $key = openssl_pkey_new([
            'private_key_bits' => 2048,
            'private_key_type' => OPENSSL_KEYTYPE_RSA,
        ]);
        if ($key === false) {
            throw new \RuntimeException('Could not generate OIDC signing key.');
        }

        $pem = '';
        if (!openssl_pkey_export($key, $pem)) {
            throw new \RuntimeException('Could not export OIDC signing key.');
        }

        file_put_contents($this->key_path, $pem);
        chmod($this->key_path, 0600);
    }

    private function base64url(string $value): string
    {
        return rtrim(strtr(base64_encode($value), '+/', '-_'), '=');
    }
}
