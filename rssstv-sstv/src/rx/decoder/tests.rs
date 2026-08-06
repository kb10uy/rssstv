//! Checks of the raster decoder, kept beside it so they can reach its
//! internals the way the decoder's own code does.

use alloc::{vec, vec::Vec};

use proptest::prelude::*;
use rstest::rstest;

use super::*;
use crate::{signal::TxComponent, time::SstvDuration, tx::TxEncoder};

const SAMPLE_RATE: u32 = 10_000;
const VIS_END_PS: u64 = 910_000_000_000;

fn source_image(mode: Mode) -> RgbImage {
    let spec = mode.spec();
    let size = ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap();
    RgbImage::new(size, Rgb8::new(64, 128, 192))
}

fn band_image(mode: Mode) -> RgbImage {
    let spec = mode.spec();
    let size = ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap();
    let mut image = RgbImage::new(size, Rgb8::default());
    let width = size.width();
    for (index, pixel) in image.pixels_mut().iter_mut().enumerate() {
        // Two-row bands keep every chrominance pair inside one colour, so
        // subsampled modes stay exactly reproducible.
        let band = (index / width / 2) as u32;
        *pixel = Rgb8::new(
            (band * 7 % 251) as u8,
            (band * 29 % 241) as u8,
            (band * 53 % 239) as u8,
        );
    }
    image
}

fn sampled_body(mode: Mode, padding: usize) -> (Vec<f32>, Vec<f32>) {
    sampled_image_body(mode, source_image(mode), padding)
}

/// Samples between the end of the raster and the end of the stream, which a
/// live receiver always has because audio keeps arriving.
const TAIL: usize = 4_096;

fn sampled_image_body(mode: Mode, image: RgbImage, padding: usize) -> (Vec<f32>, Vec<f32>) {
    let mut frequency = vec![1900.0; padding];
    let mut sync = vec![0.0; padding];
    for tone in TxEncoder::new(mode, image).unwrap().skip(13) {
        let relative_end = tone.until().as_picos() - VIS_END_PS;
        let end_sample =
            SstvDuration::from_picos(relative_end).to_samples_ceil(SAMPLE_RATE) as usize;
        let end = padding + end_sample;
        frequency.resize(end, tone.frequency().as_hz() as f32);
        sync.resize(
            end,
            if tone.component() == TxComponent::Sync {
                1.0
            } else {
                0.0
            },
        );
    }
    frequency.resize(frequency.len() + TAIL, 1900.0);
    sync.resize(frequency.len(), 0.0);
    (frequency, sync)
}

fn mmsstv_martin2_golden(padding: usize, sample_rate_hz: u32) -> (Vec<f32>, Vec<f32>) {
    const PERIOD_PS: u64 = 226_798_000_000;
    const SEGMENTS: [(u64, u64, f32, bool); 4] = [
        (0, 4_862_000_000, 1200.0, true),
        (5_434_000_000, 78_650_000_000, 1900.0, false),
        (79_222_000_000, 152_438_000_000, 2100.0, false),
        (153_010_000_000, 226_226_000_000, 1700.0, false),
    ];
    let raster_ps = PERIOD_PS * 256;
    let samples = SstvDuration::from_picos(raster_ps).to_samples_ceil(sample_rate_hz) as usize;
    let mut frequency = vec![1500.0; padding + samples + TAIL];
    let mut sync = vec![0.0; frequency.len()];
    for unit in 0..256_u64 {
        for (start_ps, end_ps, value, is_sync) in SEGMENTS {
            let edge = |offset_ps| {
                padding
                    + SstvDuration::from_picos(unit * PERIOD_PS + offset_ps)
                        .to_samples_ceil(sample_rate_hz) as usize
            };
            let range = edge(start_ps)..edge(end_ps);
            frequency[range.clone()].fill(value);
            if is_sync {
                sync[range].fill(1.0);
            }
        }
    }
    (frequency, sync)
}

fn decode(
    mode: Mode,
    frequency: &[f32],
    sync: &[f32],
    chunks: &[usize],
) -> (RgbImage, Vec<usize>, u64) {
    let absolute_start = 40_000;
    let mut decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
    let mut offset = 0;
    let mut chunk = 0;
    let mut rows = Vec::new();
    let mut epoch = None;
    while decoder.state() != RxState::Complete {
        let count = if offset == frequency.len() {
            0
        } else {
            chunks[chunk % chunks.len()].min(frequency.len() - offset)
        };
        chunk += 1;
        let result = decoder
            .process(DemodulatedBlock::new(
                absolute_start + offset as u64,
                &frequency[offset..offset + count],
                &sync[offset..offset + count],
            ))
            .unwrap();
        offset += result.consumed();
        match result.event() {
            Some(RxEvent::RasterAcquired { source_epoch, .. }) => epoch = Some(source_epoch),
            Some(RxEvent::RowDecoded { row }) => rows.push(row),
            Some(_) => {}
            None if result.consumed() == 0 => panic!(
                "decoder made no progress: mode={mode:?} state={:?} offset={offset} len={} required_end={:?} available_end={:?}",
                decoder.state(),
                frequency.len(),
                decoder.required_end(),
                decoder.input.as_ref().map(SampleBuffer::end)
            ),
            None => {}
        }
    }
    assert!(offset <= frequency.len());
    let image = match decoder.finish() {
        RxOutcome::Complete(image) => image,
        RxOutcome::Incomplete { .. } => panic!("decoder did not complete"),
        RxOutcome::Stopped { .. } => panic!("decoder stopped"),
    };
    (image, rows, epoch.unwrap())
}

