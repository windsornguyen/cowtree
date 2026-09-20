// Copyright (c) 2026 Windsor Nguyen

//! ReFS cloning preserves integrity settings and submits cluster-aligned ranges.
//!
//! Check both volumes before creating a destination. Keep its handle open while
//! cloning, then close it before reporting cleanup failures. No ordinary copy is
//! permitted when the device rejects an operation.

use crate::{
    Error, Operation, Result,
    clone::{cleanup, preserve},
};
use std::{
    fs::{File, Metadata},
    io,
    mem::size_of,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, GetVolumeInformationByHandleW},
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            DUPLICATE_EXTENTS_DATA, FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            FSCTL_GET_INTEGRITY_INFORMATION, FSCTL_GET_INTEGRITY_INFORMATION_BUFFER,
            FSCTL_SET_INTEGRITY_INFORMATION, FSCTL_SET_INTEGRITY_INFORMATION_BUFFER,
            FSCTL_SET_SPARSE,
        },
        SystemServices::FILE_SUPPORTS_BLOCK_REFCOUNTING,
    },
};

pub(crate) fn clone(source: &File, target: &Path, metadata: &Metadata) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        Error::io(
            Operation::Open,
            target,
            io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
        )
    })?;
    let directory = File::options()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)
        .map_err(|error| Error::io(Operation::Open, parent, error))?;
    let volume = volume(source).map_err(|error| Error::io(Operation::Inspect, target, error))?;
    if volume
        != self::volume(&directory).map_err(|error| Error::io(Operation::Inspect, parent, error))?
    {
        return Err(Error::io(
            Operation::Clone,
            target,
            io::Error::from(io::ErrorKind::CrossesDevices),
        ));
    }
    let output = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| Error::io(Operation::Open, target, error))?;
    let result = extents(source, &output, metadata.len())
        .map_err(|error| Error::io(Operation::Clone, target, error))
        .and_then(|()| {
            preserve(&output, metadata)
                .map_err(|error| Error::io(Operation::Metadata, target, error))
        });
    drop(output);
    result.map_err(|error| cleanup(target, error))
}

fn volume(file: &File) -> io::Result<u32> {
    let mut serial = 0;
    let mut flags = 0;
    // SAFETY: the file owns the handle and both output integers live through the call.
    #[allow(unsafe_code, reason = "Win32 volume query requires pointers to caller-owned output")]
    let success = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            &mut serial,
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_SUPPORTS_BLOCK_REFCOUNTING == 0 {
        return Err(io::Error::from(io::ErrorKind::Unsupported));
    }
    Ok(serial)
}

fn extents(source: &File, target: &File, size: u64) -> io::Result<()> {
    let mut integrity = FSCTL_GET_INTEGRITY_INFORMATION_BUFFER::default();
    control(source, FSCTL_GET_INTEGRITY_INFORMATION, &(), &mut integrity)?;
    let cluster = u64::from(integrity.ClusterSizeInBytes);
    if !cluster.is_power_of_two() || cluster > 65536 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ReFS cluster size"));
    }
    let setting = FSCTL_SET_INTEGRITY_INFORMATION_BUFFER {
        ChecksumAlgorithm: integrity.ChecksumAlgorithm,
        Reserved: 0,
        Flags: integrity.Flags,
    };
    control(target, FSCTL_SET_INTEGRITY_INFORMATION, &setting, &mut ())?;
    control(target, FSCTL_SET_SPARSE, &(), &mut ())?;
    target.set_len(size)?;
    let rounded =
        size.checked_add(cluster - 1).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?
            / cluster
            * cluster;
    let chunk = (1_u64 << 32) - cluster;
    let mut offset = 0;
    while offset < rounded {
        let offset_signed = i64::try_from(offset).map_err(io::Error::other)?;
        let data = DUPLICATE_EXTENTS_DATA {
            FileHandle: source.as_raw_handle(),
            SourceFileOffset: offset_signed,
            TargetFileOffset: offset_signed,
            ByteCount: i64::try_from(chunk.min(rounded - offset)).map_err(io::Error::other)?,
        };
        control(target, FSCTL_DUPLICATE_EXTENTS_TO_FILE, &data, &mut ())?;
        offset += chunk.min(rounded - offset);
    }
    Ok(())
}

fn control<Input, Output>(
    file: &File,
    code: u32,
    input: &Input,
    output: &mut Output,
) -> io::Result<()> {
    let input_size = u32::try_from(size_of::<Input>()).map_err(io::Error::other)?;
    let output_size = u32::try_from(size_of::<Output>()).map_err(io::Error::other)?;
    let input_pointer = if input_size == 0 { ptr::null() } else { ptr::from_ref(input).cast() };
    let output_pointer =
        if output_size == 0 { ptr::null_mut() } else { ptr::from_mut(output).cast() };
    let mut returned = 0;
    // SAFETY: initialized buffers match the submitted sizes and remain borrowed until completion.
    #[allow(
        unsafe_code,
        reason = "DeviceIoControl borrows typed buffers for one synchronous request"
    )]
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            input_pointer,
            input_size,
            output_pointer,
            output_size,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
