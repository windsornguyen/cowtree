// Copyright (c) 2026 Windsor Nguyen

//! Git control names remain outside the published resource namespace.

use cowtree_metadata::ResourcePath;

#[test]
fn git_control_components_are_reserved_in_every_ascii_spelling() {
    for path in [".git", ".GIT", ".Git/config", "src/.gIt/HEAD"] {
        assert!(ResourcePath::parse(path).is_err(), "accepted reserved resource {path}");
    }
    assert!(ResourcePath::parse(".gitignore").is_ok());
    assert!(ResourcePath::parse("src/git.rs").is_ok());
}
