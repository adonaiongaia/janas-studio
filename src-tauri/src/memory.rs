//! The autonomous memory bank.
//!
//! The chat interface and the `save_to_memory_bank` tool both write here. Facts
//! live in a small JSON file under the app's data directory, so they survive
//! restarts without a database or a sidecar process.

use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::RwLock,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    util::{now_ms, truncate_line},
};

const MAX_FACTS: usize = 1_000;
const MAX_FACT_CHARS: usize = 4_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    pub text: String,
    pub created_at: u64,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct MemorySaveOutcome {
    pub fact: MemoryFact,
    /// False when an identical fact already existed.
    pub created: bool,
}

pub struct MemoryStore {
    path: PathBuf,
    facts: RwLock<Vec<MemoryFact>>,
}

impl MemoryStore {
    pub fn load(path: PathBuf) -> Self {
        let facts = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<MemoryFact>>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            facts: RwLock::new(sanitize_facts(facts)),
        }
    }

    pub fn list(&self) -> Vec<MemoryFact> {
        self.facts
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn save(&self, text: &str, source: &str) -> AppResult<MemorySaveOutcome> {
        let text = clean_fact(text);
        if text.is_empty() {
            return Err(AppError::message("A memory needs some text."));
        }
        if text.chars().count() > MAX_FACT_CHARS {
            return Err(AppError::message(format!(
                "That memory is too long ({} characters). Keep one fact under {MAX_FACT_CHARS} characters.",
                text.chars().count()
            )));
        }

        let mut guard = self
            .facts
            .write()
            .map_err(|_| AppError::message("The memory bank is locked."))?;

        if let Some(existing) = guard
            .iter()
            .find(|fact| fact.text.eq_ignore_ascii_case(&text))
        {
            return Ok(MemorySaveOutcome {
                fact: existing.clone(),
                created: false,
            });
        }
        if guard.len() >= MAX_FACTS {
            return Err(AppError::message(format!(
                "The memory bank is full ({MAX_FACTS} facts). Delete a stale fact first."
            )));
        }

        let fact = MemoryFact {
            id: Uuid::new_v4().to_string(),
            text,
            created_at: now_ms(),
            source: source.trim().to_string(),
        };
        let mut next = guard.clone();
        next.insert(0, fact.clone());
        self.persist(&next)?;
        *guard = next;

        Ok(MemorySaveOutcome {
            fact,
            created: true,
        })
    }

    pub fn update(&self, id: &str, text: &str) -> AppResult<MemoryFact> {
        let text = clean_fact(text);
        if text.is_empty() {
            return Err(AppError::message("A memory needs some text."));
        }
        if text.chars().count() > MAX_FACT_CHARS {
            return Err(AppError::message(format!(
                "That memory is too long. Keep one fact under {MAX_FACT_CHARS} characters."
            )));
        }

        let mut guard = self
            .facts
            .write()
            .map_err(|_| AppError::message("The memory bank is locked."))?;
        let Some(index) = guard.iter().position(|fact| fact.id == id) else {
            return Err(AppError::message("That memory no longer exists."));
        };
        if guard
            .iter()
            .enumerate()
            .any(|(other, fact)| other != index && fact.text.eq_ignore_ascii_case(&text))
        {
            return Err(AppError::message("Another memory already says that."));
        }

        let mut next = guard.clone();
        next[index].text = text;
        self.persist(&next)?;
        let updated = next[index].clone();
        *guard = next;
        Ok(updated)
    }

    pub fn delete(&self, id: &str) -> AppResult<bool> {
        let mut guard = self
            .facts
            .write()
            .map_err(|_| AppError::message("The memory bank is locked."))?;
        let next: Vec<MemoryFact> = guard.iter().filter(|fact| fact.id != id).cloned().collect();
        if next.len() == guard.len() {
            return Ok(false);
        }
        self.persist(&next)?;
        *guard = next;
        Ok(true)
    }

    pub fn clear(&self) -> AppResult<()> {
        let mut guard = self
            .facts
            .write()
            .map_err(|_| AppError::message("The memory bank is locked."))?;
        self.persist(&[])?;
        guard.clear();
        Ok(())
    }

    fn persist(&self, facts: &[MemoryFact]) -> AppResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(facts)
            .map_err(|err| AppError::message(format!("Could not encode memory: {err}")))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn clean_fact(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize_facts(facts: Vec<MemoryFact>) -> Vec<MemoryFact> {
    let mut seen = HashSet::new();
    facts
        .into_iter()
        .filter_map(|fact| {
            let text = clean_fact(&fact.text);
            if text.is_empty() || text.chars().count() > MAX_FACT_CHARS {
                return None;
            }
            let key = text.to_lowercase();
            if !seen.insert(key) {
                return None;
            }
            Some(MemoryFact {
                id: if fact.id.trim().is_empty() {
                    Uuid::new_v4().to_string()
                } else {
                    fact.id
                },
                text,
                created_at: fact.created_at,
                source: fact.source,
            })
        })
        .take(MAX_FACTS)
        .collect()
}

/// Build the tool result the model reads after a call. Keeping errors inside a
/// normal tool message lets the model correct itself instead of killing the
/// whole stream.
pub fn tool_result_json(memory: &MemoryStore, fact: &str) -> serde_json::Value {
    match memory.save(fact, "model") {
        Ok(outcome) => serde_json::json!({
            "ok": true,
            "saved": outcome.created,
            "duplicate": !outcome.created,
            "fact": truncate_line(&outcome.fact.text, 500),
        }),
        Err(err) => serde_json::json!({
            "ok": false,
            "error": err.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (MemoryStore, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "janas-memory-test-{}.json",
            Uuid::new_v4()
        ));
        let _ = fs::remove_file(&path);
        (MemoryStore::load(path.clone()), path)
    }

    #[test]
    fn saves_deduplicates_and_reloads() {
        let (store, path) = temp_store();
        let first = store.save("The project uses Tauri 2.", "user").unwrap();
        assert!(first.created);
        assert_eq!(store.list().len(), 1);

        let duplicate = store
            .save("the project uses tauri 2.", "model")
            .unwrap();
        assert!(!duplicate.created);
        assert_eq!(store.list().len(), 1);

        let reloaded = MemoryStore::load(path.clone());
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].text, "The project uses Tauri 2.");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn updates_and_deletes_facts() {
        let (store, path) = temp_store();
        let fact = store.save("old fact", "user").unwrap().fact;
        let updated = store.update(&fact.id, "new fact").unwrap();
        assert_eq!(updated.text, "new fact");
        assert!(store.delete(&fact.id).unwrap());
        assert!(store.list().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_empty_facts_and_bad_ids() {
        let (store, path) = temp_store();
        assert!(store.save("   ", "user").is_err());
        assert!(store.update("missing", "text").is_err());
        assert!(!store.delete("missing").unwrap());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tool_result_never_panics_on_errors() {
        let (store, path) = temp_store();
        let result = tool_result_json(&store, "User prefers concise answers.");
        assert_eq!(result["ok"], true);
        let empty = tool_result_json(&store, "   ");
        assert_eq!(empty["ok"], false);
        let _ = fs::remove_file(path);
    }
}
