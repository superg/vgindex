<?php

namespace vgindex\oidcprovider;

class ext extends \phpbb\extension\base
{
    public function is_enableable()
    {
        return phpbb_version_compare(PHPBB_VERSION, '3.3.0', '>=')
            && PHP_VERSION_ID >= 80100
            && extension_loaded('openssl');
    }
}