#[rstest]
#[case(Mode::Martin2)]
#[case(Mode::Scottie2)]
#[case(Mode::Robot36)]
#[case(Mode::Robot72)]
#[case(Mode::Pd50)]
fn synthetic_tx_body_decodes_each_family(#[case] mode: Mode) {
    let padding = 937;
    let (frequency, sync) = sampled_body(mode, padding);
    let (image, rows, epoch) = decode(mode, &frequency, &sync, &[17, 4093, 1, 701]);
    assert_eq!(
        rows,
        (0..mode.spec().active_rows() as usize).collect::<Vec<_>>()
    );
    assert_eq!(image.size().width(), mode.spec().width() as usize);
    assert_eq!(image.size().height(), mode.spec().height() as usize);
    let expected_epoch = 40_000 + padding as u64 + if mode == Mode::Scottie2 { 90 } else { 0 };
    assert!(epoch.abs_diff(expected_epoch) <= 1);
    assert_ne!(image.get(0, 0), Some(Rgb8::default()));
}

#[rstest]
#[case(Mode::Martin1, false)]
#[case(Mode::Martin2, false)]
#[case(Mode::Scottie2, false)]
#[case(Mode::Robot72, true)]
#[case(Mode::Pd50, true)]
#[case(Mode::Robot36, true)]
fn non_uniform_image_survives_a_transmit_receive_round_trip(
    #[case] mode: Mode,
    #[case] subsampled: bool,
) {
    let source = band_image(mode);
    let (frequency, sync) = sampled_image_body(mode, source.clone(), 512);
    let (image, _, _) = decode(mode, &frequency, &sync, &[usize::MAX]);
    let width = source.size().width();
    for row in 0..mode.spec().active_rows() as usize {
        // Each even alternating row still uses one chrominance plane from
        // the preceding pair, exactly as in the reference decoder.
        if mode.spec().raster_organization() == RasterOrganization::AlternatingYCrCb && row % 2 == 0
        {
            continue;
        }
        // Component edges are left out because the acquired sample rate
        // still carries a few parts per million of fitting error.
        for x in [width / 4, width / 2, 3 * width / 4] {
            let expected = source.get(x, row).unwrap();
            let expected = if subsampled {
                y_cr_cb_to_rgb(crate::color::rgb_to_y_cr_cb(expected))
            } else {
                expected
            };
            assert_eq!(image.get(x, row), Some(expected), "{mode:?} ({x}, {row})");
        }
    }
}

#[rstest]
#[case(Mode::Martin1, 3)]
#[case(Mode::Martin2, 2)]
fn pixel_window_averages_the_expanded_transmitted_interval(
    #[case] mode: Mode,
    #[case] expected: u64,
) {
    let decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
    let clock = RasterClock::from_estimate(0.0, f64::from(SAMPLE_RATE)).unwrap();
    let segment = decoder.segment(ScanChannel::Green, 0).unwrap();
    let (first, end) = decoder.pixel_window(clock, 0, segment, 10).unwrap();
    assert_eq!(end - first, expected);
}

#[test]
fn arbitrary_block_splits_do_not_change_output() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let contiguous = decode(Mode::Martin2, &frequency, &sync, &[usize::MAX]);
    let fragmented = decode(Mode::Martin2, &frequency, &sync, &[1, 2, 31, 997, 7]);
    assert_eq!(contiguous, fragmented);
}

#[test]
fn normal_decoding_advances_the_image_revision() {
    let (frequency, sync) = sampled_body(Mode::Robot36, 311);
    let mut decoder = RxDecoder::new(Mode::Robot36, SAMPLE_RATE).unwrap();
    let mut offset = 0;
    let mut previous_revision = 0;
    while decoder.state() != RxState::Complete {
        let result = decoder
            .process(DemodulatedBlock::new(
                offset as u64,
                &frequency[offset..],
                &sync[offset..],
            ))
            .unwrap();
        offset += result.consumed();
        if matches!(result.event(), Some(RxEvent::RowDecoded { .. })) {
            assert!(decoder.image_revision() > previous_revision);
            previous_revision = decoder.image_revision();
        }
    }
    assert_eq!(
        decoder.image_revision(),
        Mode::Robot36.spec().active_rows() as u64
    );
}

