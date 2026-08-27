//! Whether two frames may be indexed against each other, and what differs
//! between them if they may.
//!
//! The split into two *types* rather than two severities is the point: a caller
//! cannot accidentally treat an [`Incompatibility`] as advisory, and cannot
//! promote a [`Mismatch`] into a refusal.
//!
//! The line between them is drawn where the arithmetic stops being **defined**,
//! not where it stops being ideal. If two frames disagree about which photosite
//! `samples[i]` is, no later correction recovers them. If they merely disagree
//! about the magnitude of a signal that is still spatially aligned, subtracting
//! one from the other is imperfect and useful, and a stacker that refuses it is
//! worse than one that does it and says so.

use astro_plugin_abi::abi::{CFA_MAX_CELLS, ImageLayout, SampleFormat};

use super::{FrameKind, FrameRecord};

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} at {},{}", self.width, self.height, self.x, self.y)
    }
}

/// The fields that decide which photosite a sample index refers to.
///
/// Everything else in [`ImageLayout`] describes what the numbers *mean* — black
/// point, white point, colour matrix, orientation — and can differ between
/// frames that still subtract correctly.
///
/// `cfa_pattern` is normalised on the way in: cells beyond `cfa_width *
/// cfa_height` are zeroed, so two frames with the same mosaic hash alike
/// whatever the plugin left in the tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GeometryKey {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub bits_per_sample: u32,
    pub sample_format: u32,
    pub active: Rect,
    pub cfa_width: u32,
    pub cfa_height: u32,
    pub cfa_pattern: [u8; CFA_MAX_CELLS],
}

impl GeometryKey {
    pub fn from_layout(layout: &ImageLayout) -> Self {
        let cells = (layout.cfa_width as usize).saturating_mul(layout.cfa_height as usize);
        let mut cfa_pattern = [0u8; CFA_MAX_CELLS];
        if cells <= CFA_MAX_CELLS {
            cfa_pattern[..cells].copy_from_slice(&layout.cfa_pattern[..cells]);
        }
        Self {
            width: layout.width,
            height: layout.height,
            components: layout.components,
            bits_per_sample: layout.bits_per_sample,
            sample_format: layout.sample_format.0,
            active: Rect {
                x: layout.active_x,
                y: layout.active_y,
                width: layout.active_width,
                height: layout.active_height,
            },
            cfa_width: layout.cfa_width,
            cfa_height: layout.cfa_height,
            cfa_pattern,
        }
    }

    /// A display name for the mosaic, e.g. `RGGB`. `None` when not mosaiced.
    pub fn cfa_name(&self) -> Option<String> {
        let mut layout = ImageLayout {
            cfa_width: self.cfa_width,
            cfa_height: self.cfa_height,
            ..Default::default()
        };
        layout.cfa_pattern = self.cfa_pattern;
        astro_plugin_abi::safe::cfa_pattern_name(&layout)
    }

