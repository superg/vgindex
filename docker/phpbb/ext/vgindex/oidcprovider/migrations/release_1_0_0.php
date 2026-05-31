<?php

namespace vgindex\oidcprovider\migrations;

class release_1_0_0 extends \phpbb\db\migration\migration
{
    public static function depends_on()
    {
        return ['\phpbb\db\migration\data\v33x\v331'];
    }

    public function update_schema()
    {
        return [
            'add_tables' => [
                $this->table_prefix . 'vgindex_oidc_clients' => [
                    'COLUMNS' => [
                        'client_id' => ['VCHAR:128', ''],
                        'client_secret_hash' => ['VCHAR:255', ''],
                        'redirect_uris' => ['TEXT_UNI', '[]'],
                        'active' => ['BOOL', 1],
                        'first_party' => ['BOOL', 1],
                        'created_at' => ['TIMESTAMP', 0],
                        'updated_at' => ['TIMESTAMP', 0],
                    ],
                    'PRIMARY_KEY' => 'client_id',
                ],
                $this->table_prefix . 'vgindex_oidc_auth_codes' => [
                    'COLUMNS' => [
                        'code_hash' => ['CHAR:64', ''],
                        'client_id' => ['VCHAR:128', ''],
                        'user_id' => ['UINT', 0],
                        'redirect_uri' => ['TEXT_UNI', ''],
                        'scope' => ['VCHAR:255', 'openid'],
                        'nonce' => ['VCHAR:255', ''],
                        'code_challenge' => ['VCHAR:128', ''],
                        'code_challenge_method' => ['VCHAR:16', ''],
                        'created_at' => ['TIMESTAMP', 0],
                        'expires_at' => ['TIMESTAMP', 0],
                    ],
                    'PRIMARY_KEY' => 'code_hash',
                    'KEYS' => [
                        'client_id' => ['INDEX', 'client_id'],
                        'expires_at' => ['INDEX', 'expires_at'],
                    ],
                ],
                $this->table_prefix . 'vgindex_oidc_access_tokens' => [
                    'COLUMNS' => [
                        'token_hash' => ['CHAR:64', ''],
                        'client_id' => ['VCHAR:128', ''],
                        'user_id' => ['UINT', 0],
                        'scope' => ['VCHAR:255', 'openid'],
                        'created_at' => ['TIMESTAMP', 0],
                        'expires_at' => ['TIMESTAMP', 0],
                    ],
                    'PRIMARY_KEY' => 'token_hash',
                    'KEYS' => [
                        'client_id' => ['INDEX', 'client_id'],
                        'user_id' => ['INDEX', 'user_id'],
                        'expires_at' => ['INDEX', 'expires_at'],
                    ],
                ],
            ],
        ];
    }

    public function revert_schema()
    {
        return [
            'drop_tables' => [
                $this->table_prefix . 'vgindex_oidc_access_tokens',
                $this->table_prefix . 'vgindex_oidc_auth_codes',
                $this->table_prefix . 'vgindex_oidc_clients',
            ],
        ];
    }

    public function update_data()
    {
        return [
            ['config.add', ['vgindex_oidc_issuer_url', '']],
            ['config.add', ['vgindex_oidc_authorize_url', '']],
        ];
    }
}