#[test]
fn mmsstv_source_golden_raster_decodes_without_the_tx_encoder() {
    let sample_rate_hz = 48_000;
    let (frequency, sync) = mmsstv_martin2_golden(503, sample_rate_hz);
    let mut decoder = RxDecoder::new(Mode::Martin2, sample_rate_hz).unwrap();
    let mut offset = 0;
    let mut rows = Vec::new();
    while decoder.state() != RxState::Complete {
        let result = decoder
            .process(DemodulatedBlock::new(
                offset as u64,
                &frequency[offset..],
                &sync[offset..],
            ))
            .unwrap();
        offset += result.consumed();
        if let Some(RxEvent::RowDecoded { row }) = result.event() {
            rows.push(row);
        }
    }
    let image = decoder.image();
    assert_eq!(rows, (0..256).collect::<Vec<_>>());
    let expected = Rgb8::new(64, 128, 192);
    assert_eq!(
        image
            .pixels()
            .iter()
            .position(|pixel| *pixel != expected)
            .map(|index| (index, image.pixels()[index])),
        None
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_valid_blocks_never_panic(
        frequency in prop::collection::vec(0.0_f32..=3_000.0, 0..8_000),
        sync in prop::collection::vec(0.0_f32..=1.0, 0..8_000),
        chunks in prop::collection::vec(1_usize..512, 1..32),
    ) {
        let len = frequency.len().min(sync.len());
        let frequency = &frequency[..len];
        let sync = &sync[..len];
        let mut decoder = RxDecoder::new(Mode::Robot36, 1_000).unwrap();
        let mut offset = 0;
        let mut chunk = 0;
        let mut steps = 0;
        while offset < len && steps < len.saturating_mul(2).saturating_add(64) {
            let end = (offset + chunks[chunk % chunks.len()]).min(len);
            let result = decoder.process(DemodulatedBlock::new(
                offset as u64,
                &frequency[offset..end],
                &sync[offset..end],
            ));
            match result {
                Ok(result) => {
                    offset += result.consumed();
                    if result.consumed() == 0 && result.event().is_none() {
                        break;
                    }
                }
                Err(_) => break,
            }
            if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
                break;
            }
            chunk += 1;
            steps += 1;
        }
    }
}

#[rstest]
#[case(Mode::Martin1, 146_432_000_000)]
#[case(Mode::Martin2, 73_216_000_000)]
#[case(Mode::Scottie1, 138_240_000_000)]
#[case(Mode::Scottie2, 88_064_000_000)]
#[case(Mode::ScottieDx, 345_600_000_000)]
#[case(Mode::Robot36, 44_000_000_000)]
#[case(Mode::Robot72, 69_000_000_000)]
#[case(Mode::Pd50, 91_520_000_000)]
#[case(Mode::Pd90, 170_240_000_000)]
#[case(Mode::Pd120, 121_600_000_000)]
#[case(Mode::Pd160, 195_584_000_000)]
#[case(Mode::Pd180, 183_040_000_000)]
#[case(Mode::Pd240, 244_480_000_000)]
#[case(Mode::Pd290, 228_800_000_000)]
fn all_supported_profiles_construct_with_exact_component_time(
    #[case] mode: Mode,
    #[case] component_ps: u64,
) {
    let decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
    assert_eq!(
        decoder
            .profile
            .pixels()
            .last()
            .map(|segment| segment.duration_ps),
        Some(component_ps)
    );
    assert_eq!(decoder.image().size().width(), mode.spec().width() as usize);
    assert_eq!(
        decoder.image().size().height(),
        mode.spec().height() as usize
    );
}

#[test]
fn support_and_sample_rate_errors_are_typed() {
    assert!(matches!(
        RxDecoder::new(Mode::Avt90, SAMPLE_RATE),
        Err(SstvError::UnsupportedRxMode(Mode::Avt90))
    ));
    assert!(matches!(
        RxDecoder::new(Mode::Martin1, 0),
        Err(SstvError::InvalidSampleRate)
    ));
}

#[test]
fn malformed_blocks_and_gaps_are_rejected() {
    let mut decoder = RxDecoder::new(Mode::Martin1, SAMPLE_RATE).unwrap();
    assert_eq!(
        decoder
            .process(DemodulatedBlock::new(0, &[1500.0], &[]))
            .unwrap_err()
            .error(),
        SstvError::DemodulatedLengthMismatch
    );
    assert_eq!(
        decoder
            .process(DemodulatedBlock::new(0, &[f32::NAN], &[0.0]))
            .unwrap_err()
            .error(),
        SstvError::InvalidDemodulatedSample { offset: 0 }
    );
    assert_eq!(
        decoder
            .process(DemodulatedBlock::new(100, &[1500.0], &[0.0]))
            .unwrap()
            .consumed(),
        1
    );
    assert_eq!(
        decoder
            .process(DemodulatedBlock::new(102, &[1500.0], &[0.0]))
            .unwrap_err()
            .error(),
        SstvError::DemodulatedGap {
            expected: 101,
            actual: 102
        }
    );
}

#[test]
fn unsuccessful_acquisition_window_is_consumed_without_error() {
    let decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
    let count = startup_window_samples(decoder.profile, SAMPLE_RATE) as usize;
    let frequency = vec![1500.0; count];
    let sync = vec![0.0; count];
    let mut decoder = decoder;
    let result = decoder
        .process(DemodulatedBlock::new(0, &frequency, &sync))
        .unwrap();
    assert_eq!(result.consumed(), count);
    assert_eq!(result.event(), None);
    assert_eq!(decoder.state(), RxState::Acquiring);
    assert!(decoder.input.as_ref().unwrap().len() < count);
}

