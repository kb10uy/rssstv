use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use decode_wav::{DecodeStatus, decode_file};

fn main() -> ExitCode {
    match run() {
        Ok(DecodeStatus::Complete) => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("warning: saved a partial image ({status:?})");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<DecodeStatus> {
    let mut args = env::args_os().skip(1);
    let Some(input) = args.next() else {
        return usage();
    };
    let Some(output) = args.next() else {
        return usage();
    };
    if args.next().is_some() {
        return usage();
    }
    let report = decode_file(&PathBuf::from(input), &PathBuf::from(output))?;
    println!(
        "mode: {}, AFC: {:+.1} Hz, raster rate: {}",
        report.mode.spec().name(),
        report.frequency_offset_hz,
        report
            .effective_sample_rate_hz
            .map(|rate| format!("{rate:.3} Hz"))
            .unwrap_or_else(|| "not acquired".to_owned())
    );
    for id in &report.fsk_ids {
        println!("fskid: {id}");
    }
    Ok(report.status)
}

fn usage<T>() -> Result<T> {
    bail!("usage: decode-wav <INPUT.wav> <OUTPUT_IMAGE>")
}
