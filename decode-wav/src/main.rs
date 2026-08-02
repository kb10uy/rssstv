use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use decode_wav::{DecodeOptions, DecodeStatus, decode_file_with_options};

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
    let mut options = DecodeOptions::default();
    let mut positional = Vec::with_capacity(2);
    while let Some(argument) = args.next() {
        if argument == "--packet-size" {
            let Some(value) = args.next() else {
                return usage();
            };
            options.pcm_packet_size = value
                .to_str()
                .and_then(|value| value.parse().ok())
                .filter(|&value| value > 0)
                .ok_or_else(|| anyhow::anyhow!("--packet-size must be a positive integer"))?;
        } else if argument.to_string_lossy().starts_with("--") {
            return usage();
        } else {
            positional.push(argument);
        }
    }
    let [input, output] = positional.as_slice() else {
        return usage();
    };
    let report = decode_file_with_options(&PathBuf::from(input), &PathBuf::from(output), options)?;
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
    bail!("usage: decode-wav [--packet-size SAMPLES] <INPUT.wav> <OUTPUT_IMAGE>")
}
