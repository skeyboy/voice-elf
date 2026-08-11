use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper,
    dsl::{count_star, max},
};
use diesel_async::{AsyncConnection, RunQueryDsl, SimpleAsyncConnection};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    config::{MailConfig, SmtpSecurity},
    schema::{data_change_history, system_email_settings},
};

use super::{Database, Paginated, paginated};

#[derive(Clone, Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = system_email_settings)]
#[allow(dead_code)]
pub struct EmailSettingRecord {
    pub id: Uuid,
    pub version: i64,
    pub record_status: String,
    pub enabled: bool,
    pub host: String,
    pub port: i32,
    pub security: String,
    pub username: String,
    pub password_secret: Option<String>,
    pub from_address: String,
    pub from_name: String,
    pub public_url: Option<String>,
    pub reset_expiry_minutes: i32,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl EmailSettingRecord {
    pub fn mail_config(&self) -> MailConfig {
        MailConfig {
            enabled: self.enabled,
            host: self.host.clone(),
            port: self.port as u16,
            security: match self.security.as_str() {
                "starttls" => SmtpSecurity::StartTls,
                "none" => SmtpSecurity::None,
                _ => SmtpSecurity::Wrapper,
            },
            username: self.username.clone(),
            password: self.password_secret.clone(),
            from_address: self.from_address.clone(),
            from_name: self.from_name.clone(),
            public_url: self.public_url.clone(),
            reset_expiry: Duration::from_secs(self.reset_expiry_minutes as u64 * 60),
        }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = system_email_settings)]
struct NewEmailSetting<'a> {
    id: Uuid,
    version: i64,
    record_status: &'a str,
    enabled: bool,
    host: &'a str,
    port: i32,
    security: &'a str,
    username: &'a str,
    password_secret: Option<&'a str>,
    from_address: &'a str,
    from_name: &'a str,
    public_url: Option<&'a str>,
    reset_expiry_minutes: i32,
    updated_by: Uuid,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = data_change_history)]
pub struct ChangeHistoryRecord {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: String,
    pub action: String,
    pub record_status: String,
    pub actor_user_id: Option<Uuid>,
    pub before_state: Option<Value>,
    pub after_state: Option<Value>,
    pub created_at: DateTime<Utc>,
}

impl Database {
    pub async fn email_setting(&self) -> Result<Option<EmailSettingRecord>> {
        let mut connection = self.pool.get().await?;
        system_email_settings::table
            .filter(system_email_settings::record_status.eq("current"))
            .order(system_email_settings::version.desc())
            .select(EmailSettingRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to load email setting")
    }

    pub async fn save_email_setting(
        &self,
        actor_user_id: Uuid,
        config: &MailConfig,
    ) -> Result<EmailSettingRecord> {
        let mut connection = self.pool.get().await?;
        let config = config.clone();
        connection
            .transaction::<EmailSettingRecord, diesel::result::Error, _>(async |connection| {
                connection
                    .batch_execute("LOCK TABLE system_email_settings IN EXCLUSIVE MODE")
                    .await?;
                let latest_version = system_email_settings::table
                    .select(max(system_email_settings::version))
                    .first::<Option<i64>>(connection)
                    .await?
                    .unwrap_or(0);
                diesel::update(
                    system_email_settings::table
                        .filter(system_email_settings::record_status.eq("current")),
                )
                .set(system_email_settings::record_status.eq("historical"))
                .execute(connection)
                .await?;
                let security = match config.security {
                    SmtpSecurity::Wrapper => "wrapper",
                    SmtpSecurity::StartTls => "starttls",
                    SmtpSecurity::None => "none",
                };
                diesel::insert_into(system_email_settings::table)
                    .values(NewEmailSetting {
                        id: Uuid::new_v4(),
                        version: latest_version + 1,
                        record_status: "current",
                        enabled: config.enabled,
                        host: &config.host,
                        port: i32::from(config.port),
                        security,
                        username: &config.username,
                        password_secret: config.password.as_deref(),
                        from_address: &config.from_address,
                        from_name: &config.from_name,
                        public_url: config.public_url.as_deref(),
                        reset_expiry_minutes: (config.reset_expiry.as_secs() / 60) as i32,
                        updated_by: actor_user_id,
                    })
                    .returning(EmailSettingRecord::as_returning())
                    .get_result(connection)
                    .await
            })
            .await
            .context("failed to save email setting")
    }

    pub async fn list_change_history(
        &self,
        entity_type: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> Result<Paginated<ChangeHistoryRecord>> {
        let mut connection = self.pool.get().await?;
        let mut count_query = data_change_history::table.into_boxed();
        if let Some(entity_type) = entity_type {
            count_query = count_query.filter(data_change_history::entity_type.eq(entity_type));
        }
        let total = count_query
            .select(count_star())
            .first(&mut connection)
            .await?;

        let mut query = data_change_history::table.into_boxed();
        if let Some(entity_type) = entity_type {
            query = query.filter(data_change_history::entity_type.eq(entity_type));
        }
        let items = query
            .order(data_change_history::created_at.desc())
            .then_order_by(data_change_history::id.desc())
            .offset((page - 1) * page_size)
            .limit(page_size)
            .select(ChangeHistoryRecord::as_select())
            .load(&mut connection)
            .await
            .context("failed to list change history")?;
        Ok(paginated(items, page, page_size, total))
    }
}
