// Copyright (c) 2026 Windsor Nguyen

//! Select the operating system's strict clone primitive at compile time.

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod native;

#[cfg(windows)]
#[path = "platform/windows.rs"]
mod native;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(crate) use native::clone;

#[cfg(unix)]
pub(crate) fn open_source(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(std::io::Error::from)
}

#[cfg(windows)]
pub(crate) fn open_source(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    std::fs::File::options().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn clone(
    _source: &std::fs::File,
    _target: &std::path::Path,
    _metadata: &std::fs::Metadata,
) -> crate::Result<()> {
    Err(crate::Error::UnsupportedPlatform)
}
