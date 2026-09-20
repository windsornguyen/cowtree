// Copyright (c) 2026 Windsor Nguyen

//! Emit one structured result or preserve the human-readable standalone output.

use serde::Serialize;
use std::io::{self, Write};

#[derive(Serialize)]
pub(crate) struct Success<T> {
    status: &'static str,
    kind: &'static str,
    value: T,
}

pub(crate) fn success<T: Serialize>(kind: &'static str, value: T) -> io::Result<()> {
    write_json(&Success { status: "ok", kind, value }, &mut io::stdout().lock())
}

pub(crate) fn write_json<T: Serialize>(value: &T, output: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)
}

pub(crate) fn failure(code: &str, message: &str, json: bool) -> io::Result<()> {
    #[derive(Serialize)]
    struct Failure<'a> {
        status: &'static str,
        code: &'a str,
        message: &'a str,
    }

    if json {
        write_json(&Failure { status: "error", code, message }, &mut io::stderr().lock())
    } else {
        writeln!(io::stderr().lock(), "cowtree: {code}: {message}")
    }
}
