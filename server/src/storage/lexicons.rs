use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::schema::{
    blocked_words, room_terminology_bindings, terminology_dictionaries, terminology_entries,
};

use super::Database;

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = terminology_dictionaries)]
pub struct TerminologyDictionary {
    pub id: Uuid,
    pub name: String,
    pub industry: String,
    pub description: String,
    pub source_language: String,
    pub target_language: String,
    pub status: String,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TerminologyDictionaryInput {
    pub name: String,
    pub industry: String,
    pub description: String,
    pub source_language: String,
    pub target_language: String,
    pub status: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = terminology_dictionaries)]
struct NewTerminologyDictionary<'a> {
    id: Uuid,
    name: &'a str,
    industry: &'a str,
    description: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    status: &'a str,
    updated_by: Uuid,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = terminology_entries)]
pub struct TerminologyEntry {
    pub id: Uuid,
    pub dictionary_id: Uuid,
    pub source_term: String,
    pub aliases: Vec<String>,
    pub target_term: String,
    pub priority: i32,
    pub status: String,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TerminologyEntryInput {
    pub source_term: String,
    pub aliases: Vec<String>,
    pub target_term: String,
    pub priority: i32,
    pub status: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = terminology_entries)]
struct NewTerminologyEntry<'a> {
    id: Uuid,
    dictionary_id: Uuid,
    source_term: &'a str,
    aliases: &'a [String],
    target_term: &'a str,
    priority: i32,
    status: &'a str,
    updated_by: Uuid,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = blocked_words)]
pub struct BlockedWord {
    pub id: Uuid,
    pub word: String,
    pub replacement: String,
    pub match_mode: String,
    pub case_sensitive: bool,
    pub status: String,
    pub note: String,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockedWordInput {
    pub word: String,
    pub replacement: String,
    pub match_mode: String,
    pub case_sensitive: bool,
    pub status: String,
    pub note: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = blocked_words)]
struct NewBlockedWord<'a> {
    id: Uuid,
    word: &'a str,
    replacement: &'a str,
    match_mode: &'a str,
    case_sensitive: bool,
    status: &'a str,
    note: &'a str,
    updated_by: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomTerminologyBinding {
    pub dictionary_id: Option<Uuid>,
    pub dictionary_name: Option<String>,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = room_terminology_bindings)]
struct NewRoomTerminologyBinding {
    room_id: Uuid,
    dictionary_id: Uuid,
    status: &'static str,
    updated_by: Uuid,
}

impl Database {
    pub async fn list_terminology_dictionaries(
        &self,
        include_disabled: bool,
    ) -> Result<Vec<TerminologyDictionary>> {
        let mut connection = self.pool.get().await?;
        let mut query = terminology_dictionaries::table
            .filter(terminology_dictionaries::deleted_at.is_null())
            .into_boxed();
        if !include_disabled {
            query = query.filter(terminology_dictionaries::status.eq("active"));
        }
        query
            .order(terminology_dictionaries::updated_at.desc())
            .select(TerminologyDictionary::as_select())
            .load(&mut connection)
            .await
            .context("failed to list terminology dictionaries")
    }

    pub async fn create_terminology_dictionary(
        &self,
        actor: Uuid,
        input: &TerminologyDictionaryInput,
    ) -> Result<TerminologyDictionary> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(terminology_dictionaries::table)
            .values(NewTerminologyDictionary {
                id: Uuid::new_v4(),
                name: &input.name,
                industry: &input.industry,
                description: &input.description,
                source_language: &input.source_language,
                target_language: &input.target_language,
                status: &input.status,
                updated_by: actor,
            })
            .returning(TerminologyDictionary::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create terminology dictionary")
    }

