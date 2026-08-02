use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use encode_wav::{encode_file, parse_mode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let mut callsign = OsString::from("N0CALL");
    let mut positional = Vec::with_capacity(4);
    while let Some(argument) = args.next() {
        if argument == "--callsign" {
            let Some(value) = args.next() else {
                return usage();
            };
            callsign = value;
        } else if argument.to_string_lossy().starts_with("--") {
            return usage();
        } else {
            positional.push(argument);
        }
    }
    let [template, background, mode, output] = positional.as_slice() else {
        return usage();
    };
    let mode = mode
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("mode must be valid UTF-8"))
        .and_then(parse_mode)?;
    let callsign = callsign
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("callsign must be valid UTF-8"))?;
    let report = encode_file(
        &PathBuf::from(template),
        &PathBuf::from(background),
        mode,
        &PathBuf::from(output),
        callsign,
    )?;
    println!(
        "mode: {}, callsign: {}, samples: {}",
        report.mode.spec().name(),
        report.callsign,
        report.samples_written
    );
    Ok(())
}

fn usage<T>() -> Result<T> {
    bail!(
        "usage: encode-wav [--callsign CALLSIGN] <TEMPLATE.kdl> <BACKGROUND_IMAGE> <MODE> <OUTPUT.wav>"
    )
}
