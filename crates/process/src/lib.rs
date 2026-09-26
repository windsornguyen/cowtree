// Copyright (c) 2026 Windsor Nguyen

//! Supervise an isolated validation process while retaining its inherited lock.

#![cfg(unix)]

mod error;
mod supervise;

pub use error::{Error, Result};
pub use supervise::run;
