use std::path::Path;

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader};
use rssstv_demodulator::{DemodulatedChunk, Demodulator, sync_detector_delay};
use rssstv_fskid::FskId;
use rssstv_sstv::{
    RxDecoder,
    image::RgbImage,
    mode::Mode,
    rx::{DemodulatedBlock, RxConfig, RxOutcome, RxState, Staging},
};

const DEFAULT_PCM_PACKET_SIZE: usize = 1_024;

/// Configuration for the offline packetized receive pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeOptions {
    /// Number of first-channel mono PCM samples processed per packet.
    pub pcm_packet_size: usize,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            pcm_packet_size: DEFAULT_PCM_PACKET_SIZE,
        }
    }
}

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
    decode_file_with_options(input, output, DecodeOptions::default())
}

pub fn decode_file_with_options(
    input: &Path,
    output: &Path,
    options: DecodeOptions,
) -> Result<DecodeReport> {
    if options.pcm_packet_size == 0 {
        bail!("PCM packet size must be greater than zero");
    }
    let mut reader = WavReader::open(input)
        .with_context(|| format!("failed to open WAV file {}", input.display()))?;
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
    let max_samples = reader.duration() as usize;
    let pipeline = ReceivePipeline::new(spec.sample_rate, max_samples)?;
    let result = match spec.sample_format {
        SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                bail!("unsupported PCM depth: {} bits", spec.bits_per_sample);
            }
            let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            let samples = reader
                .samples::<i32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map(|sample| {
                    let value = sample.context("failed to read integer PCM sample")?;
                    Ok((f64::from(value) / scale) as f32)
                });
            decode_samples(pipeline, samples, options.pcm_packet_size)
        }
        SampleFormat::Float => {
            let samples = reader
                .samples::<f32>()
                .enumerate()
                .filter_map(|(index, sample)| (index % channels == 0).then_some(sample))
                .map(|sample| {
                    let value = sample.context("failed to read floating-point PCM sample")?;
                    if !value.is_finite() {
                        bail!("WAV contains a non-finite floating-point sample");
                    }
                    Ok(value.clamp(-1.0, 1.0))
                });
            decode_samples(pipeline, samples, options.pcm_packet_size)
        }
    };
    let result = result.with_context(|| format!("failed to decode {}", input.display()))?;
    save_image(&result.image, output)?;
    Ok(result.report)
}

struct ReceivePipeline {
    demodulator: Demodulator,
    decoder: Option<RxDecoder>,
    max_samples: usize,
    fsk_ids: Vec<FskId>,
}

impl ReceivePipeline {
    fn new(sample_rate_hz: u32, max_samples: usize) -> Result<Self> {
        Ok(Self {
            demodulator: Demodulator::new(sample_rate_hz)?,
            decoder: None,
            max_samples,
            fsk_ids: Vec::new(),
        })
    }

    fn process_pcm(&mut self, samples: &[f32]) -> Result<()> {
        let output = self.demodulator.process(samples)?;
        self.process_demodulated(&output)
    }

