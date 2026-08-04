use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavSpec, WavWriter};
use image::imageops::FilterType;
use jiff::{Zoned, tz::TimeZone};
use rssstv_fskid::FskId;
use rssstv_modulator::Modulator;
use rssstv_sstv::{
    TransmissionEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::{Mode, Support},
};
use rssstv_template::{
    AssetError, AssetProvider, EncodedAsset, RenderContext, RenderSize, Renderer, Template,
    VariableValue, Variables, composite,
};

const SAMPLE_RATE_HZ: u32 = 48_000;
const PCM_PEAK: f32 = 24_578.0;
const PCM_BLOCK_SIZE: usize = 1_024;

/// Summary of a completed WAV transmission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodeReport {
    /// Selected SSTV mode.
    pub mode: Mode,
    /// Normalized station identifier used by the template and FSKID.
    pub callsign: String,
    /// Number of mono PCM samples written.
    pub samples_written: u64,
}

/// Parses a supported transmit mode, ignoring ASCII case and separators.
pub fn parse_mode(value: &str) -> Result<Mode> {
    let normalized = normalize_mode_name(value);
    Mode::ALL
        .into_iter()
        .filter(|mode| mode.spec().encode_support() == Support::Supported)
        .find(|mode| normalize_mode_name(mode.spec().name()) == normalized)
        .ok_or_else(|| {
            let supported = Mode::ALL
                .into_iter()
                .filter(|mode| mode.spec().encode_support() == Support::Supported)
                .map(|mode| mode.spec().name())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("unsupported transmit mode `{value}`; supported modes: {supported}")
        })
}

