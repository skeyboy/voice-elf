use crate::protocol::SessionConfig;

pub(super) fn normalize_config(
    mut config: SessionConfig,
) -> std::result::Result<SessionConfig, String> {
    config.source_language = config.source_language.to_ascii_lowercase();
    config.target_language = config.target_language.to_ascii_lowercase();
    config.voice = config.voice.to_ascii_lowercase();
    const LANGUAGES: &[&str] = &[
        "auto", "zh", "en", "ja", "ko", "fr", "de", "es", "it", "pt", "ru",
    ];
    const VOICES: &[&str] = &[
        "f1", "m1", "serena", "vivian", "unclefu", "ryan", "aiden", "onoanna", "sohee", "eric",
        "dylan",
    ];
    if !LANGUAGES.contains(&config.source_language.as_str()) {
        return Err(format!(
            "Unsupported source language: {}",
            config.source_language
        ));
    }
    if config.target_language == "auto" || !LANGUAGES.contains(&config.target_language.as_str()) {
        return Err(format!(
            "Unsupported target language: {}",
            config.target_language
        ));
    }
    if !VOICES.contains(&config.voice.as_str()) {
        return Err(format!("Unsupported voice: {}", config.voice));
    }
    if !(5..=20).contains(&config.max_utterance_seconds) {
        return Err(format!(
            "Maximum utterance duration must be between 5 and 20 seconds: {}",
            config.max_utterance_seconds
        ));
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_session_config() {
        let valid = normalize_config(SessionConfig {
            source_language: "EN".to_owned(),
            target_language: "ZH".to_owned(),
            voice: "Ryan".to_owned(),
            max_utterance_seconds: 20,
        })
        .unwrap();
        assert_eq!(valid.source_language, "en");
        assert_eq!(valid.voice, "ryan");
        assert!(
            normalize_config(SessionConfig {
                voice: "M1".to_owned(),
                ..SessionConfig::default()
            })
            .is_ok()
        );
        assert!(
            normalize_config(SessionConfig {
                target_language: "auto".to_owned(),
                ..SessionConfig::default()
            })
            .is_err()
        );
    }
}
