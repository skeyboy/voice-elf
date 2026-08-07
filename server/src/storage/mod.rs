use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use diesel::{
    BoolExpressionMethods, Connection, ExpressionMethods, Insertable, OptionalExtension,
    PgConnection, PgTextExpressionMethods, QueryDsl, QueryableByName, SelectableHelper,
    dsl::{count_star, max, sql},
    sql_types::{BigInt, Bool, Text},
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
        auth_sessions, password_reset_tokens, room_members, rooms, system_installations, users,
        voice_references, voice_sessions, voice_utterances,
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
    pub email: Option<String>,
    pub password_hash: String,
    pub role: String,
    pub status: String,
    pub verified_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl UserRecord {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

#[derive(Insertable)]
#[diesel(table_name = users)]
struct NewUser<'a> {
    id: Uuid,
    username: &'a str,
    email: Option<&'a str>,
    password_hash: &'a str,
    role: &'a str,
    status: &'a str,
    verified_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct ManagedUserInput {
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = system_installations)]
pub struct SystemInstallation {
    pub id: Uuid,
    pub system_name: String,
    pub organization_name: String,
    pub public_url: Option<String>,
    pub deployment_mode: String,
    pub initialized_by: Uuid,
    pub initialized_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = system_installations)]
struct NewSystemInstallation<'a> {
    id: Uuid,
    system_name: &'a str,
    organization_name: &'a str,
    public_url: Option<&'a str>,
    deployment_mode: &'a str,
    initialized_by: Uuid,
}

pub enum InitializeSystemOutcome {
    Created(UserRecord, SystemInstallation),
    AlreadyInitialized,
}

#[derive(Insertable)]
#[diesel(table_name = auth_sessions)]
struct NewAuthSession<'a> {
    id: Uuid,
    user_id: Uuid,
    token_hash: &'a str,
    expires_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = password_reset_tokens)]
