// Copyright (c) 2026 Windsor Nguyen

//! Validated identities keep Git revisions distinct from owned checkpoint names.

use cowtree_metadata::objects::ObjectId;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(ObjectId);

impl NodeId {
    pub fn parse(value: &str) -> Result<Self> {
        Ok(Self(ObjectId::parse(value)?))
    }

    /// Allocate an opaque checkpoint name. Exclusive directory creation owns collisions.
    #[must_use]
    pub fn generate() -> Self {
        Self(ObjectId::from_bytes(uuid::Uuid::new_v4().as_bytes()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitId(String);

impl CommitId {
    pub fn parse(value: String) -> Result<Self> {
        if !matches!(value.len(), 40 | 64)
            || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::InvalidCommit { value });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CommitId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(value)
    }
}

impl From<CommitId> for String {
    fn from(value: CommitId) -> Self {
        value.0
    }
}
