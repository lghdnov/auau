use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

/// A user synced from OIDC claims on login.
#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub oidc_subject: String,
    pub name: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

/// A labeling project.
#[derive(Debug, Clone, FromRow)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}

/// A label in the project palette.
#[derive(Debug, Clone, FromRow)]
pub struct Label {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
    pub hotkey: Option<String>,
    pub sort_order: i32,
    pub archived: bool,
}

/// An uploaded audio file with its processing/queue status.
#[derive(Debug, Clone, FromRow)]
pub struct AudioFile {
    pub id: Uuid,
    pub project_id: Uuid,
    pub original_name: String,
    pub storage_key: String,
    pub sha256: String,
    pub duration_ms: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub mime: String,
    pub size_bytes: i64,
    pub status: FileStatus,
    pub error_message: Option<String>,
    pub claimed_by: Option<Uuid>,
    pub claim_expires_at: Option<DateTime<Utc>>,
    pub uploaded_by: Uuid,
    pub created_at: DateTime<Utc>,
}

/// A labeled segment within an audio file.
#[derive(Debug, Clone, FromRow)]
pub struct Annotation {
    pub id: Uuid,
    pub file_id: Uuid,
    pub label_id: Uuid,
    pub start_ms: i32,
    pub end_ms: i32,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lifecycle of an audio file: media preparation and queue in one state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum FileStatus {
    Processing,
    New,
    InProgress,
    Done,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::SubsecRound;

    #[sqlx::test(migrations = "./migrations")]
    async fn models_roundtrip(pool: sqlx::PgPool) {
        let now = Utc::now().trunc_subsecs(6);

        let user = User {
            id: Uuid::new_v4(),
            oidc_subject: "pocketid|1042".into(),
            name: "Тест Тестов".into(),
            email: "tester@example.com".into(),
            created_at: now,
        };
        sqlx::query(
            "INSERT INTO users (id, oidc_subject, name, email, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(user.id)
        .bind(&user.oidc_subject)
        .bind(&user.name)
        .bind(&user.email)
        .bind(user.created_at)
        .execute(&pool)
        .await
        .unwrap();

        let project = Project {
            id: Uuid::new_v4(),
            name: "podcast".into(),
            description: Some("выпуск №1".into()),
            created_by: user.id,
            created_at: now,
        };
        sqlx::query(
            "INSERT INTO projects (id, name, description, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project.id)
        .bind(&project.name)
        .bind(&project.description)
        .bind(project.created_by)
        .bind(project.created_at)
        .execute(&pool)
        .await
        .unwrap();

        let label = Label {
            id: Uuid::new_v4(),
            project_id: project.id,
            name: "смех".into(),
            color: "#f59e0b".into(),
            hotkey: Some("1".into()),
            sort_order: 0,
            archived: false,
        };
        sqlx::query(
            "INSERT INTO labels (id, project_id, name, color, hotkey, sort_order, archived)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(label.id)
        .bind(label.project_id)
        .bind(&label.name)
        .bind(&label.color)
        .bind(&label.hotkey)
        .bind(label.sort_order)
        .bind(label.archived)
        .execute(&pool)
        .await
        .unwrap();

        let file = AudioFile {
            id: Uuid::new_v4(),
            project_id: project.id,
            original_name: "файл_042.wav".into(),
            storage_key: format!("media/{}.wav", Uuid::new_v4()),
            sha256: "ab".repeat(32),
            duration_ms: Some(132_000),
            sample_rate: Some(44_100),
            channels: Some(2),
            mime: "audio/wav".into(),
            size_bytes: 1_048_576,
            status: FileStatus::New,
            error_message: None,
            claimed_by: None,
            claim_expires_at: None,
            uploaded_by: user.id,
            created_at: now,
        };
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                duration_ms, sample_rate, channels, mime, size_bytes, status, error_message,
                claimed_by, claim_expires_at, uploaded_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(file.id)
        .bind(file.project_id)
        .bind(&file.original_name)
        .bind(&file.storage_key)
        .bind(&file.sha256)
        .bind(file.duration_ms)
        .bind(file.sample_rate)
        .bind(file.channels)
        .bind(&file.mime)
        .bind(file.size_bytes)
        .bind(file.status)
        .bind(file.error_message)
        .bind(file.claimed_by)
        .bind(file.claim_expires_at)
        .bind(file.uploaded_by)
        .bind(file.created_at)
        .execute(&pool)
        .await
        .unwrap();

        let annotation = Annotation {
            id: Uuid::new_v4(),
            file_id: file.id,
            label_id: label.id,
            start_ms: 3_100,
            end_ms: 5_400,
            created_by: user.id,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(
            "INSERT INTO annotations (id, file_id, label_id, start_ms, end_ms, created_by,
                created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(annotation.id)
        .bind(annotation.file_id)
        .bind(annotation.label_id)
        .bind(annotation.start_ms)
        .bind(annotation.end_ms)
        .bind(annotation.created_by)
        .bind(annotation.created_at)
        .bind(annotation.updated_at)
        .execute(&pool)
        .await
        .unwrap();

        let user_back: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(user_back.id, user.id);
        assert_eq!(user_back.oidc_subject, user.oidc_subject);
        assert_eq!(user_back.created_at, user.created_at);

        let project_back: Project = sqlx::query_as("SELECT * FROM projects WHERE id = $1")
            .bind(project.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(project_back.name, project.name);
        assert_eq!(project_back.description, project.description);

        let label_back: Label = sqlx::query_as("SELECT * FROM labels WHERE id = $1")
            .bind(label.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(label_back.hotkey, label.hotkey);
        assert!(!label_back.archived);

        let file_back: AudioFile = sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
            .bind(file.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(file_back.original_name, file.original_name);
        assert_eq!(file_back.size_bytes, file.size_bytes);
        assert_eq!(file_back.duration_ms, file.duration_ms);
        assert_eq!(file_back.status, FileStatus::New);

        let annotation_back: Annotation = sqlx::query_as("SELECT * FROM annotations WHERE id = $1")
            .bind(annotation.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(annotation_back.start_ms, annotation.start_ms);
        assert_eq!(annotation_back.end_ms, annotation.end_ms);

        sqlx::query("UPDATE audio_files SET status = 'in_progress' WHERE id = $1")
            .bind(file.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed: AudioFile = sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
            .bind(file.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(claimed.status, FileStatus::InProgress);
    }
}
