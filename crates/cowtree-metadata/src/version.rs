// Copyright (c) 2026 Windsor Nguyen

//! Read-only build identity, independent of any metadata store.

use std::io::{self, Write};

use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Version {
    /// Package release compiled into this executable.
    version: &'static str,
    /// Optional source identity supplied by the build, never inferred at runtime.
    revision: Option<&'static str>,
}

impl Version {
    #[must_use]
    pub(crate) fn current() -> Self {
        Self { version: env!("CARGO_PKG_VERSION"), revision: option_env!("COWTREE_BUILD_REVISION") }
    }

    /// Write one JSON record, preserving serialization and output errors.
    pub(crate) fn write(&self, mut output: impl Write) -> io::Result<()> {
        serde_json::to_writer(&mut output, self).map_err(io::Error::other)?;
        output.write_all(b"\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_revision_is_explicit() -> io::Result<()> {
        let version = Version { version: "1.2.3", revision: None };
        let mut output = Vec::new();
        version.write(&mut output)?;
        assert_eq!(output, b"{\"version\":\"1.2.3\",\"revision\":null}\n");
        Ok(())
    }

    #[test]
    fn output_errors_are_not_success() {
        let result = Version::current().write(&mut [0_u8; 0][..]);
        assert!(result.is_err());
    }
}
