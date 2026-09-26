// Copyright (c) 2026 Windsor Nguyen

//! Expose the same native CLI through Git's external-command lookup.

fn main() -> std::io::Result<std::process::ExitCode> {
    cowtree_cli::run()
}
