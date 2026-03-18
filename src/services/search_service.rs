use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
pub struct SearchResult {
    pub id: i32,
    pub title: String,
    pub system: String,
}

pub async fn quick_search(pool: &PgPool, query: &str, limit: i64) -> Vec<SearchResult> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    sqlx::query_as::<_, SearchResult>(
        "SELECT d.id, d.title, s.short_code AS system
         FROM discs d
         JOIN systems s ON s.id = d.system_id
         WHERE d.search_vector @@ plainto_tsquery('english', $1)
            OR d.title ILIKE '%' || $1 || '%'
         ORDER BY ts_rank(d.search_vector, plainto_tsquery('english', $1)) DESC
         LIMIT $2"
    )
    .bind(query)
    .bind(limit)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

pub async fn search_by_serial(pool: &PgPool, serial: &str) -> Vec<SearchResult> {
    sqlx::query_as::<_, SearchResult>(
        "SELECT d.id, d.title, s.short_code AS system
         FROM discs d
         JOIN systems s ON s.id = d.system_id
         JOIN disc_serials ds ON ds.disc_id = d.id
         WHERE ds.serial ILIKE '%' || $1 || '%'
         ORDER BY d.title
         LIMIT 50"
    )
    .bind(serial)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}