#[test]
fn process_errors_report_the_consumed_prefix() {
    let mut decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
    let count = startup_window_samples(decoder.profile, SAMPLE_RATE) as usize;
    let mut frequency = vec![1500.0; count + 1];
    let sync = vec![0.0; count + 1];
    frequency[count] = f32::NAN;
    let error = decoder
        .process(DemodulatedBlock::new(0, &frequency, &sync))
        .unwrap_err();
    assert_eq!(error.consumed(), count);
    assert_eq!(
        error.error(),
        SstvError::InvalidDemodulatedSample { offset: count }
    );
}

#[test]
fn acquisition_recovers_after_more_than_two_periods_of_noise_and_chunked_retry() {
    let profile = RasterProfile::for_mode(Mode::Martin2).unwrap();
    let noise = SstvDuration::from_picos(profile.period_ps * 3).to_samples(SAMPLE_RATE) as usize;
    let (frequency, sync) = sampled_body(Mode::Martin2, noise);
    let (_, rows, epoch) = decode(Mode::Martin2, &frequency, &sync, &[113, 997, 29]);
    assert_eq!(rows.len(), Mode::Martin2.spec().active_rows() as usize);
    assert!(
        epoch.abs_diff(40_000 + noise as u64) <= 6,
        "epoch={epoch} expected={}",
        40_000 + noise as u64
    );
}

#[rstest]
#[case(Mode::Martin2)]
#[case(Mode::Scottie2)]
#[case(Mode::Robot36)]
#[case(Mode::Pd50)]
fn first_row_is_decoded_from_the_startup_buffer(#[case] mode: Mode) {
    let (frequency, sync) = sampled_body(mode, 0);
    let mut decoder = RxDecoder::new(mode, SAMPLE_RATE).unwrap();
    let startup = startup_window_samples(decoder.profile, SAMPLE_RATE) as usize;
    let available = startup + 1;
    let absolute_start = 40_000;
    let mut offset = 0;
    let mut first_row = None;
    for _ in 0..16 {
        let result = decoder
            .process(DemodulatedBlock::new(
                absolute_start + offset as u64,
                &frequency[offset..available],
                &sync[offset..available],
            ))
            .unwrap();
        offset += result.consumed();
        if let Some(RxEvent::RowDecoded { row }) = result.event() {
            first_row = Some(row);
            break;
        }
    }
    assert!(offset <= available);
    assert_eq!(first_row, Some(0));
}

fn drive_configured(
    mode: Mode,
    frequency: &[f32],
    sync: &[f32],
    config: RxConfig,
) -> (RxDecoder, Vec<RxEvent>) {
    let absolute_start = 70_000;
    let mut decoder = RxDecoder::with_config(mode, SAMPLE_RATE, config).unwrap();
    let mut offset = 0;
    let mut events = Vec::new();
    while !matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
        let result = decoder
            .process(DemodulatedBlock::new(
                absolute_start + offset as u64,
                &frequency[offset..],
                &sync[offset..],
            ))
            .unwrap();
        offset += result.consumed();
        if let Some(event) = result.event() {
            events.push(event);
        }
        assert!(result.consumed() != 0 || result.event().is_some());
    }
    (decoder, events)
}

fn shift_sync_runs(sync: &[f32], shift_for_run: impl Fn(usize) -> Option<usize>) -> Vec<f32> {
    let mut shifted = vec![0.0; sync.len()];
    let mut index = 0;
    let mut run = 0;
    while index < sync.len() {
        if sync[index] == 0.0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < sync.len() && sync[index] != 0.0 {
            index += 1;
        }
        if let Some(displacement) = shift_for_run(run) {
            let destination = start + displacement;
            let count = (index - start).min(sync.len().saturating_sub(destination));
            shifted[destination..destination + count].copy_from_slice(&sync[start..start + count]);
        }
        run += 1;
    }
    shifted
}

#[test]
fn staging_disabled_retains_no_full_stream_and_preserves_seam() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let (decoder, _) = drive_configured(Mode::Martin2, &frequency, &sync, RxConfig::default());
    assert_eq!(decoder.staged_samples_len(), 0);
    assert!(decoder.input.as_ref().unwrap().len() < frequency.len() / 10);
    assert_eq!(decoder.state(), RxState::Complete);
}

#[test]
fn staging_overflow_is_typed_and_does_not_grow() {
    let mut decoder = RxDecoder::with_config(
        Mode::Martin2,
        SAMPLE_RATE,
        RxConfig {
            staging: Staging::Memory { max_samples: 2 },
            ..RxConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        decoder
            .process(DemodulatedBlock::new(
                0,
                &[1900.0, 1900.0, 1900.0],
                &[0.0, 0.0, 0.0]
            ))
            .unwrap_err()
            .error(),
        SstvError::StagingCapacityExceeded { max_samples: 2 }
    );
    assert_eq!(decoder.staged_samples_len(), 0);
}

#[test]
fn completed_decoder_accepts_a_contiguous_refinement_tail() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let (mut decoder, _) = drive_configured(
        Mode::Martin2,
        &frequency,
        &sync,
        RxConfig {
            staging: Staging::Memory {
                max_samples: frequency.len() + 2,
            },
            ..RxConfig::default()
        },
    );
    let first = decoder.next_sample.unwrap();
    let staged = decoder.staged_samples_len();
    decoder
        .stage_for_refinement(DemodulatedBlock::new(first, &[1900.0, 1900.0], &[0.0, 0.0]))
        .unwrap();
    assert_eq!(decoder.staged_samples_len(), staged + 2);

    let mut acquiring = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
    assert_eq!(
        acquiring.stage_for_refinement(DemodulatedBlock::new(0, &[], &[])),
        Err(SstvError::RxNotComplete)
    );
}

