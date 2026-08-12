use anyhow::Result;
use uuid::Uuid;

use crate::{backends::TranslationTerm, storage::Database};

#[derive(Clone, Debug, Default)]
pub struct LanguagePolicy {
    terms: Vec<PolicyTerm>,
    blocked: Vec<PolicyBlock>,
}

#[derive(Clone, Debug)]
struct PolicyTerm {
    source: String,
    variants: Vec<String>,
    target: String,
    priority: i32,
}

#[derive(Clone, Debug)]
struct PolicyBlock {
    word: String,
    replacement: String,
    whole_word: bool,
    case_sensitive: bool,
}

impl LanguagePolicy {
    pub async fn load(database: Option<&Database>, room_id: Uuid) -> Result<Self> {
        let Some(database) = database else {
            return Ok(Self::default());
        };
        let binding = database.room_terminology_binding(room_id).await?;
        let entries = match binding.dictionary_id {
            Some(id) => database.list_terminology_entries(id, false).await?,
            None => Vec::new(),
        };
        let mut terms = entries
            .into_iter()
            .map(|entry| {
                let mut variants = entry.aliases;
                variants.push(entry.source_term.clone());
                variants.sort_by_key(|value| std::cmp::Reverse(value.chars().count()));
                variants.dedup();
                PolicyTerm {
                    source: entry.source_term,
                    variants,
                    target: entry.target_term,
                    priority: entry.priority,
                }
            })
            .collect::<Vec<_>>();
        terms.sort_by_key(|term| {
            (
                std::cmp::Reverse(term.priority),
                std::cmp::Reverse(term.source.chars().count()),
            )
        });
        let mut blocked = database
            .list_blocked_words(false)
            .await?
            .into_iter()
            .map(|entry| PolicyBlock {
                word: entry.word,
                replacement: entry.replacement,
                whole_word: entry.match_mode == "word",
                case_sensitive: entry.case_sensitive,
            })
            .collect::<Vec<_>>();
        blocked.sort_by_key(|entry| std::cmp::Reverse(entry.word.chars().count()));
        Ok(Self { terms, blocked })
    }

    pub fn normalize_transcript(&self, text: &str) -> String {
        self.terms.iter().fold(text.to_owned(), |mut output, term| {
            for variant in &term.variants {
                output = replace_matches(
                    &output,
                    variant,
                    &term.source,
                    false,
                    is_ascii_word(variant),
                );
            }
            output
        })
    }

    pub fn sanitize(&self, text: &str) -> String {
        self.blocked
            .iter()
            .fold(text.to_owned(), |output, blocked| {
                replace_matches(
                    &output,
                    &blocked.word,
                    &blocked.replacement,
                    blocked.case_sensitive,
                    blocked.whole_word,
                )
            })
    }

    pub fn normalize_translation(&self, text: &str) -> String {
        let output = self.terms.iter().fold(text.to_owned(), |mut output, term| {
            for variant in term.variants.iter().chain(std::iter::once(&term.source)) {
                output = replace_matches(
                    &output,
                    variant,
                    &term.target,
                    false,
                    is_ascii_word(variant),
                );
            }
            output
        });
        self.sanitize(&output)
    }

    pub fn translation_terms(&self) -> Vec<TranslationTerm> {
        self.terms
            .iter()
            .map(|term| TranslationTerm {
                source: term.source.clone(),
                target: term.target.clone(),
                aliases: term.variants.clone(),
            })
            .collect()
    }
}

fn is_ascii_word(value: &str) -> bool {
    value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

fn replace_matches(
    text: &str,
    needle: &str,
    replacement: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let haystack = if case_sensitive {
        text.to_owned()
    } else {
        text.to_ascii_lowercase()
    };
    let needle_cmp = if case_sensitive {
        needle.to_owned()
    } else {
        needle.to_ascii_lowercase()
    };
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative) = haystack[cursor..].find(&needle_cmp) {
        let start = cursor + relative;
        let end = start + needle_cmp.len();
        let boundary = !whole_word
            || (is_boundary(text[..start].chars().next_back())
                && is_boundary(text[end..].chars().next()));
        if boundary {
            output.push_str(&text[cursor..start]);
            output.push_str(replacement);
            cursor = end;
        } else {
            let step = text[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            output.push_str(&text[cursor..start + step]);
            cursor = start + step;
        }
    }
    output.push_str(&text[cursor..]);
    output
}

fn is_boundary(value: Option<char>) -> bool {
    value.is_none_or(|character| !character.is_alphanumeric() && character != '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applies_terms_and_blocks_with_boundaries() {
        let policy = LanguagePolicy {
            terms: vec![PolicyTerm {
                source: "Kubernetes".into(),
                variants: vec!["k8s".into()],
                target: "Kubernetes".into(),
                priority: 100,
            }],
            blocked: vec![PolicyBlock {
                word: "secret".into(),
                replacement: "***".into(),
                whole_word: true,
                case_sensitive: false,
            }],
        };
        assert_eq!(policy.normalize_transcript("use k8s"), "use Kubernetes");
        assert_eq!(policy.sanitize("Secret secretive"), "*** secretive");
    }
}
