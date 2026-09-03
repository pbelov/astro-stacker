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

/// How much a combination may hold, where the machine will not say how much
/// memory it has.
///
/// Below this the frames are all held and each is read once; above it the
/// combination streams and reads every frame twice.
pub const FALLBACK_RESIDENT_BYTES: u64 = 2 << 30;

/// The share of the machine's memory a combination may work in.
///
/// An eighth, and deliberately a small one. This is cut from what the machine
/// *has* rather than from what is free at the moment — see
/// [`crate::machine::total_bytes`] for why the more useful number is the wrong
/// one — so the share has to leave room for everything the run is doing besides
/// this combination, including the masters it has already finished and is still
/// holding.
const RESIDENT_SHARE: u64 = 8;

/// What one photosite of a held frame costs beyond the frames themselves, as a
/// multiple of a frame's raw bytes.
///
/// Two for the `f32` output the fold writes into, and two more for each frame a
/// worker is decoding at the time. A budget that counted only the frames it
/// means to hold would grant a share and then exceed it, which is the failure
/// the streaming path exists to prevent — so the test is made against what the
/// pass actually peaks at.
const OUTPUT_MULTIPLE_OF_RAW: u64 = 2;
const DECODE_MULTIPLE_OF_RAW: u64 = 2;

/// What this machine allows a combination to hold.
pub fn resident_budget_bytes() -> u64 {
    crate::machine::total_bytes()
        .map_or(FALLBACK_RESIDENT_BYTES, |total| (total / RESIDENT_SHARE).max(FALLBACK_RESIDENT_BYTES))
}

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
        Self { method: None, kappa: KAPPA, resident_bytes: resident_budget_bytes() }
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
    // A set that is to be combined by the median is held whatever the budget
    // says. The streaming path cannot take a median — it would need every value
    // at a photosite at once, which is the thing it exists to avoid — so a
    // budget that pushed such a set into it would change the master's method
    // without being asked to, and that is a decision about the arithmetic rather
    // than about memory.
    let held = if matches!(method, Method::Median) {
        frames.len() as u64
    } else {
        MEDIAN_CEILING as u64
    };
    let floor = held.saturating_mul(frame_bytes);
    // What the resident path actually peaks at, rather than only the frames it
    // means to hold: the fold's own output, and a decode in flight per worker.
    let workers = decode_workers(frames.len()) as u64;
    let overhead =
        (OUTPUT_MULTIPLE_OF_RAW + workers.saturating_mul(DECODE_MULTIPLE_OF_RAW)) * frame_bytes;
    let wanted = (frames.len() as u64).saturating_mul(frame_bytes).saturating_add(overhead);
    let resident = wanted <= options.resident_bytes.max(floor.saturating_add(overhead));

    let shape = Shape { layout: &layout, pixels };
    if resident {
        combine_resident(host, session, frames, &shape, method, options, progress)
    } else {
        // Never a median: the floor above holds any set that is to be combined
        // by one, so this arm is only ever reached for a clipped mean. Asserted
        // rather than trusted, because the streaming path has no way to honour a
        // median and used to answer with a clipped mean instead.
        debug_assert!(matches!(method, Method::ClippedMean), "a median cannot be streamed");
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
fn decode_workers(frames: usize) -> usize {
    let cores = crate::machine::physical_cores().unwrap_or_else(|| {
        let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
        if logical >= 8 { logical / 2 } else { logical }
    });
    cores.min(frames).max(1)
}

fn pool(frames: usize) -> Option<rayon::ThreadPool> {
    let workers = decode_workers(frames);
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
    // Over `n - 1`, not `n`. The mean subtracted here was estimated from these
    // same values, so a degree of freedom has already been spent on it and
    // dividing by the count understates the spread — which tightens the
    // threshold and drops samples that are not outliers. It is also the
    // estimator this module's own reasoning is written in: the relative error
    // quoted at the top is `1/sqrt(2(n-1))`.
    //
    // And it has to match [`combine_streaming`], which has always divided by
    // `n - 1`. The two combine the same frames and which one runs is decided by
    // whether the set fits in memory, so while they disagreed, how much memory
    // was free decided how much of a set was rejected.
    let variance = values.iter().map(|value| (value - mean) * (value - mean)).sum::<f32>()
        / (count - 1.0).max(1.0);
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
        // A photosite where everything was rejected keeps its unclipped mean
        // rather than becoming a zero or a NaN, as the streaming path does — but
        // it is reported as wholly rejected, because it was. Saying nothing was
        // dropped here would hide the one condition the rejection percentage
        // exists to expose, and would disagree with the other path about a set
        // it is only running instead of because of a memory budget.
        (mean, values.len() as u64)
    } else {
        (sum / kept as f32, (values.len() - kept as usize) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One photosite through the streaming path, written out longhand.
    ///
    /// Not a second opinion about what the answer should be — a transcription of
    /// what `combine_streaming` does to one photosite, so that the two paths can
    /// be held against each other without decoding a frame.
    fn streaming_at_one_photosite(values: &[f32], kappa: f32) -> (f32, u64) {
        let (mut mean, mut m2) = (0f32, 0f32);
        for (index, value) in values.iter().enumerate() {
            let count = (index + 1) as f32;
            let delta = value - mean;
            mean += delta / count;
            m2 += delta * (value - mean);
        }
        let divisor = (values.len().saturating_sub(1)).max(1) as f32;
        let tolerance = (m2 / divisor).max(0.0).sqrt() * kappa;

        let (mut sum, mut kept) = (0f32, 0u32);
        for value in values {
            if (value - mean).abs() <= tolerance {
                sum += value;
                kept += 1;
            }
        }
        let combined = if kept == 0 { mean } else { sum / kept as f32 };
        (combined, (values.len() - kept as usize) as u64)
    }

    #[test]
    fn both_paths_reject_the_same_samples() {
        // The property that matters, and the one that was broken: which path a
        // set takes is decided by whether it fits in memory, so if the two
        // disagree then how much memory was free decides how much of a set is
        // rejected. They cannot be bit-identical — one sums and the other
        // accumulates a running mean — but they must throw away the same
        // samples, which is the decision a master is actually made of.
        let mut state = 0x853c_49e6_748f_ea9bu64;
        let mut noise = move || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (state >> 40) as f32 / 8_388_608.0 - 1.0
        };
        for count in [2usize, 3, 11, 25, 40, 85, 200] {
            for outliers in [0usize, 1, 3] {
                let mut values: Vec<f32> =
                    (0..count).map(|_| 2050.0 + noise() * 12.0).collect();
                for index in 0..outliers.min(count) {
                    values[index * 7 % count] += 4000.0;
                }
                let (mine, dropped) = clipped_mean(&values, KAPPA);
                let (theirs, dropped_streaming) = streaming_at_one_photosite(&values, KAPPA);
                assert_eq!(
                    dropped, dropped_streaming,
                    "{count} values, {outliers} outliers: the paths rejected different counts"
                );
                // Loose, because the two accumulate differently. What is being
                // asserted is that they agree about the answer, not that they
                // reach it the same way; the counts above are the strict part.
                assert!(
                    (mine - theirs).abs() <= 0.01,
                    "{count} values, {outliers} outliers: {mine} against {theirs}"
                );
            }
        }
    }

    #[test]
    fn the_paths_agree_where_nothing_survives_the_clip() {
        // A photosite where the clip keeps nothing. A zero kappa reaches it on
        // purpose; in a real run it is a photosite whose spread is zero and
        // whose values floating point puts a hair outside their own threshold.
        // Both paths fall back to the plain mean there, and both have to say the
        // same thing about it — a photosite reported as wholly rejected by one
        // path and not at all by the other moves the rejection percentage the
        // user reads, and that percentage is how a bad threshold is spotted.
        let values: Vec<f32> = (0..40).map(|i| 2048.0 + i as f32).collect();
        let (mine, dropped) = clipped_mean(&values, 0.0);
        let (theirs, dropped_streaming) = streaming_at_one_photosite(&values, 0.0);
        assert_eq!(mine, theirs, "the value written");
        assert_eq!(dropped, dropped_streaming, "the count reported");
    }

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
