FROM php:8.2-apache-bookworm

ARG PHPBB_VERSION=3.3.17
ENV PHPBB_VERSION=${PHPBB_VERSION}

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        libicu-dev \
        libpq-dev \
        libzip-dev \
        libonig-dev \
        libxml2-dev \
        postgresql-client \
        unzip \
        zlib1g-dev; \
    docker-php-ext-install -j"$(nproc)" \
        intl \
        mbstring \
        opcache \
        pdo_pgsql \
        pgsql \
        xml \
        zip; \
    a2enmod rewrite remoteip; \
    printf '%s\n' 'SetEnvIfNoCase Authorization "^(.*)$" HTTP_AUTHORIZATION=$1' \
        > /etc/apache2/conf-available/auth-headers.conf; \
    a2enconf auth-headers; \
    rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl -fsSL -o /tmp/phpbb.zip \
        "https://download.phpbb.com/pub/release/3.3/${PHPBB_VERSION}/phpBB-${PHPBB_VERSION}.zip"; \
    unzip /tmp/phpbb.zip -d /tmp; \
    rm -rf /var/www/html/*; \
    cp -a /tmp/phpBB3/. /var/www/html/; \
    rm -rf /tmp/phpbb.zip /tmp/phpBB3; \
    chown -R www-data:www-data /var/www/html

COPY docker/phpbb/ext/vgindex/oidcprovider /var/www/html/ext/vgindex/oidcprovider

COPY docker/phpbb/entrypoint.sh /usr/local/bin/phpbb-entrypoint.sh
RUN set -eux; \
    chown -R www-data:www-data /var/www/html/ext/vgindex; \
    chmod +x /usr/local/bin/phpbb-entrypoint.sh

WORKDIR /var/www/html

ENTRYPOINT ["phpbb-entrypoint.sh"]
CMD ["apache2-foreground"]
