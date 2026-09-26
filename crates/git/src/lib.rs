// Copyright (c) 2026 Windsor Nguyen

//! Synchronous Git execution shared by both workflows.

mod client;
mod error;
mod process;

pub use client::{Client, decode_path};
pub use error::{Error, Result};
pub use process::checked;
