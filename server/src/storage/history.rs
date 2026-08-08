use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use diesel::{
    BoolExpressionMethods, ExpressionMethods, Insertable, PgTextExpressionMethods, QueryDsl,
    SelectableHelper,
};
use diesel_async::RunQueryDsl;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    protocol::{LatencyReport, SpeakerIdentity, TranscriptionSegment},
    schema::{
        rooms, voice_sessions, voice_utterance_refinements, voice_utterance_speakers,
        voice_utterances,
    },
};

use super::{Database, Paginated, paginated};

pub struct NewUtteranceAttempt<'a> {
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub room_id: Uuid,
    pub source_language: &'a str,
    pub target_language: &'a str,
    pub source_audio_path: Option<&'a str>,
    pub source_audio_url: Option<&'a str>,
    pub latency: &'a LatencyReport,
    pub speakers: &'a [SpeakerIdentity],
}

pub struct TranscriptUpdate<'a> {
    pub id: Uuid,
    pub source_text: &'a str,
    pub source_language: &'a str,
    pub latency: &'a LatencyReport,
}

pub struct TranslationUpdate<'a> {
    pub id: Uuid,
    pub translated_text: &'a str,
    pub target_language: &'a str,
    pub latency: &'a LatencyReport,
}

pub struct UtteranceAudioUpdate<'a> {
    pub id: Uuid,
    pub translated_audio_path: &'a str,
    pub translated_audio_url: &'a str,
    pub latency: &'a LatencyReport,
}

pub struct RefinementUpdate<'a> {
    pub utterance_id: Uuid,
    pub engine: &'a str,
    pub text: &'a str,
    pub language: &'a str,
    pub segments: &'a [TranscriptionSegment],
}

#[derive(Insertable)]
#[diesel(table_name = voice_utterances)]
struct UtteranceRow<'a> {
    id: Uuid,
    session_id: Uuid,
    user_id: Option<Uuid>,
    room_id: Option<Uuid>,
    source_text: &'a str,
    translated_text: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    source_audio_path: Option<&'a str>,
    source_audio_url: Option<&'a str>,
    translated_audio_path: Option<&'a str>,
    translated_audio_url: Option<&'a str>,
    audio_ms: i64,
    vad_ms: i64,
    stt_ms: i64,
    translation_ms: i64,
    tts_ms: i64,
    total_ms: i64,
    t0_unix_ms: i64,
    t1_unix_ms: i64,
    t2_unix_ms: i64,
    t3_unix_ms: i64,
    t4_unix_ms: i64,
    status: &'a str,
    processing_error: Option<&'a str>,
}

#[derive(Insertable)]
#[diesel(table_name = voice_utterance_speakers)]
struct UtteranceSpeakerRow<'a> {
    id: Uuid,
    utterance_id: Uuid,
    user_id: Option<Uuid>,
    username: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = voice_utterance_refinements)]
struct RefinementRow<'a> {
    id: Uuid,
    utterance_id: Uuid,
    engine: &'a str,
    text: &'a str,
    language: &'a str,
    segments_json: &'a str,
    status: &'a str,
    processing_error: Option<&'a str>,
}

#[derive(diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = voice_utterance_speakers)]
struct UtteranceSpeakerHistoryRow {
    utterance_id: Uuid,
    user_id: Option<Uuid>,
    username: String,
}

#[derive(diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = voice_utterance_refinements)]
struct RefinementHistoryRow {
    utterance_id: Uuid,
    engine: String,
    text: String,
    language: String,
    segments_json: String,
    status: String,
    processing_error: Option<String>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = voice_utterances)]
