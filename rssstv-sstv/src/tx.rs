//! Streaming encoders for the conventional-VIS SSTV modes supported by RSSSTV.

mod image;
mod transmission;

pub use image::TxEncoder;
pub use transmission::TransmissionEncoder;

/// Picoseconds in one millisecond, which is what MMSSTV states timings in.
pub(crate) const PS_PER_MS: u64 = 1_000_000_000;

/// When conventional VIS framing ends and the first segment begins.
pub(crate) const VIS_END_PS: u64 = 910 * PS_PER_MS;

#[cfg(test)]
pub(crate) fn test_image(
    mode: crate::mode::Mode,
    fill: crate::image::Rgb8,
) -> crate::image::RgbImage {
    let spec = mode.spec();
    crate::image::RgbImage::new(
        crate::image::ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap(),
        fill,
    )
}
