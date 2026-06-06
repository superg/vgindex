<?php

namespace vgindex\oidcprovider\service;

class repository
{
    /** @var \phpbb\db\driver\driver_interface */
    private $db;

    /** @var string */
    private $table_prefix;

    public function __construct(\phpbb\db\driver\driver_interface $db, string $table_prefix)
    {
        $this->db = $db;
        $this->table_prefix = $table_prefix;
    }

    public function find_client(string $client_id): ?array
    {
        $sql = 'SELECT * FROM ' . $this->clients_table()
            . " WHERE client_id = '" . $this->db->sql_escape($client_id) . "'";
        return $this->fetch_one($sql);
    }

    public function client_redirect_uris(array $client): array
    {
        $uris = json_decode((string) ($client['redirect_uris'] ?? '[]'), true);
        return is_array($uris) ? array_values(array_filter($uris, 'is_string')) : [];
    }

    public function store_authorization_code(
        string $code_hash,
        string $client_id,
        int $user_id,
        string $redirect_uri,
        string $scope,
        string $nonce,
        string $code_challenge,
        string $code_challenge_method,
        int $expires_at
    ): void {
        $now = time();
        $this->db->sql_query('INSERT INTO ' . $this->auth_codes_table() . ' ' . $this->db->sql_build_array('INSERT', [
            'code_hash' => $code_hash,
            'client_id' => $client_id,
            'user_id' => $user_id,
            'redirect_uri' => $redirect_uri,
            'scope' => $scope,
            'nonce' => $nonce,
            'code_challenge' => $code_challenge,
            'code_challenge_method' => $code_challenge_method,
            'created_at' => $now,
            'expires_at' => $expires_at,
        ]));
    }

    public function consume_authorization_code(string $code_hash): ?array
    {
        $sql = 'SELECT * FROM ' . $this->auth_codes_table()
            . " WHERE code_hash = '" . $this->db->sql_escape($code_hash) . "'";
        $row = $this->fetch_one($sql);
        if (!$row) {
            return null;
        }

        $this->db->sql_query(
            'DELETE FROM ' . $this->auth_codes_table()
            . " WHERE code_hash = '" . $this->db->sql_escape($code_hash) . "'"
        );

        if ((int) $row['expires_at'] < time()) {
            return null;
        }

        return $row;
    }

    public function store_access_token(
        string $token_hash,
        string $client_id,
        int $user_id,
        string $scope,
        int $expires_at
    ): void {
        $now = time();
        $this->db->sql_query('INSERT INTO ' . $this->access_tokens_table() . ' ' . $this->db->sql_build_array('INSERT', [
            'token_hash' => $token_hash,
            'client_id' => $client_id,
            'user_id' => $user_id,
            'scope' => $scope,
            'created_at' => $now,
            'expires_at' => $expires_at,
        ]));
    }

    public function find_access_token(string $token_hash): ?array
    {
        $sql = 'SELECT * FROM ' . $this->access_tokens_table()
            . " WHERE token_hash = '" . $this->db->sql_escape($token_hash) . "'"
            . ' AND expires_at >= ' . time();
        return $this->fetch_one($sql);
    }

    public function find_user(int $user_id): ?array
    {
        $sql = 'SELECT user_id, username, user_email, user_type, user_avatar, user_avatar_type, user_avatar_width, user_avatar_height FROM ' . $this->table_prefix . 'users'
            . ' WHERE user_id = ' . $user_id;
        return $this->fetch_one($sql);
    }

    public function is_user_banned(int $user_id, string $email): bool
    {
        $conditions = ['ban_userid = ' . $user_id];
        if ($email !== '') {
            $conditions[] = "LOWER(ban_email) = LOWER('" . $this->db->sql_escape($email) . "')";
        }

        $sql = 'SELECT 1 FROM ' . $this->table_prefix . 'banlist'
            . ' WHERE ban_exclude = 0'
            . ' AND (ban_end = 0 OR ban_end > ' . time() . ')'
            . ' AND (' . implode(' OR ', $conditions) . ')';

        return $this->fetch_one($sql) !== null;
    }

    public function group_names_for_user(int $user_id): array
    {
        $sql = 'SELECT g.group_name FROM ' . $this->table_prefix . 'user_group ug'
            . ' JOIN ' . $this->table_prefix . 'groups g ON g.group_id = ug.group_id'
            . ' WHERE ug.user_id = ' . $user_id
            . ' AND ug.user_pending = 0';
        $result = $this->db->sql_query($sql);
        $groups = [];
        while ($row = $this->db->sql_fetchrow($result)) {
            $groups[] = (string) $row['group_name'];
        }
        $this->db->sql_freeresult($result);
        return $groups;
    }

    private function fetch_one(string $sql): ?array
    {
        $result = $this->db->sql_query($sql);
        $row = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
        return $row ?: null;
    }

    private function clients_table(): string
    {
        return $this->table_prefix . 'vgindex_oidc_clients';
    }

    private function auth_codes_table(): string
    {
        return $this->table_prefix . 'vgindex_oidc_auth_codes';
    }

    private function access_tokens_table(): string
    {
        return $this->table_prefix . 'vgindex_oidc_access_tokens';
    }
}
