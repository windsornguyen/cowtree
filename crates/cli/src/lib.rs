// Copyright (c) 2026 Windsor Nguyen

//! Native command-line adapters for the shared Cowtree library.

mod arguments;
mod dispatch;
mod output;
mod version;
#[cfg(unix)]
mod workspace;
#[cfg(unix)]
mod workspace_arguments;

pub use dispatch::run;
