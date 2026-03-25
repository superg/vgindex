<?php

namespace vgindex\oidc\event;

use Symfony\Component\EventDispatcher\EventSubscriberInterface;
use vgindex\oidc\service\vgindex as vgindex_service;

class listener implements EventSubscriberInterface
{
    /** @var \phpbb\db\driver\driver_interface */
    protected $db;

    /** @var \phpbb\config\config */
    protected $config;

    /** @var \phpbb\passwords\manager */
    protected $passwords_manager;

    /** @var \phpbb\user */
    protected $user;

    /** @var string */
    protected $users_table;

    /** @var string */
    protected $oauth_account_table;

    public function __construct(
        \phpbb\db\driver\driver_interface $db,
        \phpbb\config\config $config,
        \phpbb\passwords\manager $passwords_manager,
        \phpbb\user $user,
        $users_table,
        $oauth_account_table
    )
    {
        $this->db = $db;
        $this->config = $config;
        $this->passwords_manager = $passwords_manager;
        $this->user = $user;
        $this->users_table = $users_table;
        $this->oauth_account_table = $oauth_account_table;
    }

    public static function getSubscribedEvents()
    {
        return [
            'core.user_setup' => 'load_language',
            'core.oauth_login_after_check_if_provider_id_has_match' => 'auto_provision',
        ];
    }

    public function load_language($event)
    {
        $lang_set_ext = $event['lang_set_ext'];
        $lang_set_ext[] = [
            'ext_name' => 'vgindex/oidc',
            'lang_set' => 'auth_provider_oauth',
        ];
        $event['lang_set_ext'] = $lang_set_ext;
    }

    /**
     * If the OAuth provider ID has no linked phpBB account, create one
     * automatically from the OIDC userinfo claims.
     */
    public function auto_provision($event)
    {
        $row = $event['row'];

        if ($row)
        {
            return;
        }

        $userinfo = vgindex_service::get_last_userinfo();
        if (!$userinfo || empty($userinfo['sub']))
        {
            return;
        }

        $data = $event['data'];

        $username = $this->resolve_username($userinfo);
        $email = !empty($userinfo['email']) ? $userinfo['email'] : $username . '@sso.local';

        if (!function_exists('user_add'))
        {
            include($this->get_phpbb_root_path() . 'includes/functions_user.php');
        }

        $group_id = $this->get_registered_group_id();

        $user_row = [
            'username'      => $username,
            'user_password' => $this->passwords_manager->hash(bin2hex(random_bytes(16))),
            'user_email'    => $email,
            'group_id'      => $group_id,
            'user_type'     => USER_NORMAL,
            'user_regdate'  => time(),
            'user_ip'       => $this->user->ip,
            'user_lang'     => $this->config['default_lang'],
            'user_style'    => (int) $this->config['default_style'],
            'user_timezone' => $this->config['board_timezone'],
        ];

        $user_id = user_add($user_row);

        if ($user_id === false)
        {
            return;
        }

        $link = [
            'user_id'           => (int) $user_id,
            'provider'          => $data['provider'],
            'oauth_provider_id' => $data['oauth_provider_id'],
        ];
        $sql = 'INSERT INTO ' . $this->oauth_account_table . ' '
             . $this->db->sql_build_array('INSERT', $link);
        $this->db->sql_query($sql);

        $sql = 'SELECT user_id FROM ' . $this->users_table
             . ' WHERE user_id = ' . (int) $user_id;
        $result = $this->db->sql_query($sql);
        $event['row'] = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
    }

    /**
     * Pick a phpBB username from OIDC claims, handling collisions
     * by appending a deterministic numeric suffix.
     */
    protected function resolve_username(array $userinfo)
    {
        $base = !empty($userinfo['preferred_username'])
            ? $userinfo['preferred_username']
            : ('user_' . substr($userinfo['sub'], 0, 8));

        $base = $this->sanitize_username($base);

        if (!$this->username_exists($base))
        {
            return $base;
        }

        for ($i = 2; $i < 1000; $i++)
        {
            $candidate = $base . $i;
            if (!$this->username_exists($candidate))
            {
                return $candidate;
            }
        }

        return $base . '_' . substr(md5($userinfo['sub']), 0, 6);
    }

    protected function sanitize_username($name)
    {
        $name = preg_replace('/[^\w\-.]/', '_', $name);
        $name = substr($name, 0, 60);
        if (empty($name))
        {
            $name = 'oidc_user';
        }
        return $name;
    }

    protected function username_exists($username)
    {
        $clean = utf8_clean_string($username);
        $sql = 'SELECT user_id FROM ' . $this->users_table
             . " WHERE username_clean = '" . $this->db->sql_escape($clean) . "'";
        $result = $this->db->sql_query($sql);
        $row = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
        return (bool) $row;
    }

    protected function get_registered_group_id()
    {
        $sql = "SELECT group_id FROM " . GROUPS_TABLE
             . " WHERE group_name = 'REGISTERED'";
        $result = $this->db->sql_query($sql);
        $row = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
        return $row ? (int) $row['group_id'] : 2;
    }

    protected function get_phpbb_root_path()
    {
        global $phpbb_root_path;
        return $phpbb_root_path ?: '/var/www/html/';
    }
}
