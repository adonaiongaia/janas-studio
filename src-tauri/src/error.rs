use serde::{Serialize, Serializer};

/// Errors returned by Tauri commands. They are serialized as a single readable
/// string so the frontend can surface them directly in its error banners.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),

    #[error("Could not find the engine binary \"{name}\". Install it from Settings or set its full path.")]
    BinaryNotFound { name: String },

    #[error("The inference engine is not running. Start it before sending a message.")]
    EngineNotReady,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn message(value: impl Into<String>) -> Self {
        Self::Message(value.into())
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
