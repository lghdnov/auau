use sqlx::PgPool;
use uuid::Uuid;

use crate::models::User;

/// Insert the user, or refresh name/email when `oidc_subject` already exists.
pub async fn upsert(
    pool: &PgPool,
    oidc_subject: &str,
    name: &str,
    email: &str,
) -> anyhow::Result<User> {
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (id, oidc_subject, name, email)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (oidc_subject)
         DO UPDATE SET name = EXCLUDED.name, email = EXCLUDED.email
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(oidc_subject)
    .bind(name)
    .bind(email)
    .fetch_one(pool)
    .await?;
    Ok(user)
}

/// Load a user by primary key, if present.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_creates_then_updates(pool: sqlx::PgPool) {
        let created = upsert(&pool, "pocketid|1042", "Анна", "anna@example.com")
            .await
            .unwrap();
        assert_eq!(created.oidc_subject, "pocketid|1042");
        assert_eq!(created.name, "Анна");
        assert_eq!(created.email, "anna@example.com");

        let updated = upsert(&pool, "pocketid|1042", "Анна Новая", "anna@example.com")
            .await
            .unwrap();
        assert_eq!(updated.id, created.id, "upsert must reuse the existing row");
        assert_eq!(updated.name, "Анна Новая");
        assert_eq!(updated.email, "anna@example.com");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "second upsert must not create a new row");
    }
}