/// Composes a template and background and streams one complete SSTV transmission to WAV.
pub fn encode_file(
    template_path: &Path,
    background_path: &Path,
    mode: Mode,
    output_path: &Path,
    callsign: &str,
) -> Result<EncodeReport> {
    if mode.spec().encode_support() != Support::Supported {
        bail!(
            "transmit encoding is not implemented for {}",
            mode.spec().name()
        );
    }
    let callsign = callsign.trim().to_ascii_uppercase();
    let station_id = FskId::new(&callsign).context("invalid callsign")?;
    let template_source = fs::read_to_string(template_path)
        .with_context(|| format!("failed to read template {}", template_path.display()))?;
    let template = Template::parse(&template_source)
        .with_context(|| format!("failed to parse template {}", template_path.display()))?;
    let background = load_background(background_path, mode)?;
    let frame = render_frame(&template, template_path, &background, &callsign)?;
    let transmission = TransmissionEncoder::new(mode, frame, station_id)?;
    let mut modulator = Modulator::new(transmission, SAMPLE_RATE_HZ)?;
    let mut writer = WavWriter::create(
        output_path,
        WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE_HZ,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .with_context(|| format!("failed to create WAV file {}", output_path.display()))?;
    let mut block = [0.0_f32; PCM_BLOCK_SIZE];
    let mut samples_written = 0_u64;
    loop {
        let count = modulator.process(&mut block)?;
        if count == 0 {
            break;
        }
        for &sample in &block[..count] {
            writer
                .write_sample((sample.clamp(-1.0, 1.0) * PCM_PEAK) as i16)
                .context("failed to write PCM sample")?;
        }
        samples_written += count as u64;
    }
    writer
        .finalize()
        .with_context(|| format!("failed to finalize WAV file {}", output_path.display()))?;
    Ok(EncodeReport {
        mode,
        callsign,
        samples_written,
    })
}

fn normalize_mode_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn load_background(path: &Path, mode: Mode) -> Result<RgbImage> {
    let decoded = image::open(path)
        .with_context(|| format!("failed to open background image {}", path.display()))?
        .to_rgb8();
    let prepared = cover_image(
        &decoded,
        u32::from(mode.spec().width()),
        u32::from(mode.spec().height()),
    );
    let size = ImageSize::new(prepared.width() as usize, prepared.height() as usize)?;
    let pixels = prepared
        .pixels()
        .map(|pixel| Rgb8::new(pixel[0], pixel[1], pixel[2]))
        .collect();
    Ok(RgbImage::from_pixels(size, pixels)?)
}

fn cover_image(source: &image::RgbImage, target_width: u32, target_height: u32) -> image::RgbImage {
    let source_width = source.width();
    let source_height = source.height();
    let (resized_width, resized_height) = if u64::from(target_width) * u64::from(source_height)
        >= u64::from(target_height) * u64::from(source_width)
    {
        (
            target_width,
            u32::try_from(
                (u64::from(source_height) * u64::from(target_width))
                    .div_ceil(u64::from(source_width)),
            )
            .expect("resized image height fits u32"),
        )
    } else {
        (
            u32::try_from(
                (u64::from(source_width) * u64::from(target_height))
                    .div_ceil(u64::from(source_height)),
            )
            .expect("resized image width fits u32"),
            target_height,
        )
    };
    let resized =
        image::imageops::resize(source, resized_width, resized_height, FilterType::Lanczos3);
    let left = (resized_width - target_width) / 2;
    let top = (resized_height - target_height) / 2;
    image::imageops::crop_imm(&resized, left, top, target_width, target_height).to_image()
}

fn render_frame(
    template: &Template,
    template_path: &Path,
    background: &RgbImage,
    callsign: &str,
) -> Result<RgbImage> {
    let size = RenderSize::new(
        u32::try_from(background.size().width())?,
        u32::try_from(background.size().height())?,
    )?;
    let assets = TemplateAssetProvider {
        base: template_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
    };
    let variables = template_variables(callsign);
    let mut context = RenderContext::new(&variables, &assets);
    context.received_image = Some(background);
    let mut renderer = Renderer::new();
    renderer.load_system_fonts();
    let overlay = renderer.render(template, size, &context)?;
    Ok(composite(background, &overlay)?)
}

/// The values a template may read here.
///
/// The command names one station and one image, so the station callsign and
/// the time the file is being written are all there is to offer; a template
/// built for the desktop application may well ask for more and be refused.
/// The encode stands in for a transmission, so it dates one.
fn template_variables(callsign: &str) -> Variables {
    let mut variables = Variables::new();
    variables.insert("station.callsign", VariableValue::Text(callsign.to_owned()));
    let now = Zoned::now();
    variables.insert(
        "tx.timestamp.utc",
        VariableValue::Timestamp(now.with_time_zone(TimeZone::UTC)),
    );
    variables.insert("tx.timestamp.local", VariableValue::Timestamp(now));
    variables
}

struct TemplateAssetProvider {
    base: PathBuf,
}

impl AssetProvider for TemplateAssetProvider {
    fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
        let path = self.base.join(reference);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(EncodedAsset::png(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AssetError::new(format!(
                "failed to read {}: {error}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use decode_wav::{DecodeStatus, decode_file};
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("robot36", Mode::Robot36)]
    #[case("Robot 36", Mode::Robot36)]
    #[case("scottie-dx", Mode::ScottieDx)]
    #[case("PD_120", Mode::Pd120)]
    fn parses_normalized_mode_names(#[case] value: &str, #[case] expected: Mode) {
        assert_eq!(parse_mode(value).unwrap(), expected);
    }

    #[test]
    fn cover_resize_crops_from_the_center() {
        let source = image::RgbImage::from_fn(4, 2, |x, _| image::Rgb([x as u8, 0, 0]));
        let result = cover_image(&source, 2, 2);
        assert_eq!(result.dimensions(), (2, 2));
        assert!(result[(0, 0)][0] > 0);
        assert!(result[(1, 0)][0] < 3);
    }

    #[test]
    fn supplies_callsign_template_variable() {
        assert_eq!(
            template_variables("N0CALL").get("station.callsign"),
            Some(&VariableValue::Text("N0CALL".to_owned()))
        );
    }

    #[test]
    fn generated_wav_round_trips_with_callsign() {
        let unique = format!("encode-wav-{}", std::process::id());
        let directory = std::env::temp_dir().join(unique);
        fs::create_dir_all(&directory).unwrap();
        let template = directory.join("template.kdl");
        let background = directory.join("background.png");
        let wav = directory.join("transmission.wav");
        let decoded = directory.join("decoded.png");
        fs::write(
            &template,
            r#"rximage {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)100 fit="stretch"
}
"#,
        )
        .unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([80, 140, 200]))
            .save(&background)
            .unwrap();

        let report = encode_file(&template, &background, Mode::Robot36, &wav, " n0call ").unwrap();
        assert_eq!(report.callsign, "N0CALL");
        let reader = hound::WavReader::open(&wav).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE_HZ);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(u64::from(reader.duration()), report.samples_written);

        let decoded_report = decode_file(&wav, &decoded).unwrap();
        assert_eq!(decoded_report.mode, Mode::Robot36);
        assert_eq!(decoded_report.status, DecodeStatus::Complete);
        assert!(
            decoded_report
                .fsk_ids
                .iter()
                .any(|id| id.as_str() == "N0CALL")
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
