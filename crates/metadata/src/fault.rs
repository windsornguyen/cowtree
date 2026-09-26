// Copyright (c) 2026 Windsor Nguyen

//! Explicit process-crash boundaries available only to the test build.

pub(crate) fn checkpoint(point: &str) {
    #[cfg(feature = "fault-injection")]
    pause(point);
    #[cfg(feature = "fault-injection")]
    if std::env::var("COWTREE_CRASH_AT").as_deref() == Ok(point) {
        std::process::exit(73);
    }
    #[cfg(not(feature = "fault-injection"))]
    let _ = point;
}

#[cfg(feature = "fault-injection")]
fn pause(point: &str) {
    use std::time::{Duration, Instant};

    if std::env::var("COWTREE_PAUSE_AT").as_deref() != Ok(point) {
        return;
    }
    let Some(path) = std::env::var_os("COWTREE_PAUSE_FILE") else {
        std::process::exit(74);
    };
    let path = std::path::PathBuf::from(path);
    if std::fs::write(&path, point).is_err() {
        std::process::exit(74);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match path.try_exists() {
            Ok(false) => return,
            Ok(true) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(true) | Err(_) => std::process::exit(74),
        }
    }
}

/// Return a deterministic I/O failure only in the explicit fault-injection build.
pub(crate) fn io_error(point: &str) -> std::io::Result<()> {
    #[cfg(feature = "fault-injection")]
    if std::env::var("COWTREE_IO_ERROR_AT").as_deref() == Ok(point) {
        return Err(std::io::Error::other(format!("injected I/O failure at {point}")));
    }
    let _ = point;
    Ok(())
}
