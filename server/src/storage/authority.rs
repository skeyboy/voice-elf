use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::{
    BoolExpressionMethods, ExpressionMethods, Insertable, OptionalExtension,
    PgTextExpressionMethods, QueryDsl, SelectableHelper,
    dsl::{count_star, max},
};
use diesel_async::RunQueryDsl;
use serde::Serialize;
use uuid::Uuid;

use super::{Database, Paginated, paginated};
use crate::schema::{
    authority_access_tokens, authority_audit_events, authority_instances, authority_tenants,
};

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = authority_tenants)]
pub struct AuthorityTenantRecord {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub license_expires_at: DateTime<Utc>,
    pub grace_ends_at: DateTime<Utc>,
    pub warning_days: i32,
    pub offline_lease_minutes: i32,
    pub asr_backend_id: Option<String>,
    pub tts_backend_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = authority_tenants)]
struct NewAuthorityTenant<'a> {
    id: Uuid,
    name: &'a str,
    slug: &'a str,
    status: &'a str,
    license_expires_at: DateTime<Utc>,
    grace_ends_at: DateTime<Utc>,
    warning_days: i32,
    offline_lease_minutes: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthorityTenantSummary {
    #[serde(flatten)]
    pub tenant: AuthorityTenantRecord,
    pub instance_count: i64,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = authority_instances)]
pub struct AuthorityInstanceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub client_id: String,
    pub secret_hash: String,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_authorized_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthorityInstanceSummary {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub client_id: String,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_authorized_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<AuthorityInstanceRecord> for AuthorityInstanceSummary {
    fn from(instance: AuthorityInstanceRecord) -> Self {
        Self {
            id: instance.id,
            tenant_id: instance.tenant_id,
            name: instance.name,
            client_id: instance.client_id,
            status: instance.status,
            last_seen_at: instance.last_seen_at,
            last_authorized_at: instance.last_authorized_at,
            created_at: instance.created_at,
            updated_at: instance.updated_at,
        }
    }
}

#[derive(Insertable)]
#[diesel(table_name = authority_instances)]
struct NewAuthorityInstance<'a> {
    id: Uuid,
    tenant_id: Uuid,
    name: &'a str,
    client_id: &'a str,
    secret_hash: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = authority_access_tokens)]
struct NewAuthorityAccessToken<'a> {
    id: Uuid,
    instance_id: Uuid,
    token_hash: &'a str,
    expires_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = authority_audit_events)]
struct NewAuthorityAuditEvent<'a> {
    id: Uuid,
    tenant_id: Option<Uuid>,
    instance_id: Option<Uuid>,
    event_type: &'a str,
    detail: &'a str,
}

#[derive(Clone, Debug)]
pub struct AuthorityTokenContext {
    pub tenant: AuthorityTenantRecord,
    pub instance: AuthorityInstanceRecord,
}