    /// How the sensor reads in a report.
    pub fn describe(&self) -> String {
        let mosaic = self.cfa_name().unwrap_or_else(|| "no mosaic".to_owned());
        format!("{}x{} {} {} bit", self.width, self.height, mosaic, self.bits_per_sample)
    }
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// A difference that makes the arithmetic undefined. Always a refusal.
#[derive(Debug, Clone, PartialEq)]
pub enum Incompatibility {
    Dimensions { expected: (u32, u32), found: (u32, u32) },
    Components { expected: u32, found: u32 },
    SampleFormat { expected: u32, found: u32 },
    BitDepth { expected: u32, found: u32 },
    CfaPattern { expected: Option<String>, found: Option<String> },
    ActiveArea { expected: Rect, found: Rect },
    /// Only ever raised for a calibration pairing, never within a light set.
    CameraModel { expected: String, found: String },
    /// Only ever raised for a dark or a bias against what it calibrates.
    Gain { expected: f64, found: f64 },
    /// A rule that had to run and could not. Distinct from a match: two frames
    /// that both failed to record their gain have not been shown to agree.
    Unrecorded { property: Property },
}

impl std::fmt::Display for Incompatibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dimensions { expected, found } => write!(
                f,
                "{}x{}, expected {}x{}",
                found.0, found.1, expected.0, expected.1
            ),
            Self::Components { expected, found } => {
                write!(f, "{found} components per pixel, expected {expected}")
            }
            Self::SampleFormat { expected, found } => write!(
                f,
                "{} samples, expected {}",
                SampleFormat(*found).name(),
                SampleFormat(*expected).name()
            ),
            Self::BitDepth { expected, found } => write!(f, "{found} bit, expected {expected}"),
            Self::CfaPattern { expected, found } => write!(
                f,
                "{} mosaic, expected {}",
                found.as_deref().unwrap_or("no"),
                expected.as_deref().unwrap_or("no")
            ),
            Self::ActiveArea { expected, found } => {
                write!(f, "active area {found}, expected {expected}")
            }
            Self::CameraModel { expected, found } => write!(f, "shot on {found}, not {expected}"),
            Self::Gain { expected, found } => write!(f, "ISO {found:.0}, expected ISO {expected:.0}"),
            Self::Unrecorded { property } => {
                write!(f, "{property} was not recorded, so it could not be matched")
            }
        }
    }
}

/// A difference that changes the magnitude of a signal that is still spatially
/// aligned. Always reported, never a refusal.
#[derive(Debug, Clone, PartialEq)]
pub enum Mismatch {
    Exposure { expected: f64, found: f64 },
    Gain { expected: f64, found: f64 },
    SensorTemperature { expected: f64, found: f64, severity: Severity },
    BlackLevel { expected: f32, found: f32 },
    WhiteLevel { expected: f32, found: f32 },
    Orientation { expected: u32, found: u32 },
    CameraModel { expected: String, found: String },
    OpticalTrain { property: Property, expected: Option<f64>, found: Option<f64> },
    /// Seconds between the calibration frames and the lights they would
    /// calibrate.
    Elapsed { seconds: i64 },
    /// A rule that could not run because the data is absent.
    ///
    /// A third state, not a pass. A report that said "temperatures match" when
    /// neither frame recorded one would be a lie the user acts on — and on the
    /// hardware this project targets, neither frame ever records one.
    Unrecorded { property: Property },
}

impl Mismatch {
    pub fn severity(&self) -> Severity {
        match self {
            Self::SensorTemperature { severity, .. } => *severity,
            Self::Exposure { .. } | Self::Gain { .. } | Self::Elapsed { .. } => Severity::Warning,
            Self::BlackLevel { .. }
            | Self::WhiteLevel { .. }
            | Self::Orientation { .. }
            | Self::CameraModel { .. }
            | Self::OpticalTrain { .. }
            | Self::Unrecorded { .. } => Severity::Note,
        }
    }
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exposure { expected, found } => {
                write!(f, "exposure {found} s against {expected} s")
            }
            Self::Gain { expected, found } => write!(f, "ISO {found:.0} against ISO {expected:.0}"),
            Self::SensorTemperature { expected, found, .. } => {
                write!(f, "sensor at {found:.1} C against {expected:.1} C")
            }
            Self::BlackLevel { expected, found } => {
                write!(f, "black level {found:.1} against {expected:.1}")
            }
            Self::WhiteLevel { expected, found } => {
                write!(f, "white level {found:.0} against {expected:.0}")
            }
            Self::Orientation { expected, found } => {
                write!(f, "orientation flag {found} against {expected}; the camera may have been rotated")
            }
            Self::CameraModel { expected, found } => write!(f, "shot on {found}, not {expected}"),
            Self::OpticalTrain { property, expected, found } => {
                let show = |value: &Option<f64>| {
                    value.map_or_else(|| "not recorded".to_owned(), |v| format!("{v}"))
                };
                write!(f, "{property} {} against {}", show(found), show(expected))
            }
            Self::Elapsed { seconds } => {
                let hours = *seconds as f64 / 3600.0;
                write!(
                    f,
                    "shot {hours:.0} h apart; a flat is only valid while the optical train has not been touched"
                )
            }
            Self::Unrecorded { property } => write!(f, "{property} not recorded, not compared"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,
    Warning,
}

/// A property two frames can be compared on, named so that a finding about it
/// reads the same whether the values differed or were never recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    Exposure,
    Gain,
    SensorTemperature,
    FocalLength,
    Aperture,
    LensModel,
}

