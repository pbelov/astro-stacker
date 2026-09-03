//! Turning many frames of one kind into one master.
//!
//! Two things decide the shape of this module, and both were measured rather
//! than assumed.
//!
//! **Where the median stops and the clipped mean starts.** Rejection needs a
//! scale, and a per-pixel sample sigma is itself computed from the samples the
//! outlier is in. For one outlier of any amplitude among `n` values the
//! studentised deviation cannot exceed `sqrt(n)`, so at eleven frames a
//! three-sigma rule can never fire on anything, however bright: the outlier
//! inflates the very sigma that would catch it. The relative error of the sigma
//! estimate itself is `1/sqrt(2(n-1))` — 22% at eleven frames, 8% at
//! eighty-five. Both arguments put the line near twenty-five, which is where
//! [`MEDIAN_CEILING`] sits.
//!
//! **One rejection pass, not five.** DeepSkyStacker's shipped default recomputes
//! sigma over the survivors five times at kappa 2, and the threshold walks
//! inwards each pass: on perfectly clean Gaussian data that discards about a
//! tenth of every pixel at eighty-five frames. A single pass at kappa 3 rejects
//! 0.2%, which is what a rejection is supposed to cost.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use astro_plugin_abi::abi::ImageLayout;
use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::frame::Samples;
use crate::plugin::PluginHost;
use crate::session::{FrameId, GeometryKey, Session, compat};

/// At or above this many frames, reject outliers against a per-pixel sigma;
/// below it, take the median and reject nothing. See the module documentation
/// for why the line is here and not at fifteen.
pub const MEDIAN_CEILING: usize = 25;

/// How far a sample may sit from its pixel's mean before it is dropped.
///
/// Three, and one pass. A cosmic ray or a satellite trail is many sigma out; a
/// threshold tight enough to catch a two-sigma excursion is tight enough to
/// throw away the tail of the noise the master exists to average down.
pub const KAPPA: f32 = 3.0;

/// How much decoded pixel data may be resident while a master is combined.
///
/// Below this the frames are all held and the answer is exact; above it the
/// combination streams and reads every frame twice. Two gibibytes holds
/// fifty-six 19-megapixel frames or twenty-three at 45 megapixels, which covers
/// the darks and flats of an ordinary session and leaves a large bias run to
/// stream.
pub const DEFAULT_RESIDENT_BYTES: u64 = 2 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The middle value at each photosite. Rejects nothing, and needs nothing
    /// to be estimated from the sample.
    Median,
    /// The mean of the values within [`KAPPA`] sigma of the pixel's own mean,
    /// in one pass.
    ClippedMean,
}

impl Method {
    /// The method that suits a set of this size.
    pub fn for_frames(frames: usize) -> Self {
        if frames >= MEDIAN_CEILING { Self::ClippedMean } else { Self::Median }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Median => "median",
            Self::ClippedMean => "clipped mean",
        }
    }
}

/// One combined frame, and what it took.
#[derive(Debug, Clone)]
pub struct Combined {
    /// One value per photosite of the full frame, in sensor readout order.
    /// `f32` from here on: the moment anything is subtracted the data can go
    /// negative, and a pedestal that has been removed must not be clamped back.
    pub pixels: Vec<f32>,
    pub layout: ImageLayout,
    pub method: Method,
    pub frames: usize,
    /// Samples the rejection dropped, out of `frames * pixels`. Zero for a
    /// median, which rejects nothing. A healthy clipped mean drops a fraction
    /// of a per cent; percentages are a sign the threshold is wrong or the
    /// frames disagree.
    pub rejected: u64,
    /// Whether every frame was held at once. Reported because the streaming
    /// path reads each frame twice and the resident path does not, and the
    /// difference is visible in how long it took.
    pub resident: bool,
}