    pub async fn update_terminology_dictionary(
        &self,
        id: Uuid,
        actor: Uuid,
        input: &TerminologyDictionaryInput,
    ) -> Result<Option<TerminologyDictionary>> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            terminology_dictionaries::table
                .filter(terminology_dictionaries::id.eq(id))
                .filter(terminology_dictionaries::deleted_at.is_null()),
        )
        .set((
            terminology_dictionaries::name.eq(&input.name),
            terminology_dictionaries::industry.eq(&input.industry),
            terminology_dictionaries::description.eq(&input.description),
            terminology_dictionaries::source_language.eq(&input.source_language),
            terminology_dictionaries::target_language.eq(&input.target_language),
            terminology_dictionaries::status.eq(&input.status),
            terminology_dictionaries::updated_by.eq(actor),
            terminology_dictionaries::updated_at.eq(Utc::now()),
        ))
        .returning(TerminologyDictionary::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to update terminology dictionary")
    }

    pub async fn delete_terminology_dictionary(&self, id: Uuid, actor: Uuid) -> Result<bool> {
        let mut connection = self.pool.get().await?;
        Ok(diesel::update(
            terminology_dictionaries::table
                .filter(terminology_dictionaries::id.eq(id))
                .filter(terminology_dictionaries::deleted_at.is_null()),
        )
        .set((
            terminology_dictionaries::status.eq("deleted"),
            terminology_dictionaries::updated_by.eq(actor),
            terminology_dictionaries::updated_at.eq(Utc::now()),
            terminology_dictionaries::deleted_at.eq(Some(Utc::now())),
        ))
        .execute(&mut connection)
        .await?
            > 0)
    }

    pub async fn list_terminology_entries(
        &self,
        dictionary_id: Uuid,
        include_disabled: bool,
    ) -> Result<Vec<TerminologyEntry>> {
        let mut connection = self.pool.get().await?;
        let mut query = terminology_entries::table
            .filter(terminology_entries::dictionary_id.eq(dictionary_id))
            .filter(terminology_entries::deleted_at.is_null())
            .into_boxed();
        if !include_disabled {
            query = query.filter(terminology_entries::status.eq("active"));
        }
        query
            .order(terminology_entries::priority.desc())
            .then_order_by(terminology_entries::source_term.asc())
            .select(TerminologyEntry::as_select())
            .load(&mut connection)
            .await
            .context("failed to list terminology entries")
    }

    pub async fn create_terminology_entry(
        &self,
        dictionary_id: Uuid,
        actor: Uuid,
        input: &TerminologyEntryInput,
    ) -> Result<TerminologyEntry> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(terminology_entries::table)
            .values(NewTerminologyEntry {
                id: Uuid::new_v4(),
                dictionary_id,
                source_term: &input.source_term,
                aliases: &input.aliases,
                target_term: &input.target_term,
                priority: input.priority,
                status: &input.status,
                updated_by: actor,
            })
            .returning(TerminologyEntry::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create terminology entry")
    }

    pub async fn update_terminology_entry(
        &self,
        id: Uuid,
        actor: Uuid,
        input: &TerminologyEntryInput,
    ) -> Result<Option<TerminologyEntry>> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            terminology_entries::table
                .filter(terminology_entries::id.eq(id))
                .filter(terminology_entries::deleted_at.is_null()),
        )
        .set((
            terminology_entries::source_term.eq(&input.source_term),
            terminology_entries::aliases.eq(&input.aliases),
            terminology_entries::target_term.eq(&input.target_term),
            terminology_entries::priority.eq(input.priority),
            terminology_entries::status.eq(&input.status),
            terminology_entries::updated_by.eq(actor),
            terminology_entries::updated_at.eq(Utc::now()),
        ))
        .returning(TerminologyEntry::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to update terminology entry")
    }

    pub async fn delete_terminology_entry(&self, id: Uuid, actor: Uuid) -> Result<bool> {
        let mut connection = self.pool.get().await?;
        Ok(diesel::update(
            terminology_entries::table
                .filter(terminology_entries::id.eq(id))
                .filter(terminology_entries::deleted_at.is_null()),
        )
        .set((
            terminology_entries::status.eq("deleted"),
            terminology_entries::updated_by.eq(actor),
            terminology_entries::updated_at.eq(Utc::now()),
            terminology_entries::deleted_at.eq(Some(Utc::now())),
        ))
        .execute(&mut connection)
        .await?
            > 0)
    }

    pub async fn list_blocked_words(&self, include_disabled: bool) -> Result<Vec<BlockedWord>> {
        let mut connection = self.pool.get().await?;
        let mut query = blocked_words::table
            .filter(blocked_words::deleted_at.is_null())
            .into_boxed();
        if !include_disabled {
            query = query.filter(blocked_words::status.eq("active"));
        }
        query
            .order(blocked_words::updated_at.desc())
            .select(BlockedWord::as_select())
            .load(&mut connection)
            .await
            .context("failed to list blocked words")
    }

    pub async fn create_blocked_word(
        &self,
        actor: Uuid,
        input: &BlockedWordInput,
    ) -> Result<BlockedWord> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(blocked_words::table)
            .values(NewBlockedWord {
                id: Uuid::new_v4(),
                word: &input.word,
                replacement: &input.replacement,
                match_mode: &input.match_mode,
                case_sensitive: input.case_sensitive,
                status: &input.status,
                note: &input.note,
                updated_by: actor,
            })
            .returning(BlockedWord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create blocked word")
    }

    pub async fn update_blocked_word(
        &self,
        id: Uuid,
        actor: Uuid,
        input: &BlockedWordInput,
    ) -> Result<Option<BlockedWord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            blocked_words::table
                .filter(blocked_words::id.eq(id))
                .filter(blocked_words::deleted_at.is_null()),
        )
        .set((
            blocked_words::word.eq(&input.word),
            blocked_words::replacement.eq(&input.replacement),
            blocked_words::match_mode.eq(&input.match_mode),
            blocked_words::case_sensitive.eq(input.case_sensitive),
            blocked_words::status.eq(&input.status),
            blocked_words::note.eq(&input.note),
            blocked_words::updated_by.eq(actor),
            blocked_words::updated_at.eq(Utc::now()),
        ))
        .returning(BlockedWord::as_returning())
        .get_result(&mut connection)
        .await
        .optional()
        .context("failed to update blocked word")
    }

    pub async fn delete_blocked_word(&self, id: Uuid, actor: Uuid) -> Result<bool> {
        let mut connection = self.pool.get().await?;
        Ok(diesel::update(
            blocked_words::table
                .filter(blocked_words::id.eq(id))
                .filter(blocked_words::deleted_at.is_null()),
        )
        .set((
            blocked_words::status.eq("deleted"),
            blocked_words::updated_by.eq(actor),
            blocked_words::updated_at.eq(Utc::now()),
            blocked_words::deleted_at.eq(Some(Utc::now())),
        ))
        .execute(&mut connection)
        .await?
            > 0)
    }

    pub async fn room_terminology_binding(&self, room_id: Uuid) -> Result<RoomTerminologyBinding> {
        let mut connection = self.pool.get().await?;
        let row = room_terminology_bindings::table
            .inner_join(terminology_dictionaries::table)
            .filter(room_terminology_bindings::room_id.eq(room_id))
            .filter(room_terminology_bindings::status.eq("active"))
            .filter(room_terminology_bindings::deleted_at.is_null())
            .filter(terminology_dictionaries::status.eq("active"))
            .filter(terminology_dictionaries::deleted_at.is_null())
            .select((terminology_dictionaries::id, terminology_dictionaries::name))
            .first::<(Uuid, String)>(&mut connection)
            .await
            .optional()?;
        Ok(RoomTerminologyBinding {
            dictionary_id: row.as_ref().map(|value| value.0),
            dictionary_name: row.map(|value| value.1),
        })
    }

    pub async fn set_room_terminology_binding(
        &self,
        room_id: Uuid,
        dictionary_id: Option<Uuid>,
        actor: Uuid,
    ) -> Result<()> {
        let mut connection = self.pool.get().await?;
        if let Some(dictionary_id) = dictionary_id {
            diesel::insert_into(room_terminology_bindings::table)
                .values(NewRoomTerminologyBinding {
                    room_id,
                    dictionary_id,
                    status: "active",
                    updated_by: actor,
                })
                .on_conflict(room_terminology_bindings::room_id)
                .do_update()
                .set((
                    room_terminology_bindings::dictionary_id.eq(dictionary_id),
                    room_terminology_bindings::status.eq("active"),
                    room_terminology_bindings::updated_by.eq(actor),
                    room_terminology_bindings::updated_at.eq(Utc::now()),
                    room_terminology_bindings::deleted_at.eq::<Option<DateTime<Utc>>>(None),
                ))
                .execute(&mut connection)
                .await?;
        } else {
            diesel::update(
                room_terminology_bindings::table
                    .filter(room_terminology_bindings::room_id.eq(room_id))
                    .filter(room_terminology_bindings::deleted_at.is_null()),
            )
            .set((
                room_terminology_bindings::status.eq("deleted"),
                room_terminology_bindings::updated_by.eq(actor),
                room_terminology_bindings::updated_at.eq(Utc::now()),
                room_terminology_bindings::deleted_at.eq(Some(Utc::now())),
            ))
            .execute(&mut connection)
            .await?;
        }
        Ok(())
    }
}
