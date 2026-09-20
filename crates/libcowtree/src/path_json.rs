// Copyright (c) 2026 Windsor Nguyen

//! JSON filenames retain operating-system bytes using Python's surrogate escape contract.

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;
use std::{ffi::OsStr, path::Path};

pub(crate) fn path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    let encoded = encode(path.as_os_str()).map_err(serde::ser::Error::custom)?;
    encoded.serialize(serializer)
}

pub(crate) fn optional_path<S: Serializer>(
    value: &Option<std::path::PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => path(value, serializer),
        None => serializer.serialize_none(),
    }
}

#[cfg(unix)]
fn encode(value: &OsStr) -> serde_json::Result<Box<RawValue>> {
    use std::os::unix::ffi::OsStrExt;

    let mut json = String::from("\"");
    for chunk in value.as_bytes().utf8_chunks() {
        append_text(&mut json, chunk.valid())?;
        for byte in chunk.invalid() {
            json.push_str(&format!("\\udc{byte:02x}"));
        }
    }
    json.push('"');
    RawValue::from_string(json)
}

#[cfg(windows)]
fn encode(value: &OsStr) -> serde_json::Result<Box<RawValue>> {
    use std::os::windows::ffi::OsStrExt;

    let mut json = String::from("\"");
    for character in char::decode_utf16(value.encode_wide()) {
        match character {
            Ok(character) => append_text(&mut json, character.encode_utf8(&mut [0; 4]))?,
            Err(error) => json.push_str(&format!("\\u{:04x}", error.unpaired_surrogate())),
        }
    }
    json.push('"');
    RawValue::from_string(json)
}

fn append_text(json: &mut String, text: &str) -> serde_json::Result<()> {
    let escaped = serde_json::to_string(text)?;
    json.push_str(&escaped[1..escaped.len() - 1]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::encode;
    use std::ffi::OsStr;

    #[test]
    fn unicode_quotes_and_controls_round_trip() -> serde_json::Result<()> {
        let value = "filename \"with\"\ncontrols";
        let raw = encode(OsStr::new(value))?;
        assert_eq!(serde_json::from_str::<String>(raw.get())?, value);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_bytes_remain_explicit_surrogates() -> serde_json::Result<()> {
        use std::os::unix::ffi::OsStrExt;

        let raw = encode(OsStr::from_bytes(b"bad-\xff"))?;
        assert_eq!(raw.get(), "\"bad-\\udcff\"");
        assert_eq!(serde_json::to_string(&raw)?, "\"bad-\\udcff\"");
        Ok(())
    }
}
