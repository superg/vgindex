FROM mediawiki:1.46.2@sha256:17d7c4fa00f2073c18800a08885020ec36c4fb1bab04f9195d9253c285d2b387

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends libpq-dev git unzip postgresql-client; \
    docker-php-ext-install -j"$(nproc)" pgsql pdo_pgsql; \
    rm -rf /var/lib/apt/lists/*

COPY docker/apache/mpm_prefork.conf /etc/apache2/mods-available/mpm_prefork.conf
COPY docker/php/security.ini /usr/local/etc/php/conf.d/security.ini

COPY --from=composer:2@sha256:7725eb4545c438629ae8bde3ef0bb9a5038ef566126ad878442a69007242d267 /usr/bin/composer /usr/bin/composer

RUN set -eux; \
    cd /var/www/html/extensions; \
    git clone --branch REL1_46 \
        https://github.com/wikimedia/mediawiki-extensions-PluggableAuth PluggableAuth; \
    git -C PluggableAuth checkout --detach b1f17d7930e25fbc495a1b872316251fcc546694; \
    git clone --branch REL1_46 \
        https://github.com/wikimedia/mediawiki-extensions-OpenIDConnect OpenIDConnect; \
    git -C OpenIDConnect checkout --detach ebf21a832f26a17541ab03f9b0a45dbb88b2dcdf; \
    cd OpenIDConnect && \
    COMPOSER_ALLOW_SUPERUSER=1 composer remove --dev --no-update --no-interaction \
        mediawiki/mediawiki-codesniffer \
        mediawiki/minus-x \
        php-parallel-lint/php-console-highlighter \
        php-parallel-lint/php-parallel-lint; \
    COMPOSER_ALLOW_SUPERUSER=1 composer require --no-update --no-interaction \
        composer/installers:2.3.0 \
        paragonie/constant_time_encoding:3.1.3 \
        paragonie/random_compat:9.99.100 \
        phpseclib/phpseclib:3.0.57; \
    COMPOSER_ALLOW_SUPERUSER=1 composer install --no-dev --no-interaction; \
    rm -rf /var/www/html/extensions/*/.git

COPY docker/mediawiki/entrypoint.sh /usr/local/bin/mediawiki-entrypoint.sh
ENTRYPOINT ["mediawiki-entrypoint.sh"]
CMD ["apache2-foreground"]
