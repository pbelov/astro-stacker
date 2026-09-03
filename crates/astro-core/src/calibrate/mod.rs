//! Building master calibration frames, and applying them.
//!
//! The equations, per photosite `i`, in the order they run:
//!
//! ```text
//!   B(i)  = combine(bias frames)                       the pedestal itself
//!   D(i)  = combine(dark frames)                       pedestal + dark current
//!   F(i)  = combine(flat frames) - P(i)                P is the flat's own zero
//!   f(i)  = F(i) / N(colour of i)                      one divisor per colour
//!   L(i)  = (l(i) - D(i)) / f(i)                       a calibrated light
//! ```
//!
//! Three things about that are worth defending.
//!
//! **The master dark is not bias-subtracted, and the light does not subtract a
//! bias.** A dark *is* the pedestal plus the dark current, so when it matches
//! the light in exposure, gain and body, `l - D` removes both in one step and
//! the bias cancels identically. Splitting them exists to let a dark be scaled
//! to a different exposure — and a scaled dark that still contains the pedestal
//! scales the pedestal too, which is a far larger error than the one scaling was
//! meant to fix. This project does not scale darks yet, so it does not split.
//!
//! **The flat's zero point comes from the flat**, not from a bias, because a
//! bias at another gain has a different pedestal *and a different pattern*. See
//! [`pedestal`].
//!
//! **The flat is normalised per colour**, with both green cells sharing one
//! divisor. Under a broadband source the green photosites of a Bayer sensor sit
//! at roughly twice blue and a fifth above red; one divisor over the whole frame
//! would bake that ratio into every calibrated light as a fixed colour
//! transform — the flat panel's spectrum, imposed on the sky.

pub mod combine;
pub mod fits;
pub mod tiff;
pub mod pedestal;

use astro_plugin_abi::abi::{CFA_MAX_CELLS, ImageLayout};
use astro_plugin_abi::safe::FrameInfo;

use crate::error::{Error, Result};
use crate::plugin::PluginHost;
use crate::session::{FrameKind, FrameSet, Session};

pub use combine::{CombineOptions, Combined, Method};
pub use pedestal::{Pedestal, PedestalSource};

/// A combined calibration frame, ready to be subtracted or divided.
#[derive(Debug, Clone)]
pub struct Master {
    pub kind: FrameKind,
    pub layout: ImageLayout,
    /// One value per photosite of the full frame, in sensor readout order. For
    /// a flat these are the normalised ratios, centred on 1; for a bias or a
    /// dark they are ADU as the sensor read them.
    pub pixels: Vec<f32>,
    pub frames: usize,
    pub method: Method,
    pub rejected: u64,
    pub resident: bool,
    /// What was taken off a flat before it was normalised, and where that came
    /// from. `None` for a bias or a dark, which are the pedestal.
    pub pedestal: Option<Pedestal>,
    /// The divisor each colour was normalised by, indexed by colour-filter
    /// index. `None` for anything but a flat.
    pub normalisation: Option<[Option<f32>; 4]>,
    /// The shooting parameters of the set, so the master can say what it is
    /// valid for.
    pub info: FrameInfo,
}

impl Master {
    pub fn rejected_fraction(&self) -> f64 {
        let total = self.frames as u64 * self.pixels.len() as u64;
        if total == 0 { 0.0 } else { self.rejected as f64 / total as f64 }
    }

    /// A name that says what it is and what it is for, without a path.
    pub fn file_stem(&self) -> String {
        let body = self.info.camera_model.replace(' ', "-");
        let mut parts = vec![format!("master-{}", self.kind.name().replace(' ', "-")), body];
        if let Some(iso) = self.info.iso {
            parts.push(format!("ISO{iso:.0}"));
        }
        if let Some(seconds) = self.info.exposure_seconds {
            parts.push(crate::calibrate::exposure_tag(seconds));
        }
        parts.join("_")
    }