struct NewPasswordResetToken<'a> {
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
    pub status: String,
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
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_owner: bool,
    pub is_member: bool,
    pub member_count: i64,
    pub utterance_count: i64,
    pub duration_ms: i64,
    pub last_activity_at: DateTime<Utc>,
    pub preview_text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminUserSummary {
    pub id: Uuid,
    pub username: String,
    pub email: Option<String>,
    pub role: String,
    pub status: String,
    pub verified_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub owned_room_count: i64,
    pub joined_room_count: i64,
    pub utterance_count: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminOverview {
    pub total_users: i64,
    pub pending_users: i64,
    pub suspended_users: i64,
    pub active_rooms: i64,
    pub total_rooms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub total_pages: i64,
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

    pub async fn create_user(
        &self,
        username: &str,
        email: &str,
        password_hash: &str,
    ) -> Result<UserRecord> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(users::table)
            .values(NewUser {
                id: Uuid::new_v4(),
                username,
                email: Some(email),
                password_hash,
                role: "member",
                status: "pending",
                verified_at: None,
            })
            .returning(UserRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create user")
    }

    pub async fn system_installation(&self) -> Result<Option<SystemInstallation>> {
        let mut connection = self.pool.get().await?;
        system_installations::table
            .select(SystemInstallation::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to read system installation")
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_system(
        &self,
        system_name: &str,
        organization_name: &str,
        public_url: Option<&str>,
        deployment_mode: &str,
        username: &str,
        email: &str,
        password_hash: &str,
    ) -> Result<InitializeSystemOutcome> {
        let mut connection = self.pool.get().await?;
        connection
            .transaction::<InitializeSystemOutcome, diesel::result::Error, _>(async |connection| {
                connection
                    .batch_execute(
                        "LOCK TABLE system_installations IN EXCLUSIVE MODE; \
                             LOCK TABLE users IN EXCLUSIVE MODE",
                    )
                    .await?;
                let initialized = system_installations::table
                    .select(count_star())
                    .first::<i64>(connection)
                    .await?
                    > 0;
                if initialized {
                    return Ok(InitializeSystemOutcome::AlreadyInitialized);
                }

                let user = diesel::insert_into(users::table)
                    .values(NewUser {
                        id: Uuid::new_v4(),
                        username,
                        email: Some(email),
                        password_hash,
                        role: "admin",
                        status: "active",
                        verified_at: Some(Utc::now()),
                    })
                    .returning(UserRecord::as_returning())
                    .get_result(connection)
                    .await?;
                let installation = diesel::insert_into(system_installations::table)
                    .values(NewSystemInstallation {
                        id: Uuid::from_u128(1),
                        system_name,
                        organization_name,
                        public_url,
                        deployment_mode,
                        initialized_by: user.id,
                    })
                    .returning(SystemInstallation::as_returning())
                    .get_result(connection)
                    .await?;
                Ok(InitializeSystemOutcome::Created(user, installation))
            })
            .await
            .context("failed to initialize system")
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

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        users::table
            .filter(users::email.eq(Some(email)))
            .select(UserRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to find user by email")
    }

    pub async fn find_user_by_account(&self, account: &str) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        users::table
            .filter(
                users::username
                    .eq(account)
                    .or(users::email.eq(Some(account.to_ascii_lowercase()))),
            )
            .select(UserRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to find user by account")
    }

    pub async fn get_user(&self, user_id: Uuid) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        users::table
            .find(user_id)
            .select(UserRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to get user")
    }

    pub async fn create_managed_users(
        &self,
        inputs: &[ManagedUserInput],
    ) -> Result<Vec<UserRecord>> {
        let mut connection = self.pool.get().await?;
        connection
            .transaction::<Vec<UserRecord>, diesel::result::Error, _>(async |connection| {
                let mut created = Vec::with_capacity(inputs.len());
                for input in inputs {
                    let verified_at = (input.status == "active").then(Utc::now);
                    let user = diesel::insert_into(users::table)
                        .values(NewUser {
                            id: Uuid::new_v4(),
                            username: &input.username,
                            email: Some(&input.email),
                            password_hash: &input.password_hash,
                            role: &input.role,
                            status: &input.status,
                            verified_at,
                        })
                        .returning(UserRecord::as_returning())
                        .get_result(connection)
                        .await?;
                    created.push(user);
                }
                Ok(created)
            })
            .await
            .context("failed to create managed users")
    }

    pub async fn password_reset_request_count(
        &self,
        user_id: Uuid,
        since: DateTime<Utc>,
    ) -> Result<i64> {
        let mut connection = self.pool.get().await?;
        password_reset_tokens::table
            .filter(password_reset_tokens::user_id.eq(user_id))
            .filter(password_reset_tokens::created_at.gt(since))
            .select(count_star())
            .first(&mut connection)
            .await
            .context("failed to count password reset requests")
    }

    pub async fn create_password_reset_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let mut connection = self.pool.get().await?;
        connection
            .transaction::<(), diesel::result::Error, _>(async |connection| {
                diesel::update(
                    password_reset_tokens::table
                        .filter(password_reset_tokens::user_id.eq(user_id))
                        .filter(password_reset_tokens::consumed_at.is_null()),
                )
                .set(password_reset_tokens::consumed_at.eq(diesel::dsl::now))
                .execute(connection)
                .await?;
                diesel::insert_into(password_reset_tokens::table)
                    .values(NewPasswordResetToken {
                        id: Uuid::new_v4(),
                        user_id,
                        token_hash,
                        expires_at,
                    })
                    .execute(connection)
                    .await?;
                Ok(())
            })
            .await
            .context("failed to create password reset token")
    }

    pub async fn reset_password(
        &self,
        token_hash: &str,
        password_hash: &str,
    ) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        connection
            .transaction::<Option<UserRecord>, diesel::result::Error, _>(async |connection| {
                let token = password_reset_tokens::table
                    .filter(password_reset_tokens::token_hash.eq(token_hash))
                    .filter(password_reset_tokens::consumed_at.is_null())
                    .filter(password_reset_tokens::expires_at.gt(diesel::dsl::now))
                    .for_update()
                    .select((password_reset_tokens::id, password_reset_tokens::user_id))
                    .first::<(Uuid, Uuid)>(connection)
                    .await
                    .optional()?;
                let Some(token) = token else {
                    return Ok(None);
                };
                let user = diesel::update(users::table.find(token.1))
                    .set(users::password_hash.eq(password_hash))
                    .returning(UserRecord::as_returning())
                    .get_result(connection)
                    .await?;
                diesel::update(password_reset_tokens::table.find(token.0))
                    .set(password_reset_tokens::consumed_at.eq(diesel::dsl::now))
                    .execute(connection)
                    .await?;
                diesel::delete(auth_sessions::table.filter(auth_sessions::user_id.eq(user.id)))
                    .execute(connection)
                    .await?;
                Ok(Some(user))
            })
            .await
            .context("failed to reset password")
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
            .filter(users::status.eq("active"))
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

    pub async fn record_login(&self, user_id: Uuid) -> Result<UserRecord> {
        let mut connection = self.pool.get().await?;
        diesel::update(users::table.find(user_id))
            .set(users::last_login_at.eq(diesel::dsl::now))
            .returning(UserRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to record user login")
    }

    pub async fn list_voice_references(&self, user_id: Uuid) -> Result<Vec<VoiceReferenceRecord>> {
        let mut connection = self.pool.get().await?;
        voice_references::table
            .filter(voice_references::user_id.eq(user_id))
            .filter(voice_references::deleted_at.is_null())
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
            .filter(voice_references::deleted_at.is_null())
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
        diesel::update(
            voice_references::table
                .filter(voice_references::id.eq(id))
                .filter(voice_references::user_id.eq(user_id))
                .filter(voice_references::deleted_at.is_null()),
        )
        .set(voice_references::deleted_at.eq(diesel::dsl::now))
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
            .filter(rooms::deleted_at.is_null())
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
        let visible_room_ids = room_members::table
            .filter(room_members::user_id.eq(user_id))
            .select(room_members::room_id);
        let mut query = rooms::table
            .filter(rooms::deleted_at.is_null())
            .filter(rooms::status.ne("archived"))
            .filter(
                rooms::owner_id
                    .eq(user_id)
                    .or(rooms::id.eq_any(visible_room_ids)),
            )
            .into_boxed();
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
        let (utterance_count, duration_ms, last_activity_at) = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room.id)))
            .select((
                count_star(),
                sql::<BigInt>("COALESCE(SUM(audio_ms), 0)::bigint"),
                max(voice_utterances::created_at),
            ))
            .first::<(i64, i64, Option<DateTime<Utc>>)>(&mut connection)
            .await?;
        let last_activity_at = last_activity_at.unwrap_or(room.created_at);
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
            status: room.status.clone(),
            created_at: room.created_at,
            updated_at: room.updated_at,
            is_owner: room.owner_id == user_id,
            is_member,
            member_count,
            utterance_count,
            duration_ms,
            last_activity_at,
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
                .filter(rooms::owner_id.eq(owner_id))
                .filter(rooms::deleted_at.is_null()),
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
        let affected = diesel::update(
            rooms::table
                .filter(rooms::id.eq(room_id))
                .filter(rooms::owner_id.eq(owner_id))
                .filter(rooms::deleted_at.is_null()),
        )
        .set((
            rooms::deleted_at.eq(diesel::dsl::now),
            rooms::updated_at.eq(diesel::dsl::now),
        ))
        .execute(&mut connection)
        .await?;
        Ok(affected > 0)
    }

    pub async fn admin_overview(&self) -> Result<AdminOverview> {
        let mut connection = self.pool.get().await?;
        let total_users = users::table
            .select(count_star())
            .first(&mut connection)
            .await?;
        let pending_users = users::table
            .filter(users::status.eq("pending"))
            .select(count_star())
            .first(&mut connection)
            .await?;
        let suspended_users = users::table
            .filter(users::status.eq("suspended"))
            .select(count_star())
            .first(&mut connection)
            .await?;
        let active_rooms = rooms::table
            .filter(rooms::deleted_at.is_null())
            .filter(rooms::status.eq("active"))
            .select(count_star())
            .first(&mut connection)
            .await?;
        let total_rooms = rooms::table
            .filter(rooms::deleted_at.is_null())
            .select(count_star())
            .first(&mut connection)
            .await?;
        Ok(AdminOverview {
            total_users,
            pending_users,
            suspended_users,
            active_rooms,
            total_rooms,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_admin_users(
        &self,
        search: Option<&str>,
        status: Option<&str>,
        role: Option<&str>,
        sort: &str,
        descending: bool,
        page: i64,
        page_size: i64,
    ) -> Result<Paginated<AdminUserSummary>> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let mut connection = self.pool.get().await?;

        let mut count_query = users::table.into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            count_query = count_query.filter(
                users::username
                    .ilike(pattern.clone())
                    .or(users::email.ilike(pattern)),
            );
        }
        if let Some(status) = status {
            count_query = count_query.filter(users::status.eq(status));
        }
        if let Some(role) = role {
            count_query = count_query.filter(users::role.eq(role));
        }
        let total = count_query
            .select(count_star())
            .first::<i64>(&mut connection)
            .await?;

        let mut query = users::table.into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            query = query.filter(
                users::username
                    .ilike(pattern.clone())
                    .or(users::email.ilike(pattern)),
            );
        }
        if let Some(status) = status {
            query = query.filter(users::status.eq(status));
        }
        if let Some(role) = role {
            query = query.filter(users::role.eq(role));
        }
        query = match (sort, descending) {
            ("username", false) => query.order(users::username.asc()),
            ("username", true) => query.order(users::username.desc()),
            ("last_login", false) => query.order(users::last_login_at.asc()),
            ("last_login", true) => query.order(users::last_login_at.desc()),
            ("created_at", false) => query.order(users::created_at.asc()),
            _ => query.order(users::created_at.desc()),
        };
        let records = query
            .offset((page - 1) * page_size)
            .limit(page_size)
            .select(UserRecord::as_select())
            .load::<UserRecord>(&mut connection)
            .await?;

        let mut items = Vec::with_capacity(records.len());
        for user in records {
            let owned_room_count = rooms::table
                .filter(rooms::owner_id.eq(user.id))
                .filter(rooms::deleted_at.is_null())
                .select(count_star())
                .first::<i64>(&mut connection)
                .await?;
            let joined_room_count = room_members::table
                .inner_join(rooms::table)
                .filter(room_members::user_id.eq(user.id))
                .filter(rooms::deleted_at.is_null())
                .select(count_star())
                .first::<i64>(&mut connection)
                .await?;
            let utterance_count = voice_utterances::table
                .filter(voice_utterances::user_id.eq(Some(user.id)))
                .select(count_star())
                .first::<i64>(&mut connection)
                .await?;
            let last_activity_at = voice_utterances::table
                .filter(voice_utterances::user_id.eq(Some(user.id)))
                .select(max(voice_utterances::created_at))
                .first::<Option<DateTime<Utc>>>(&mut connection)
                .await?;
            items.push(AdminUserSummary {
                id: user.id,
                username: user.username,
                email: user.email,
                role: user.role,
                status: user.status,
                verified_at: user.verified_at,
                last_login_at: user.last_login_at,
                created_at: user.created_at,
                owned_room_count,
                joined_room_count,
                utterance_count,
                last_activity_at,
            });
        }

        Ok(paginated(items, page, page_size, total))
    }

    pub async fn update_admin_user(
        &self,
        user_id: Uuid,
        role: &str,
        status: &str,
        email: Option<&str>,
    ) -> Result<Option<UserRecord>> {
        let mut connection = self.pool.get().await?;
        let current = users::table
            .find(user_id)
            .select(UserRecord::as_select())
            .first::<UserRecord>(&mut connection)
            .await
            .optional()?;
        let Some(current) = current else {
            return Ok(None);
        };
        let verified_at = if status == "active" {
            current.verified_at.or_else(|| Some(Utc::now()))
        } else {
            current.verified_at
        };
        let email = email.or(current.email.as_deref());
        let updated = diesel::update(users::table.find(user_id))
            .set((
                users::email.eq(email),
                users::role.eq(role),
                users::status.eq(status),
                users::verified_at.eq(verified_at),
            ))
            .returning(UserRecord::as_returning())
            .get_result::<UserRecord>(&mut connection)
            .await?;
        if status != "active" {
            diesel::delete(auth_sessions::table.filter(auth_sessions::user_id.eq(user_id)))
                .execute(&mut connection)
                .await?;
        }
        Ok(Some(updated))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_admin_rooms(
        &self,
        viewer_id: Uuid,
        search: Option<&str>,
        status: Option<&str>,
        sort: &str,
        descending: bool,
        page: i64,
        page_size: i64,
    ) -> Result<Paginated<RoomSummary>> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let mut connection = self.pool.get().await?;

        let mut count_query = rooms::table
            .filter(rooms::deleted_at.is_null())
            .into_boxed();
        if let Some(search) = search {
            let matching_owners = users::table
                .filter(users::username.ilike(format!("%{search}%")))
                .select(users::id);
            count_query = count_query.filter(
                rooms::name
                    .ilike(format!("%{search}%"))
                    .or(rooms::owner_id.eq_any(matching_owners)),
            );
        }
        if let Some(status) = status {
            count_query = count_query.filter(rooms::status.eq(status));
        }
        let total = count_query
            .select(count_star())
            .first::<i64>(&mut connection)
            .await?;

        let mut query = rooms::table
            .filter(rooms::deleted_at.is_null())
            .into_boxed();
        if let Some(search) = search {
            let matching_owners = users::table
                .filter(users::username.ilike(format!("%{search}%")))
                .select(users::id);
            query = query.filter(
                rooms::name
                    .ilike(format!("%{search}%"))
                    .or(rooms::owner_id.eq_any(matching_owners)),
            );
        }
        if let Some(status) = status {
            query = query.filter(rooms::status.eq(status));
        }
        query = match (sort, descending) {
            ("name", false) => query.order(rooms::name.asc()),
            ("name", true) => query.order(rooms::name.desc()),
            ("created_at", false) => query.order(rooms::created_at.asc()),
            ("created_at", true) => query.order(rooms::created_at.desc()),
            ("updated_at", false) => query.order(rooms::updated_at.asc()),
            _ => query.order(rooms::updated_at.desc()),
        };
        let records = query
            .offset((page - 1) * page_size)
            .limit(page_size)
            .select(RoomRecord::as_select())
            .load::<RoomRecord>(&mut connection)
            .await?;
        drop(connection);

        let mut items = Vec::with_capacity(records.len());
        for room in records {
            items.push(self.room_summary(&room, viewer_id).await?);
        }
        Ok(paginated(items, page, page_size, total))
    }

    pub async fn update_admin_room_status(
        &self,
        room_id: Uuid,
        status: &str,
    ) -> Result<Option<RoomRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            rooms::table
                .find(room_id)
                .filter(rooms::deleted_at.is_null()),
        )
        .set((
            rooms::status.eq(status),
            rooms::updated_at.eq(diesel::dsl::now),
        ))
        .returning(RoomRecord::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to update room status")
    }
}

fn paginated<T>(items: Vec<T>, page: i64, page_size: i64, total: i64) -> Paginated<T> {
    Paginated {
        items,
        page,
        page_size,
        total,
        total_pages: (total + page_size - 1) / page_size,
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
    use super::{database_admin_url, paginated, quote_postgres_identifier};

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

    #[test]
    fn calculates_paginated_metadata() {
        let page = paginated(vec![1, 2], 2, 20, 41);
        assert_eq!(page.page, 2);
        assert_eq!(page.page_size, 20);
        assert_eq!(page.total, 41);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.items, vec![1, 2]);
    }
}
mod asr;
mod authority;
mod history;
mod tts;

pub use asr::AsrSystemSetting;
pub use authority::{
    AuthorityInstanceRecord, AuthorityInstanceSummary, AuthorityTenantRecord,
    AuthorityTenantSummary, AuthorityTokenContext,
};
pub use tts::TtsSystemSetting;

pub use history::{
    NewUtteranceAttempt, RefinementUpdate, TranscriptUpdate, TranslationUpdate,
    UtteranceAudioUpdate, UtteranceHistory,
};
