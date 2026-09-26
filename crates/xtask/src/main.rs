// Copyright (c) 2026 Windsor Nguyen

//! Run repository development checks without a language runtime dependency.

mod command;
mod schema;
mod specs;

use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Arguments {
    /// One repository development operation.
    #[command(subcommand)]
    operation: Operation,
}

#[derive(Subcommand)]
enum Operation {
    Schema(schema::Arguments),
    Specs(specs::Arguments),
}

fn main() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    match Arguments::parse().operation {
        Operation::Schema(arguments) => schema::run(&root, arguments),
        Operation::Specs(arguments) => specs::run(&root, arguments),
    }
}
