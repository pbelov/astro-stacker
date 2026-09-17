//! Turning a stack into something a person can look at.
//!
//! Everything here is a *decision about how to show* the result and never a
//! measurement of it. The FITS beside it keeps the sensor's own numbers; these
//! transforms exist because a linear, unbalanced stack is a nearly black frame
//! with a violent green cast, which is true and useless to look at.
//!
//! Shared by the command line's TIFF and by the window's preview so the two
//! cannot show the same stack differently.

use crate::frame::ImageLayout;

use crate::session::mosaic::Mosaic;

use super::stack::Stacked;

/// A stack prepared for viewing, and a sentence saying what was done to it.
pub struct Viewable {
    pub planes: Vec<Vec<f32>>,
    /// The multiplier applied to each plane, all ones where the camera recorded
    /// no white balance.
    pub multipliers: Vec<f32>,
    /// Where the background of every plane now sits, which is where a stretch
    /// must put its black point.
    pub common: f32,
    /// The brightest sample the balanced stack holds.
    pub full: f32,
    pub note: String,
}

/// White balance, then bring every plane's background to one level.
///
/// The two are separate on purpose. White balance is the camera's own
/// multiplier and belongs on any copy meant for the eye. Levelling is not: the
/// sky is not a white source — light pollution is orange, airglow green — so a
/// daylight balance leaves the background coloured, and a single black point
/// across three coloured skies renders the frame in whichever channel sits
/// highest above it. `levelled` therefore says whether it was applied, and the
/// linear copy asks for it to be left off.
pub fn for_viewing(
    stacked: &Stacked,
    layout: &ImageLayout,
    mosaic: &Mosaic,
    levelled: bool,
) -> Viewable {
    let gauge = layout.wb_coeffs.get(mosaic.dominant).copied().unwrap_or(f32::NAN);
    let usable = gauge.is_finite()
        && gauge > 0.0
        && (0..stacked.colours)
            .all(|c| layout.wb_coeffs.get(c).is_some_and(|v| v.is_finite() && *v > 0.0));

    // Not recorded, so nothing is applied and nothing is invented: a green
    // picture that says why beats a neutral one built on a guess.
    let multipliers: Vec<f32> = if usable {
        (0..stacked.planes.len()).map(|c| layout.wb_coeffs[c.min(3)] / gauge).collect()
    } else {
        vec![1.0; stacked.planes.len()]
    };

    // The pedestal was measured before the balance and has to move with it, or
    // the levelling below would subtract the wrong sky.
    let pedestal: Vec<f32> = stacked
        .pedestal
        .iter()
        .enumerate()
        .map(|(colour, level)| *level as f32 * multipliers.get(colour).copied().unwrap_or(1.0))
        .collect();
    let common = pedestal.get(mosaic.dominant).copied().unwrap_or(0.0);

    let planes: Vec<Vec<f32>> = stacked
        .planes
        .iter()
        .enumerate()
        .map(|(colour, plane)| {
            let gain = multipliers.get(colour).copied().unwrap_or(1.0);
            let own = pedestal.get(colour).copied().unwrap_or(0.0);
            plane
                .iter()
                .map(|value| {
                    let balanced = value * gain;
                    if levelled { balanced - own + common } else { balanced }
                })
                .collect()
        })
        .collect();

    let mut full = 0f32;
    for plane in &planes {
        for value in plane {
            if value.is_finite() && *value > full {
                full = *value;
            }
        }
    }

    let listed: Vec<String> = multipliers.iter().map(|m| format!("{m:.3}")).collect();
    let note = if !usable {
        "astro-stacker stack. The camera recorded no white balance, so none was applied and the \
         raw green cast is the sensor's own."
            .to_owned()
    } else if levelled {
        format!(
            "astro-stacker stack, white balanced ({}) and its background levelled between the \
             channels. For viewing only.",
            listed.join(", ")
        )
    } else {
        format!(
            "astro-stacker stack, with the camera's as-shot white balance applied ({}). The FITS \
             beside it has not been balanced.",
            listed.join(", ")
        )
    };

    Viewable { planes, multipliers, common, full, note }
}

/// One quantile of a plane, from a sample.
///
/// Used to put a stretch's black point a little below the sky rather than on
/// it: exactly on the sky clips half the noise to zero, which reads as a clean
/// background but has thrown away the faintest half of everything.
pub fn percentile(plane: &[f32], fraction: f64) -> f32 {
    let mut sample: Vec<f32> =
        plane.iter().copied().step_by(97).filter(|value| value.is_finite()).collect();
    if sample.is_empty() {
        return 0.0;
    }
    sample.sort_by(f32::total_cmp);
    sample[((sample.len() - 1) as f64 * fraction) as usize]
}

/// A downsampled RGBA image of a viewable stack, for showing on screen.
///
/// Averaged over each block rather than sampled, so a preview of a star field
/// does not lose most of its stars to the gaps between sampled pixels — which
/// is what makes a nearest-neighbour thumbnail of a sparse image look emptier
/// than the image is.
pub struct Preview {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Renders a preview with an asinh stretch, at most `longest` pixels on a side.
pub fn preview(view: &Viewable, stacked: &Stacked, longest: usize, softening: f32) -> Preview {
    let (w, h) = (stacked.canvas.width, stacked.canvas.height);
    let step = ((w.max(h) as f64 / longest.max(1) as f64).ceil() as usize).max(1);
    let (pw, ph) = (w.div_ceil(step), h.div_ceil(step));

    let black = percentile(&view.planes[stacked.colours.min(view.planes.len()) - 1], 0.02)
        .min(view.common);
    let span = view.full - black;
    let stretch = |value: f32| -> u8 {
        if !value.is_finite() || span <= 0.0 || span.is_nan() || softening <= 0.0 {
            return 0;
        }
        let unit = (((value - black) / span) * softening).asinh() / softening.asinh();
        (unit.clamp(0.0, 1.0) * 255.0).round() as u8
    };

    let mut rgba = vec![255u8; pw * ph * 4];
    for py in 0..ph {
        for px in 0..pw {
            for channel in 0..3 {
                let plane = &view.planes[channel.min(view.planes.len() - 1)];
                let (mut total, mut count) = (0f32, 0usize);
                for y in py * step..((py + 1) * step).min(h) {
                    for x in px * step..((px + 1) * step).min(w) {
                        let value = plane[y * w + x];
                        if value.is_finite() {
                            total += value;
                            count += 1;
                        }
                    }
                }
                // A block no frame reached stays black rather than borrowing a
                // neighbour's value.
                rgba[(py * pw + px) * 4 + channel] =
                    if count > 0 { stretch(total / count as f32) } else { 0 };
            }
        }
    }
    Preview { width: pw, height: ph, rgba }
}
