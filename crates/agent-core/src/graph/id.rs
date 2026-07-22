use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

const MAX_ID_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphIdError {
    #[error("{kind} 不能为空")]
    Empty { kind: &'static str },
    #[error("{kind} 长度不能超过 {max} 字节，实际为 {actual} 字节")]
    TooLong {
        kind: &'static str,
        max: usize,
        actual: usize,
    },
    #[error("{kind} 在字节位置 {index} 包含非法字符 `{character}`")]
    InvalidCharacter {
        kind: &'static str,
        index: usize,
        character: char,
    },
}

fn validate_id(value: &str, kind: &'static str) -> Result<(), GraphIdError> {
    if value.is_empty() {
        return Err(GraphIdError::Empty { kind });
    }
    if value.len() > MAX_ID_BYTES {
        return Err(GraphIdError::TooLong {
            kind,
            max: MAX_ID_BYTES,
            actual: value.len(),
        });
    }

    if let Some((index, character)) = value.char_indices().find(|(_, character)| {
        !character.is_ascii_alphanumeric() && !matches!(character, '.' | '_' | '-')
    }) {
        return Err(GraphIdError::InvalidCharacter {
            kind,
            index,
            character,
        });
    }

    Ok(())
}

macro_rules! validated_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = GraphIdError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                validate_id(value, $kind)?;
                Ok(Self(value.to_owned()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = GraphIdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                validate_id(&value, $kind)?;
                Ok(Self(value))
            }
        }

        impl FromStr for $name {
            type Err = GraphIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_from(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

validated_id!(GraphId, "GraphId");
validated_id!(NodeId, "NodeId");
validated_id!(RouteKey, "RouteKey");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for RunId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}
