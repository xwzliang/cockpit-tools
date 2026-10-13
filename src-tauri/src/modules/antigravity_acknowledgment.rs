//! Reuse the user's existing desktop acknowledgment across account switches.
use serde_json::{Map, Value};
use std::path::Path;

const EMAIL: &str = "jetski.onboarding.lastLoginUsername";
const TERMS: &str = "jetski.onboarding.lastLoginIsGcpTos";

fn reuse_acknowledgment(storage: &mut Map<String, Value>, email: &str) -> bool {
    // Preserve the terms type and every privacy preference. Only reuse an actual
    // acknowledgment; a fresh profile must still show its initial notice.
    let accepted = matches!(storage.get(EMAIL), Some(Value::String(previous)) if !previous.is_empty())
        && matches!(storage.get(TERMS), Some(Value::String(terms)) if matches!(terms.as_str(), "true" | "false"));
    if !accepted || email.is_empty() { return false; }
    storage.insert(EMAIL.into(), Value::String(email.into()));
    true
}

/// Call after Antigravity exits, before replacing the account and relaunching.
pub fn prepare(profile: &Path, email: &str) -> Result<(), String> {
    let path = profile.join("app_storage.json");
    if !path.exists() { return Ok(()); }
    let mut storage: Map<String, Value> = serde_json::from_str(
        &std::fs::read_to_string(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if reuse_acknowledgment(&mut storage, email) {
        super::atomic_write::write_string_atomic(&path,
            &serde_json::to_string_pretty(&storage).map_err(|e| e.to_string())?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgment_reused_for_new_accounts_preserves_terms_and_privacy() {
        let mut storage = Map::new();
        storage.insert(EMAIL.into(), Value::String("a@example.test".into()));
        storage.insert(TERMS.into(), Value::String("false".into()));
        storage.insert("telemetryEnabled".into(), Value::Bool(false));
        storage.insert("unrelated".into(), serde_json::json!({"layout": [1, 2]}));
        let original = storage.clone();
        assert!(reuse_acknowledgment(&mut storage, "new@example.test"));
        assert_eq!(storage[EMAIL], "new@example.test");
        for (key, value) in original {
            if key != EMAIL { assert_eq!(storage[&key], value); }
        }
        assert!(reuse_acknowledgment(&mut storage, "another@example.test"));
        assert_eq!(storage[TERMS], "false");
    }
    #[test]
    fn acknowledgment_does_not_invent_initial_acceptance() {
        let mut storage = Map::new();
        assert!(!reuse_acknowledgment(&mut storage, "a@example.test"));
        storage.insert(EMAIL.into(), Value::String("a@example.test".into()));
        assert!(!reuse_acknowledgment(&mut storage, "b@example.test"));
        storage.insert(TERMS.into(), Value::String("invalid".into()));
        assert!(!reuse_acknowledgment(&mut storage, "b@example.test"));
        assert_eq!(storage[EMAIL], "a@example.test");
    }
}
