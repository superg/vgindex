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
        return [
            'sub' => 'phpbb:' . $user_id,
            'preferred_username' => (string) $user['username'],
            'email' => (string) $user['user_email'],
            'email_verified' => true,
            'role' => $role,
            'groups' => $this->group_claims_for_role($role),
        ];
    }
}