impl std::fmt::Display for Property {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Exposure => "exposure",
            Self::Gain => "ISO",
            Self::SensorTemperature => "sensor temperature",
            Self::FocalLength => "focal length",
            Self::Aperture => "aperture",
            Self::LensModel => "lens",
        };
        f.write_str(name)
    }
}

/// Which optical relationship two frames stand in.
///
/// The rules are asymmetric and the asymmetry is optical, not arbitrary: a flat
/// is normalised to unit mean before it divides anything, so its exposure
/// carries no information and refusing it for one is a bug; a dark's entire
/// content is a function of exposure and temperature, so its exposure carries
/// everything. Every check therefore names the pairing it is checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// Two frames destined for the same integration.
    SameStack,
    DarkToLight,
    BiasToLight,
    FlatToLight,
    /// Note the target: a dark flat is compared with the flats, never the
    /// lights.
    DarkFlatToFlat,
    BiasToFlat,
}

impl Pairing {
    /// The pairing under which `calibration` is judged against what it
    /// calibrates.
    pub fn for_calibration(calibration: FrameKind) -> Option<Self> {
        match calibration {
            FrameKind::Dark => Some(Self::DarkToLight),
            FrameKind::Bias => Some(Self::BiasToLight),
            FrameKind::Flat => Some(Self::FlatToLight),
            FrameKind::DarkFlat => Some(Self::DarkFlatToFlat),
            FrameKind::Light => None,
        }
    }

    /// Whether gain must match exactly, rather than merely being reported.
    ///
    /// True for darks and biases: gain scales read noise and the fixed-pattern
    /// amplitude together, so a dark at half the gain lays the pattern down at
    /// half strength and under-removes the dark current. There is no benign
    /// reason for the mismatch, and the damage is a faintly wrong background
    /// that nobody traces back to the calibration frames.
    ///
    /// False for flats, which are normalised before division, and false within
    /// a light set, where gain is a linear scale factor that normalisation
    /// removes — DeepSkyStacker stacks mixed-ISO lights into one image and only
    /// reports the result's ISO as unknown.
    fn gain_must_match(self) -> bool {
        matches!(self, Self::DarkToLight | Self::BiasToLight | Self::DarkFlatToFlat | Self::BiasToFlat)
    }

    /// Whether exposure is worth reporting on.
    ///
    /// Never for a flat: its absolute level carries no information, so a
    /// "mismatch" would be noise in the report and an invitation to a later
    /// refactor to start refusing on it.
    fn exposure_is_meaningful(self) -> bool {
        matches!(self, Self::SameStack | Self::DarkToLight | Self::DarkFlatToFlat)
    }

    /// Whether the two frames must have come off the same camera body.
    fn body_must_match(self) -> bool {
        !matches!(self, Self::SameStack)
    }
}

// ---------------------------------------------------------------------------
// Tolerances
// ---------------------------------------------------------------------------

