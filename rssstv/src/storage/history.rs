use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use image::{ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder};
use image_webp::{ColorType as WebPColorType, WebPEncoder};

use crate::worker::receive::HistoryCandidate;

const JPEG_QUALITY: u8 = 90;
const XMP_JPEG_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HistoryFormat {
    #[default]
    Webp,
    Png,
    Jpeg,
}

impl HistoryFormat {
    pub const ALL: [Self; 3] = [Self::Webp, Self::Png, Self::Jpeg];

    pub const fn config_name(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    pub fn from_config(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("jpg") {
            return Some(Self::Jpeg);
        }
        Self::ALL
            .into_iter()
            .find(|format| format.config_name().eq_ignore_ascii_case(name))
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Webp => "history-format-webp",
            Self::Png => "history-format-png",
            Self::Jpeg => "history-format-jpeg",
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

pub fn save(
    directory: &Path,
    candidate: HistoryCandidate,
    format: HistoryFormat,
) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let path = history_path(directory, &candidate, format);
    let xmp = xmp_packet(&candidate);
    match format {
        HistoryFormat::Webp => save_webp(&path, candidate, xmp)?,
        HistoryFormat::Png => save_png(&path, candidate, xmp)?,
        HistoryFormat::Jpeg => save_jpeg(&path, candidate, xmp)?,
    }
    Ok(path)
}

fn history_path(directory: &Path, candidate: &HistoryCandidate, format: HistoryFormat) -> PathBuf {
    let digits = candidate
        .received_at
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect::<String>();
    let timestamp = if digits.len() == 14 {
        format!("{}-{}", &digits[..8], &digits[8..])
    } else {
        "unknown-time".to_owned()
    };
    let stem = format!("{timestamp}-{:?}", candidate.mode);
    directory.join(format!("{stem}.{}", format.extension()))
}

fn save_webp(path: &Path, candidate: HistoryCandidate, xmp: String) -> Result<(), String> {
    let writer = BufWriter::new(File::create(path).map_err(|error| error.to_string())?);
    let mut encoder = WebPEncoder::new(writer);
    encoder.set_xmp_metadata(xmp.into_bytes());
    encoder
        .encode(
            &candidate.frame.rgba,
            candidate.frame.width,
            candidate.frame.height,
            WebPColorType::Rgba8,
        )
        .map_err(|error| error.to_string())
}

fn save_png(path: &Path, candidate: HistoryCandidate, xmp: String) -> Result<(), String> {
    let writer = BufWriter::new(File::create(path).map_err(|error| error.to_string())?);
    let mut encoder = png::Encoder::new(writer, candidate.frame.width, candidate.frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_itxt_chunk("XML:com.adobe.xmp".to_owned(), xmp)
        .map_err(|error| error.to_string())?;
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&candidate.frame.rgba))
        .map_err(|error| error.to_string())
}

fn save_jpeg(path: &Path, candidate: HistoryCandidate, xmp: String) -> Result<(), String> {
    let rgb = candidate
        .frame
        .rgba
        .chunks_exact(4)
        .flat_map(|pixel| &pixel[..3])
        .copied()
        .collect::<Vec<_>>();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .write_image(
            &rgb,
            candidate.frame.width,
            candidate.frame.height,
            ExtendedColorType::Rgb8,
        )
        .map_err(|error| error.to_string())?;
    let payload_len = XMP_JPEG_HEADER.len() + xmp.len();
    let segment_len = u16::try_from(payload_len + 2)
        .map_err(|_| "XMP packet is too large for a JPEG APP1 segment".to_owned())?;
    let mut writer = BufWriter::new(File::create(path).map_err(|error| error.to_string())?);
    writer
        .write_all(&jpeg[..2])
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&[0xff, 0xe1])
        .and_then(|()| writer.write_all(&segment_len.to_be_bytes()))
        .and_then(|()| writer.write_all(XMP_JPEG_HEADER))
        .and_then(|()| writer.write_all(xmp.as_bytes()))
        .and_then(|()| writer.write_all(&jpeg[2..]))
        .map_err(|error| error.to_string())
}

