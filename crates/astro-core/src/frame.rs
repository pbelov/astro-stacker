/// Decoded sample data for one frame, still in the format the plugin produced.
///
/// Kept in the sensor's native representation rather than converted to `f32` on
/// the spot: a 45-megapixel frame is 90 MB as `u16` and 180 MB as `f32`, and a
/// stack is hundreds of frames. Conversion happens per tile, when calibration
/// actually needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Self::U16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Smallest and largest sample. `None` for an empty frame.
    ///
    /// Cheap enough to be worth having on the raw data: a light frame whose
    /// maximum never approaches the white level is under-exposed, and one whose
    /// minimum sits at the black level everywhere is a lens cap.
    pub fn range(&self) -> Option<(f64, f64)> {
        match self {
            Self::U16(v) => v
                .iter()
                .fold(None, |acc: Option<(u16, u16)>, &s| {
                    Some(acc.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))))
                })
                .map(|(lo, hi)| (lo as f64, hi as f64)),
            Self::F32(v) => v
                .iter()
                .copied()
                .filter(|s| s.is_finite())
                .fold(None, |acc: Option<(f32, f32)>, s| {
                    Some(acc.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))))
                })
                .map(|(lo, hi)| (lo as f64, hi as f64)),
        }
    }
}