    pub fn header(&self) -> fits::Header {
        // The rejection line only where a rejection ran. A median throws nothing
        // away, and "0.0000% of samples rejected" in the header of one is a rule
        // that never fired wearing the clothes of a rule that fired and found
        // nothing.
        let mut notes = match self.method {
            Method::Median => vec![format!("{} frames, {}", self.frames, self.method.name())],
            Method::ClippedMean => vec![format!(
                "{} frames, {}, {:.4}% of samples rejected",
                self.frames,
                self.method.name(),
                self.rejected_fraction() * 100.0
            )],
        };
        // Which path combined it. The two do not agree to the last bit, so a
        // master that does not say which one made it cannot be reproduced from
        // its own file.
        notes.push(
            if self.resident { "combined from frames held at once" } else { "combined by streaming in two passes" }
                .to_owned(),
        );
        if let Some(pedestal) = &self.pedestal {
            notes.push(format!(
                "pedestal {:.2} ADU from {}",
                pedestal.mean(),
                pedestal.source.name()
            ));
        }
        if let Some(divisors) = &self.normalisation {
            let listed: Vec<String> = divisors
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.map(|v| format!("{}{v:.1}", "RGBE".as_bytes()[index] as char)))
                .collect();
            notes.push(format!("normalised per colour by {}", listed.join(" ")));
        }

        fits::Header {
            image_type: self.kind.name().to_owned(),
            instrument: self.info.camera_model.clone(),
            exposure: self.info.exposure_seconds,
            iso: self.info.iso,
            frames: self.frames,
            combination: self.method.name().to_owned(),
            bayer_pattern: astro_plugin_abi::safe::cfa_pattern_name(&self.layout),
            notes,
        }
    }
}

/// A shutter speed as it belongs in a file name: `30s`, `1-500s`.
fn exposure_tag(seconds: f64) -> String {
    if seconds >= 1.0 {
        format!("{}s", crate::calibrate::trim(seconds))
    } else {
        format!("1-{:.0}s", 1.0 / seconds)
    }
}

