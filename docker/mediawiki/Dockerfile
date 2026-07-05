FROM mediawiki:1.46.0@sha256:51756779bb215c32842dfa38276813f504111c9624e2e4ec0c52204745d52aba

LABEL org.vgindex.mediawiki.schema-generation="1.46"

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends libpq-dev git unzip postgresql-client; \
    docker-php-ext-install -j"$(nproc)" pgsql pdo_pgsql; \
    rm -rf /var/lib/apt/lists/*

COPY --from=composer:2@sha256:7725eb4545c438629ae8bde3ef0bb9a5038ef566126ad878442a69007242d267 /usr/bin/composer /usr/bin/composer

RUN set -eux; \
    cd /var/www/html/extensions; \
    git clone --branch REL1_46 \
        https://gerrit.wikimedia.org/r/mediawiki/extensions/PluggableAuth; \
    git -C PluggableAuth checkout --detach b1f17d7930e25fbc495a1b872316251fcc546694; \
    git clone --branch REL1_46 \
        https://gerrit.wikimedia.org/r/mediawiki/extensions/OpenIDConnect; \
    git -C OpenIDConnect checkout --detach ebf21a832f26a17541ab03f9b0a45dbb88b2dcdf; \
    cd OpenIDConnect && \
    COMPOSER_ALLOW_SUPERUSER=1 composer require --no-update --no-interaction \
        composer/installers:2.3.0 \
        paragonie/constant_time_encoding:3.1.3 \
        paragonie/random_compat:9.99.100 \
        phpseclib/phpseclib:3.0.55; \
    COMPOSER_ALLOW_SUPERUSER=1 composer install --no-dev --no-interaction; \
    rm -rf /var/www/html/extensions/*/.git

COPY docker/mediawiki/entrypoint.sh /usr/local/bin/mediawiki-entrypoint.sh
COPY docker/mediawiki/healthcheck.php /usr/local/bin/mediawiki-healthcheck.php
ENTRYPOINT ["mediawiki-entrypoint.sh"]
CMD ["serve"]
