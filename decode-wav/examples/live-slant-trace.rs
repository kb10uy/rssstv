//! Traces the receive events the application's configuration produces.
//!
//! `decode-wav` decodes offline, with one precise fit after the transmission
//! ends. The application decodes live instead, correcting the raster while the
//! picture arrives, so its failures do not reproduce through the binary. This
//! drives that configuration over a recording and prints every clock decision.
//!
//! ```text
//! cargo run -p decode-wav --example live-slant-trace -- RECORDING.wav [OPTIONS]
//!
//!   --no-slant       decode with live slant tracking off
//!   --rate HZ        resample first, as playing into a capture device does
//!   --skew PPM       stretch the recording against the rate, which is slant
//! ```

use std::{env, path::PathBuf};

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader};
use rssstv_demodulator::{Demodulator, sync_detector_delay};
use rssstv_sstv::{
    RxDecoder,
    rx::{DemodulatedBlock, RxConfig, RxEvent, RxState, Staging},
};

/// Matches the application's receive worker.
const STAGING_SECONDS: usize = 300;

struct Options {
    input: PathBuf,
    live_slant: bool,
    rate: Option<u32>,
    skew_ppm: f64,
}

fn parse_options() -> Result<Options> {
    let mut arguments = env::args().skip(1);
    let mut options = Options {
        input: PathBuf::new(),
        live_slant: true,
        rate: None,
        skew_ppm: 0.0,
    };
    let mut input = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--no-slant" => options.live_slant = false,
            "--rate" => {
                options.rate = Some(arguments.next().context("--rate needs a value")?.parse()?);
            }
            "--skew" => {
                options.skew_ppm = arguments.next().context("--skew needs a value")?.parse()?;
            }
            value if value.starts_with("--") => bail!("unknown option {value}"),
            value => input = Some(PathBuf::from(value)),
        }
    }
    options.input = input.context("usage: live-slant-trace <RECORDING.wav> [OPTIONS]")?;
    Ok(options)
}

fn read_mono(reader: &mut WavReader<std::io::BufReader<std::fs::File>>) -> Result<Vec<f32>> {
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
    let mono = |index: usize| index.is_multiple_of(channels);
    Ok(match spec.sample_format {
        SampleFormat::Int => reader
            .samples::<i32>()
            .enumerate()
            .filter(|(index, _)| mono(*index))
            .map(|(_, sample)| Ok((f64::from(sample?) / scale) as f32))
            .collect::<Result<_>>()?,
        SampleFormat::Float => reader
            .samples::<f32>()
            .enumerate()
            .filter(|(index, _)| mono(*index))
            .map(|(_, sample)| Ok(sample?))
            .collect::<Result<_>>()?,
    })
}

/// Resamples by linear interpolation, which is enough to move timing about.
fn resample(samples: &[f32], ratio: f64) -> Vec<f32> {
    (0..(samples.len() as f64 / ratio) as usize)
        .map(|index| {
            let position = index as f64 * ratio;
            let left = position as usize;
            let fraction = (position - left as f64) as f32;
            let right = samples.get(left + 1).copied().unwrap_or(samples[left]);
            samples[left] * (1.0 - fraction) + right * fraction
        })
        .collect()
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let mut reader = WavReader::open(&options.input)?;
    let source_rate = reader.spec().sample_rate;
    let samples = read_mono(&mut reader)?;
    let rate = options.rate.unwrap_or(source_rate);
    let played = f64::from(rate) * (1.0 + options.skew_ppm / 1.0e6);
    let samples = if played == f64::from(source_rate) {
        samples
    } else {
        resample(&samples, f64::from(source_rate) / played)
    };
    println!(
        "{} samples at {rate} Hz (source {source_rate} Hz), skew {:+} ppm, live slant {}",
        samples.len(),
        options.skew_ppm,
        if options.live_slant { "on" } else { "off" },
    );

    let mut demodulator = Demodulator::new(rate)?;
    let mut decoder: Option<RxDecoder> = None;
    let mut rows = 0_usize;
    for packet in samples.chunks(decode_wav::DEFAULT_PCM_PACKET_SIZE) {
        let chunk = demodulator.process(packet)?;
        if let Some(mode) = chunk.detected_mode() {
            println!("detected {}", mode.spec().name());
            decoder = Some(RxDecoder::with_config(
                mode,
                rate,
                RxConfig {
                    live_sync: true,
                    live_slant: options.live_slant,
                    auto_stop: false,
                    sync_detector_delay: sync_detector_delay(mode),
                    staging: Staging::Memory {
                        max_samples: (rate as usize).saturating_mul(STAGING_SECONDS),
                    },
                },
            )?);
        }
        let Some(decoder) = decoder.as_mut() else {
            continue;
        };
        let mut offset = 0;
        while offset < chunk.frequency_hz().len() {
            if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
                break;
            }
            let processed = decoder
                .process(DemodulatedBlock::new(
                    chunk.first_sample() + offset as u64,
                    &chunk.frequency_hz()[offset..],
                    &chunk.sync_strength()[offset..],
                ))
                .map_err(|error| anyhow::anyhow!("{}", error.error()))?;
            match processed.event() {
                Some(RxEvent::RowDecoded { .. }) => rows += 1,
                Some(RxEvent::SlantAdjusted {
                    unit,
                    effective_sample_rate_hz,
                }) => println!(
                    "slant at unit {unit}: {effective_sample_rate_hz:.3} Hz ({:+.1} ppm)",
                    (effective_sample_rate_hz / f64::from(rate) - 1.0) * 1.0e6
                ),
                Some(RxEvent::PhaseAdjusted {
                    unit,
                    displacement_samples,
                    ..
                }) => println!("phase at unit {unit}: {displacement_samples:+} samples"),
                Some(event) => println!("{event:?}"),
                None => {}
            }
            offset += processed.consumed();
            if processed.consumed() == 0 && processed.event().is_none() {
                break;
            }
        }
    }
    println!(
        "{rows} rows, state {:?}, raster rate {:?}",
        decoder.as_ref().map(RxDecoder::state),
        decoder
            .as_ref()
            .and_then(RxDecoder::effective_sample_rate_hz),
    );
    if let Some(decoder) = decoder.as_mut()
        && decoder.state() == RxState::Complete
    {
        match decoder.refine_staged() {
            Ok(result) => println!(
                "staged fit: {:.3} Hz ({:+.1} ppm), residual {:.2} samples over {} units",
                result.slant.effective_sample_rate_hz,
                result.slant.error_ppm,
                result.slant.residual_samples,
                result.slant.observations,
            ),
            Err(error) => println!("staged fit: {error}"),
        }
    }
    Ok(())
}