/// Displaces whole sync pulses in both streams, as a timing slip does.
fn shift_sync(
    frequency: &[f32],
    sync: &[f32],
    shift_for_run: impl Fn(usize) -> Option<usize>,
) -> (Vec<f32>, Vec<f32>) {
    let mut shifted_frequency = frequency.to_vec();
    let mut shifted_sync = vec![0.0; sync.len()];
    let mut index = 0;
    let mut run = 0;
    while index < sync.len() {
        if sync[index] == 0.0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < sync.len() && sync[index] != 0.0 {
            index += 1;
        }
        if let Some(displacement) = shift_for_run(run) {
            let destination = start + displacement;
            let count = (index - start).min(sync.len().saturating_sub(destination));
            shifted_sync[destination..destination + count]
                .copy_from_slice(&sync[start..start + count]);
            let filler = if start == 0 {
                1900.0
            } else {
                frequency[start - 1]
            };
            shifted_frequency[start..destination.min(frequency.len())].fill(filler);
            shifted_frequency[destination..destination + count]
                .copy_from_slice(&frequency[start..start + count]);
        }
        run += 1;
    }
    (shifted_frequency, shifted_sync)
}

#[test]
fn live_phase_correction_is_stable_and_held_off() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let (mut frequency, mut shifted) =
        shift_sync(&frequency, &sync, |run| Some(if run < 6 { 0 } else { 4 }));
    frequency.resize(frequency.len() + 64, 1900.0);
    shifted.resize(frequency.len(), 0.0);
    let (decoder, events) = drive_configured(
        Mode::Martin2,
        &frequency,
        &shifted,
        RxConfig {
            live_sync: true,
            ..RxConfig::default()
        },
    );
    let adjustments: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RxEvent::PhaseAdjusted { unit, .. } => Some(*unit),
            _ => None,
        })
        .collect();
    assert!(!adjustments.is_empty());
    assert!(adjustments.windows(2).all(|units| units[1] - units[0] >= 6));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RxEvent::RowDecoded { .. }))
            .count(),
        Mode::Martin2.spec().active_rows() as usize
    );
    assert_eq!(decoder.sync_observations().len(), 16);
}

/// A reception that locks onto an early offset and then sees the sync return
/// corrects its raster backwards, onto samples the previous unit's decode
/// had already stepped past.
#[test]
fn a_backward_phase_correction_still_reaches_its_unit() {
    let (frequency, sync) = sampled_body(Mode::Scottie2, 311);
    let (frequency, sync) =
        shift_sync(&frequency, &sync, |run| Some(if run < 12 { 24 } else { 0 }));
    let (decoder, events) = drive_configured(
        Mode::Scottie2,
        &frequency,
        &sync,
        RxConfig {
            live_sync: true,
            ..RxConfig::default()
        },
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            RxEvent::PhaseAdjusted {
                displacement_samples,
                ..
            } if *displacement_samples < 0
        )),
        "the raster never moved backwards: {events:?}"
    );
    assert_eq!(decoder.state(), RxState::Complete);
}

#[test]
fn auto_stop_is_a_terminal_event_and_outcome() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let missing = shift_sync_runs(&sync, |run| (run < 6).then_some(0));
    let (decoder, events) = drive_configured(
        Mode::Martin2,
        &frequency,
        &missing,
        RxConfig {
            auto_stop: true,
            ..RxConfig::default()
        },
    );
    assert!(matches!(decoder.state(), RxState::Stopped { .. }));
    assert!(matches!(events.last(), Some(RxEvent::Stopped { .. })));
    assert!(matches!(decoder.finish(), RxOutcome::Stopped { .. }));
}

