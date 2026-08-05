//! End-to-end checks of the receive front end.
//!
//! Driven through the public interface only, so what they exercise is what a
//! caller can reach.

use core::f64::consts::TAU;

use rssstv_demodulator::{Demodulator, DemodulatorError, demodulate, sync_detector_delay};
use rssstv_sstv::{
    RxDecoder, TxEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
    rx::{DemodulatedBlock, RxConfig},
    time::SstvDuration,
};
use rstest::rstest;

/// Measured delay of the band-pass and discriminator chain, in milliseconds.
const GROUP_DELAY_MS: f64 = 2.05;

fn tone(samples: &mut Vec<f32>, rate: u32, frequency: f64, seconds: f64, phase: &mut f64) {
    for _ in 0..(rate as f64 * seconds).round() as usize {
        samples.push((*phase).sin() as f32 * 0.8);
        *phase = (*phase + TAU * frequency / rate as f64).rem_euclid(TAU);
    }
}

fn vis_signal(mode: Mode, rate: u32, offset: f64) -> Vec<f32> {
    let mut samples = Vec::new();
    let mut phase = 0.0_f64;
    tone(&mut samples, rate, 1_900.0 + offset, 0.3, &mut phase);
    tone(&mut samples, rate, 1_200.0 + offset, 0.01, &mut phase);
    tone(&mut samples, rate, 1_900.0 + offset, 0.3, &mut phase);
    tone(&mut samples, rate, 1_200.0 + offset, 0.03, &mut phase);
    let raw = mode.spec().raw_vis().unwrap();
    for bit in 0..8 {
        let frequency = if raw & (1 << bit) == 0 {
            1_300.0
        } else {
            1_100.0
        };
        tone(&mut samples, rate, frequency + offset, 0.03, &mut phase);
    }
    tone(&mut samples, rate, 1_200.0 + offset, 0.03, &mut phase);
    tone(&mut samples, rate, 1_900.0 + offset, 0.1, &mut phase);
    samples
}

fn fsk_id_signal(samples: &mut Vec<f32>, rate: u32, phase: &mut f64) {
    const SYMBOLS: [u8; 9] = [0x2a, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25];
    tone(samples, rate, 2_100.0, 0.1, phase);
    tone(samples, rate, 1_900.0, 0.022, phase);
    for symbol in SYMBOLS {
        for bit in 0..6 {
            let frequency = if symbol & (1 << bit) == 0 {
                2_100.0
            } else {
                1_900.0
            };
            tone(samples, rate, frequency, 0.022, phase);
        }
    }
    tone(samples, rate, 2_100.0, 0.1, phase);
}

