// Copyright (c) 2026 Windsor Nguyen

//! Start the line-delimited JSON metadata interface.

mod json_cli;

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        eprintln!("usage: cowtree-metadata STORE_ROOT < requests.jsonl");
        return std::process::ExitCode::FAILURE;
    }
    match json_cli::run(std::path::Path::new(&args[1])) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
