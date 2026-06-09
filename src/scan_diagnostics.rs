use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub stage: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SkippedFile {
    pub fn new(
        path: impl Into<String>,
        stage: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            stage: stage.into(),
            reason: reason.into(),
            message: None,
        }
    }

    pub fn with_message(
        path: impl Into<String>,
        stage: impl Into<String>,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            stage: stage.into(),
            reason: reason.into(),
            message: Some(message.into()),
        }
    }
}