/// Transmits a vertical edge and checks where the decoded raster puts it.
///
/// This is the end-to-end statement of horizontal alignment: the modulated
/// audio, the demodulator's own group delay, and the decoder's raster phase
/// all have to agree before a picture stops sliding off one side.
#[rstest]
#[case(Mode::Martin1, 48_000)]
#[case(Mode::Martin2, 48_000)]
#[case(Mode::Scottie2, 48_000)]
#[case(Mode::Robot36, 48_000)]
#[case(Mode::Robot72, 48_000)]
#[case(Mode::Pd50, 48_000)]
#[case(Mode::Martin2, 8_000)]
#[case(Mode::Scottie2, 8_000)]
#[case(Mode::Robot36, 8_000)]
#[case(Mode::Pd50, 8_000)]
fn a_transmitted_edge_decodes_where_it_was_sent(#[case] mode: Mode, #[case] rate: u32) {
    let width = mode.spec().width() as usize;
    let size = ImageSize::new(width, mode.spec().height() as usize).unwrap();
    let mut image = RgbImage::new(size, Rgb8::new(0, 0, 0));
    for row in 0..size.height() {
        for x in width / 2..width {
            if let Some(pixel) = image.row_mut(row).and_then(|row| row.get_mut(x)) {
                *pixel = Rgb8::new(255, 255, 255);
            }
        }
    }
    let scan = mode.scan();
    let leading_ps = scan
        .leading()
        .iter()
        .map(|segment| segment.duration().as_picos())
        .sum::<u64>();
    let stop_ps = 910_000_000_000 + leading_ps + mode.spec().period().as_picos() * 14;
    let stop_sample = SstvDuration::from_picos(stop_ps).to_samples(rate);
    let mut samples = Vec::new();
    let mut phase = 0.0_f64;
    let mut written = 0_u64;
    for timed in TxEncoder::new(mode, image).unwrap() {
        let deadline = timed.until().to_samples(rate);
        while written < deadline && written < stop_sample {
            samples.push((phase.sin() * 0.8) as f32);
            phase = (phase + TAU * f64::from(timed.frequency().as_hz()) / f64::from(rate))
                .rem_euclid(TAU);
            written += 1;
        }
        if written >= stop_sample {
            break;
        }
    }
    let output = demodulate(&samples, rate).unwrap();
    let mut decoder = RxDecoder::with_config(
        mode,
        rate,
        RxConfig {
            sync_detector_delay: output.sync_detector_delay(),
            ..RxConfig::default()
        },
    )
    .unwrap();
    let mut offset = 0;
    while let Ok(processed) = decoder.process(DemodulatedBlock::new(
        output.first_sample() + offset as u64,
        &output.frequency_hz()[offset..],
        &output.sync_strength()[offset..],
    )) {
        offset += processed.consumed();
        if processed.consumed() == 0 && processed.event().is_none() {
            break;
        }
    }
    let rows = mode.spec().rows_per_raster_unit() as usize;
    let row = decoder.image().row(rows * 6).unwrap();
    let edge = (0..width).find(|x| row[*x].g > 128);
    assert!(
        edge.is_some_and(|edge| edge.abs_diff(width / 2) <= 1),
        "{mode:?} @{rate}: the edge decoded at {edge:?} instead of {}",
        width / 2,
    );
}

fn raster_epoch_error_ms(mode: Mode, rate: u32) -> f64 {
    let size = ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize).unwrap();
    let image = RgbImage::new(size, Rgb8::new(128, 128, 128));
    let scan = mode.scan();
    let leading_ps = scan
        .leading()
        .iter()
        .map(|segment| segment.duration().as_picos())
        .sum::<u64>();
    let epoch_ps = 910_000_000_000 + leading_ps;
    let stop_ps = epoch_ps + mode.spec().period().as_picos() * 40;
    let stop_sample = SstvDuration::from_picos(stop_ps).to_samples(rate);
    let mut samples = Vec::new();
    let mut phase = 0.0_f64;
    let mut written = 0_u64;
    for timed in TxEncoder::new(mode, image).unwrap() {
        let deadline = timed.until().to_samples(rate);
        while written < deadline && written < stop_sample {
            samples.push((phase.sin() * 0.8) as f32);
            phase = (phase + TAU * f64::from(timed.frequency().as_hz()) / f64::from(rate))
                .rem_euclid(TAU);
            written += 1;
        }
        if written >= stop_sample {
            break;
        }
    }

    let output = demodulate(&samples, rate).unwrap();
    let mut decoder = RxDecoder::with_config(
        mode,
        rate,
        RxConfig {
            sync_detector_delay: output.sync_detector_delay(),
            ..RxConfig::default()
        },
    )
    .unwrap();
    let mut offset = 0;
    while offset < output.frequency_hz().len() && decoder.source_epoch().is_none() {
        let processed = decoder
            .process(DemodulatedBlock::new(
                output.first_sample() + offset as u64,
                &output.frequency_hz()[offset..],
                &output.sync_strength()[offset..],
            ))
            .unwrap();
        offset += processed.consumed();
    }
    let actual = decoder.source_epoch().unwrap() as f64;
    let expected = epoch_ps as f64 * f64::from(rate) / 1.0e12;
    let period = mode.spec().period().as_picos() as f64 * f64::from(rate) / 1.0e12;
    let mut error = (actual - expected).rem_euclid(period);
    if error > period / 2.0 {
        error -= period;
    }
    error * 1_000.0 / f64::from(rate)
}

