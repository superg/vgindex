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

    protected static $sso_managed_groups = ['GLOBAL_MODERATORS', 'ADMINISTRATORS'];

    public static function getSubscribedEvents()
    {
        return [
            'core.user_setup' => 'load_language',
            'core.oauth_login_after_check_if_provider_id_has_match' => 'auto_provision',
            'core.auth_oauth_login_after' => 'sync_roles',
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
     * On every SSO login, sync the user's phpBB group memberships
     * to match the role claim from the IdP.
     *
     * Mapping:
     *   User / User+   -> REGISTERED only
     *   Moderator      -> REGISTERED + GLOBAL_MODERATORS
     *   Admin          -> REGISTERED + GLOBAL_MODERATORS + ADMINISTRATORS
     */
    public function sync_roles($event)
    {
        $userinfo = vgindex_service::get_last_userinfo();
        if (!$userinfo || empty($userinfo['role']))
        {
            return;
        }

        $user_row = isset($event['user_row']) && is_array($event['user_row'])
            ? $event['user_row']
            : null;
        if ($user_row === null || !isset($user_row['user_id']))
        {
            return;
        }

        $user_id = (int) $user_row['user_id'];
        if (!$user_id)
        {
            return;
        }

        $role = $userinfo['role'];

        $desired = $this->role_to_groups($role);
        $this->sync_managed_groups($user_id, $desired);
    }

    /**
     * Map an IdP role string to the set of SSO-managed phpBB groups
     * the user should belong to (beyond REGISTERED).
     */
    protected function role_to_groups($role)
    {
        switch ($role)
        {
            case 'Admin':
                return ['GLOBAL_MODERATORS', 'ADMINISTRATORS'];
            case 'Moderator':
                return ['GLOBAL_MODERATORS'];
            default:
                return [];
        }
    }

    /**
     * Idempotently add/remove the user from SSO-managed groups so
     * their memberships match $desired exactly. Non-managed groups
     * (e.g. REGISTERED, custom groups) are never touched.
     */
    protected function sync_managed_groups($user_id, array $desired)
    {
        if (!function_exists('group_user_add'))
        {
            include($this->get_phpbb_root_path() . 'includes/functions_user.php');
        }

        foreach (self::$sso_managed_groups as $group_name)
        {
            $group_id = $this->get_group_id_by_name($group_name);
            if (!$group_id)
            {
                continue;
            }

            $is_member = $this->is_group_member($user_id, $group_id);
            $should_be_member = in_array($group_name, $desired, true);

            if ($should_be_member && !$is_member)
            {
                group_user_add($group_id, [$user_id]);
            }
            else if (!$should_be_member && $is_member)
            {
                group_user_del($group_id, [$user_id]);
            }
        }
    }

    protected function get_group_id_by_name($group_name)
    {
        $sql = 'SELECT group_id FROM ' . GROUPS_TABLE
             . " WHERE group_name = '" . $this->db->sql_escape($group_name) . "'";
        $result = $this->db->sql_query($sql);
        $row = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
        return $row ? (int) $row['group_id'] : 0;
    }

    protected function is_group_member($user_id, $group_id)
    {
        $sql = 'SELECT user_id FROM ' . USER_GROUP_TABLE
             . ' WHERE user_id = ' . (int) $user_id
             . ' AND group_id = ' . (int) $group_id;
        $result = $this->db->sql_query($sql);
        $row = $this->db->sql_fetchrow($result);
        $this->db->sql_freeresult($result);
        return (bool) $row;
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