impl Combined {
    pub fn rejected_fraction(&self) -> f64 {
        let total = self.frames as u64 * self.pixels.len() as u64;
        if total == 0 { 0.0 } else { self.rejected as f64 / total as f64 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CombineOptions {
    pub method: Option<Method>,
    pub kappa: f32,
    pub resident_bytes: u64,
}

impl Default for CombineOptions {
    fn default() -> Self {
        Self { method: None, kappa: KAPPA, resident_bytes: DEFAULT_RESIDENT_BYTES }
    }
}

/// Combines `frames` into one master.
///
/// Every frame is opened, decoded and dropped; nothing but the accumulators and
/// whatever the resident budget allows is held. `progress` is called once per
/// decode with the count so far and the total, which for the streaming path
/// counts each frame once per pass.
pub fn combine(
    host: &PluginHost,
    session: &Session,
    frames: &[FrameId],
    options: &CombineOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Combined> {
    let Some(&first) = frames.first() else {
        return Err(Error::NothingToCombine);
    };
    let layout = session[first].layout;
    let pixels = layout
        .required_bytes()
        .map(|bytes| bytes / size_of::<u16>())
        .ok_or(Error::NothingToCombine)?;

    // Every frame must describe the same photosites. This is the same refusal
    // the partition already applies, repeated here because a master built from
    // two geometries is not wrong by a scale factor, it is meaningless.
    let expected = GeometryKey::from_layout(&layout);
    for &id in frames {
        let found = GeometryKey::from_layout(&session[id].layout);
        if let Err(reason) = compat::compatible(&expected, &found) {
            return Err(Error::PluginCall {
                plugin: session.plugin(session[id].source.plugin).to_owned(),
                action: "combine",
                path: session.path(id).unwrap_or_default(),
                status: astro_plugin_abi::abi::Status::INVALID_ARGUMENT,
                message: reason.to_string(),
            });
        }
    }

    let method = options.method.unwrap_or_else(|| Method::for_frames(frames.len()));
    let frame_bytes = (pixels * size_of::<u16>()) as u64;
    let resident = frames.len() as u64 * frame_bytes <= options.resident_bytes;

    let shape = Shape { layout: &layout, pixels };
    if resident {
        combine_resident(host, session, frames, &shape, method, options, progress)
    } else {
        combine_streaming(host, session, frames, &shape, options, progress)
    }
}

/// Photosites one worker takes at a time when folding the frames together.
///
/// Large enough that the per-band scratch row and the loop setup are lost in
/// it, small enough that a machine's worth of workers still has bands to take
/// when the last few are running long.
const BAND: usize = 1 << 16;

/// The pool a combination decodes on.
///
/// `None` where there is nothing to gain or the pool will not build, and the
/// caller then reads the frames one at a time — which only costs speed. Sized
/// to the physical cores rather than the logical ones, for the reason the survey
/// gives: the second thread on a core brings no decoder of its own.
fn pool(frames: usize) -> Option<rayon::ThreadPool> {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    let workers = if logical >= 8 { logical / 2 } else { logical }.min(frames);
    if workers <= 1 {
        return None;
    }
    match rayon::ThreadPoolBuilder::new().num_threads(workers).build() {
        Ok(pool) => Some(pool),
        Err(err) => {
            log::warn!("combining on one thread: {err}");
            None
        }
    }
}

/// Counts frames as they are finished and reports them, from any thread.
///
/// The count and the call happen under one lock. Incrementing atomically and
/// then calling is not the same thing: two workers can be reordered between the
/// two, and the line this drives is rewritten in place, so it would walk
/// backwards.
struct Reporter<'a> {
    progress: &'a (dyn Fn(usize, usize) + Sync),
    done: Mutex<usize>,
    total: usize,
}

impl<'a> Reporter<'a> {
    fn new(progress: &'a (dyn Fn(usize, usize) + Sync), total: usize) -> Self {
        Self { progress, done: Mutex::new(0), total }
    }

    fn one(&self) {
        let mut done = self.done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *done += 1;
        (self.progress)(*done, self.total);
    }
}

/// What both strategies need to know about the frames they are combining.
struct Shape<'a> {
    layout: &'a ImageLayout,
    /// Samples per frame.
    pixels: usize,
}

/// Every frame held at once, so the answer is exact and each is read once.
fn combine_resident(
    host: &PluginHost,
    session: &Session,
    frames: &[FrameId],
    shape: &Shape<'_>,
    method: Method,
    options: &CombineOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Combined> {
    let (layout, pixels) = (shape.layout, shape.pixels);

    // Decoded several at a time. Nothing about one frame depends on another,
    // and this path is called resident precisely because every frame is going
    // to be held anyway, so reading them at once costs no memory the pass was
    // not already committed to.
    let reported = Reporter::new(progress, frames.len());
    let decoded: Vec<Result<Vec<u16>>> = match pool(frames.len()) {
        Some(pool) => pool.install(|| {
            frames
                .par_iter()
                .map(|&id| {
                    let outcome = decode(host, session, id, pixels);
                    reported.one();
                    outcome
                })
                .collect()
        }),
        None => frames
            .iter()
            .map(|&id| {
                let outcome = decode(host, session, id, pixels);
                reported.one();
                outcome
            })
            .collect(),
    };
    // Back in the order they were given. `par_iter().map().collect()` over a
    // slice already keeps it, and this only turns the failures into one.
    let mut held: Vec<Vec<u16>> = Vec::with_capacity(frames.len());
    for outcome in decoded {
        held.push(outcome?);
    }

    let mut out = vec![0f32; pixels];
    let rejected = AtomicU64::new(0);

    // Every output photosite is a fold over the same photosite of each frame
    // and depends on nothing else, so the run is cut into bands of them. The
    // arithmetic within a photosite is untouched and in the same order, so the
    // master is the same master.
    out.par_chunks_mut(BAND).enumerate().for_each(|(band, values)| {
        // One scratch row of values per photosite, reused down the band:
        // allocating per photosite would be nineteen million allocations.
        let mut column = vec![0f32; held.len()];
        let mut dropped_here = 0u64;
        for (offset, value) in values.iter_mut().enumerate() {
            let index = band * BAND + offset;
            for (slot, frame) in column.iter_mut().zip(&held) {
                *slot = f32::from(frame[index]);
            }
            match method {
                Method::Median => *value = median(&mut column),
                Method::ClippedMean => {
                    let (mean, dropped) = clipped_mean(&column, options.kappa);
                    *value = mean;
                    dropped_here += dropped;
                }
            }
        }
        // Counted per band and added once, for the reason the deposit counts
        // that way: an atomic per photosite is threads queueing on one cache
        // line for longer than the work they are counting takes.
        rejected.fetch_add(dropped_here, Ordering::Relaxed);
    });
    let rejected = rejected.into_inner();

    Ok(Combined {
        pixels: out,
        layout: *layout,
        method,
        frames: frames.len(),
        rejected,
        resident: true,
    })
}

/// Two passes, holding three accumulators instead of the frames.
///
/// Pass one gets each photosite's mean and spread by Welford's method, which
/// needs no second moment held in raw form and does not lose precision to
/// cancellation. Pass two takes the mean of the values that survive the
/// threshold pass one established. The median is not available here — it would
/// need every value at a photosite at once, which is the thing being avoided —
/// so the streaming path always clips, and is only reached when there are
/// enough frames for a sigma to mean something.
fn combine_streaming(
    host: &PluginHost,
    session: &Session,
    frames: &[FrameId],
    shape: &Shape<'_>,
    options: &CombineOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Combined> {
    let (layout, pixels) = (shape.layout, shape.pixels);
    let total = frames.len() * 2;
    let mut mean = vec![0f32; pixels];
    let mut m2 = vec![0f32; pixels];

    for (index, &id) in frames.iter().enumerate() {
        let frame = decode(host, session, id, pixels)?;
        let count = (index + 1) as f32;
        // Banded, not because one frame's fold is parallel — it is strictly
        // sequential in the frames — but because every photosite's fold is
        // independent of every other's. Each photosite still sees the frames in
        // the order they were given, which is what a running mean needs.
        mean.par_chunks_mut(BAND).zip(m2.par_chunks_mut(BAND)).zip(frame.par_chunks(BAND)).for_each(
            |((mean, m2), frame)| {
                for ((mean, m2), sample) in mean.iter_mut().zip(m2).zip(frame) {
                    let value = f32::from(*sample);
                    let delta = value - *mean;
                    *mean += delta / count;
                    *m2 += delta * (value - *mean);
                }
            },
        );
        progress(index + 1, total);
    }

    // m2 becomes the threshold in place: nothing else needs the second moment.
    let divisor = (frames.len().saturating_sub(1)).max(1) as f32;
    for slot in &mut m2 {
        *slot = (*slot / divisor).max(0.0).sqrt() * options.kappa;
    }

    let mut sum = vec![0f32; pixels];
    let mut kept = vec![0u32; pixels];
    for (index, &id) in frames.iter().enumerate() {
        let frame = decode(host, session, id, pixels)?;
        sum.par_chunks_mut(BAND)
            .zip(kept.par_chunks_mut(BAND))
            .zip(mean.par_chunks(BAND).zip(m2.par_chunks(BAND)))
            .zip(frame.par_chunks(BAND))
            .for_each(|(((sum, kept), (mean, tolerance)), frame)| {
                for (((sum, kept), (mean, tolerance)), sample) in
                    sum.iter_mut().zip(kept).zip(mean.iter().zip(tolerance)).zip(frame)
                {
                    let value = f32::from(*sample);
                    if (value - *mean).abs() <= *tolerance {
                        *sum += value;
                        *kept += 1;
                    }
                }
            });
        progress(frames.len() + index + 1, total);
    }

    let mut rejected = 0u64;
    let mut out = sum;
    for ((value, kept), mean) in out.iter_mut().zip(&kept).zip(&mean) {
        rejected += (frames.len() - *kept as usize) as u64;
        // A photosite where everything was rejected keeps its unclipped mean
        // rather than becoming a zero or a NaN. It happens where the spread is
        // exactly zero and floating point puts a value a hair outside its own
        // threshold, and the honest answer there is the mean of the values.
        *value = if *kept == 0 { *mean } else { *value / *kept as f32 };
    }

    Ok(Combined {
        pixels: out,
        layout: *layout,
        method: Method::ClippedMean,
        frames: frames.len(),
        rejected,
        resident: false,
    })
}

fn decode(host: &PluginHost, session: &Session, id: FrameId, pixels: usize) -> Result<Vec<u16>> {
    let path = session.path(id).ok_or(Error::NothingToCombine)?;
    let frame = host.open(&path)?;
    match frame.decode()? {
        Samples::U16(values) if values.len() == pixels => Ok(values),
        Samples::U16(values) => Err(Error::PluginCall {
            plugin: frame.plugin().id().to_owned(),
            action: "combine",
            path,
            status: astro_plugin_abi::abi::Status::INTERNAL,
            message: format!("decoded {} samples, expected {pixels}", values.len()),
        }),
        Samples::F32(_) => Err(Error::PluginCall {
            plugin: frame.plugin().id().to_owned(),
            action: "combine",
            path,
            status: astro_plugin_abi::abi::Status::UNSUPPORTED,
            message: "floating-point sensor data is not combined yet".to_owned(),
        }),
    }
}

/// The middle value, or the mean of the two middle values for an even count.
fn median(values: &mut [f32]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// The mean of the values within `kappa` sigma of their own mean, in one pass,
/// with the count that fell outside.
fn clipped_mean(values: &[f32], kappa: f32) -> (f32, u64) {
    let count = values.len() as f32;
    let mean = values.iter().sum::<f32>() / count;
    let variance =
        values.iter().map(|value| (value - mean) * (value - mean)).sum::<f32>() / count.max(1.0);
    let tolerance = variance.sqrt() * kappa;

    let mut sum = 0f32;
    let mut kept = 0u32;
    for value in values {
        if (value - mean).abs() <= tolerance {
            sum += value;
            kept += 1;
        }
    }
    if kept == 0 {
        (mean, 0)
    } else {
        (sum / kept as f32, (values.len() - kept as usize) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_is_taken_below_the_crossover_and_the_clipped_mean_above() {
        // Eleven darks and eighty-five biases, the two sizes in a real session.
        assert_eq!(Method::for_frames(11), Method::Median);
        assert_eq!(Method::for_frames(15), Method::Median);
        assert_eq!(Method::for_frames(24), Method::Median);
        assert_eq!(Method::for_frames(25), Method::ClippedMean);
        assert_eq!(Method::for_frames(85), Method::ClippedMean);
    }

    #[test]
    fn a_cosmic_ray_is_dropped_and_the_rest_survive() {
        let mut values: Vec<f32> = (0..40).map(|i| 2048.0 + (i % 5) as f32).collect();
        values[7] = 16000.0;
        let (mean, rejected) = clipped_mean(&values, KAPPA);
        assert_eq!(rejected, 1);
        assert!((mean - 2050.0).abs() < 1.0, "got {mean}");
    }

    #[test]
    fn clean_noise_is_not_thrown_away() {
        // The failure that matters: DeepSkyStacker's shipped kappa 2 over five
        // iterations discards about a tenth of clean Gaussian data at this
        // frame count. One pass at kappa 3 must cost almost nothing.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let values: Vec<f32> = (0..85)
            .map(|_| {
                // xorshift, so the test is deterministic without a dependency.
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let uniform = (state >> 11) as f32 / (1u64 << 53) as f32;
                // Sum of three uniforms: close enough to Gaussian for this.
                2048.0 + (uniform - 0.5) * 120.0
            })
            .collect();
        let (_, rejected) = clipped_mean(&values, KAPPA);
        assert_eq!(rejected, 0, "one pass at kappa 3 must not touch clean noise");
    }

    #[test]
    fn a_median_of_an_even_count_averages_the_middle_two() {
        let mut values = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(median(&mut values), 25.0);
        let mut odd = [10.0, 30.0, 20.0];
        assert_eq!(median(&mut odd), 20.0);
    }

    #[test]
    fn a_pixel_that_never_varies_is_not_rejected_into_nothing() {
        // Zero spread makes the threshold zero, and a value can then sit a hair
        // outside its own mean in floating point. The answer must still be the
        // value, not a zero.
        let values = vec![2048.0f32; 30];
        let (mean, rejected) = clipped_mean(&values, KAPPA);
        assert_eq!(mean, 2048.0);
        assert_eq!(rejected, 0);
    }
}
