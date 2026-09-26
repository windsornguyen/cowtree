// Copyright (c) 2026 Windsor Nguyen

//! Native CLI provenance supplied by the build rather than inferred from the caller.

use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Version {
    /// Package release compiled into this executable.
    pub version: &'static str,
    /// Optional immutable source revision provided by the build.
    pub revision: Option<&'static str>,
}

impl Version {
    pub fn current() -> Self {
        Self { version: env!("CARGO_PKG_VERSION"), revision: option_env!("COWTREE_BUILD_REVISION") }
    }
}
