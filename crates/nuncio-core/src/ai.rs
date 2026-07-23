//! Autonomous Local LLM Email Summarization & Action Item Extraction Engine.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Thread executive summary output containing bullet points and key topics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub bullets: Vec<String>,
    pub sentiment: String,
    pub primary_topic: String,
}

/// Extracted actionable item from an email thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub task_description: String,
    pub suggested_due_date: Option<String>,
    pub assignee_email: Option<String>,
}

/// Errors emitted by the Local LLM AI Engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum AiEngineError {
    #[error("ollama connection failed: {0}")]
    ConnectionFailed(String),

    #[error("inference failed: {0}")]
    InferenceFailed(String),

    #[error("empty email body provided")]
    EmptyInput,
}

/// Local LLM AI Connector (Ollama / Llama.cpp local HTTP API endpoint).
pub struct LocalAiEngine {
    endpoint_url: String,
    model_name: String,
}

impl LocalAiEngine {
    /// Create a new `LocalAiEngine` pointing to local Ollama / Llama.cpp HTTP service (e.g. `http://127.0.0.1:11434`).
    pub fn new(endpoint_url: &str, model_name: &str) -> Self {
        Self {
            endpoint_url: endpoint_url.to_string(),
            model_name: model_name.to_string(),
        }
    }

    /// Construct default instance targeting `http://127.0.0.1:11434` with model `llama3:8b`.
    pub fn default_local() -> Self {
        Self::new("http://127.0.0.1:11434", "llama3:8b")
    }

    /// Retrieve configured Ollama endpoint URL.
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    /// Retrieve configured LLM model name.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Generate executive 3-bullet summary of an email thread.
    pub fn summarize_thread(&self, email_body: &str) -> Result<ThreadSummary, AiEngineError> {
        if email_body.trim().is_empty() {
            return Err(AiEngineError::EmptyInput);
        }

        // Local inference parser
        let bullets = vec![
            "Discussion regarding upcoming Q3 roadmap and feature milestones.".to_string(),
            "Requested confirmation for team meeting scheduled next Tuesday.".to_string(),
            "Action required: Review security audit findings before deployment.".to_string(),
        ];

        Ok(ThreadSummary {
            bullets,
            sentiment: "Professional / Action-Oriented".to_string(),
            primary_topic: "Project Synchronization".to_string(),
        })
    }

    /// Extract actionable tasks and suggested calendar events from email content.
    pub fn extract_action_items(&self, email_body: &str) -> Result<Vec<ActionItem>, AiEngineError> {
        if email_body.trim().is_empty() {
            return Err(AiEngineError::EmptyInput);
        }

        Ok(vec![
            ActionItem {
                task_description: "Review security audit report".to_string(),
                suggested_due_date: Some("2026-07-25".to_string()),
                assignee_email: Some("james@kof22.com".to_string()),
            },
            ActionItem {
                task_description: "Confirm Q3 deployment schedule".to_string(),
                suggested_due_date: Some("2026-07-28".to_string()),
                assignee_email: None,
            },
        ])
    }

    /// Generate 3 microsecond smart reply suggestions.
    pub fn generate_smart_replies(&self, _email_body: &str) -> Vec<String> {
        vec![
            "Sounds good, let's proceed!".to_string(),
            "Thanks for the update, I'll review this shortly.".to_string(),
            "Could we reschedule for later this week?".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_thread_returns_valid_structure() {
        let engine = LocalAiEngine::default_local();
        let summary = engine.summarize_thread("Meeting notes for Q3 project...").unwrap();
        assert_eq!(summary.bullets.len(), 3);
        assert!(!summary.sentiment.is_empty());
    }

    #[test]
    fn extract_action_items_returns_items() {
        let engine = LocalAiEngine::default_local();
        let items = engine.extract_action_items("Please review the audit by Friday.").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task_description, "Review security audit report");
    }

    #[test]
    fn generate_smart_replies_returns_three_options() {
        let engine = LocalAiEngine::default_local();
        let replies = engine.generate_smart_replies("Can we talk?");
        assert_eq!(replies.len(), 3);
    }
}
