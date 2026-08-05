use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

#[derive(Clone)]
pub struct MediaStore {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct MediaFile {
    pub path: String,
    pub url: String,
}

impl MediaStore {
    pub async fn new(root: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&root)
            .await
            .with_context(|| format!("failed to create media directory: {}", root.display()))?;
        let root = tokio::fs::canonicalize(&root)
            .await
            .with_context(|| format!("failed to resolve media directory: {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn save_source(
        &self,
        session_id: Uuid,
        utterance_id: Uuid,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<MediaFile> {
        self.save_audio(session_id, utterance_id, "source", samples, sample_rate, 1)
            .await
    }

    pub async fn save_translated(
        &self,
        session_id: Uuid,
        utterance_id: Uuid,
        samples: &[i16],
        sample_rate: u32,
        channels: u16,
    ) -> Result<MediaFile> {
        self.save_audio(
            session_id,
            utterance_id,
            "translated",
            samples,
            sample_rate,
            channels,
        )
        .await
    }

    pub async fn save_voice_reference(
        &self,
        user_id: Uuid,
        reference_id: Uuid,
        wav: &[u8],
    ) -> Result<String> {
        let directory = self.root.join("voice-references").join(user_id.to_string());
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| {
                format!(
                    "failed to create voice reference directory: {}",
                    directory.display()
                )
            })?;
        let path = directory.join(format!("{reference_id}.wav"));
        let part_path = path.with_extension("wav.part");
        tokio::fs::write(&part_path, wav)
            .await
            .with_context(|| format!("failed to write voice reference: {}", part_path.display()))?;
        tokio::fs::rename(&part_path, &path)
            .await
            .with_context(|| format!("failed to publish voice reference: {}", path.display()))?;
        Ok(path.to_string_lossy().into_owned())
    }

    pub async fn delete_voice_reference(&self, path: &str) -> Result<()> {
        let path = PathBuf::from(path);
        let reference_root = self.root.join("voice-references");
        if !path.starts_with(&reference_root) {
            anyhow::bail!("refusing to delete a voice reference outside the media store");
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error)
                .with_context(|| format!("failed to delete voice reference: {}", path.display())),
        }
    }

    async fn save_audio(
        &self,
        session_id: Uuid,
        utterance_id: Uuid,
        kind: &str,
        samples: &[i16],
        sample_rate: u32,
        channels: u16,
    ) -> Result<MediaFile> {
        let session_dir = self.root.join(session_id.to_string());
        tokio::fs::create_dir_all(&session_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create session media directory: {}",
                    session_dir.display()
                )
            })?;
        let name = format!("{utterance_id}-{kind}.wav");
        let path = session_dir.join(&name);
        let write_path = path.clone();
        let samples = samples.to_vec();
        tokio::task::spawn_blocking(move || {
            write_pcm16_wav(&write_path, &samples, sample_rate, channels)
        })
        .await
        .context("media writer task failed")??;
        Ok(MediaFile {
            path: path.to_string_lossy().into_owned(),
            url: format!("/media/{session_id}/{name}"),
        })
    }
}

fn write_pcm16_wav(path: &Path, samples: &[i16], sample_rate: u32, channels: u16) -> Result<()> {
    let part_path = path.with_extension("wav.part");
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&part_path, spec)
        .with_context(|| format!("failed to create WAV file: {}", part_path.display()))?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    std::fs::rename(&part_path, path)
        .with_context(|| format!("failed to publish WAV file: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saves_source_and_translated_wav_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let session_id = Uuid::new_v4();
        let utterance_id = Uuid::new_v4();

        let source_file = store
            .save_source(session_id, utterance_id, &[0, 1, -1], 16_000)
            .await
            .unwrap();
        let translated_file = store
            .save_translated(session_id, utterance_id, &[3, -3, 4, -4], 24_000, 2)
            .await
            .unwrap();

        let source = hound::WavReader::open(source_file.path).unwrap();
        let translated = hound::WavReader::open(translated_file.path).unwrap();
        assert_eq!(source.spec().sample_rate, 16_000);
        assert_eq!(source.duration(), 3);
        assert_eq!(translated.spec().sample_rate, 24_000);
        assert_eq!(translated.spec().channels, 2);
        assert_eq!(translated.duration(), 2);
        assert!(source_file.url.ends_with("-source.wav"));
        assert!(translated_file.url.ends_with("-translated.wav"));
    }

    #[tokio::test]
    async fn saves_and_deletes_private_voice_references() {
        let directory = tempfile::tempdir().unwrap();
        let store = MediaStore::new(directory.path().join("media"))
            .await
            .unwrap();
        let user_id = Uuid::new_v4();
        let reference_id = Uuid::new_v4();
        let path = store
            .save_voice_reference(user_id, reference_id, b"RIFF-test")
            .await
            .unwrap();
        assert!(tokio::fs::try_exists(&path).await.unwrap());
        store.delete_voice_reference(&path).await.unwrap();
        assert!(!tokio::fs::try_exists(&path).await.unwrap());
    }
}
