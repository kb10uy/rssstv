use std::path::Path;

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader};
use rssstv_demodulator::demodulate;
use rssstv_fskid::FskId;
use rssstv_sstv::RxDecoder;
use rssstv_sstv::image::RgbImage;
use rssstv_sstv::mode::Mode;
use rssstv_sstv::rx::{DemodulatedBlock, RxConfig, RxOutcome, RxState, Staging};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeStatus {
    Complete,
    Incomplete,
    SynchronizationLost,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodeReport {
    pub mode: Mode,
    pub status: DecodeStatus,
    pub frequency_offset_hz: f64,
    pub effective_sample_rate_hz: Option<f64>,
    pub fsk_ids: Vec<FskId>,
}

pub fn decode_file(input: &Path, output: &Path) -> Result<DecodeReport> {
    let (samples, sample_rate_hz) = read_wav(input)?;
    let demodulated = demodulate(&samples, sample_rate_hz)
        .with_context(|| format!("failed to demodulate {}", input.display()))?;
    let mode = demodulated.mode();
    let max_samples = demodulated.frequency_hz().len();
    let mut decoder = RxDecoder::with_config(
        mode,
        sample_rate_hz,
        RxConfig {
            live_sync: true,
            auto_stop: false,
            sync_detector_delay: demodulated.sync_detector_delay(),
            staging: Staging::Memory { max_samples },
        },
    )
    .with_context(|| format!("cannot decode VIS mode {}", mode.spec().name()))?;

    let mut offset = 0;
    while offset < demodulated.frequency_hz().len()
        && !matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. })
    {
        let block = DemodulatedBlock::new(
            demodulated.first_sample() + offset as u64,
            &demodulated.frequency_hz()[offset..],
            &demodulated.sync_strength()[offset..],
        );
        let processed = decoder.process(block).map_err(|error| {
            anyhow::anyhow!(
                "receive decoding failed at sample {}: {}",
                demodulated.first_sample() + offset as u64 + error.consumed() as u64,
                error.error()
            )
        })?;
        offset += processed.consumed();
        if processed.consumed() == 0 && processed.event().is_none() {
            bail!("receive decoder made no progress");
        }
    }

    if decoder.state() == RxState::Complete {
        if offset < demodulated.frequency_hz().len() {
            decoder.stage_for_refinement(DemodulatedBlock::new(
                demodulated.first_sample() + offset as u64,
                &demodulated.frequency_hz()[offset..],
                &demodulated.sync_strength()[offset..],
            ))?;
        }
        decoder
            .refine_staged()
            .context("failed to refine raster slant from staged synchronization")?;
    }
    let effective_sample_rate_hz = decoder.effective_sample_rate_hz();
    let (image, status) = match decoder.finish() {
        RxOutcome::Complete(image) => (image, DecodeStatus::Complete),
        RxOutcome::Incomplete { image, .. } => (image, DecodeStatus::Incomplete),
        RxOutcome::Stopped { image, .. } => (image, DecodeStatus::SynchronizationLost),
    };
    save_image(&image, output)?;
    Ok(DecodeReport {
        mode,
        status,
        frequency_offset_hz: demodulated.frequency_offset_hz(),
        effective_sample_rate_hz,
        fsk_ids: demodulated.fsk_ids().to_vec(),
    })
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut reader = WavReader::open(path)
        .with_context(|| format!("failed to open WAV file {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("WAV file has no channels");
    }
    if spec.sample_rate < 6_000 {
        bail!(
            "WAV sample rate {} Hz is too low for SSTV",
            spec.sample_rate
        );
    }
    let channels = spec.channels as usize;
    let samples = match spec.sample_format {
        SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                bail!("unsupported PCM depth: {} bits", spec.bits_per_sample);
            }
            let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            reader
                .samples::<i32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map(|sample| {
                    sample
                        .map(|value| (f64::from(value) / scale) as f32)
                        .context("failed to read integer PCM sample")
                })
                .collect::<Result<Vec<_>>>()?
        }
        SampleFormat::Float => reader
            .samples::<f32>()
            .enumerate()
            .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
            .map(|sample| {
                let value = sample.context("failed to read floating-point PCM sample")?;
                if !value.is_finite() {
                    bail!("WAV contains a non-finite floating-point sample");
                }
                Ok(value.clamp(-1.0, 1.0))
            })
            .collect::<Result<Vec<_>>>()?,
    };
    if samples.is_empty() {
        bail!("WAV file contains no samples");
    }
    Ok((samples, spec.sample_rate))
}