/// A signal that stops arriving cannot be ended by AutoStop, which only
/// scores lines that actually arrive. The caller ends it instead, and the
/// rows decoded up to that point are the whole value of what is left.
#[test]
fn an_externally_stopped_reception_keeps_its_decoded_rows() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let mut decoder = RxDecoder::with_config(
        Mode::Martin2,
        SAMPLE_RATE,
        RxConfig {
            live_sync: true,
            ..RxConfig::default()
        },
    )
    .unwrap();
    let absolute_start = 70_000;
    let mut offset = 0;
    let rows = loop {
        if let RxState::Decoding { completed_rows } = decoder.state()
            && completed_rows >= 4
        {
            break completed_rows;
        }
        let result = decoder
            .process(DemodulatedBlock::new(
                absolute_start + offset as u64,
                &frequency[offset..],
                &sync[offset..],
            ))
            .unwrap();
        offset += result.consumed();
        assert!(result.consumed() != 0 || result.event().is_some());
    };
    let revision = decoder.image_revision();
    assert!(revision > 0, "no rows were drawn before stopping");

    decoder.stop(StopReason::SynchronizationLost);

    assert_eq!(
        decoder.state(),
        RxState::Stopped {
            completed_rows: rows,
            reason: StopReason::SynchronizationLost,
        }
    );
    let mut events = Vec::new();
    while let Some(event) = decoder.poll_event() {
        events.push(event);
    }
    assert_eq!(
        events.last(),
        Some(&RxEvent::Stopped {
            reason: StopReason::SynchronizationLost
        })
    );
    assert_eq!(
        decoder.image_revision(),
        revision,
        "stopping must not disturb the image"
    );
    assert!(matches!(decoder.finish(), RxOutcome::Stopped { .. }));
}

/// Stopping a reception that already finished would rewrite a complete
/// image as a partial one.
#[test]
fn stopping_a_finished_reception_leaves_it_complete() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let (mut decoder, _) = drive_configured(Mode::Martin2, &frequency, &sync, RxConfig::default());
    assert_eq!(decoder.state(), RxState::Complete);

    decoder.stop(StopReason::SynchronizationLost);

    assert_eq!(decoder.state(), RxState::Complete);
}

#[test]
fn auto_stop_score_leaks_instead_of_resetting_after_one_good_line() {
    let mut decoder = RxDecoder::new(Mode::Martin2, SAMPLE_RATE).unwrap();
    for _ in 0..AUTO_STOP_WARMUP {
        decoder.note_bad_sync().unwrap();
    }
    assert_eq!(decoder.bad_sync_score, 0);
    for _ in 0..5 {
        decoder.note_bad_sync().unwrap();
    }
    assert_eq!(decoder.bad_sync_score, 5);
    decoder.note_good_sync();
    assert_eq!(decoder.bad_sync_score, 3);
    decoder.note_bad_sync().unwrap();
    assert_eq!(decoder.bad_sync_score, 4);
}

#[rstest]
#[case(Mode::Martin2)]
#[case(Mode::Robot36)]
#[case(Mode::Pd50)]
fn staged_rebuild_is_deterministic_and_resets_family_state(#[case] mode: Mode) {
    let (frequency, sync) = sampled_body(mode, 311);
    let (mut decoder, _) = drive_configured(
        mode,
        &frequency,
        &sync,
        RxConfig {
            staging: Staging::Memory {
                max_samples: frequency.len(),
            },
            ..RxConfig::default()
        },
    );
    let staged_len = decoder.staged_samples_len();
    let decoded_revision = decoder.image_revision();
    let first = decoder.refine_staged().unwrap();
    let first_image = decoder.image().clone();
    assert_eq!(decoder.poll_event(), None);
    assert_eq!(decoder.staged_samples_len(), staged_len);
    let second = decoder.refine_staged().unwrap();
    assert_eq!(first.slant, second.slant);
    assert_eq!(first_image, *decoder.image());
    assert_eq!(first.revision, decoded_revision + 1);
    assert_eq!(second.revision, decoded_revision + 2);
}

#[test]
fn staged_rebuild_reobserves_raw_sync_independently_of_live_tracking() {
    let (frequency, sync) = sampled_body(Mode::Pd50, 311);
    let config = RxConfig {
        staging: Staging::Memory {
            max_samples: frequency.len(),
        },
        ..RxConfig::default()
    };
    let (mut expected, _) = drive_configured(Mode::Pd50, &frequency, &sync, config);
    let (mut biased, _) = drive_configured(Mode::Pd50, &frequency, &sync, config);
    for observation in &mut biased.staged_observations {
        observation.center_sample += observation.unit as u64 * 4;
    }

    let expected_refinement = expected.refine_staged().unwrap();
    let biased_refinement = biased.refine_staged().unwrap();
    assert_eq!(biased_refinement.slant, expected_refinement.slant);
    assert_eq!(biased.image(), expected.image());
}

#[test]
fn partial_staged_rebuild_is_typed_and_preserves_image_and_state() {
    let (frequency, sync) = sampled_body(Mode::Martin2, 311);
    let limit = frequency.len();
    let mut decoder = RxDecoder::with_config(
        Mode::Martin2,
        SAMPLE_RATE,
        RxConfig {
            staging: Staging::Memory { max_samples: limit },
            ..RxConfig::default()
        },
    )
    .unwrap();
    let partial = frequency.len() / 8;
    let mut offset = 0;
    while offset < partial {
        let result = decoder
            .process(DemodulatedBlock::new(
                offset as u64,
                &frequency[offset..partial],
                &sync[offset..partial],
            ))
            .unwrap();
        offset += result.consumed();
        if result.consumed() == 0 && result.event().is_none() {
            break;
        }
    }
    assert!(decoder.staged_observations.len() >= 6);
    let image = decoder.decode.image.clone();
    let state = decoder.decode.state;
    let raster_unit = decoder.decode.raster_unit;
    let revision = decoder.image_revision;
    assert!(matches!(
        decoder.refine_staged(),
        Err(SstvError::InsufficientStagedData { .. })
    ));
    assert_eq!(decoder.decode.image, image);
    assert_eq!(decoder.decode.state, state);
    assert_eq!(decoder.decode.raster_unit, raster_unit);
    assert_eq!(decoder.image_revision, revision);
}

