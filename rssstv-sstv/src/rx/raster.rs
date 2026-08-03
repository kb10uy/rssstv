use crate::mode::{Mode, RasterOrganization, ScanChannel, ScanContent};
use crate::signal::TxComponent;

pub(super) const MAX_PIXEL_SEGMENTS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PixelSegment {
    pub(super) channel: ScanChannel,
    pub(super) row_offset: u8,
    pub(super) start_ps: u64,
    pub(super) duration_ps: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PixelSegments {
    items: [Option<PixelSegment>; MAX_PIXEL_SEGMENTS],
    len: usize,
}

impl PixelSegments {
    fn push(&mut self, segment: PixelSegment) -> bool {
        let Some(slot) = self.items.get_mut(self.len) else {
            return false;
        };
        *slot = Some(segment);
        self.len += 1;
        true
    }

    pub(super) fn iter(self) -> impl Iterator<Item = PixelSegment> {
        self.items.into_iter().take(self.len).flatten()
    }

    pub(super) fn get(self, channel: ScanChannel, row_offset: u8) -> Option<PixelSegment> {
        self.iter()
            .find(|segment| segment.channel == channel && segment.row_offset == row_offset)
    }

    pub(super) fn last(self) -> Option<PixelSegment> {
        self.iter().last()
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RasterProfile {
    pub(super) organization: RasterOrganization,
    pub(super) period_ps: u64,
    pub(super) sync_center_ps: u64,
    pub(super) sync_duration_ps: u64,
    pixels: PixelSegments,
    selector_ps: Option<(u64, u64)>,
}

impl RasterProfile {
    pub(super) fn for_mode(mode: Mode) -> Option<Self> {
        let scan = mode.scan();
        if scan.is_empty() {
            return None;
        }
        let mut pixels = PixelSegments::default();
        let mut selector_ps = None;
        let mut sync_duration_ps = 0;
        // Both parities of an alternating raster share one geometry, so unit
        // zero fully describes receive timing.
        for (offset, segment) in scan.offsets(0) {
            let start_ps = offset.as_picos();
            let duration_ps = segment.duration().as_picos();
            match segment.content() {
                ScanContent::Pixels {
                    channel,
                    row_offset,
                } => {
                    if !pixels.push(PixelSegment {
                        channel,
                        row_offset,
                        start_ps,
                        duration_ps,
                    }) {
                        return None;
                    }
                }
                ScanContent::Tone(_) if segment.component() == TxComponent::ChrominanceSelector => {
                    selector_ps = Some((start_ps, start_ps + duration_ps));
                }
                ScanContent::Tone(_) if segment.component() == TxComponent::Sync => {
                    if sync_duration_ps == 0 {
                        sync_duration_ps = duration_ps;
                    }
                }
                ScanContent::Tone(_) => {}
            }
        }
        pixels.last()?;
        Some(Self {
            organization: mode.spec().raster_organization(),
            period_ps: mode.spec().period().as_picos(),
            sync_center_ps: scan.sync_center(0)?.as_picos(),
            sync_duration_ps,
            pixels,
            selector_ps,
        })
    }

    pub(super) fn pixels(self) -> PixelSegments {
        self.pixels
    }

    pub(super) fn selector_window_ps(self) -> Option<(u64, u64)> {
        self.selector_ps
    }
}