fn xmp_packet(candidate: &HistoryCandidate) -> String {
    let identifiers = candidate
        .fsk_ids
        .iter()
        .map(|id| format!("<rdf:li>{}</rdf:li>", escape_xml(id)))
        .collect::<String>();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
<rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:sstv=\"https://github.com/kb10uy/rssstv/ns/1.0/\" xmp:CreatorTool=\"RSSSTV\" xmp:CreateDate=\"{}\" sstv:Mode=\"{}\">\n\
<sstv:FskId><rdf:Bag>{identifiers}</rdf:Bag></sstv:FskId>\n\
</rdf:Description>\n\
</rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>",
        escape_xml(&candidate.received_at),
        escape_xml(candidate.mode.spec().name()),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;

    use image::{
        ImageDecoder,
        codecs::{jpeg::JpegDecoder, png::PngDecoder, webp::WebPDecoder},
    };
    use rssstv_sstv::mode::Mode;
    use rstest::rstest;

    use super::*;
    use crate::{test_util::TempDir, worker::receive::Frame};

    fn candidate() -> HistoryCandidate {
        HistoryCandidate {
            mode: Mode::Robot36,
            frame: Frame {
                width: 2,
                height: 1,
                rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            },
            received_at: "2026-08-04T12:34:56+09:00".to_owned(),
            fsk_ids: vec!["JA1&ABC".to_owned(), "JA2XYZ".to_owned()],
        }
    }

    fn read_xmp(path: &Path, format: HistoryFormat) -> Vec<u8> {
        match format {
            HistoryFormat::Webp => WebPDecoder::new(BufReader::new(File::open(path).unwrap()))
                .unwrap()
                .xmp_metadata()
                .unwrap()
                .unwrap(),
            HistoryFormat::Png => PngDecoder::new(BufReader::new(File::open(path).unwrap()))
                .unwrap()
                .xmp_metadata()
                .unwrap()
                .unwrap(),
            HistoryFormat::Jpeg => JpegDecoder::new(BufReader::new(File::open(path).unwrap()))
                .unwrap()
                .xmp_metadata()
                .unwrap()
                .unwrap(),
        }
    }

    #[rstest]
    #[case(HistoryFormat::Webp, "webp")]
    #[case(HistoryFormat::Png, "png")]
    #[case(HistoryFormat::Jpeg, "jpg")]
    fn saves_each_format_with_xmp(#[case] format: HistoryFormat, #[case] extension: &str) {
        let directory = TempDir::new();

        let path = save(directory.path(), candidate(), format).unwrap();

        assert_eq!(path.extension().unwrap(), extension);
        assert_eq!(image::open(&path).unwrap().width(), 2);
        if format == HistoryFormat::Webp {
            assert!(
                fs::read(&path)
                    .unwrap()
                    .windows(4)
                    .any(|chunk| chunk == b"VP8L")
            );
        }
        let xmp = String::from_utf8(read_xmp(&path, format)).unwrap();
        assert!(xmp.contains("xmp:CreateDate=\"2026-08-04T12:34:56+09:00\""));
        assert!(xmp.contains("sstv:Mode=\"Robot 36\""));
        assert!(xmp.contains("<rdf:li>JA1&amp;ABC</rdf:li>"));
        assert!(xmp.contains("<rdf:li>JA2XYZ</rdf:li>"));
    }

    #[test]
    fn the_same_reception_second_uses_the_same_path() {
        let directory = TempDir::new();
        let first = save(directory.path(), candidate(), HistoryFormat::Webp).unwrap();
        let second = save(directory.path(), candidate(), HistoryFormat::Webp).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[rstest]
    #[case("webp", Some(HistoryFormat::Webp))]
    #[case("PNG", Some(HistoryFormat::Png))]
    #[case("jpg", Some(HistoryFormat::Jpeg))]
    #[case("jpeg", Some(HistoryFormat::Jpeg))]
    #[case("gif", None)]
    fn configuration_names_are_tolerant(
        #[case] name: &str,
        #[case] expected: Option<HistoryFormat>,
    ) {
        assert_eq!(HistoryFormat::from_config(name), expected);
    }
}
