use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, Insertable, OptionalExtension, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use serde::Serialize;
use uuid::Uuid;

use super::Database;
use crate::schema::asr_system_settings;

const SYSTEM_ASR_SETTING_ID: Uuid = Uuid::from_u128(2);

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = asr_system_settings)]
pub struct AsrSystemSetting {
    pub id: Uuid,
    pub backend_id: String,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = asr_system_settings)]
struct NewAsrSystemSetting<'a> {
    id: Uuid,
    backend_id: &'a str,
    updated_by: Option<Uuid>,
}

impl Database {
    pub async fn ensure_asr_system_setting(&self, backend_id: &str) -> Result<AsrSystemSetting> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(asr_system_settings::table)
            .values(NewAsrSystemSetting {
                id: SYSTEM_ASR_SETTING_ID,
                backend_id,
                updated_by: None,
            })
            .on_conflict(asr_system_settings::id)
            .do_nothing()
            .execute(&mut connection)
            .await?;
        asr_system_settings::table
            .find(SYSTEM_ASR_SETTING_ID)
            .select(AsrSystemSetting::as_select())
            .first(&mut connection)
            .await
            .context("failed to load system ASR setting")
    }

    pub async fn asr_system_setting(&self) -> Result<Option<AsrSystemSetting>> {
        let mut connection = self.pool.get().await?;
        asr_system_settings::table
            .find(SYSTEM_ASR_SETTING_ID)
            .select(AsrSystemSetting::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to load system ASR setting")
    }

    pub async fn update_asr_system_setting(
        &self,
        backend_id: &str,
        updated_by: Uuid,
    ) -> Result<AsrSystemSetting> {
        let mut connection = self.pool.get().await?;
        diesel::update(asr_system_settings::table.find(SYSTEM_ASR_SETTING_ID))
            .set((
                asr_system_settings::backend_id.eq(backend_id),
                asr_system_settings::updated_by.eq(Some(updated_by)),
                asr_system_settings::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AsrSystemSetting::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to update system ASR setting")
    }
}
