use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use diesel::{
    BoolExpressionMethods, Connection, ExpressionMethods, Insertable, OptionalExtension,
    PgConnection, PgTextExpressionMethods, QueryDsl, QueryableByName, SelectableHelper,
    sql_types::{Bool, Text},
};
use diesel_async::{
    AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection,
    pooled_connection::{AsyncDieselConnectionManager, bb8::Pool},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde::Serialize;
use url::Url;
use uuid::Uuid;

use crate::{
    protocol::SessionConfig,
    schema::{
        auth_sessions, room_members, rooms, users, voice_references, voice_sessions,
        voice_utterances,
    },
};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[derive(QueryableByName)]
struct DatabaseExists {
    #[diesel(sql_type = Bool)]
    exists: bool,
}

#[derive(Clone)]
pub struct Database {
    pool: Pool<AsyncPgConnection>,
}

#[derive(Insertable)]
#[diesel(table_name = voice_sessions)]
struct NewSession<'a> {
    id: Uuid,
    user_id: Option<Uuid>,
    room_id: Option<Uuid>,
    backend: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    voice: &'a str,
    max_utterance_seconds: i32,
}

#[derive(Clone, Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = users)]
pub struct UserRecord {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = users)]
struct NewUser<'a> {
    id: Uuid,
    username: &'a str,
    password_hash: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = auth_sessions)]
struct NewAuthSession<'a> {
    id: Uuid,
    user_id: Uuid,
    token_hash: &'a str,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = voice_references)]
pub struct VoiceReferenceRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub audio_path: String,
    pub duration_ms: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = voice_references)]
struct NewVoiceReference<'a> {
    id: Uuid,
    user_id: Uuid,
    name: &'a str,
    audio_path: &'a str,
    duration_ms: i64,
}

#[derive(Clone, Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = rooms)]
pub struct RoomRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    pub max_utterance_seconds: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = rooms)]
struct NewRoom<'a> {
    id: Uuid,
    owner_id: Uuid,
    name: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    max_utterance_seconds: i32,
}

#[derive(Insertable)]
#[diesel(table_name = room_members)]
struct NewRoomMember {
    room_id: Uuid,
    user_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomSummary {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub owner_username: String,
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    pub max_utterance_seconds: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_owner: bool,
    pub is_member: bool,
    pub member_count: i64,
    pub utterance_count: i64,
    pub preview_text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomMemberRecord {
    pub user_id: Uuid,
    pub username: String,
    pub is_owner: bool,
    pub is_muted: bool,
    pub joined_at: DateTime<Utc>,
}

impl Database {
    pub async fn connect(url: &str) -> Result<Self> {
        ensure_database_exists(url).await?;
        let migration_url = url.to_owned();
        let applied = tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut connection = PgConnection::establish(&migration_url)
                .context("failed to connect to PostgreSQL for migrations")?;
            connection
                .run_pending_migrations(MIGRATIONS)
                .map(|versions| versions.len())
                .map_err(|error| anyhow!("failed to run PostgreSQL migrations: {error}"))
        })
        .await
        .context("PostgreSQL migration task failed")??;
        if applied > 0 {
            tracing::info!(applied, "applied PostgreSQL migrations");
        }

        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .await
            .context("failed to create PostgreSQL connection pool")?;
        let database = Self { pool };
        let recovered = database
            .recover_interrupted_utterances()
            .await
            .context("failed to recover interrupted voice records")?;
        if recovered > 0 {
            tracing::warn!(recovered, "marked stale voice records as interrupted");
        }
        tracing::info!("PostgreSQL voice history enabled");
        Ok(database)
    }

