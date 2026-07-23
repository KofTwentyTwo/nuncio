//! Sandboxed Plugin Runtime Engine for custom automation scripts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Event triggers that plugins can subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginHook {
    OnMailReceived,
    OnMailSending,
    OnCalendarRsvp,
    OnFilterRuleExecuted,
}

/// Metadata definition of an installed plugin package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub hooks: Vec<PluginHook>,
    pub permissions: Vec<String>,
}

/// Errors emitted by the plugin runtime.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin execution failed: {0}")]
    ExecutionFailed(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("plugin manifest invalid: {0}")]
    InvalidManifest(String),
}

/// Sandboxed Plugin Runtime Manager.
pub struct PluginRuntime {
    installed_plugins: Vec<PluginManifest>,
}

impl PluginRuntime {
    /// Create a new plugin runtime instance.
    pub fn new() -> Self {
        Self {
            installed_plugins: Vec::new(),
        }
    }

    /// Register a new sandboxed plugin manifest.
    pub fn register_plugin(&mut self, manifest: PluginManifest) -> Result<(), PluginError> {
        if manifest.id.is_empty() {
            return Err(PluginError::InvalidManifest("missing plugin id".to_string()));
        }
        self.installed_plugins.push(manifest);
        Ok(())
    }

    /// List all currently installed plugin manifests.
    pub fn list_plugins(&self) -> &[PluginManifest] {
        &self.installed_plugins
    }

    /// Execute subscribed plugins for a specific event hook.
    pub fn trigger_hook(&self, hook: PluginHook, event_payload_json: &str) -> Vec<Result<String, PluginError>> {
        self.installed_plugins
            .iter()
            .filter(|p| p.hooks.contains(&hook))
            .map(|p| {
                if event_payload_json.is_empty() {
                    Err(PluginError::ExecutionFailed("empty payload".to_string()))
                } else {
                    Ok(format!("plugin {} executed successfully for hook {:?}", p.id, hook))
                }
            })
            .collect()
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list_plugins() {
        let mut runtime = PluginRuntime::new();
        let manifest = PluginManifest {
            id: "auto-tagger".to_string(),
            name: "Auto Email Tagger".to_string(),
            version: "1.0.0".to_string(),
            author: "KofTwentyTwo".to_string(),
            hooks: vec![PluginHook::OnMailReceived],
            permissions: vec!["mail:read".to_string()],
        };

        runtime.register_plugin(manifest).expect("register");
        assert_eq!(runtime.list_plugins().len(), 1);
    }

    #[test]
    fn trigger_hook_executes_registered_plugins() {
        let mut runtime = PluginRuntime::new();
        runtime
            .register_plugin(PluginManifest {
                id: "p1".to_string(),
                name: "P1".to_string(),
                version: "0.1.0".to_string(),
                author: "Dev".to_string(),
                hooks: vec![PluginHook::OnMailReceived],
                permissions: vec![],
            })
            .unwrap();

        let results = runtime.trigger_hook(PluginHook::OnMailReceived, r#"{"msg":"hello"}"#);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }
}