#[test]
fn detects_parity_inclusive_vis() {
    let samples = vis_signal(Mode::Scottie2, 8_000, 0.0);
    let output = demodulate(&samples, 8_000).unwrap();
    assert_eq!(output.mode(), Mode::Scottie2);
}

/// A station that starts over sends a second header while the picture it
/// abandoned is still being decoded. Reading it is the only way the new
/// mode is known outright, so the front end has to keep listening for one.
#[test]
fn a_header_starts_the_reception_over_when_restarts_are_enabled() {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Robot36, rate, 0.0);
    let mut phase = 0.0;
    tone(&mut samples, rate, 1_500.0, 0.5, &mut phase);
    let restart = samples.len();
    samples.extend(vis_signal(Mode::Scottie2, rate, 0.0));

    let mut demodulator = Demodulator::new(rate).unwrap();
    demodulator.set_header_restart(true);
    let mut detected = Vec::new();
    let mut first_sample = 0;
    for packet in samples.chunks(1_024) {
        let output = demodulator.process(packet).unwrap();
        if let Some(mode) = output.detected_mode() {
            detected.push(mode);
            first_sample = output.first_sample();
        }
    }

    assert_eq!(detected, [Mode::Robot36, Mode::Scottie2]);
    assert_eq!(demodulator.mode(), Some(Mode::Scottie2));
    assert!(
        first_sample as usize > restart,
        "the second reception must start after the header that named it"
    );
}

/// An offline decode of one transmission keeps the mode it identified, so
/// the restart cannot be something a caller gets without asking.
#[test]
fn a_second_header_is_ignored_by_default() {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Robot36, rate, 0.0);
    let mut phase = 0.0;
    tone(&mut samples, rate, 1_500.0, 0.5, &mut phase);
    samples.extend(vis_signal(Mode::Scottie2, rate, 0.0));

    let mut demodulator = Demodulator::new(rate).unwrap();
    let mut detected = Vec::new();
    for packet in samples.chunks(1_024) {
        detected.extend(demodulator.process(packet).unwrap().detected_mode());
    }

    assert_eq!(detected, [Mode::Robot36]);
}

#[test]
fn synchronization_envelope_is_causal_at_end_of_input() {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Robot36, rate, 0.0);
    let mut phase = 0.0;
    tone(&mut samples, rate, 1_200.0, 0.05, &mut phase);
    let output = demodulate(&samples, rate).unwrap();
    assert!(output.sync_strength().last().copied().unwrap() > 0.5);
}

#[test]
fn detector_delay_is_calibrated_only_for_supported_raster_families() {
    let calibrated = sync_detector_delay(Mode::Martin1);
    assert_ne!(calibrated, SstvDuration::ZERO);
    for mode in [Mode::Martin2, Mode::Scottie2, Mode::Robot36, Mode::Pd50] {
        assert_eq!(sync_detector_delay(mode), calibrated, "{mode:?}");
    }
    assert_eq!(sync_detector_delay(Mode::Avt90), SstvDuration::ZERO);
}

#[test]
fn detects_trailing_jl1his_fskid() {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Scottie2, rate, 0.0);
    let mut phase = 0.0;
    fsk_id_signal(&mut samples, rate, &mut phase);

    let output = demodulate(&samples, rate).unwrap();

    assert_eq!(output.fsk_ids().len(), 1);
    assert_eq!(output.fsk_ids()[0].as_str(), "JL1HIS");
}