    fn process_demodulated(&mut self, output: &DemodulatedChunk) -> Result<()> {
        self.fsk_ids.extend_from_slice(output.fsk_ids());
        if let Some(mode) = output.detected_mode() {
            self.decoder = Some(
                RxDecoder::with_config(
                    mode,
                    self.demodulator.sample_rate_hz(),
                    RxConfig {
                        live_sync: true,
                        live_slant: false,
                        auto_stop: false,
                        sync_detector_delay: sync_detector_delay(mode),
                        staging: Staging::Memory {
                            max_samples: self.max_samples,
                        },
                    },
                )
                .with_context(|| format!("cannot decode VIS mode {}", mode.spec().name()))?,
            );
        }
        if output.frequency_hz().is_empty() {
            return Ok(());
        }
        let decoder = self
            .decoder
            .as_mut()
            .context("demodulator produced image data before VIS detection")?;
        let mut offset = 0;
        while offset < output.frequency_hz().len() {
            match decoder.state() {
                RxState::Complete => {
                    decoder.stage_for_refinement(DemodulatedBlock::new(
                        output.first_sample() + offset as u64,
                        &output.frequency_hz()[offset..],
                        &output.sync_strength()[offset..],
                    ))?;
                    break;
                }
                RxState::Stopped { .. } => break,
                _ => {}
            }
            let processed = decoder
                .process(DemodulatedBlock::new(
                    output.first_sample() + offset as u64,
                    &output.frequency_hz()[offset..],
                    &output.sync_strength()[offset..],
                ))
                .map_err(|error| {
                    anyhow::anyhow!(
                        "receive decoding failed at sample {}: {}",
                        output.first_sample() + offset as u64 + error.consumed() as u64,
                        error.error()
                    )
                })?;
            offset += processed.consumed();
            if processed.consumed() == 0 && processed.event().is_none() {
                bail!("receive decoder made no progress");
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<PipelineResult> {
        let mode = self.demodulator.finish()?;
        let frequency_offset_hz = self.demodulator.frequency_offset_hz();
        let mut decoder = self.decoder.take().context("VIS mode was not detected")?;
        if decoder.state() == RxState::Complete {
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
        Ok(PipelineResult {
            image,
            report: DecodeReport {
                mode,
                status,
                frequency_offset_hz,
                effective_sample_rate_hz,
                fsk_ids: self.fsk_ids,
            },
        })
    }
}

struct PipelineResult {
    image: RgbImage,
    report: DecodeReport,
}

fn decode_samples<I>(
    mut pipeline: ReceivePipeline,
    samples: I,
    packet_size: usize,
) -> Result<PipelineResult>
where
    I: Iterator<Item = Result<f32>>,
{
    let mut packet = Vec::with_capacity(packet_size);
    let mut sample_count = 0_u64;
    for sample in samples {
        packet.push(sample?);
        sample_count += 1;
        if packet.len() == packet_size {
            pipeline.process_pcm(&packet)?;
            packet.clear();
        }
    }
    if sample_count == 0 {
        bail!("WAV file contains no samples");
    }
    if !packet.is_empty() {
        pipeline.process_pcm(&packet)?;
    }
    pipeline.finish()
}

fn save_image(image: &RgbImage, path: &Path) -> Result<()> {
    let size = image.size();
    let output = image::RgbImage::from_raw(
        size.width() as u32,
        size.height() as u32,
        image.to_rgb_bytes(),
    )
    .context("decoded image dimensions are invalid")?;
    output
        .save(path)
        .with_context(|| format!("failed to save image {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::{f64::consts::TAU, fs};

    use hound::{WavSpec, WavWriter};
    use rssstv_sstv::{
        TxEncoder,
        image::{ImageSize, Rgb8},
    };

    use super::*;

    #[test]
    fn packet_sizes_preserve_stereo_wav_decode() {
        let mode = Mode::Robot36;
        let size =
            ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize).unwrap();
        let source = RgbImage::new(size, Rgb8::new(80, 140, 200));
        let sample_rate = 8_000_u32;
        let unique = format!("decode-wav-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{unique}.wav"));
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
            let deadline = tone.until().to_samples(sample_rate);
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

        let mut expected_report = None;
        let mut expected_image = None;
        let mut outputs = Vec::new();
        for packet_size in [73, 1_024, 4_093] {
            let output = std::env::temp_dir().join(format!("{unique}-{packet_size}.png"));
            let report = decode_file_with_options(
                &input,
                &output,
                DecodeOptions {
                    pcm_packet_size: packet_size,
                },
            )
            .unwrap();
            assert_eq!(report.mode, mode);
            assert_eq!(report.status, DecodeStatus::Complete);
            assert_eq!(report.fsk_ids[0].as_str(), "JL1HIS");
            let saved = image::open(&output).unwrap().to_rgb8();
            assert_eq!(saved.width(), size.width() as u32);
            assert_eq!(saved.height(), size.height() as u32);
            if let Some(expected) = &expected_report {
                assert_eq!(&report, expected);
                assert_eq!(&saved, expected_image.as_ref().unwrap());
            } else {
                expected_report = Some(report);
                expected_image = Some(saved);
            }
            outputs.push(output);
        }

        fs::remove_file(input).unwrap();
        for output in outputs {
            fs::remove_file(output).unwrap();
        }
    }
}