impl Database {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_authority_tenant(
        &self,
        name: &str,
        slug: &str,
        status: &str,
        license_expires_at: DateTime<Utc>,
        grace_ends_at: DateTime<Utc>,
        warning_days: i32,
        offline_lease_minutes: i32,
    ) -> Result<AuthorityTenantRecord> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(authority_tenants::table)
            .values(NewAuthorityTenant {
                id: Uuid::new_v4(),
                name,
                slug,
                status,
                license_expires_at,
                grace_ends_at,
                warning_days,
                offline_lease_minutes,
            })
            .returning(AuthorityTenantRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create authority tenant")
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_authority_tenant(
        &self,
        tenant_id: Uuid,
        name: &str,
        status: &str,
        license_expires_at: DateTime<Utc>,
        grace_ends_at: DateTime<Utc>,
        warning_days: i32,
        offline_lease_minutes: i32,
    ) -> Result<Option<AuthorityTenantRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(authority_tenants::table.find(tenant_id))
            .set((
                authority_tenants::name.eq(name),
                authority_tenants::status.eq(status),
                authority_tenants::license_expires_at.eq(license_expires_at),
                authority_tenants::grace_ends_at.eq(grace_ends_at),
                authority_tenants::warning_days.eq(warning_days),
                authority_tenants::offline_lease_minutes.eq(offline_lease_minutes),
                authority_tenants::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AuthorityTenantRecord::as_returning())
            .get_result(&mut connection)
            .await
            .optional()
            .context("failed to update authority tenant")
    }

    pub async fn update_authority_tenant_asr(
        &self,
        tenant_id: Uuid,
        backend_id: Option<&str>,
    ) -> Result<Option<AuthorityTenantRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(authority_tenants::table.find(tenant_id))
            .set((
                authority_tenants::asr_backend_id.eq(backend_id),
                authority_tenants::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AuthorityTenantRecord::as_returning())
            .get_result(&mut connection)
            .await
            .optional()
            .context("failed to update authority tenant ASR backend")
    }

    pub async fn update_authority_tenant_tts(
        &self,
        tenant_id: Uuid,
        backend_id: Option<&str>,
    ) -> Result<Option<AuthorityTenantRecord>> {
        let mut connection = self.pool.get().await?;
        diesel::update(authority_tenants::table.find(tenant_id))
            .set((
                authority_tenants::tts_backend_id.eq(backend_id),
                authority_tenants::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AuthorityTenantRecord::as_returning())
            .get_result(&mut connection)
            .await
            .optional()
            .context("failed to update authority tenant TTS backend")
    }

    pub async fn get_authority_tenant(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<AuthorityTenantRecord>> {
        let mut connection = self.pool.get().await?;
        authority_tenants::table
            .find(tenant_id)
            .select(AuthorityTenantRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to get authority tenant")
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_authority_tenants(
        &self,
        search: Option<&str>,
        status: Option<&str>,
        sort: &str,
        descending: bool,
        page: i64,
        page_size: i64,
    ) -> Result<Paginated<AuthorityTenantSummary>> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let mut connection = self.pool.get().await?;
        let mut count_query = authority_tenants::table.into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            count_query = count_query.filter(
                authority_tenants::name
                    .ilike(pattern.clone())
                    .or(authority_tenants::slug.ilike(pattern)),
            );
        }
        if let Some(status) = status {
            count_query = count_query.filter(authority_tenants::status.eq(status));
        }
        let total = count_query
            .select(count_star())
            .first::<i64>(&mut connection)
            .await?;

        let mut query = authority_tenants::table.into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            query = query.filter(
                authority_tenants::name
                    .ilike(pattern.clone())
                    .or(authority_tenants::slug.ilike(pattern)),
            );
        }
        if let Some(status) = status {
            query = query.filter(authority_tenants::status.eq(status));
        }
        query = match (sort, descending) {
            ("name", false) => query.order(authority_tenants::name.asc()),
            ("name", true) => query.order(authority_tenants::name.desc()),
            ("license_expires_at", false) => {
                query.order(authority_tenants::license_expires_at.asc())
            }
            ("license_expires_at", true) => {
                query.order(authority_tenants::license_expires_at.desc())
            }
            ("created_at", false) => query.order(authority_tenants::created_at.asc()),
            _ => query.order(authority_tenants::created_at.desc()),
        };
        let records = query
            .offset((page - 1) * page_size)
            .limit(page_size)
            .select(AuthorityTenantRecord::as_select())
            .load::<AuthorityTenantRecord>(&mut connection)
            .await?;
        let mut items = Vec::with_capacity(records.len());
        for tenant in records {
            let instance_count = authority_instances::table
                .filter(authority_instances::tenant_id.eq(tenant.id))
                .select(count_star())
                .first::<i64>(&mut connection)
                .await?;
            let last_seen_at = authority_instances::table
                .filter(authority_instances::tenant_id.eq(tenant.id))
                .select(max(authority_instances::last_seen_at))
                .first::<Option<DateTime<Utc>>>(&mut connection)
                .await?;
            items.push(AuthorityTenantSummary {
                tenant,
                instance_count,
                last_seen_at,
            });
        }
        Ok(paginated(items, page, page_size, total))
    }

    pub async fn create_authority_instance(
        &self,
        tenant_id: Uuid,
        name: &str,
        client_id: &str,
        secret_hash: &str,
    ) -> Result<AuthorityInstanceRecord> {
        let mut connection = self.pool.get().await?;
        diesel::insert_into(authority_instances::table)
            .values(NewAuthorityInstance {
                id: Uuid::new_v4(),
                tenant_id,
                name,
                client_id,
                secret_hash,
            })
            .returning(AuthorityInstanceRecord::as_returning())
            .get_result(&mut connection)
            .await
            .context("failed to create authority instance")
    }

    pub async fn list_authority_instances(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<AuthorityInstanceSummary>> {
        let mut connection = self.pool.get().await?;
        let records = authority_instances::table
            .filter(authority_instances::tenant_id.eq(tenant_id))
            .order(authority_instances::created_at.desc())
            .select(AuthorityInstanceRecord::as_select())
            .load::<AuthorityInstanceRecord>(&mut connection)
            .await?;
        Ok(records.into_iter().map(Into::into).collect())
    }

    pub async fn get_authority_instance_by_client_id(
        &self,
        client_id: &str,
    ) -> Result<Option<AuthorityInstanceRecord>> {
        let mut connection = self.pool.get().await?;
        authority_instances::table
            .filter(authority_instances::client_id.eq(client_id))
            .select(AuthorityInstanceRecord::as_select())
            .first(&mut connection)
            .await
            .optional()
            .context("failed to get authority instance")
    }

    pub async fn update_authority_instance_status(
        &self,
        instance_id: Uuid,
        status: &str,
    ) -> Result<Option<AuthorityInstanceRecord>> {
        let mut connection = self.pool.get().await?;
        let instance = diesel::update(authority_instances::table.find(instance_id))
            .set((
                authority_instances::status.eq(status),
                authority_instances::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AuthorityInstanceRecord::as_returning())
            .get_result(&mut connection)
            .await
            .optional()?;
        if status != "active" {
            diesel::delete(
                authority_access_tokens::table
                    .filter(authority_access_tokens::instance_id.eq(instance_id)),
            )
            .execute(&mut connection)
            .await?;
        }
        Ok(instance)
    }

    pub async fn rotate_authority_instance_secret(
        &self,
        instance_id: Uuid,
        secret_hash: &str,
    ) -> Result<Option<AuthorityInstanceRecord>> {
        let mut connection = self.pool.get().await?;
        let instance = diesel::update(authority_instances::table.find(instance_id))
            .set((
                authority_instances::secret_hash.eq(secret_hash),
                authority_instances::updated_at.eq(diesel::dsl::now),
            ))
            .returning(AuthorityInstanceRecord::as_returning())
            .get_result(&mut connection)
            .await
            .optional()?;
        diesel::delete(
            authority_access_tokens::table
                .filter(authority_access_tokens::instance_id.eq(instance_id)),
        )
        .execute(&mut connection)
        .await?;
        Ok(instance)
    }

    pub async fn create_authority_access_token(
        &self,
        instance_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::delete(
            authority_access_tokens::table
                .filter(authority_access_tokens::expires_at.le(Utc::now())),
        )
        .execute(&mut connection)
        .await?;
        diesel::insert_into(authority_access_tokens::table)
            .values(NewAuthorityAccessToken {
                id: Uuid::new_v4(),
                instance_id,
                token_hash,
                expires_at,
            })
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn authority_context_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<AuthorityTokenContext>> {
        let mut connection = self.pool.get().await?;
        let instance_id = authority_access_tokens::table
            .filter(authority_access_tokens::token_hash.eq(token_hash))
            .filter(authority_access_tokens::expires_at.gt(Utc::now()))
            .select(authority_access_tokens::instance_id)
            .first::<Uuid>(&mut connection)
            .await
            .optional()?;
        let Some(instance_id) = instance_id else {
            return Ok(None);
        };
        let instance = authority_instances::table
            .find(instance_id)
            .select(AuthorityInstanceRecord::as_select())
            .first::<AuthorityInstanceRecord>(&mut connection)
            .await?;
        let tenant = authority_tenants::table
            .find(instance.tenant_id)
            .select(AuthorityTenantRecord::as_select())
            .first::<AuthorityTenantRecord>(&mut connection)
            .await?;
        Ok(Some(AuthorityTokenContext { tenant, instance }))
    }

    pub async fn record_authority_check(
        &self,
        tenant_id: Uuid,
        instance_id: Uuid,
        allowed: bool,
        status: &str,
    ) -> Result<()> {
        let mut connection = self.pool.get().await?;
        let now = Utc::now();
        diesel::update(authority_instances::table.find(instance_id))
            .set((
                authority_instances::last_seen_at.eq(Some(now)),
                authority_instances::last_authorized_at.eq(if allowed { Some(now) } else { None }),
                authority_instances::updated_at.eq(diesel::dsl::now),
            ))
            .execute(&mut connection)
            .await?;
        diesel::insert_into(authority_audit_events::table)
            .values(NewAuthorityAuditEvent {
                id: Uuid::new_v4(),
                tenant_id: Some(tenant_id),
                instance_id: Some(instance_id),
                event_type: "entitlement_check",
                detail: status,
            })
            .execute(&mut connection)
            .await?;
        Ok(())
    }
}