/// Five per cent of the longer exposure.
pub const EXPOSURE_RELATIVE: f64 = 0.05;
/// Below this, a relative tolerance is meaningless: 1/8000 s and 1/7900 s are
/// the same bias exposure and differ by far more than five per cent of nothing.
pub const EXPOSURE_FLOOR_SECONDS: f64 = 0.01;
/// Dark current roughly doubles every six degrees, so two degrees is about a
/// quarter more dark current than the darks recorded. Worth saying.
pub const TEMPERATURE_NOTE_C: f64 = 2.0;
/// Five degrees is about eighty per cent more. Worth saying loudly.
pub const TEMPERATURE_WARN_C: f64 = 5.0;
/// Twelve hours: a flat from the night before was almost certainly shot after
/// the camera came off the telescope, which is the one thing that invalidates
/// it.
pub const FLAT_AGE_NOTE_SECONDS: i64 = 12 * 3600;
/// Canon bodies re-measure the black level per frame and it wanders by a few
/// units. Below this, a difference is that measurement, not a difference in
/// linearisation.
pub const BLACK_LEVEL_NOTE: f32 = 8.0;

/// The numbers that decide whether two frames are "the same".
///
/// Every one of them is this project's own choice. Not one of DeepSkyStacker,
/// Siril, PixInsight's WBPP or AstroPixelProcessor publishes a numeric
/// temperature tolerance at all, so each constant carries the reasoning that
/// produced it rather than a citation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerances {
    pub exposure_relative: f64,
    pub exposure_floor_seconds: f64,
    pub temperature_note_c: f64,
    pub temperature_warn_c: f64,
    pub flat_age_note_seconds: i64,
    pub black_level_note: f32,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            exposure_relative: EXPOSURE_RELATIVE,
            exposure_floor_seconds: EXPOSURE_FLOOR_SECONDS,
            temperature_note_c: TEMPERATURE_NOTE_C,
            temperature_warn_c: TEMPERATURE_WARN_C,
            flat_age_note_seconds: FLAT_AGE_NOTE_SECONDS,
            black_level_note: BLACK_LEVEL_NOTE,
        }
    }
}

/// Whether two exposures are the same exposure.
///
/// Relative rather than absolute, because a session in scope here runs 60 to 600
/// seconds: an absolute ten-second window would merge 30-second and 40-second
/// frames, which differ by a third in dark current, while five per cent of the
/// longer splits them and still merges 300 and 305.
pub fn same_exposure(a: f64, b: f64, tolerances: &Tolerances) -> bool {
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
        return false;
    }
    let allowed = (a.max(b) * tolerances.exposure_relative).max(tolerances.exposure_floor_seconds);
    (a - b).abs() <= allowed
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

/// Whether two frames may be indexed against each other at all.
///
/// One reason ladder, in the order a user would want to hear them: the coarsest
/// difference first, so a stray JPEG among raw frames is reported as its
/// dimensions rather than as its mosaic.
pub fn compatible(expected: &GeometryKey, found: &GeometryKey) -> Result<(), Incompatibility> {
    if (expected.width, expected.height) != (found.width, found.height) {
        return Err(Incompatibility::Dimensions {
            expected: (expected.width, expected.height),
            found: (found.width, found.height),
        });
    }
    if expected.components != found.components {
        return Err(Incompatibility::Components {
            expected: expected.components,
            found: found.components,
        });
    }
    if expected.sample_format != found.sample_format {
        return Err(Incompatibility::SampleFormat {
            expected: expected.sample_format,
            found: found.sample_format,
        });
    }
    // Same photosites, numbers differing by a factor of four: the R5 shoots
    // 14 bit on the mechanical shutter and 12 bit in some electronic modes, at
    // identical geometry. A bare "different sizes" check would never see it.
    if expected.bits_per_sample != found.bits_per_sample {
        return Err(Incompatibility::BitDepth {
            expected: expected.bits_per_sample,
            found: found.bits_per_sample,
        });
    }
    // The mosaic phase, not merely whether there is one. A half-pixel crop
    // shifts the phase by one photosite, and subtracting a dark whose phase
    // differs by one subtracts red dark current from green sites.
    if (expected.cfa_width, expected.cfa_height, expected.cfa_pattern)
        != (found.cfa_width, found.cfa_height, found.cfa_pattern)
    {
        return Err(Incompatibility::CfaPattern {
            expected: expected.cfa_name(),
            found: found.cfa_name(),
        });
    }
    if expected.active != found.active {
        return Err(Incompatibility::ActiveArea { expected: expected.active, found: found.active });
    }
    Ok(())
}

