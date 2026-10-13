use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshServerStore {
    pub version: String,
    #[serde(default)]
    pub selected_antigravity_server_id: Option<String>,
    pub selected_server_id: Option<String>,
    pub servers: Vec<SshServer>,
}

impl Default for SshServerStore {
    fn default() -> Self {
        Self {
            version: "1".to_string(),
            selected_server_id: None,
            selected_antigravity_server_id: None,
            servers: Vec::new(),
        }
    }
}

fn default_antigravity_home() -> String {
    "~/.gemini".to_string()
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshServer {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub codex_home: String,
    #[serde(default = "default_antigravity_home")]
    pub antigravity_home: String,
    pub auth: SshAuthConfig,
    pub sync_on_codex_switch: bool,
    #[serde(default = "default_false")]
    pub sync_on_antigravity_switch: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_sync: Option<SshCodexSyncStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_antigravity_sync: Option<SshAntigravitySyncStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshAuthConfig {
    Agent,
    SshConfig,
    PrivateKeyFile { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshCodexSyncStatus {
    pub account_id: String,
    pub account_email: String,
    pub token_generation: u64,
    pub bundle_hash: String,
    pub synced_at: i64,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshCodexSyncResult {
    pub server_id: String,
    pub server_name: String,
    pub account_id: String,
    pub account_email: String,
    pub token_generation: u64,
    pub bundle_hash: String,
    pub verified: bool,
    pub error: Option<String>,
    pub synced_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAntigravitySyncStatus {
    pub account_id: String,
    pub account_email: String,
    pub bundle_hash: String,
    pub synced_at: i64,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAntigravitySyncResult {
    pub server_id: String,
    pub server_name: String,
    pub account_id: String,
    pub account_email: String,
    pub bundle_hash: String,
    pub verified: bool,
    pub error: Option<String>,
    pub synced_at: i64,
}
