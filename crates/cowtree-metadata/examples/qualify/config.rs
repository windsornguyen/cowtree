// Copyright (c) 2026 Windsor Nguyen

//! Explicit limits and seed for a preserved, caller-owned qualification run.

use serde::Serialize;
use std::{error::Error, path::PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Serialize)]
pub struct Config {
    pub root: PathBuf,
    pub source: Option<PathBuf>,
    pub files: usize,
    pub operations: u32,
    pub trials: u32,
    pub seed: u64,
    pub readers: usize,
}
impl Config {
    pub fn parse() -> Result<Self> {
        let mut args = std::env::args_os().skip(1);
        let mut config = Self {
            root: PathBuf::new(),
            source: None,
            files: 10_000,
            operations: 100,
            trials: 3,
            seed: 1,
            readers: 2,
        };
        while let Some(key) = args.next() {
            let value = args.next().ok_or("every option requires a value")?;
            match key.to_str().ok_or("option must be UTF-8")? {
                "--root" => config.root = value.into(),
                "--source" => config.source = Some(value.into()),
                "--files" => config.files = value.to_str().ok_or("invalid files")?.parse()?,
                "--operations" => config.operations = value.to_str().ok_or("invalid operations")?.parse()?,
                "--trials" => config.trials = value.to_str().ok_or("invalid trials")?.parse()?,
                "--seed" => config.seed = value.to_str().ok_or("invalid seed")?.parse()?,
                "--readers" => config.readers = value.to_str().ok_or("invalid readers")?.parse()?,
                _ => return Err("unknown option; use --root NEW_DIRECTORY [--source GIT_CHECKOUT] [--files 10000] [--operations 100] [--trials 3] [--seed 1] [--readers 2]".into()),
            }
        }
        if config.root.as_os_str().is_empty()
            || config.files < 4
            || config.files > 100_000
            || config.operations == 0
            || config.operations > 1000
            || config.trials == 0
            || config.trials > 10
            || config.readers > 64
        {
            return Err(
                "required root; files 4..100000, operations 1..1000, trials 1..10, readers 0..64"
                    .into(),
            );
        }
        Ok(config)
    }
}

pub fn random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}