/// [`compatible`], plus the hard rules that depend on the pairing rather than on
/// the geometry.
pub fn admissible(
    expected: &FrameRecord,
    found: &FrameRecord,
    pairing: Pairing,
) -> Result<(), Incompatibility> {
    compatible(&GeometryKey::from_layout(&expected.layout), &GeometryKey::from_layout(&found.layout))?;

    // The hot-pixel map and the fixed-pattern noise belong to the individual
    // sensor. Deliberately not applied within a light set: two bodies of one
    // model are legitimately stackable, which is why none of the surveyed tools
    // puts the model in its hard test either.
    if pairing.body_must_match() && expected.body != found.body {
        return Err(Incompatibility::CameraModel {
            expected: expected.body.display(),
            found: found.body.display(),
        });
    }

    if pairing.gain_must_match() {
        match (expected.info.iso, found.info.iso) {
            (Some(expected_iso), Some(found_iso)) => {
                if (expected_iso - found_iso).abs() > f64::EPSILON {
                    return Err(Incompatibility::Gain { expected: expected_iso, found: found_iso });
                }
            }
            // Two absences are not agreement. On this project's target hardware
            // ISO is always recorded, so reaching here means something is
            // genuinely odd and saying so beats guessing.
            _ => return Err(Incompatibility::Unrecorded { property: Property::Gain }),
        }
    }

    Ok(())
}

/// Everything that differs and is worth saying, given the pairing.
///
/// Never refuses. A stacker that will not use a 295-second dark on a
/// 300-second light because a threshold said no is worse than one that uses it
/// and says so.
pub fn differences(
    expected: &FrameRecord,
    found: &FrameRecord,
    pairing: Pairing,
    tolerances: &Tolerances,
) -> Vec<Mismatch> {
    let mut findings = Vec::new();

    if pairing.exposure_is_meaningful() {
        match (expected.info.exposure_seconds, found.info.exposure_seconds) {
            (Some(a), Some(b)) if !same_exposure(a, b, tolerances) => {
                findings.push(Mismatch::Exposure { expected: a, found: b });
            }
            (Some(_), Some(_)) => {}
            _ => findings.push(Mismatch::Unrecorded { property: Property::Exposure }),
        }
    }

    // Reported, never refused, wherever it is not already a hard rule: on a
    // CMOS body ISO is analogue gain, and normalisation removes a linear scale.
    if !pairing.gain_must_match()
        && let (Some(a), Some(b)) = (expected.info.iso, found.info.iso)
        && (a - b).abs() > f64::EPSILON
    {
        findings.push(Mismatch::Gain { expected: a, found: b });
    }

    findings.extend(temperature(expected, found, tolerances));

    if !pairing.body_must_match() && expected.body != found.body {
        findings.push(Mismatch::CameraModel {
            expected: expected.body.display(),
            found: found.body.display(),
        });
    }

    if let (Some(a), Some(b)) = (mean_black_level(&expected.layout), mean_black_level(&found.layout))
        && (a - b).abs() > tolerances.black_level_note
    {
        findings.push(Mismatch::BlackLevel { expected: a, found: b });
    }
    if let (Some(a), Some(b)) = (saturation(&expected.layout), saturation(&found.layout))
        && (a - b).abs() > f32::EPSILON
    {
        findings.push(Mismatch::WhiteLevel { expected: a, found: b });
    }

    if pairing == Pairing::FlatToLight {
        findings.extend(optical_train(expected, found));
        if let (Some(a), Some(b)) = (expected.info.capture_time_unix, found.info.capture_time_unix) {
            let elapsed = (a - b).abs();
            if elapsed > tolerances.flat_age_note_seconds {
                findings.push(Mismatch::Elapsed { seconds: elapsed });
            }
        }
        // Weak evidence that the camera was rotated, which is one of the few
        // things that genuinely invalidates a flat. Never a refusal: the
        // decoder does not permute the sample buffer, so two frames with
        // different orientation flags are byte-for-byte index-identical.
        if expected.layout.orientation != 0
            && found.layout.orientation != 0
            && expected.layout.orientation != found.layout.orientation
        {
            findings.push(Mismatch::Orientation {
                expected: expected.layout.orientation,
                found: found.layout.orientation,
            });
        }
    }

    findings
}

