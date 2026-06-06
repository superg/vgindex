<?php

namespace vgindex\oidcprovider\service;

class claims
{
    /** @var repository */
    private $repository;

    public function __construct(repository $repository)
    {
        $this->repository = $repository;
    }

    public function role_for_user(int $user_id): string
    {
        $groups = $this->repository->group_names_for_user($user_id);
        if (in_array('ADMINISTRATORS', $groups, true)) {
            return 'Admin';
        }
        if (in_array('GLOBAL_MODERATORS', $groups, true)) {
            return 'Moderator';
        }
        if (in_array('User+', $groups, true)) {
            return 'User+';
        }
        return 'User';
    }

    public function group_claims_for_role(string $role): array
    {
        return match ($role) {
            'Admin' => ['User', 'User+', 'Moderator', 'Admin'],
            'Moderator' => ['User', 'User+', 'Moderator'],
            'User+' => ['User', 'User+'],
            default => ['User'],
        };
    }

    public function user_claims(array $user, string $role): array
    {
        $user_id = (int) $user['user_id'];
        $claims = [
            'sub' => 'phpbb:' . $user_id,
            'preferred_username' => (string) $user['username'],
            'email' => (string) $user['user_email'],
            'email_verified' => true,
            'role' => $role,
            'groups' => $this->group_claims_for_role($role),
        ];

        $picture = $this->picture_for_user($user);
        if ($picture !== '') {
            $claims['picture'] = $picture;
        }

        return $claims;
    }

    private function picture_for_user(array $user): string
    {
        if ((string) ($user['user_avatar'] ?? '') === '' || (string) ($user['user_avatar_type'] ?? '') === '') {
            return '';
        }
        if (!function_exists('phpbb_get_user_avatar')) {
            return '';
        }

        $html = phpbb_get_user_avatar($user, 'USER_AVATAR', true);
        if (!is_string($html) || $html === '') {
            return '';
        }
        if (!preg_match('/\bsrc="([^"]+)"/', $html, $matches)) {
            return '';
        }

        return $this->absolute_picture_url(html_entity_decode($matches[1], ENT_QUOTES | ENT_HTML5, 'UTF-8'));
    }

    private function absolute_picture_url(string $url): string
    {
        $url = trim($url);
        if ($url === '') {
            return '';
        }
        if (preg_match('#^https?://#i', $url)) {
            return $url;
        }

        $board_url = rtrim(generate_board_url(true), '/');
        if (str_starts_with($url, '//')) {
            $parts = parse_url($board_url);
            if (!is_array($parts) || empty($parts['scheme'])) {
                return '';
            }
            return $parts['scheme'] . ':' . $url;
        }
        if (str_starts_with($url, '/')) {
            $parts = parse_url($board_url);
            if (!is_array($parts) || empty($parts['scheme']) || empty($parts['host'])) {
                return '';
            }
            $origin = $parts['scheme'] . '://' . $parts['host'];
            if (!empty($parts['port'])) {
                $origin .= ':' . $parts['port'];
            }
            return $origin . $url;
        }

        return $board_url . '/' . ltrim($url, './');
    }
}