    pub async fn create_session(
        &self,
        id: Uuid,
        user_id: Uuid,
        room_id: Uuid,
        backend: &str,
        config: &SessionConfig,
    ) -> Result<()> {
        let row = NewSession {
            id,
            user_id: Some(user_id),
            room_id: Some(room_id),
            backend,
            source_language: &config.source_language,
            target_language: &config.target_language,
            voice: &config.voice,
            max_utterance_seconds: config.max_utterance_seconds as i32,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(voice_sessions::table)
            .values(row)
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn update_session_config(&self, id: Uuid, config: &SessionConfig) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::update(voice_sessions::table.find(id))
            .set((
                voice_sessions::source_language.eq(&config.source_language),
                voice_sessions::target_language.eq(&config.target_language),
                voice_sessions::voice.eq(&config.voice),
                voice_sessions::max_utterance_seconds.eq(config.max_utterance_seconds as i32),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn complete_session(&self, id: Uuid) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::update(voice_sessions::table.find(id))
            .set(voice_sessions::ended_at.eq(diesel::dsl::now))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn create_user(&self, username: &str, password_hash: &str) -> Result<UserRecord> {
        let row = NewUser {
            id: Uuid::new_v4(),
            username,
            password_hash,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(users::table)
            .values(row)
            .returning(UserRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create user")
    }

    pub async fn find_user_by_username(&self, username: &str) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        users::table
            .filter(users::username.eq(username))
            .select(UserRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to find user")
    }

    pub async fn create_auth_session(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let row = NewAuthSession {
            id: Uuid::new_v4(),
            user_id,
            token_hash,
            expires_at,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(auth_sessions::table)
            .values(row)
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn user_by_session_hash(&self, token_hash: &str) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        auth_sessions::table
            .inner_join(users::table)
            .filter(auth_sessions::token_hash.eq(token_hash))
            .filter(auth_sessions::expires_at.gt(diesel::dsl::now))
            .select(UserRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to authenticate session")
    }

    pub async fn delete_auth_session(&self, token_hash: &str) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::delete(auth_sessions::table.filter(auth_sessions::token_hash.eq(token_hash)))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn list_voice_references(&self, user_id: Uuid) -> Result<Vec<VoiceReferenceRecord>> {
        let mut connection = self.pool.get().await?;
        voice_references::table
            .filter(voice_references::user_id.eq(user_id))
            .order(voice_references::created_at.desc())
            .select(VoiceReferenceRecord::as_select())
            .load(&mut connection)
            .await
            .context("failed to list voice references")
    }

    pub async fn get_voice_reference(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<VoiceReferenceRecord>> {
        let mut connection = self.pool.get().await?;
        voice_references::table
            .filter(voice_references::id.eq(id))
            .filter(voice_references::user_id.eq(user_id))
            .select(VoiceReferenceRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to find voice reference")
    }

    pub async fn create_voice_reference(
        &self,
        id: Uuid,
        user_id: Uuid,
        name: &str,
        audio_path: &str,
        duration_ms: i64,
    ) -> Result<VoiceReferenceRecord> {
        let row = NewVoiceReference {
            id,
            user_id,
            name,
            audio_path,
            duration_ms,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(voice_references::table)
            .values(row)
            .returning(VoiceReferenceRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create voice reference")
    }

    pub async fn delete_voice_reference(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<VoiceReferenceRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::delete(
            voice_references::table
                .filter(voice_references::id.eq(id))
                .filter(voice_references::user_id.eq(user_id)),
        )
        .returning(VoiceReferenceRecord::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to delete voice reference")
    }

    pub async fn create_room(
        &self,
        owner_id: Uuid,
        name: &str,
        source_language: &str,
        target_language: &str,
        max_utterance_seconds: i32,
    ) -> Result<RoomRecord> {
        let room_id = Uuid::new_v4();
        let row = NewRoom {
            id: room_id,
            owner_id,
            name,
            source_language,
            target_language,
            max_utterance_seconds,
        };
        let mut connection = self.pool.get().await?;
        let room = diesel::insert_into(rooms::table)
            .values(row)
            .returning(RoomRecord::as_returning())
            .get_result(&mut connection)
            .await?;
        diesel::insert_into(room_members::table)
            .values(NewRoomMember {
                room_id,
                user_id: owner_id,
            })
            .on_conflict_do_nothing()
            .execute(&mut connection)
            .await?;
        Ok(room)
    }

    pub async fn get_room(&self, room_id: Uuid) -> Result<Option<RoomRecord>> {
        let mut connection = self.pool.get().await?;
        rooms::table
            .find(room_id)
            .select(RoomRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to find room")
    }

    pub async fn list_rooms(
        &self,
        user_id: Uuid,
        search: Option<&str>,
    ) -> Result<Vec<RoomSummary>> {
        let mut connection = self.pool.get().await?;
        let mut query = rooms::table.into_boxed();
        if let Some(search) = search.filter(|value| !value.trim().is_empty()) {
            query = query.filter(rooms::name.ilike(format!("%{}%", search.trim())));
        }
        let records = query
            .order(rooms::updated_at.desc())
            .limit(100)
            .select(RoomRecord::as_select())
            .load(&mut connection)
            .await?;
        drop(connection);

        let mut summaries = Vec::with_capacity(records.len());
        for room in records {
            summaries.push(self.room_summary(&room, user_id).await?);
        }
        Ok(summaries)
    }

    pub async fn room_summary(&self, room: &RoomRecord, user_id: Uuid) -> Result<RoomSummary> {
        let mut connection = self.pool.get().await?;
        let owner_username = users::table
            .find(room.owner_id)
            .select(users::username)
            .first(&mut connection)
            .await?;
        let member_count = room_members::table
            .filter(room_members::room_id.eq(room.id))
            .count()
            .get_result(&mut connection)
            .await?;
        let is_member = room_members::table
            .filter(room_members::room_id.eq(room.id))
            .filter(room_members::user_id.eq(user_id))
            .select(room_members::user_id)
            .first::<Uuid>(&mut connection)
            .await
            .optional()?
            .is_some();
        let utterance_count = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room.id)))
            .count()
            .get_result(&mut connection)
            .await?;
        let preview_text = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room.id)))
            .order(voice_utterances::created_at.desc())
            .select(voice_utterances::translated_text)
            .first::<String>(&mut connection)
            .await
            .optional()?;
        Ok(RoomSummary {
            id: room.id,
            owner_id: room.owner_id,
            owner_username,
            name: room.name.clone(),
            source_language: room.source_language.clone(),
            target_language: room.target_language.clone(),
            max_utterance_seconds: room.max_utterance_seconds,
            created_at: room.created_at,
            updated_at: room.updated_at,
            is_owner: room.owner_id == user_id,
            is_member,
            member_count,
            utterance_count,
            preview_text,
        })
    }

    pub async fn join_room(&self, room_id: Uuid, user_id: Uuid) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(room_members::table)
            .values(NewRoomMember { room_id, user_id })
            .on_conflict_do_nothing()
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn list_room_members(&self, room_id: Uuid) -> Result<Vec<RoomMemberRecord>> {
        let room = self
            .get_room(room_id)
            .await?
            .context("room does not exist")?;
        let mut connection = self.pool.get().await?;
        let rows = room_members::table
            .inner_join(users::table)
            .filter(room_members::room_id.eq(room_id))
            .order((room_members::joined_at.asc(), users::username.asc()))
            .select((
                room_members::user_id,
                users::username,
                room_members::is_muted,
                room_members::joined_at,
            ))
            .load::<(Uuid, String, bool, DateTime<Utc>)>(&mut connection)
            .await?;
        Ok(rows
            .into_iter()
            .map(
                |(user_id, username, is_muted, joined_at)| RoomMemberRecord {
                    user_id,
                    username,
                    is_owner: user_id == room.owner_id,
                    is_muted,
                    joined_at,
                },
            )
            .collect())
    }

    pub async fn set_room_member_muted(
        &self,
        room_id: Uuid,
        owner_id: Uuid,
        user_id: Uuid,
        is_muted: bool,
    ) -> Result<Option<RoomMemberRecord>> {
        let Some(room) = self.get_room(room_id).await? else {
            return Ok(None);
        };
        if room.owner_id != owner_id || user_id == room.owner_id {
            return Ok(None);
        }
        let mut connection = self.pool.get().await?;
        let changed = diesel::update(
            room_members::table
                .filter(room_members::room_id.eq(room_id))
                .filter(room_members::user_id.eq(user_id)),
        )
        .set(room_members::is_muted.eq(is_muted))
        .execute(&mut connection)
        .await?;
        drop(connection);
        if changed == 0 {
            return Ok(None);
        }
        Ok(self
            .list_room_members(room_id)
            .await?
            .into_iter()
            .find(|member| member.user_id == user_id))
    }

    pub async fn can_view_room(&self, room_id: Uuid, user_id: Uuid) -> Result<bool> {
        let Some(room) = self.get_room(room_id).await? else {
            return Ok(false);
        };
        if room.owner_id == user_id {
            return Ok(true);
        }
        let mut connection = self.pool.get().await?;
        Ok(room_members::table
            .filter(room_members::room_id.eq(room_id))
            .filter(room_members::user_id.eq(user_id))
            .select(room_members::user_id)
            .first::<Uuid>(&mut connection)
            .await
            .optional()?
            .is_some())
    }

    pub async fn media_room_for_url(&self, url: &str) -> Result<Option<Uuid>> {
        let mut connection = self.pool.get().await?;
        Ok(voice_utterances::table
            .filter(
                voice_utterances::source_audio_url
                    .eq(Some(url))
                    .or(voice_utterances::translated_audio_url.eq(Some(url))),
            )
            .select(voice_utterances::room_id)
            .first::<Option<Uuid>>(&mut connection)
            .await
            .optional()?
            .flatten())
    }

    pub async fn room_media_paths(&self, room_id: Uuid) -> Result<Vec<String>> {
        let mut connection = self.pool.get().await?;
        let rows = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room_id)))
            .select((
                voice_utterances::source_audio_path,
                voice_utterances::translated_audio_path,
            ))
            .load::<(Option<String>, Option<String>)>(&mut connection)
            .await?;
        Ok(rows
            .into_iter()
            .flat_map(|(source, translated)| [source, translated])
            .flatten()
            .collect())
    }

    pub async fn update_room(
        &self,
        room_id: Uuid,
        owner_id: Uuid,
        name: &str,
        source_language: &str,
        target_language: &str,
        max_utterance_seconds: i32,
    ) -> Result<Option<RoomRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            rooms::table
                .filter(rooms::id.eq(room_id))
                .filter(rooms::owner_id.eq(owner_id)),
        )
        .set((
            rooms::name.eq(name),
            rooms::source_language.eq(source_language),
            rooms::target_language.eq(target_language),
            rooms::max_utterance_seconds.eq(max_utterance_seconds),
            rooms::updated_at.eq(diesel::dsl::now),
        ))
        .returning(RoomRecord::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to update room")
    }

    pub async fn delete_room(&self, room_id: Uuid, owner_id: Uuid) -> Result<bool> {
        let mut connection = self.pool.get().await?;
        let affected = diesel::delete(
            rooms::table
                .filter(rooms::id.eq(room_id))
                .filter(rooms::owner_id.eq(owner_id)),
        )
        .execute(&mut connection)
        .await?;
        Ok(affected > 0)
    }
}

async fn ensure_database_exists(database_url: &str) -> Result<()> {
    let (admin_url, database_name) = database_admin_url(database_url)?;
    let mut connection = AsyncPgConnection::establish(admin_url.as_str())
        .await
        .context("failed to connect to the PostgreSQL maintenance database")?;
    let exists =
        diesel::sql_query("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1) AS exists")
            .bind::<Text, _>(&database_name)
            .get_result::<DatabaseExists>(&mut connection)
            .await
            .context("failed to inspect PostgreSQL databases")?
            .exists;
    if exists {
        return Ok(());
    }

    let identifier = quote_postgres_identifier(&database_name);
    connection
        .batch_execute(&format!("CREATE DATABASE {identifier}"))
        .await
        .with_context(|| format!("failed to create PostgreSQL database '{database_name}'"))?;
    tracing::info!(database = %database_name, "created PostgreSQL database");
    Ok(())
}

fn database_admin_url(database_url: &str) -> Result<(Url, String)> {
    let mut url =
        Url::parse(database_url).context("DATABASE_URL must be a valid PostgreSQL URL")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        anyhow::bail!("DATABASE_URL must use the postgres or postgresql scheme");
    }
    let database_name = url.path().trim_matches('/').to_owned();
    if database_name.is_empty() || database_name.contains('/') {
        anyhow::bail!("DATABASE_URL must include exactly one database name");
    }
    url.set_path("/postgres");
    Ok((url, database_name))
}

fn quote_postgres_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod database_setup_tests {
    use super::{database_admin_url, quote_postgres_identifier};

    #[test]
    fn derives_maintenance_url_without_losing_connection_options() {
        let (url, database) =
            database_admin_url("postgres://user:secret@localhost:5432/voice_elf?sslmode=disable")
                .unwrap();
        assert_eq!(database, "voice_elf");
        assert_eq!(url.path(), "/postgres");
        assert_eq!(url.query(), Some("sslmode=disable"));
    }

    #[test]
    fn quotes_database_identifiers() {
        assert_eq!(quote_postgres_identifier("voice\"elf"), "\"voice\"\"elf\"");
    }
}
mod history;

pub use history::{
    NewUtteranceAttempt, RefinementUpdate, TranscriptUpdate, TranslationUpdate,
    UtteranceAudioUpdate, UtteranceHistory,
};