fn temperature(
    expected: &FrameRecord,
    found: &FrameRecord,
    tolerances: &Tolerances,
) -> Option<Mismatch> {
    let (Some(a), Some(b)) = (expected.info.sensor_temperature_c, found.info.sensor_temperature_c)
    else {
        return Some(Mismatch::Unrecorded { property: Property::SensorTemperature });
    };
    let difference = (a - b).abs();
    let severity = if difference > tolerances.temperature_warn_c {
        Severity::Warning
    } else if difference > tolerances.temperature_note_c {
        Severity::Note
    } else {
        return None;
    };
    Some(Mismatch::SensorTemperature { expected: a, found: b, severity })
}

/// A flat records the illumination geometry — vignetting, dust shadows,
/// per-photosite response — and anything in the light path changing invalidates
/// it. These three are the only metadata proxies that exist and they are all
/// weak: through a telescope with no electronic lens, all three are absent,
/// which is the target use case.
fn optical_train(expected: &FrameRecord, found: &FrameRecord) -> Vec<Mismatch> {
    let mut findings = Vec::new();
    let mut compare = |property, a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) if (a - b).abs() > f64::EPSILON => {
            findings.push(Mismatch::OpticalTrain { property, expected: Some(a), found: Some(b) });
        }
        (Some(_), Some(_)) => {}
        _ => findings.push(Mismatch::Unrecorded { property }),
    };
    compare(Property::FocalLength, expected.info.focal_length_mm, found.info.focal_length_mm);
    compare(Property::Aperture, expected.info.aperture, found.info.aperture);

    if expected.info.lens_model != found.info.lens_model {
        findings.push(Mismatch::OpticalTrain {
            property: Property::LensModel,
            expected: None,
            found: None,
        });
    }
    findings
}

/// The mean of the black-level grid, which is what a comparison across two
/// differently-shaped grids can honestly mean.
fn mean_black_level(layout: &ImageLayout) -> Option<f32> {
    let cells = (layout.black_level_width as usize).checked_mul(layout.black_level_height as usize)?;
    if cells == 0 || cells > CFA_MAX_CELLS {
        return None;
    }
    let levels = &layout.black_level[..cells];
    levels
        .iter()
        .all(|level| level.is_finite())
        .then(|| levels.iter().sum::<f32>() / cells as f32)
}