#[test]
fn robot36_tcs_classification_overrides_parity_and_initial_selector() {
    let (mut frequency, sync) = sampled_body(Mode::Robot36, 0);
    fill_robot36_selector(&mut frequency, 0..1, 2300.0);
    fill_robot36_selector(&mut frequency, 1..2, 1500.0);
    let mut decoder = RxDecoder::new(Mode::Robot36, SAMPLE_RATE).unwrap();
    let block = DemodulatedBlock::new(0, &frequency, &sync);
    let mut input = SampleBuffer::new(0);
    input.append(block, frequency.len());
    decoder.input = Some(input);
    decoder.decode.clock = Some(RasterClock::from_estimate(0.0, SAMPLE_RATE as f64).unwrap());
    assert_eq!(
        decoder.robot_selector_at(0).unwrap(),
        Some(RobotSelector::Cb)
    );
    assert_eq!(
        decoder.robot_selector_at(1).unwrap(),
        Some(RobotSelector::Cr)
    );
    let (_, rows, _) = decode(Mode::Robot36, &frequency, &sync, &[37, 1009, 3]);
    assert_eq!(
        rows,
        (0..Mode::Robot36.spec().active_rows() as usize).collect::<Vec<_>>()
    );
}

#[test]
fn robot36_ambiguous_selector_matches_mmsstv_initial_alternation() {
    let (mut frequency, sync) = sampled_body(Mode::Robot36, 0);
    fill_robot36_selector(&mut frequency, 0..2, 1700.0);
    let mut decoder = RxDecoder::new(Mode::Robot36, SAMPLE_RATE).unwrap();
    let block = DemodulatedBlock::new(0, &frequency, &sync);
    let mut input = SampleBuffer::new(0);
    input.append(block, frequency.len());
    decoder.input = Some(input);
    decoder.decode.clock = Some(RasterClock::from_estimate(0.0, SAMPLE_RATE as f64).unwrap());
    assert_eq!(decoder.robot_selector_at(0).unwrap(), None);
    decoder.decode_next().unwrap();
    assert_eq!(decoder.decode.robot_selector, Some(RobotSelector::Cb));
    decoder.decode_next().unwrap();
    assert_eq!(decoder.decode.robot_selector, Some(RobotSelector::Cr));
}

fn fill_robot36_selector(frequency: &mut [f32], units: impl Iterator<Item = u64>, value: f32) {
    let profile = RasterProfile::for_mode(Mode::Robot36).unwrap();
    let (start_ps, end_ps) = profile.selector_window_ps().unwrap();
    for unit in units {
        let unit_start_picos = profile.period_ps * unit;
        let sample = |offset_ps: u64| {
            SstvDuration::from_picos(unit_start_picos + offset_ps).to_samples(SAMPLE_RATE) as usize
        };
        let (start, end) = (sample(start_ps), sample(end_ps));
        if end <= frequency.len() {
            frequency[start..end].fill(value);
        }
    }
}

#[test]
fn robot36_delivers_every_active_row_when_the_selector_never_alternates() {
    let (mut frequency, sync) = sampled_body(Mode::Robot36, 311);
    fill_robot36_selector(&mut frequency, 0..240, 1500.0);
    let (image, rows, _) = decode(Mode::Robot36, &frequency, &sync, &[71, 4093, 13]);
    assert_eq!(
        rows,
        (0..Mode::Robot36.spec().active_rows() as usize).collect::<Vec<_>>()
    );
    assert_eq!(
        image.get(0, Mode::Robot36.spec().active_rows() as usize),
        None
    );
}

#[test]
fn frequency_inverse_matches_transmit_integer_mapping() {
    let decoder = RxDecoder::new(Mode::Martin1, SAMPLE_RATE).unwrap();
    let band = Mode::Martin1.spec().signal_band();
    for level in 0..=255_u8 {
        let frequency = f64::from(band.level_to_frequency(level).as_hz());
        assert_eq!(decoder.frequency_to_level(frequency), level);
    }
}

/// Renders a raster whose transmitter clock runs `offset_ppm` fast or slow,
/// which is what puts slant into a real reception.
fn mistimed_body(mode: Mode, image: RgbImage, offset_ppm: f64) -> (Vec<f32>, Vec<f32>) {
    let rate = f64::from(SAMPLE_RATE) * (1.0 + offset_ppm / 1.0e6);
    let mut frequency = vec![1900.0_f32; 0];
    let mut sync = vec![0.0_f32; 0];
    for tone in TxEncoder::new(mode, image).unwrap().skip(13) {
        let relative_end = tone.until().as_picos() - VIS_END_PS;
        let end = (relative_end as f64 * rate / 1.0e12).ceil() as usize;
        frequency.resize(end, tone.frequency().as_hz() as f32);
        sync.resize(
            end,
            if tone.component() == TxComponent::Sync {
                1.0
            } else {
                0.0
            },
        );
    }
    frequency.resize(frequency.len() + 96_000, 1900.0);
    sync.resize(frequency.len(), 0.0);
    (frequency, sync)
}