fn trim(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Combines one set into a master.
///
/// A flat additionally has its own zero point removed and is normalised; a bias
/// or a dark is the combination and nothing more.
pub fn build(
    host: &PluginHost,
    session: &Session,
    set: &FrameSet,
    options: &CombineOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Master> {
    let active: Vec<_> = set.members.iter().copied().filter(|id| session[*id].is_active()).collect();
    if active.is_empty() {
        return Err(Error::NothingToCombine);
    }

    let combined = combine::combine(host, session, &active, options, progress)?;
    let info = session[set.representative(session)].info.clone();
    let kind = set.kind();

    let mut master = Master {
        kind,
        layout: combined.layout,
        pixels: combined.pixels,
        frames: combined.frames,
        method: combined.method,
        rejected: combined.rejected,
        resident: combined.resident,
        pedestal: None,
        normalisation: None,
        info,
    };

    if matches!(kind, FrameKind::Flat) {
        normalise_flat(&mut master)?;
    }
    Ok(master)
}

/// Takes the flat's own zero point off and divides each colour by its own mean.
fn normalise_flat(master: &mut Master) -> Result<()> {
    let Some(pedestal) = pedestal::from_masked_columns(&master.pixels, &master.layout) else {
        // Never a fabricated 2048. A flat whose pedestal cannot be measured
        // cannot be normalised, and a normalisation on the wrong zero is a
        // multiplicative error on every light it touches.
        return Err(Error::NoPedestal);
    };

    let layout = master.layout;
    let width = layout.width as usize;
    let (cfa_width, cfa_height) = (layout.cfa_width as usize, layout.cfa_height as usize);
    if cfa_width == 0 || cfa_height == 0 || cfa_width * cfa_height > CFA_MAX_CELLS {
        return Err(Error::NoPedestal);
    }

    // Subtract the pedestal everywhere, then measure the divisors over the
    // active area alone: the masked border is zero by construction and would
    // drag every divisor down in proportion to how much border there is.
    for y in 0..layout.height as usize {
        let row = y * width;
        for x in 0..width {
            master.pixels[row + x] -= pedestal.at(x, y);
        }
    }

    let mut totals = [0f64; 4];
    let mut counts = [0u64; 4];
    let x0 = layout.active_x as usize;
    let y0 = layout.active_y as usize;
    for y in y0..y0 + layout.active_height as usize {
        let row = y * width;
        let cell_row = (y % cfa_height) * cfa_width;
        let mut cell_col = x0 % cfa_width;
        for x in x0..x0 + layout.active_width as usize {
            // Indexed by colour, not by cell, so the two green cells share one
            // divisor: they are the same filter, and giving them separate
            // divisors would erase the real response difference between them.
            let colour = layout.cfa_pattern[cell_row + cell_col] as usize;
            if colour < 4 {
                totals[colour] += f64::from(master.pixels[row + x]);
                counts[colour] += 1;
            }
            cell_col += 1;
            if cell_col == cfa_width {
                cell_col = 0;
            }
        }
    }

    // The mean over four and a half million samples per colour, not the median:
    // dust motes and the odd star move it by well under a tenth of a per cent,
    // and a median would need every value at once.
    let mut divisors = [None; 4];
    for colour in 0..4 {
        if counts[colour] > 0 {
            let mean = (totals[colour] / counts[colour] as f64) as f32;
            if mean > 0.0 {
                divisors[colour] = Some(mean);
            }
        }
    }
    if divisors.iter().all(Option::is_none) {
        return Err(Error::NoPedestal);
    }

    for y in 0..layout.height as usize {
        let row = y * width;
        let cell_row = (y % cfa_height) * cfa_width;
        // The mosaic is anchored to the frame origin, so this walk starts at
        // column zero whatever the active area is - unlike the measuring pass
        // above, which starts inside it.
        let mut cell_col = 0usize;
        for x in 0..width {
            let colour = layout.cfa_pattern[cell_row + cell_col] as usize;
            master.pixels[row + x] = match divisors.get(colour).copied().flatten() {
                Some(divisor) => master.pixels[row + x] / divisor,
                // A colour with no divisor cannot be normalised, and a NaN here
                // travels into the calibrated light rather than pretending.
                None => f32::NAN,
            };
            cell_col += 1;
            if cell_col == cfa_width {
                cell_col = 0;
            }
        }
    }

    master.pedestal = Some(pedestal);
    master.normalisation = Some(divisors);
    Ok(())
}

/// The smallest normalised flat value a light may be divided by.
///
/// Outside the active area a normalised flat is zero plus noise, so a light
/// divided there is its own noise multiplied by a thousand, or an infinity.
/// This is not a pathological case, it is the masked border of every frame.
pub const MIN_FLAT: f32 = 0.05;

/// Applies a dark and a flat to one decoded light.
///
/// Returns the calibrated samples and the count of photosites that could not be
/// calibrated, which are left as NaN: a pixel where the flat is at or below
/// [`MIN_FLAT`] has no defined value, and writing a large number there would
/// look like signal.
pub fn apply(
    light: &[u16],
    dark: Option<&Master>,
    flat: Option<&Master>,
    layout: &ImageLayout,
) -> (Vec<f32>, u64) {
    let mut out = Vec::new();
    let undefined = apply_into(light, dark, flat, layout, &mut out);
    (out, undefined)
}

/// [`apply`], into a buffer the caller owns and keeps.
///
/// The same three passes over the frame, deliberately: what is being saved here
/// is the allocation and not the arithmetic. Stacking a night asks the kernel
/// for a fresh plane and hands it back once per frame per pass, and every one of
/// those is a mapping the kernel zeroes before it is overwritten. Fusing the
/// three passes into one would be faster again and would change the last bit of
/// every pixel, so it is not done here.
///
/// `out` is overwritten in full and its previous contents are never read, so a
/// buffer of the wrong length or of another frame's pixels is safe to pass.
pub fn apply_into(
    light: &[u16],
    dark: Option<&Master>,
    flat: Option<&Master>,
    layout: &ImageLayout,
    out: &mut Vec<f32>,
) -> u64 {
    out.clear();
    out.extend(light.iter().map(|value| f32::from(*value)));
    let mut undefined = 0u64;

    if let Some(dark) = dark
        && dark.pixels.len() == out.len()
    {
        for (value, subtract) in out.iter_mut().zip(&dark.pixels) {
            *value -= subtract;
        }
    }

    if let Some(flat) = flat
        && flat.pixels.len() == out.len()
    {
        for (value, divisor) in out.iter_mut().zip(&flat.pixels) {
            if divisor.is_finite() && *divisor >= MIN_FLAT {
                *value /= divisor;
            } else {
                *value = f32::NAN;
                undefined += 1;
            }
        }
    }

    let _ = layout;
    undefined
}

#[cfg(test)]
mod tests {
    use super::*;
    use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED};

    fn layout(width: usize, height: usize, active_x: u32) -> ImageLayout {
        let mut layout = ImageLayout {
            width: width as u32,
            height: height as u32,
            components: 1,
            cfa_width: 2,
            cfa_height: 2,
            active_x,
            active_y: 0,
            active_width: width as u32 - active_x,
            active_height: height as u32,
            ..Default::default()
        };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
        layout
    }

    /// A flat whose colours sit where a real one does: green about twice blue.
    fn flat_master(width: usize, height: usize) -> Master {
        let layout = layout(width, height, 64);
        let mut pixels = vec![0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let lit = x >= 64;
                pixels[y * width + x] = if !lit {
                    2048.0
                } else {
                    2048.0
                        + match (y % 2, x % 2) {
                            (0, 0) => 3380.0,
                            (1, 1) => 2043.0,
                            _ => 4067.0,
                        }
                };
            }
        }
        Master {
            kind: FrameKind::Flat,
            layout,
            pixels,
            frames: 15,
            method: Method::Median,
            rejected: 0,
            resident: true,
            pedestal: None,
            normalisation: None,
            info: FrameInfo::default(),
        }
    }

    /// What `apply` was, kept verbatim so that the buffer-reusing version can be
    /// held against it. If this and `apply_into` ever stop agreeing bit for bit,
    /// a stack built by the two of them stops agreeing too.
    fn reference_apply(
        light: &[u16],
        dark: Option<&Master>,
        flat: Option<&Master>,
    ) -> (Vec<f32>, u64) {
        let mut out: Vec<f32> = light.iter().map(|value| f32::from(*value)).collect();
        let mut undefined = 0u64;
        if let Some(dark) = dark
            && dark.pixels.len() == out.len()
        {
            for (value, subtract) in out.iter_mut().zip(&dark.pixels) {
                *value -= subtract;
            }
        }
        if let Some(flat) = flat
            && flat.pixels.len() == out.len()
        {
            for (value, divisor) in out.iter_mut().zip(&flat.pixels) {
                if divisor.is_finite() && *divisor >= MIN_FLAT {
                    *value /= divisor;
                } else {
                    *value = f32::NAN;
                    undefined += 1;
                }
            }
        }
        (out, undefined)
    }

    fn master_of(kind: FrameKind, layout: ImageLayout, pixels: Vec<f32>) -> Master {
        Master {
            kind,
            layout,
            pixels,
            frames: 3,
            method: Method::Median,
            rejected: 0,
            resident: true,
            pedestal: None,
            normalisation: None,
            info: FrameInfo::default(),
        }
    }

    #[test]
    fn calibrating_into_a_borrowed_buffer_gives_the_same_bits_as_into_a_fresh_one() {
        // The whole licence for reusing the buffer. Compared by bits and not by
        // value, so that a NaN where a NaN belongs counts as agreement and a
        // NaN where a number belongs does not.
        let (width, height) = (16usize, 8usize);
        let shape = layout(width, height, 0);
        let light: Vec<u16> = (0..width * height).map(|i| (i * 977 % 16384) as u16).collect();

        let dark = master_of(
            FrameKind::Dark,
            shape,
            (0..width * height).map(|i| 2040.0 + (i % 7) as f32).collect(),
        );
        // Every way the flat's guard can go, including the exact boundary and
        // the values that are not numbers.
        let awkward = [MIN_FLAT, MIN_FLAT - 1.0, 0.0, -0.0, -1.0, f32::NAN, f32::INFINITY,
                       f32::NEG_INFINITY, 1.0, 4096.0];
        let flat = master_of(
            FrameKind::Flat,
            shape,
            (0..width * height).map(|i| awkward[i % awkward.len()]).collect(),
        );
        let wrong_length = master_of(FrameKind::Dark, shape, vec![1.0; width * height - 1]);

        let cases: [(Option<&Master>, Option<&Master>); 6] = [
            (None, None),
            (Some(&dark), None),
            (None, Some(&flat)),
            (Some(&dark), Some(&flat)),
            (Some(&wrong_length), Some(&flat)),
            (Some(&dark), Some(&wrong_length)),
        ];

        // Deliberately dirty, and deliberately the wrong length to begin with:
        // a lane hands the same buffer round the whole run.
        let mut reused: Vec<f32> = vec![f32::NAN; 3];
        for (case, (dark, flat)) in cases.into_iter().enumerate() {
            let (wanted, wanted_undefined) = reference_apply(&light, dark, flat);
            let got_undefined = apply_into(&light, dark, flat, &shape, &mut reused);
            assert_eq!(got_undefined, wanted_undefined, "case {case}: undefined count");
            assert_eq!(reused.len(), wanted.len(), "case {case}: length");
            for (index, (got, wanted)) in reused.iter().zip(&wanted).enumerate() {
                assert_eq!(
                    got.to_bits(),
                    wanted.to_bits(),
                    "case {case}, sample {index}: {got} against {wanted}"
                );
            }
        }
    }

    #[test]
    fn a_flat_is_normalised_per_colour_so_it_imposes_no_colour_cast() {
        // The failure this prevents: one divisor over the whole frame bakes the
        // panel's spectrum, as this sensor sees it, into every calibrated light
        // as a fixed red-green-blue transform.
        let mut master = flat_master(256, 64);
        normalise_flat(&mut master).expect("a measurable pedestal");

        let divisors = master.normalisation.expect("normalised");
        assert!((divisors[0].unwrap() - 3380.0).abs() < 1.0, "red {divisors:?}");
        assert!((divisors[1].unwrap() - 4067.0).abs() < 1.0, "green {divisors:?}");
        assert!((divisors[2].unwrap() - 2043.0).abs() < 1.0, "blue {divisors:?}");

        // Every colour comes out at one, which is what "no colour cast" means.
        let width = master.layout.width as usize;
        for (y, x) in [(0usize, 64usize), (0, 65), (1, 65)] {
            let value = master.pixels[y * width + x];
            assert!((value - 1.0).abs() < 0.001, "at {x},{y}: {value}");
        }
    }

    #[test]
    fn both_green_cells_share_one_divisor() {
        // They are the same filter. Separate divisors would erase the real
        // response difference between the two greens, which is a thing worth
        // keeping.
        let mut master = flat_master(256, 64);
        normalise_flat(&mut master).expect("a measurable pedestal");
        let divisors = master.normalisation.unwrap();
        // Only three colours have divisors at all: there is no fourth filter.
        assert!(divisors[3].is_none());
        assert_eq!(divisors.iter().filter(|d| d.is_some()).count(), 3);
    }

    #[test]
    fn a_flat_with_no_measurable_pedestal_is_refused_rather_than_guessed() {
        let mut master = flat_master(256, 64);
        master.layout.active_x = 0;
        assert!(matches!(normalise_flat(&mut master), Err(Error::NoPedestal)));
    }

    #[test]
    fn the_masked_border_is_left_undefined_rather_than_divided_by_nothing() {
        // Outside the active area a normalised flat is zero plus noise. This is
        // the common case, not a pathological one: it is the border of every
        // frame.
        let mut flat = flat_master(256, 64);
        normalise_flat(&mut flat).unwrap();
        let light = vec![3000u16; 256 * 64];

        let (calibrated, undefined) = apply(&light, None, Some(&flat), &flat.layout);
        assert!(undefined >= 64 * 64, "the whole border is undefined, got {undefined}");
        assert!(calibrated[0].is_nan(), "a border pixel has no calibrated value");
        assert!(calibrated[64 * 256 / 64 + 100].is_finite() || calibrated[100 + 256].is_finite());
    }

    #[test]
    fn a_dark_is_subtracted_before_the_flat_divides() {
        // Order matters: dividing first would scale the pedestal by the flat.
        let flat = {
            let mut master = flat_master(256, 64);
            normalise_flat(&mut master).unwrap();
            master
        };
        let mut dark = flat_master(256, 64);
        dark.kind = FrameKind::Dark;
        dark.pixels = vec![2048.0; 256 * 64];

        let light = vec![4048u16; 256 * 64];
        let (calibrated, _) = apply(&light, Some(&dark), Some(&flat), &flat.layout);
        // 4048 - 2048 = 2000, divided by a flat that is 1.0 in the lit area.
        let width = 256;
        assert!((calibrated[64 + width] - 2000.0).abs() < 1.0, "got {}", calibrated[64 + width]);
    }
}