/// The saturation point: the largest white level any colour plane recorded.
fn saturation(layout: &ImageLayout) -> Option<f32> {
    layout.white_level.iter().copied().filter(|level| level.is_finite()).reduce(f32::max)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, flat, light};
    use super::*;

    fn geometry(record: &FrameRecord) -> GeometryKey {
        GeometryKey::from_layout(&record.layout)
    }

    #[test]
    fn a_flat_with_a_different_exposure_from_the_lights_is_accepted() {
        // The rule most likely to be broken by a refactor that unifies the
        // pairings, and breaking it silently discards every flat in a session.
        let lights = testing::record("light.cr3", light());
        let flats = testing::record("flat.cr3", flat());

        assert!(admissible(&lights, &flats, Pairing::FlatToLight).is_ok());
        let findings = differences(&lights, &flats, Pairing::FlatToLight, &Tolerances::default());
        assert!(
            !findings.iter().any(|m| matches!(m, Mismatch::Exposure { .. })),
            "a flat must never be faulted for its exposure: {findings:?}"
        );
    }

    #[test]
    fn a_dark_at_a_different_iso_is_refused_for_the_lights_it_would_calibrate() {
        let lights = testing::record("light.cr3", light());
        let dark = testing::record("dark.cr3", testing::info(300.0, 800.0));

        let refusal = admissible(&lights, &dark, Pairing::DarkToLight).unwrap_err();
        assert!(matches!(refusal, Incompatibility::Gain { .. }), "got {refusal:?}");
    }

    #[test]
    fn mixed_iso_lights_share_a_stack() {
        // The companion to the rule above: normalisation removes analogue gain,
        // so within one integration ISO is a reporting concern, not a stacking
        // one.
        let a = testing::record("a.cr3", light());
        let b = testing::record("b.cr3", testing::info(300.0, 800.0));

        assert!(admissible(&a, &b, Pairing::SameStack).is_ok());
        let findings = differences(&a, &b, Pairing::SameStack, &Tolerances::default());
        assert!(findings.iter().any(|m| matches!(m, Mismatch::Gain { .. })), "{findings:?}");
    }

    #[test]
    fn a_dark_with_an_unrecorded_iso_is_refused_rather_than_assumed_to_match() {
        let mut lights = testing::record("light.cr3", light());
        let mut dark = testing::record("dark.cr3", light());
        lights.info.iso = None;
        dark.info.iso = None;

        let refusal = admissible(&lights, &dark, Pairing::DarkToLight).unwrap_err();
        assert_eq!(refusal, Incompatibility::Unrecorded { property: Property::Gain });
    }

    #[test]
    fn two_unknown_temperatures_do_not_report_as_matching() {
        // This is the whole target hardware: the Canon plugin reports None for
        // every frame, so an empty finding list here would tell every user
        // their darks were temperature-matched when nothing was compared.
        let lights = testing::record("light.cr3", light());
        let dark = testing::record("dark.cr3", light());
        assert_eq!(lights.info.sensor_temperature_c, None);

        let findings = differences(&lights, &dark, Pairing::DarkToLight, &Tolerances::default());
        assert!(
            findings.contains(&Mismatch::Unrecorded { property: Property::SensorTemperature }),
            "{findings:?}"
        );
    }

    #[test]
    fn temperature_is_graded_rather_than_refused() {
        let mut lights = testing::record("light.cr3", light());
        let mut dark = testing::record("dark.cr3", light());
        lights.info.sensor_temperature_c = Some(10.0);

        let tolerances = Tolerances::default();
        for (dark_temperature, expected) in
            [(11.0, None), (13.0, Some(Severity::Note)), (17.0, Some(Severity::Warning))]
        {
            dark.info.sensor_temperature_c = Some(dark_temperature);
            let findings = differences(&lights, &dark, Pairing::DarkToLight, &tolerances);
            let severity = findings.iter().find_map(|m| match m {
                Mismatch::SensorTemperature { severity, .. } => Some(*severity),
                _ => None,
            });
            assert_eq!(severity, expected, "at {dark_temperature} C: {findings:?}");
            // Whatever the gap, it never blocks the dark.
            assert!(admissible(&lights, &dark, Pairing::DarkToLight).is_ok());
        }
    }

    #[test]
    fn a_crop_mode_frame_does_not_join_a_full_frame_set() {
        let full = testing::record("full.cr3", light());
        let mut cropped = testing::record("crop.cr3", light());
        cropped.layout.active_x = 132;
        cropped.layout.active_y = 100;
        cropped.layout.active_width = 5000;
        cropped.layout.active_height = 3336;

        let refusal = compatible(&geometry(&full), &geometry(&cropped)).unwrap_err();
        let Incompatibility::ActiveArea { expected, found } = refusal else {
            panic!("expected an active-area refusal, got {refusal:?}");
        };
        assert_eq!(found, Rect { x: 132, y: 100, width: 5000, height: 3336 });
        assert_ne!(expected, found);
    }

    #[test]
    fn rggb_and_grbg_frames_of_the_same_size_do_not_share_a_set() {
        // A one-photosite phase error subtracts red dark current from green
        // sites. The geometry check alone would never see it.
        let rggb = testing::record("a.cr3", light());
        let mut grbg = testing::record("b.cr3", light());
        grbg.layout.cfa_pattern[..4].copy_from_slice(&[1, 0, 2, 1]);

        let refusal = compatible(&geometry(&rggb), &geometry(&grbg)).unwrap_err();
        assert_eq!(
            refusal,
            Incompatibility::CfaPattern {
                expected: Some("RGGB".to_owned()),
                found: Some("GRBG".to_owned()),
            }
        );
    }

    #[test]
    fn twelve_bit_and_fourteen_bit_frames_of_one_sensor_do_not_share_a_set() {
        let mechanical = testing::record("a.cr3", light());
        let mut electronic = testing::record("b.cr3", light());
        electronic.layout.bits_per_sample = 12;

        assert_eq!(
            compatible(&geometry(&mechanical), &geometry(&electronic)).unwrap_err(),
            Incompatibility::BitDepth { expected: 14, found: 12 }
        );
    }

    #[test]
    fn orientation_alone_does_not_refuse_a_frame() {
        // rawler never permutes the sample buffer, so two frames of one sensor
        // with different orientation flags are byte-for-byte index-identical.
        // Refusing here would throw away a good dark shot with the camera
        // pointed at the floor.
        let mut lights = testing::record("light.cr3", light());
        let mut flats = testing::record("flat.cr3", flat());
        lights.layout.orientation = 1;
        flats.layout.orientation = 3;

        assert!(admissible(&lights, &flats, Pairing::FlatToLight).is_ok());
        let findings = differences(&lights, &flats, Pairing::FlatToLight, &Tolerances::default());
        assert!(
            findings.contains(&Mismatch::Orientation { expected: 1, found: 3 }),
            "worth a note, though: {findings:?}"
        );
    }

    #[test]
    fn cfa_pattern_tail_bytes_do_not_split_a_set() {
        // Whatever a plugin leaves beyond cfa_width * cfa_height is not data.
        let clean = testing::record("a.cr3", light());
        let mut noisy = testing::record("b.cr3", light());
        noisy.layout.cfa_pattern[10] = 3;
        noisy.layout.cfa_pattern[35] = 2;

        assert!(compatible(&geometry(&clean), &geometry(&noisy)).is_ok());
        assert_eq!(geometry(&clean), geometry(&noisy));
    }

    #[test]
    fn exposures_are_compared_relative_to_the_longer_one() {
        let tolerances = Tolerances::default();
        assert!(same_exposure(300.0, 295.0, &tolerances));
        assert!(!same_exposure(300.0, 280.0, &tolerances));
        // Thirty and forty seconds differ by a third in dark current.
        assert!(!same_exposure(30.0, 40.0, &tolerances));
        // Two bias exposures, where a relative tolerance means nothing.
        assert!(same_exposure(1.0 / 8000.0, 1.0 / 4000.0, &tolerances));
        // Absent or nonsensical values are never "the same".
        assert!(!same_exposure(0.0, 0.0, &tolerances));
        assert!(!same_exposure(f64::NAN, 300.0, &tolerances));
    }

    #[test]
    fn a_telescope_reports_an_unrecorded_optical_train_rather_than_a_match() {
        // Through a telescope there is no electronic lens, so focal length and
        // aperture are absent on both sides. That is not agreement.
        let lights = testing::record("light.cr3", light());
        let flats = testing::record("flat.cr3", flat());
        assert_eq!(lights.info.focal_length_mm, None);

        let findings = differences(&lights, &flats, Pairing::FlatToLight, &Tolerances::default());
        assert!(
            findings.contains(&Mismatch::Unrecorded { property: Property::FocalLength }),
            "{findings:?}"
        );
    }
}
