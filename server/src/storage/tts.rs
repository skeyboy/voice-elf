use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, Insertable, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use serde::Serialize;
use uuid::Uuid;

use super::Database;
use crate::schema::{tts_system_settings, tts_voice_aliases};

const SYSTEM_TTS_SETTING_ID: Uuid = Uuid::from_u128(3);

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = tts_system_settings)]
pub struct TtsSystemSetting {
    pub id: Uuid,
    pub backend_id: String,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = tts_system_settings)]
struct NewTtsSystemSetting<'a> {
    id: Uuid,
    backend_id: &'a str,
    updated_by: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = tts_voice_aliases)]
pub struct TtsVoiceAlias {
    pub provider_id: String,
    pub voice_id: String,
    pub alias: String,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub record_status: String,
}

#[derive(Insertable)]
#[diesel(table_name = tts_voice_aliases)]
struct NewTtsVoiceAlias<'a> {
    provider_id: &'a str,
    voice_id: &'a str,
    alias: &'a str,
    updated_by: Option<Uuid>,
}

impl Database {
    pub async fn ensure_tts_system_setting(&self, backend_id: &str) -> Result<TtsSystemSetting> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(tts_system_settings::table)
            .values(NewTtsSystemSetting {
                id: SYSTEM_TTS_SETTING_ID,
                backend_id,
                updated_by: None,
            })
            .on_conflict(tts_system_settings::id)
            .do_nothing()
            .execute(&mut connection)
            .await?;
        tts_system_settings::table
            .find(SYSTEM_TTS_SETTING_ID)
            .select(TtsSystemSetting::as_select())
            .first(&mut connection)
            .await
            .context("failed to load system TTS setting")
    }

    pub async fn tts_system_setting(&self) -> Result<Option<TtsSystemSetting>> {
        let mut connection = self.pool.get().await?;
        tts_system_settings::table
            .find(SYSTEM_TTS_SETTING_ID)
            .select(TtsSystemSetting::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to load system TTS setting")
    }

    pub async fn update_tts_system_setting(
        &self,
        backend_id: &str,
        updated_by: Uuid,
    ) -> Result<TtsSystemSetting> {
        let mut connection = self.pool.get().await?;
        diesel::update(tts_system_settings::table.find(SYSTEM_TTS_SETTING_ID))
            .set((
                tts_system_settings::backend_id.eq(backend_id),
                tts_system_settings::updated_by.eq(Some(updated_by)),
                tts_system_settings::updated_at.eq(diesel::dsl::now),
            ))
            .returning(TtsSystemSetting::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to update system TTS setting")
    }

    pub async fn list_tts_voice_aliases(&self, provider_id: &str) -> Result<Vec<TtsVoiceAlias>> {
        let mut connection = self.pool.get().await?;
        tts_voice_aliases::table
            .filter(tts_voice_aliases::provider_id.eq(provider_id))
            .filter(tts_voice_aliases::record_status.eq("current"))
            .order(tts_voice_aliases::voice_id.asc())
            .select(TtsVoiceAlias::as_select())
            .load(&mut connection)
            .await
            .context("failed to list TTS voice aliases")
    }

    pub async fn update_tts_voice_alias(
        &self,
        provider_id: &str,
        voice_id: &str,
        alias: Option<&str>,
        updated_by: Uuid,
    ) -> Result<Option<TtsVoiceAlias>> {
        let mut connection = self.pool.get().await?;
        let Some(alias) = alias else {
            diesel::update(tts_voice_aliases::table.find((provider_id, voice_id)))
                .set((
                    tts_voice_aliases::record_status.eq("deleted"),
                    tts_voice_aliases::updated_by.eq(Some(updated_by)),
                    tts_voice_aliases::updated_at.eq(diesel::dsl::now),
                ))
                .execute(&mut connection)
                .await?;
            return Ok(None);
        };
        let row = NewTtsVoiceAlias {
            provider_id,
            voice_id,
            alias,
            updated_by: Some(updated_by),
        };
        diesel::insert_into(tts_voice_aliases::table)
            .values(row)
            .on_conflict((tts_voice_aliases::provider_id, tts_voice_aliases::voice_id))
            .do_update()
            .set((
                tts_voice_aliases::alias.eq(alias),
                tts_voice_aliases::record_status.eq("current"),
                tts_voice_aliases::updated_by.eq(Some(updated_by)),
                tts_voice_aliases::updated_at.eq(diesel::dsl::now),
            ))
            .returning(TtsVoiceAlias::as_returning())
            .get_result(&mut connection)
            .await
            .map(Some)
            .context("failed to update TTS voice alias")
    }
}