fn save_image(image: &RgbImage, path: &Path) -> Result<()> {
    let size = image.size();
    let mut bytes = Vec::with_capacity(size.pixel_count() * 3);
    for pixel in image.pixels() {
        bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    let output = image::RgbImage::from_raw(size.width() as u32, size.height() as u32, bytes)
        .context("decoded image dimensions are invalid")?;
    output
        .save(path)
        .with_context(|| format!("failed to save image {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;
    use std::fs;

    use hound::{WavSpec, WavWriter};
    use rssstv_sstv::TxEncoder;
    use rssstv_sstv::image::{ImageSize, Rgb8};

    use super::*;

    #[test]
    fn decodes_stereo_wav_and_saves_png_by_extension() {
        let mode = Mode::Robot36;
        let size =
            ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize).unwrap();
        let source = RgbImage::new(size, Rgb8::new(80, 140, 200));
        let sample_rate = 8_000_u32;
        let unique = format!("decode-wav-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{unique}.wav"));
        let output = std::env::temp_dir().join(format!("{unique}.png"));
        let mut writer = WavWriter::create(
            &input,
            WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        let mut phase = 0.0_f64;
        let mut written = 0_u64;
        for tone in TxEncoder::new(mode, source).unwrap() {
            let deadline = tone.until().as_picos() * u64::from(sample_rate) / 1_000_000_000_000;
            while written < deadline {
                let sample = (phase.sin() * 24_000.0) as i16;
                writer.write_sample(sample).unwrap();
                writer.write_sample(0_i16).unwrap();
                phase = (phase
                    + TAU * f64::from(tone.frequency().as_hz()) / f64::from(sample_rate))
                .rem_euclid(TAU);
                written += 1;
            }
        }
        {
            let mut deadline = written as f64;
            let mut write_tone = |frequency: f64, seconds: f64| {
                deadline += f64::from(sample_rate) * seconds;
                while written < deadline as u64 {
                    let sample = (phase.sin() * 24_000.0) as i16;
                    writer.write_sample(sample).unwrap();
                    writer.write_sample(0_i16).unwrap();
                    phase = (phase + TAU * frequency / f64::from(sample_rate)).rem_euclid(TAU);
                    written += 1;
                }
            };
            write_tone(1_500.0, 0.3);
            write_tone(2_100.0, 0.1);
            write_tone(1_900.0, 0.022);
            for symbol in [0x2a_u8, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25] {
                for bit in 0..6 {
                    write_tone(
                        if symbol & (1 << bit) == 0 {
                            2_100.0
                        } else {
                            1_900.0
                        },
                        0.022,
                    );
                }
            }
            write_tone(2_100.0, 0.1);
        }
        for _ in 0..sample_rate / 10 {
            writer.write_sample(0_i16).unwrap();
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();

        let report = decode_file(&input, &output).unwrap();
        assert_eq!(report.mode, mode);
        assert_eq!(report.status, DecodeStatus::Complete);
        assert_eq!(report.fsk_ids[0].as_str(), "JL1HIS");
        let saved = image::open(&output).unwrap();
        assert_eq!(saved.width(), size.width() as u32);
        assert_eq!(saved.height(), size.height() as u32);

        fs::remove_file(input).unwrap();
        fs::remove_file(output).unwrap();
    }
}