struct UtteranceHistoryRow {
    id: Uuid,
    source_text: String,
    translated_text: String,
    source_language: String,
    target_language: String,
    source_audio_url: Option<String>,
    translated_audio_url: Option<String>,
    status: String,
    processing_error: Option<String>,
    audio_ms: i64,
    vad_ms: i64,
    stt_ms: i64,
    translation_ms: i64,
    tts_ms: i64,
    total_ms: i64,
    t0_unix_ms: i64,
    t1_unix_ms: i64,
    t2_unix_ms: i64,
    t3_unix_ms: i64,
    t4_unix_ms: i64,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UtteranceHistory {
    pub id: Uuid,
    pub source_text: String,
    pub translated_text: String,
    pub source_language: String,
    pub target_language: String,
    pub source_audio_url: Option<String>,
    pub translated_audio_url: Option<String>,
    pub status: String,
    pub processing_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub latency: LatencyReport,
    pub speakers: Vec<SpeakerIdentity>,
    pub refinements: Vec<UtteranceRefinement>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UtteranceRefinement {
    pub engine: String,
    pub text: String,
    pub language: String,
    pub segments: Vec<TranscriptionSegment>,
    pub status: String,
    pub processing_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Database {
    pub async fn recover_interrupted_utterances(&self) -> Result<usize> {
        let mut connection = self.pool.get().await?;
        let mut updated = 0;
        updated += diesel::update(
            voice_utterances::table.filter(voice_utterances::status.eq("recognizing")),
        )
        .set((
            voice_utterances::status.eq("recognition_interrupted"),
            voice_utterances::processing_error
                .eq(Some("Server restarted before recognition completed")),
        ))
        .execute(&mut connection)
        .await?;
        updated += diesel::update(
            voice_utterances::table.filter(voice_utterances::status.eq("translating")),
        )
        .set((
            voice_utterances::status.eq("translation_interrupted"),
            voice_utterances::processing_error
                .eq(Some("Server restarted before translation completed")),
        ))
        .execute(&mut connection)
        .await?;
        updated += diesel::update(
            voice_utterances::table.filter(voice_utterances::status.eq("text_ready")),
        )
        .set((
            voice_utterances::status.eq("tts_interrupted"),
            voice_utterances::processing_error.eq(Some("Server restarted before TTS completed")),
        ))
        .execute(&mut connection)
        .await?;
        diesel::update(voice_sessions::table.filter(voice_sessions::ended_at.is_null()))
            .set(voice_sessions::ended_at.eq(diesel::dsl::now))
            .execute(&mut connection)
            .await?;
        diesel::update(
            voice_utterance_refinements::table
                .filter(voice_utterance_refinements::status.eq("processing")),
        )
        .set((
            voice_utterance_refinements::status.eq("interrupted"),
            voice_utterance_refinements::processing_error
                .eq(Some("Server restarted before refinement completed")),
        ))
        .execute(&mut connection)
        .await?;
        Ok(updated)
    }

    pub async fn interrupt_session_utterances(
        &self,
        session_id: Uuid,
        reason: &str,
    ) -> Result<usize> {
        let mut connection = self.pool.get().await?;
        let mut updated = 0;
        updated += diesel::update(
            voice_utterances::table
                .filter(voice_utterances::session_id.eq(session_id))
                .filter(voice_utterances::status.eq("recognizing")),
        )
        .set((
            voice_utterances::status.eq("recognition_interrupted"),
            voice_utterances::processing_error.eq(Some(reason)),
        ))
        .execute(&mut connection)
        .await?;
        updated += diesel::update(
            voice_utterances::table
                .filter(voice_utterances::session_id.eq(session_id))
                .filter(voice_utterances::status.eq("translating")),
        )
        .set((
            voice_utterances::status.eq("translation_interrupted"),
            voice_utterances::processing_error.eq(Some(reason)),
        ))
        .execute(&mut connection)
        .await?;
        updated += diesel::update(
            voice_utterances::table
                .filter(voice_utterances::session_id.eq(session_id))
                .filter(voice_utterances::status.eq("text_ready")),
        )
        .set((
            voice_utterances::status.eq("tts_interrupted"),
            voice_utterances::processing_error.eq(Some(reason)),
        ))
        .execute(&mut connection)
        .await?;
        Ok(updated)
    }

    pub async fn create_utterance_attempt(&self, utterance: NewUtteranceAttempt<'_>) -> Result<()> {
        let latency = utterance.latency;
        let row = UtteranceRow {
            id: utterance.id,
            session_id: utterance.session_id,
            user_id: match utterance.speakers {
                [speaker] => speaker.user_id,
                [] => Some(utterance.user_id),
                _ => None,
            },
            room_id: Some(utterance.room_id),
            source_text: "",
            translated_text: "",
            source_language: utterance.source_language,
            target_language: utterance.target_language,
            source_audio_path: utterance.source_audio_path,
            source_audio_url: utterance.source_audio_url,
            translated_audio_path: None,
            translated_audio_url: None,
            audio_ms: to_i64(latency.audio_ms),
            vad_ms: to_i64(latency.vad_ms),
            stt_ms: to_i64(latency.stt_ms),
            translation_ms: to_i64(latency.translation_ms),
            tts_ms: to_i64(latency.tts_ms),
            total_ms: to_i64(latency.total_ms),
            t0_unix_ms: to_i64(latency.t0_unix_ms),
            t1_unix_ms: to_i64(latency.t1_unix_ms),
            t2_unix_ms: to_i64(latency.t2_unix_ms),
            t3_unix_ms: to_i64(latency.t3_unix_ms),
            t4_unix_ms: to_i64(latency.t4_unix_ms),
            status: "recognizing",
            processing_error: None,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(voice_utterances::table)
            .values(row)
            .execute(&mut connection)
            .await?;
        if !utterance.speakers.is_empty() {
            let speakers = utterance
                .speakers
                .iter()
                .map(|speaker| UtteranceSpeakerRow {
                    id: Uuid::new_v4(),
                    utterance_id: utterance.id,
                    user_id: speaker.user_id,
                    username: &speaker.username,
                })
                .collect::<Vec<_>>();
            diesel::insert_into(voice_utterance_speakers::table)
                .values(speakers)
                .on_conflict_do_nothing()
                .execute(&mut connection)
                .await?;
        }
        diesel::update(rooms::table.find(utterance.room_id))
            .set(rooms::updated_at.eq(diesel::dsl::now))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn save_utterance_transcript(&self, update: TranscriptUpdate<'_>) -> Result<()> {
        let latency = update.latency;
        let mut connection = self.pool.get().await?;
        diesel::update(voice_utterances::table.find(update.id))
            .set((
                voice_utterances::source_text.eq(update.source_text),
                voice_utterances::source_language.eq(update.source_language),
                voice_utterances::stt_ms.eq(to_i64(latency.stt_ms)),
                voice_utterances::total_ms.eq(to_i64(latency.total_ms)),
                voice_utterances::t2_unix_ms.eq(to_i64(latency.t2_unix_ms)),
                voice_utterances::t3_unix_ms.eq(to_i64(latency.t3_unix_ms)),
                voice_utterances::t4_unix_ms.eq(to_i64(latency.t4_unix_ms)),
                voice_utterances::status.eq("translating"),
                voice_utterances::processing_error.eq(None::<String>),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn save_utterance_translation(&self, update: TranslationUpdate<'_>) -> Result<()> {
        let latency = update.latency;
        let mut connection = self.pool.get().await?;
        diesel::update(voice_utterances::table.find(update.id))
            .set((
                voice_utterances::translated_text.eq(update.translated_text),
                voice_utterances::target_language.eq(update.target_language),
                voice_utterances::translation_ms.eq(to_i64(latency.translation_ms)),
                voice_utterances::total_ms.eq(to_i64(latency.total_ms)),
                voice_utterances::t3_unix_ms.eq(to_i64(latency.t3_unix_ms)),
                voice_utterances::t4_unix_ms.eq(to_i64(latency.t4_unix_ms)),
                voice_utterances::status.eq("text_ready"),
                voice_utterances::processing_error.eq(None::<String>),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn mark_utterance_failed(&self, id: Uuid, status: &str, error: &str) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::update(voice_utterances::table.find(id))
            .set((
                voice_utterances::status.eq(status),
                voice_utterances::processing_error.eq(Some(error)),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn complete_utterance_audio(&self, update: UtteranceAudioUpdate<'_>) -> Result<()> {
        let latency = update.latency;
        let mut connection = self.pool.get().await?;
        diesel::update(voice_utterances::table.find(update.id))
            .set((
                voice_utterances::translated_audio_path.eq(Some(update.translated_audio_path)),
                voice_utterances::translated_audio_url.eq(Some(update.translated_audio_url)),
                voice_utterances::tts_ms.eq(to_i64(latency.tts_ms)),
                voice_utterances::total_ms.eq(to_i64(latency.total_ms)),
                voice_utterances::t4_unix_ms.eq(to_i64(latency.t4_unix_ms)),
                voice_utterances::status.eq("completed"),
                voice_utterances::processing_error.eq(None::<String>),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn start_utterance_refinement(&self, utterance_id: Uuid, engine: &str) -> Result<()> {
        let row = RefinementRow {
            id: Uuid::new_v4(),
            utterance_id,
            engine,
            text: "",
            language: "auto",
            segments_json: "[]",
            status: "processing",
            processing_error: None,
        };
        let mut connection = self.pool.get().await?;
        diesel::insert_into(voice_utterance_refinements::table)
            .values(row)
            .on_conflict((
                voice_utterance_refinements::utterance_id,
                voice_utterance_refinements::engine,
            ))
            .do_update()
            .set((
                voice_utterance_refinements::text.eq(""),
                voice_utterance_refinements::language.eq("auto"),
                voice_utterance_refinements::segments_json.eq("[]"),
                voice_utterance_refinements::status.eq("processing"),
                voice_utterance_refinements::processing_error.eq(None::<String>),
                voice_utterance_refinements::completed_at.eq(None::<DateTime<Utc>>),
            ))
            .execute(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn save_utterance_refinement(&self, update: RefinementUpdate<'_>) -> Result<()> {
        let segments_json = serde_json::to_string(update.segments)?;
        let mut connection = self.pool.get().await?;
        diesel::update(
            voice_utterance_refinements::table
                .filter(voice_utterance_refinements::utterance_id.eq(update.utterance_id))
                .filter(voice_utterance_refinements::engine.eq(update.engine)),
        )
        .set((
            voice_utterance_refinements::text.eq(update.text),
            voice_utterance_refinements::language.eq(update.language),
            voice_utterance_refinements::segments_json.eq(segments_json),
            voice_utterance_refinements::status.eq("completed"),
            voice_utterance_refinements::processing_error.eq(None::<String>),
            voice_utterance_refinements::completed_at.eq(diesel::dsl::now),
        ))
        .execute(&mut connection)
        .await?;
        Ok(())
    }

    pub async fn fail_utterance_refinement(
        &self,
        utterance_id: Uuid,
        engine: &str,
        error: &str,
    ) -> Result<()> {
        let mut connection = self.pool.get().await?;
        diesel::update(
            voice_utterance_refinements::table
                .filter(voice_utterance_refinements::utterance_id.eq(utterance_id))
                .filter(voice_utterance_refinements::engine.eq(engine)),
        )
        .set((
            voice_utterance_refinements::status.eq("failed"),
            voice_utterance_refinements::processing_error.eq(Some(error)),
            voice_utterance_refinements::completed_at.eq(diesel::dsl::now),
        ))
        .execute(&mut connection)
        .await?;
        Ok(())
    }

    pub async fn list_utterances(
        &self,
        room_id: Uuid,
        search: Option<&str>,
    ) -> Result<Vec<UtteranceHistory>> {
        Ok(self
            .list_utterances_page(room_id, search, 1, 200)
            .await?
            .items)
    }

    pub async fn list_utterances_page(
        &self,
        room_id: Uuid,
        search: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> Result<Paginated<UtteranceHistory>> {
        let mut connection = self.pool.get().await?;
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let mut count_query = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room_id)))
            .into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            count_query = count_query.filter(
                voice_utterances::source_text
                    .ilike(pattern.clone())
                    .or(voice_utterances::translated_text.ilike(pattern)),
            );
        }
        let total = count_query.count().get_result(&mut connection).await?;
        let mut query = voice_utterances::table
            .filter(voice_utterances::room_id.eq(Some(room_id)))
            .into_boxed();
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            query = query.filter(
                voice_utterances::source_text
                    .ilike(pattern.clone())
                    .or(voice_utterances::translated_text.ilike(pattern)),
            );
        }
        let rows = query
            .order(voice_utterances::created_at.desc())
            .offset((page - 1) * page_size)
            .limit(page_size)
            .select(UtteranceHistoryRow::as_select())
            .load(&mut connection)
            .await?;
        let utterance_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let speaker_rows = if utterance_ids.is_empty() {
            Vec::new()
        } else {
            voice_utterance_speakers::table
                .filter(voice_utterance_speakers::utterance_id.eq_any(&utterance_ids))
                .order(voice_utterance_speakers::created_at.asc())
                .select(UtteranceSpeakerHistoryRow::as_select())
                .load(&mut connection)
                .await?
        };
        let mut speakers_by_utterance = HashMap::<Uuid, Vec<SpeakerIdentity>>::new();
        for speaker in speaker_rows {
            speakers_by_utterance
                .entry(speaker.utterance_id)
                .or_default()
                .push(SpeakerIdentity {
                    user_id: speaker.user_id,
                    username: speaker.username,
                });
        }
        let refinement_rows = if utterance_ids.is_empty() {
            Vec::new()
        } else {
            voice_utterance_refinements::table
                .filter(voice_utterance_refinements::utterance_id.eq_any(&utterance_ids))
                .order(voice_utterance_refinements::created_at.asc())
                .select(RefinementHistoryRow::as_select())
                .load(&mut connection)
                .await?
        };
        let mut refinements_by_utterance = HashMap::<Uuid, Vec<UtteranceRefinement>>::new();
        for refinement in refinement_rows {
            refinements_by_utterance
                .entry(refinement.utterance_id)
                .or_default()
                .push(UtteranceRefinement {
                    engine: refinement.engine,
                    text: refinement.text,
                    language: refinement.language,
                    segments: serde_json::from_str(&refinement.segments_json).unwrap_or_default(),
                    status: refinement.status,
                    processing_error: refinement.processing_error,
                    created_at: refinement.created_at,
                    completed_at: refinement.completed_at,
                });
        }
        let items = rows
            .into_iter()
            .map(|row| {
                let row_id = row.id;
                let mut history = UtteranceHistory::from(row);
                history.speakers = speakers_by_utterance.remove(&row_id).unwrap_or_default();
                history.refinements = refinements_by_utterance.remove(&row_id).unwrap_or_default();
                history
            })
            .collect();
        Ok(paginated(items, page, page_size, total))
    }
}

impl From<UtteranceHistoryRow> for UtteranceHistory {
    fn from(row: UtteranceHistoryRow) -> Self {
        Self {
            id: row.id,
            source_text: row.source_text,
            translated_text: row.translated_text,
            source_language: row.source_language,
            target_language: row.target_language,
            source_audio_url: row.source_audio_url,
            translated_audio_url: row.translated_audio_url,
            status: row.status,
            processing_error: row.processing_error,
            created_at: row.created_at,
            latency: LatencyReport {
                audio_ms: to_u64(row.audio_ms),
                vad_ms: to_u64(row.vad_ms),
                stt_ms: to_u64(row.stt_ms),
                translation_ms: to_u64(row.translation_ms),
                tts_ms: to_u64(row.tts_ms),
                total_ms: to_u64(row.total_ms),
                t0_unix_ms: to_u64(row.t0_unix_ms),
                t1_unix_ms: to_u64(row.t1_unix_ms),
                t2_unix_ms: to_u64(row.t2_unix_ms),
                t3_unix_ms: to_u64(row.t3_unix_ms),
                t4_unix_ms: to_u64(row.t4_unix_ms),
            },
            speakers: Vec::new(),
            refinements: Vec::new(),
        }
    }
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn to_u64(value: i64) -> u64 {
    value.max(0) as u64
}