fn config(live_slant: bool, samples: usize) -> RxConfig {
    RxConfig {
        live_sync: true,
        live_slant,
        auto_stop: false,
        sync_detector_delay: crate::time::SstvDuration::ZERO,
        staging: Staging::Memory {
            max_samples: samples,
        },
    }
}

fn mean_abs_error(decoded: &RgbImage, expected: &RgbImage) -> f64 {
    let mut total = 0_u64;
    for (left, right) in decoded.pixels().iter().zip(expected.pixels()) {
        total += u64::from(left.r.abs_diff(right.r));
        total += u64::from(left.g.abs_diff(right.g));
        total += u64::from(left.b.abs_diff(right.b));
    }
    total as f64 / (expected.size().pixel_count() * 3) as f64
}

/// Live tracking must fix the raster while it is still being decoded, so
/// this asserts on the image as it stands at completion, before any staged
/// refinement runs.
#[test]
fn live_tracking_corrects_a_mistimed_raster_before_completion() {
    let mode = Mode::Martin2;
    let expected = source_image(mode);
    let (frequency, sync) = mistimed_body(mode, expected.clone(), 4_000.0);

    let (tracked, events) =
        drive_configured(mode, &frequency, &sync, config(true, frequency.len()));
    let (untracked, _) = drive_configured(mode, &frequency, &sync, config(false, frequency.len()));

    let first_adjustment = events.iter().find_map(|event| match event {
        RxEvent::SlantAdjusted { unit, .. } => Some(*unit),
        _ => None,
    });
    assert!(
        first_adjustment.is_some_and(|unit| unit <= LIVE_SLANT_MIN_UNITS),
        "live tracking did not adjust the raster at its first opportunity: {first_adjustment:?}"
    );
    let tracked_error = mean_abs_error(tracked.image(), &expected);
    let untracked_error = mean_abs_error(untracked.image(), &expected);
    assert!(
        untracked_error > 8.0,
        "the untracked reception was supposed to be slanted: {untracked_error}"
    );
    assert!(
        tracked_error < untracked_error * 0.5,
        "live tracking scored {tracked_error} against {untracked_error} untracked"
    );
}

/// A refit restates the delivery count for the rows it redrew, and the row
/// events still queued at that moment deliver afterwards. Counting a row on
/// both sides would complete the reception before its last rows, so every
/// active row has to arrive exactly once however many refits run.
#[rstest]
#[case(Mode::Martin2)]
#[case(Mode::Robot36)]
fn every_row_is_delivered_once_across_live_slant_refits(#[case] mode: Mode) {
    let (frequency, sync) = mistimed_body(mode, source_image(mode), 4_000.0);
    let (decoder, events) =
        drive_configured(mode, &frequency, &sync, config(true, frequency.len()));

    assert!(
        events
            .iter()
            .any(|event| matches!(event, RxEvent::SlantAdjusted { .. })),
        "the reception was supposed to be refitted while decoding"
    );
    let mut delivered: Vec<usize> = events
        .iter()
        .filter_map(|event| match event {
            RxEvent::RowDecoded { row } => Some(*row),
            _ => None,
        })
        .collect();
    delivered.sort_unstable();
    delivered.dedup();
    assert_eq!(delivered.len(), mode.spec().active_rows() as usize);
    assert_eq!(decoder.state(), RxState::Complete);
}

/// MMSSTV starts a reception on its calibrated sample rate, so acquisition
/// must not hand the decoder a rate fitted from the few startup periods.
#[test]
fn a_reception_starts_on_the_configured_sample_rate() {
    let mode = Mode::Martin2;
    let (frequency, sync) = mistimed_body(mode, source_image(mode), 4_000.0);
    let mut decoder =
        RxDecoder::with_config(mode, SAMPLE_RATE, config(true, frequency.len())).unwrap();
    let mut offset = 0;
    while decoder.effective_sample_rate_hz().is_none() {
        let result = decoder
            .process(DemodulatedBlock::new(
                offset as u64,
                &frequency[offset..],
                &sync[offset..],
            ))
            .unwrap();
        offset += result.consumed();
        assert!(result.consumed() != 0 || result.event().is_some());
    }
    assert_eq!(
        decoder.effective_sample_rate_hz(),
        Some(f64::from(SAMPLE_RATE))
    );
}

/// Tracking must not disturb a reception whose clock already matches.
#[test]
fn a_matched_raster_is_left_alone() {
    let mode = Mode::Martin2;
    let expected = source_image(mode);
    let (frequency, sync) = mistimed_body(mode, expected.clone(), 0.0);

    let (tracked, _) = drive_configured(mode, &frequency, &sync, config(true, frequency.len()));
    let (untracked, _) = drive_configured(mode, &frequency, &sync, config(false, frequency.len()));
    assert_eq!(tracked.image(), untracked.image());
}
