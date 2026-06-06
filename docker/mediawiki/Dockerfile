FROM mediawiki:1.42

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends libpq-dev git unzip postgresql-client; \
    docker-php-ext-install -j"$(nproc)" pgsql pdo_pgsql; \
    rm -rf /var/lib/apt/lists/*

COPY --from=composer:2 /usr/bin/composer /usr/bin/composer

RUN set -eux; \
    cd /var/www/html/extensions; \
    git clone --depth 1 -b REL1_42 \
        https://gerrit.wikimedia.org/r/mediawiki/extensions/PluggableAuth; \
    git clone --depth 1 -b REL1_42 \
        https://gerrit.wikimedia.org/r/mediawiki/extensions/OpenIDConnect; \
    sed -i \
        's/DROP[[:space:]][[:space:]]*INDEX "primary";/ALTER TABLE openid_connect DROP CONSTRAINT IF EXISTS openid_connect_pkey;/' \
        /var/www/html/extensions/OpenIDConnect/sql/postgres/ChangePrimaryKey.sql; \
    grep -q 'DROP CONSTRAINT IF EXISTS openid_connect_pkey' \
        /var/www/html/extensions/OpenIDConnect/sql/postgres/ChangePrimaryKey.sql; \
    cd OpenIDConnect && \
    COMPOSER_ALLOW_SUPERUSER=1 composer install --no-dev --no-interaction; \
    rm -rf /var/www/html/extensions/*/.git

COPY docker/mediawiki/entrypoint.sh /usr/local/bin/mediawiki-entrypoint.sh
ENTRYPOINT ["mediawiki-entrypoint.sh"]
CMD ["apache2-foreground"]
