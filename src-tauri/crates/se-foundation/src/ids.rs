//! Canonical session identity shared by the terminal runtime and session stores.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Se-owned Conversation UUID, allocated before any ACP `session/new` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConversationId(Uuid);

impl ConversationId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse any UUID spelling accepted by `uuid`; display and serde always emit its canonical form.
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(value).map(Self)
    }

    /// Parse a canonical path component, rejecting aliases such as uppercase or simple UUID forms.
    pub fn parse_path_component(value: &str) -> Result<Self, ConversationIdPathError> {
        let parsed = Self::parse(value).map_err(ConversationIdPathError::InvalidUuid)?;
        let canonical = parsed.to_string();
        if value == canonical {
            Ok(parsed)
        } else {
            Err(ConversationIdPathError::NonCanonical { canonical })
        }
    }

    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let canonical = self.0.hyphenated().to_string().to_ascii_lowercase();
        formatter.write_str(&canonical)
    }
}

impl Serialize for ConversationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Path-component validation error with a migration-safe canonical replacement.
#[derive(Debug)]
pub enum ConversationIdPathError {
    InvalidUuid(uuid::Error),
    NonCanonical { canonical: String },
}

impl ConversationIdPathError {
    #[must_use]
    pub fn canonical_replacement(&self) -> Option<&str> {
        match self {
            Self::InvalidUuid(_) => None,
            Self::NonCanonical { canonical } => Some(canonical),
        }
    }
}

impl fmt::Display for ConversationIdPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUuid(error) => write!(formatter, "invalid ConversationId: {error}"),
            Self::NonCanonical { canonical } => write!(
                formatter,
                "non-canonical ConversationId path component; use {canonical}"
            ),
        }
    }
}

impl std::error::Error for ConversationIdPathError {}