/// The transmit encoder and the receive front end agree about the contest
/// record as well as about the identifier it follows.
#[rstest]
#[case("001")]
#[case("13H")]
fn detects_a_trailing_contest_number(#[case] text: &str) {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Scottie2, rate, 0.0);
    let mut phase = 0.0;
    let id = rssstv_fskid::FskId::new("JL1HIS").unwrap();
    let number = rssstv_fskid::FskNumber::new(text).unwrap();
    for event in id.encoder_with_number(number) {
        let seconds = f64::from(event.duration_micros()) / 1_000_000.0;
        let frequency = f64::from(event.tone().frequency_hz());
        tone(&mut samples, rate, frequency, seconds, &mut phase);
    }

    let output = demodulate(&samples, rate).unwrap();

    assert_eq!(output.fsk_ids().len(), 1);
    assert_eq!(output.fsk_ids()[0].as_str(), "JL1HIS");
    assert_eq!(output.fsk_numbers().len(), 1);
    assert_eq!(output.fsk_numbers()[0].as_str(), text);
}

#[rstest]
#[case(1)]
#[case(73)]
#[case(1_024)]
fn incremental_packets_match_batch_output(#[case] packet_size: usize) {
    let rate = 8_000;
    let mut samples = vis_signal(Mode::Scottie2, rate, 0.0);
    let mut phase = 0.0;
    fsk_id_signal(&mut samples, rate, &mut phase);
    let expected = demodulate(&samples, rate).unwrap();
    let mut demodulator = Demodulator::new(rate).unwrap();
    let mut detected = Vec::new();
    let mut first_sample = None;
    let mut frequency_hz = Vec::new();
    let mut sync_strength = Vec::new();
    let mut fsk_ids = Vec::new();
    for packet in samples.chunks(packet_size) {
        let output = demodulator.process(packet).unwrap();
        detected.extend(output.detected_mode());
        if !output.frequency_hz().is_empty() {
            first_sample.get_or_insert(output.first_sample());
        }
        frequency_hz.extend_from_slice(output.frequency_hz());
        sync_strength.extend_from_slice(output.sync_strength());
        fsk_ids.extend_from_slice(output.fsk_ids());
    }
    assert_eq!(demodulator.finish().unwrap(), expected.mode());
    assert_eq!(detected, [expected.mode()]);
    assert_eq!(first_sample, Some(expected.first_sample()));
    assert_eq!(frequency_hz, expected.frequency_hz());
    assert_eq!(sync_strength, expected.sync_strength());
    assert_eq!(fsk_ids, expected.fsk_ids());
    assert_eq!(
        demodulator.process(&[]).unwrap_err().to_string(),
        DemodulatorError::AlreadyFinished.to_string()
    );
}

#[test]
fn incremental_error_reports_absolute_sample_position_without_consuming_packet() {
    let mut demodulator = Demodulator::new(8_000).unwrap();
    demodulator.process(&[0.0; 10]).unwrap();
    assert!(matches!(
        demodulator.process(&[0.0, f32::NAN]),
        Err(DemodulatorError::NonFiniteSample { index: 11 })
    ));
    assert_eq!(demodulator.next_sample(), 10);
}

/// The acquired epoch trails the protocol raster by the demodulator's own
/// group delay, which is the delay the picture samples carry as well. That
/// figure has to be the same for every mode and rate: anything mode-specific
/// left in it would displace that mode's picture horizontally.
#[rstest]
#[case(Mode::Martin2, 8_000)]
#[case(Mode::Martin2, 48_000)]
#[case(Mode::Scottie2, 8_000)]
#[case(Mode::Robot36, 8_000)]
#[case(Mode::Robot36, 48_000)]
#[case(Mode::Pd50, 8_000)]
#[case(Mode::Pd50, 48_000)]
fn aligns_causal_sync_with_frequency_output(#[case] mode: Mode, #[case] rate: u32) {
    let offset_ms = raster_epoch_error_ms(mode, rate);
    assert!(
        (offset_ms - GROUP_DELAY_MS).abs() <= 0.4,
        "raster epoch offset was {offset_ms} ms"
    );
}

#[test]
fn rejects_low_sample_rate() {
    assert!(matches!(
        demodulate(&[], 4_000),
        Err(DemodulatorError::SampleRateTooLow(4_000))
    ));
}
