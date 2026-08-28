//! Writing a result as 16-bit TIFF, for programs that do not read FITS.
//!
//! Hand-rolled for the same reason the FITS writer is: the part of TIFF needed
//! here is a header, one image file directory of fifteen tags, and uncompressed
//! samples. A crate that reads the whole of TIFF — with its tiles, its
//! compressions and its twenty photometric interpretations — would be more
//! surface than the format it hides.
//!
//! **The FITS is the measurement and this is the copy.** A stack is 32-bit float
//! in ADU, and no 16-bit integer file can hold that: the range is wider and the
//! faint end is finer than 65536 steps. So this format exists to be opened and
//! looked at, and every one of the choices below follows from that being its
//! only job.
//!
//! **Linear by default, and the scale is written into the file.** A linear
//! 16-bit TIFF of a deep-sky stack looks black on screen, because it is: the
//! sky sits a few hundred ADU above zero and the nebula a few tens above that.
//! That is the honest rendering and the one another program can undo, since
//! `ImageDescription` carries the divisor. A stretch is available, and says in
//! the same tag that it was applied — a stretched file is for the eye and a
//! measurement taken from it would be wrong.
//!
//! **A pixel no frame covered is written as zero, and the description says so.**
//! TIFF has no NaN. The coverage is not lost — it is in the FITS, where an
//! uncovered pixel is still NaN — but a reader of the TIFF alone cannot tell an
//! uncovered corner from a genuinely black one, and must be told.

use std::io::{BufWriter, Write};
use std::path::Path;

/// How 32-bit ADU become 16-bit samples.
#[derive(Debug, Clone, Copy)]
pub enum Mapping {
    /// `sample = value / full * 65535`, clipped at both ends.
    ///
    /// Reversible: a reader that divides by 65535 and multiplies by `full` has
    /// the ADU back, to within the quantisation. This is the one to hand to
    /// another program.
    Linear {
        /// The ADU value that becomes full scale.
        full: f32,
    },
    /// An inverse hyperbolic sine stretch, for looking at.
    ///
    /// Chosen over a gamma or a logarithm because it is linear near zero and
    /// logarithmic far from it, so it lifts the faint end without turning the
    /// noise around the black point into a wall of grey — and unlike a
    /// logarithm it is defined at and below zero, which matters because a
    /// sky-subtracted stack has plenty of pixels there.
    Asinh {
        /// The ADU that becomes black. Usually the sky level.
        black: f32,
        /// The ADU that becomes white.
        white: f32,
        /// How hard the lift is. Larger is gentler; 1 is nearly linear.
        softening: f32,
    },
}

impl Mapping {
    fn apply(&self, value: f32) -> u16 {
        // A pixel no frame covered has no value, and TIFF cannot say so. Zero
        // is what it gets; the module documentation and the file's own
        // description both say that is what happened.
        if !value.is_finite() {
            return 0;
        }
        let unit = match *self {
            Mapping::Linear { full } => {
                if full > 0.0 {
                    value / full
                } else {
                    0.0
                }
            }
            Mapping::Asinh { black, white, softening } => {
                let span = white - black;
                if span.is_nan() || span <= 0.0 || softening.is_nan() || softening <= 0.0 {
                    0.0
                } else {
                    let scaled = (value - black) / span;
                    // Normalised by the stretch of full scale, so that white
                    // stays white whatever the softening.
                    (scaled * softening).asinh() / softening.asinh()
                }
            }
        };
        (unit.clamp(0.0, 1.0) * 65_535.0).round() as u16
    }

    fn describe(&self) -> String {
        match *self {
            Mapping::Linear { full } => format!(
                "linear: ADU = sample / 65535 * {full}. Uncovered pixels are written as zero."
            ),
            Mapping::Asinh { black, white, softening } => format!(
                "asinh stretch for viewing only, black {black} white {white} softening \
                 {softening} ADU; NOT linear, do not measure from this file. Uncovered \
                 pixels are written as zero."
            ),
        }
    }
}

/// One image: one plane per channel, each `width * height` in readout order.
pub struct Image<'a> {
    pub width: usize,
    pub height: usize,
    pub planes: &'a [&'a [f32]],
}

const HEADER: usize = 8;
/// Rows are grouped so a strip lands near this size, which keeps a reader from
/// having to hold the whole image to decode one band of it.
const STRIP_TARGET: usize = 1 << 23;

/// Writes an uncompressed 16-bit TIFF.
///
/// One plane is written as greyscale, three as RGB. Any other count is an
/// error rather than a guess about what the caller meant.
pub fn write(path: &Path, image: &Image, mapping: &Mapping, note: &str) -> std::io::Result<()> {
    let samples = image.planes.len();
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    if samples != 1 && samples != 3 {
        return Err(invalid(format!("{samples} planes is neither greyscale nor RGB")));
    }
    let pixels = image.width * image.height;
    if pixels == 0 {
        return Err(invalid(format!("a {}x{} image", image.width, image.height)));
    }
    if let Some(plane) = image.planes.iter().find(|plane| plane.len() != pixels) {
        return Err(invalid(format!(
            "{} samples for a {}x{} plane",
            plane.len(),
            image.width,
            image.height
        )));
    }

    let row_bytes = image.width * samples * 2;
    let rows_per_strip = (STRIP_TARGET / row_bytes.max(1)).clamp(1, image.height);
    let strips = image.height.div_ceil(rows_per_strip);
    let data_bytes = row_bytes * image.height;

    // The samples go first and the directory after them, which is legal — the
    // header points at the directory explicitly — and removes a circularity:
    // the strip offsets would otherwise depend on the size of the directory
    // that holds them.
    let description = format!("{note}{}{}", if note.is_empty() { "" } else { " " }, mapping.describe());
    let software = concat!("astro-stacker ", env!("CARGO_PKG_VERSION"));

    let mut entries: Vec<Entry> = Vec::new();
    entries.push(Entry::long(256, &[image.width as u32]));
    entries.push(Entry::long(257, &[image.height as u32]));
    entries.push(Entry::short(258, &vec![16u16; samples]));
    entries.push(Entry::short(259, &[1])); // uncompressed
    entries.push(Entry::short(262, &[if samples == 3 { 2 } else { 1 }]));
    entries.push(Entry::ascii(270, &description));
    entries.push(Entry::long(
        273,
        &(0..strips)
            .map(|strip| (HEADER + strip * rows_per_strip * row_bytes) as u32)
            .collect::<Vec<_>>(),
    ));
    entries.push(Entry::short(277, &[samples as u16]));
    entries.push(Entry::long(278, &[rows_per_strip as u32]));
    entries.push(Entry::long(
        279,
        &(0..strips)
            .map(|strip| {
                let rows = rows_per_strip.min(image.height - strip * rows_per_strip);
                (rows * row_bytes) as u32
            })
            .collect::<Vec<_>>(),
    ));
    entries.push(Entry::rational(282, 72, 1));
    entries.push(Entry::rational(283, 72, 1));
    entries.push(Entry::short(296, &[2])); // inches
    entries.push(Entry::ascii(305, software));
    entries.push(Entry::short(339, &vec![1u16; samples])); // unsigned integer

    // Tags must be in ascending order, and a reader is entitled to binary
    // search them.
    entries.sort_by_key(|entry| entry.tag);

    let directory = HEADER + data_bytes.next_multiple_of(2);
    let directory_bytes = 2 + entries.len() * 12 + 4;
    let mut overflow = directory + directory_bytes;
    for entry in &mut entries {
        if entry.payload.len() > 4 {
            entry.offset = Some(overflow as u32);
            overflow += entry.payload.len().next_multiple_of(2);
        }
    }

    let file = std::fs::File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);

    out.write_all(b"II")?;
    out.write_all(&42u16.to_le_bytes())?;
    out.write_all(&(directory as u32).to_le_bytes())?;

    // Interleaved, which is what `PlanarConfiguration` defaults to and what
    // every reader handles without thinking about it.
    let mut buffer: Vec<u8> = Vec::with_capacity(row_bytes);
    for y in 0..image.height {
        buffer.clear();
        for x in 0..image.width {
            for plane in image.planes {
                buffer.extend_from_slice(&mapping.apply(plane[y * image.width + x]).to_le_bytes());
            }
        }
        out.write_all(&buffer)?;
    }
    if data_bytes % 2 == 1 {
        out.write_all(&[0])?;
    }

    out.write_all(&(entries.len() as u16).to_le_bytes())?;
    for entry in &entries {
        entry.write(&mut out)?;
    }
    out.write_all(&0u32.to_le_bytes())?; // no next directory

    for entry in &entries {
        if entry.offset.is_some() {
            out.write_all(&entry.payload)?;
            if entry.payload.len() % 2 == 1 {
                out.write_all(&[0])?;
            }
        }
    }

    out.flush()
}

/// One directory entry, with its value already rendered.
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    payload: Vec<u8>,
    /// Where the payload was put, for one that does not fit in the four bytes
    /// the entry itself holds.
    offset: Option<u32>,
}

impl Entry {
    fn short(tag: u16, values: &[u16]) -> Self {
        let payload = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        Self { tag, kind: 3, count: values.len() as u32, payload, offset: None }
    }

    fn long(tag: u16, values: &[u32]) -> Self {
        let payload = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        Self { tag, kind: 4, count: values.len() as u32, payload, offset: None }
    }

    fn rational(tag: u16, numerator: u32, denominator: u32) -> Self {
        let mut payload = numerator.to_le_bytes().to_vec();
        payload.extend_from_slice(&denominator.to_le_bytes());
        Self { tag, kind: 5, count: 1, payload, offset: None }
    }

    fn ascii(tag: u16, text: &str) -> Self {
        // TIFF strings are NUL-terminated and the terminator is counted.
        let mut payload = text.as_bytes().to_vec();
        payload.push(0);
        Self { tag, kind: 2, count: payload.len() as u32, payload, offset: None }
    }

    fn write(&self, out: &mut impl Write) -> std::io::Result<()> {
        out.write_all(&self.tag.to_le_bytes())?;
        out.write_all(&self.kind.to_le_bytes())?;
        out.write_all(&self.count.to_le_bytes())?;
        match self.offset {
            Some(offset) => out.write_all(&offset.to_le_bytes()),
            None => {
                let mut inline = [0u8; 4];
                inline[..self.payload.len()].copy_from_slice(&self.payload);
                out.write_all(&inline)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(name: &str, image: &Image, mapping: &Mapping) -> Vec<u8> {
        let dir = std::env::temp_dir().join("astro-stacker-tiff-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.tif"));
        write(&path, image, mapping, "test").unwrap();
        std::fs::read(&path).unwrap()
    }

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    /// Walks the directory the way a reader would, and returns every tag with
    /// its value bytes resolved.
    fn tags(bytes: &[u8]) -> Vec<(u16, u16, u32, Vec<u8>)> {
        assert_eq!(&bytes[..2], b"II", "little-endian");
        assert_eq!(u16_at(bytes, 2), 42, "classic TIFF");
        let directory = u32_at(bytes, 4) as usize;
        let count = u16_at(bytes, directory) as usize;
        (0..count)
            .map(|i| {
                let at = directory + 2 + i * 12;
                let (tag, kind, values) = (u16_at(bytes, at), u16_at(bytes, at + 2), u32_at(bytes, at + 4));
                let width = match kind {
                    1 | 2 => 1,
                    3 => 2,
                    4 => 4,
                    5 => 8,
                    other => panic!("unexpected field type {other}"),
                };
                let size = width * values as usize;
                let payload = if size <= 4 {
                    bytes[at + 8..at + 8 + size].to_vec()
                } else {
                    let offset = u32_at(bytes, at + 8) as usize;
                    bytes[offset..offset + size].to_vec()
                };
                (tag, kind, values, payload)
            })
            .collect()
    }

    #[test]
    fn a_reader_can_walk_the_directory_and_find_the_samples() {
        let plane: Vec<f32> = (0..12).map(|i| i as f32 * 100.0).collect();
        let bytes = written("grey", &Image { width: 4, height: 3, planes: &[&plane] }, &Mapping::Linear {
            full: 1100.0,
        });
        let found = tags(&bytes);

        // Ascending order is not decoration: a reader may binary search.
        let order: Vec<u16> = found.iter().map(|(tag, ..)| *tag).collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted, "tags must ascend");

        let value = |tag: u16| found.iter().find(|(t, ..)| *t == tag).expect("tag present").clone();
        assert_eq!(u32_at(&value(256).3, 0), 4, "width");
        assert_eq!(u32_at(&value(257).3, 0), 3, "height");
        assert_eq!(u16_at(&value(258).3, 0), 16, "bits per sample");
        assert_eq!(u16_at(&value(259).3, 0), 1, "uncompressed");
        assert_eq!(u16_at(&value(262).3, 0), 1, "black is zero");
        assert_eq!(u16_at(&value(277).3, 0), 1, "one sample per pixel");
        assert_eq!(u16_at(&value(339).3, 0), 1, "unsigned integer");

        // And the samples really are where the strip says they are.
        let offset = u32_at(&value(273).3, 0) as usize;
        let counts = u32_at(&value(279).3, 0) as usize;
        assert_eq!(counts, 4 * 3 * 2, "one strip of the whole image");
        assert_eq!(u16_at(&bytes, offset), 0, "the first pixel is zero ADU");
        assert_eq!(u16_at(&bytes, offset + 22), 65_535, "the last is full scale");
    }

    #[test]
    fn three_planes_are_written_interleaved_as_rgb() {
        // The failure this catches: writing the planes one after another
        // instead of pixel by pixel gives a file that opens, has the right
        // size, and shows the red channel stretched across the top third.
        let red = vec![1000f32, 0.0];
        let green = vec![0f32, 1000.0];
        let blue = vec![0f32, 0.0];
        let bytes = written(
            "rgb",
            &Image { width: 2, height: 1, planes: &[&red, &green, &blue] },
            &Mapping::Linear { full: 1000.0 },
        );
        let found = tags(&bytes);
        let value = |tag: u16| found.iter().find(|(t, ..)| *t == tag).expect("tag present").clone();
        assert_eq!(u16_at(&value(262).3, 0), 2, "RGB");
        assert_eq!(u16_at(&value(277).3, 0), 3, "three samples per pixel");
        assert_eq!(value(258).2, 3, "three bits-per-sample entries");

        let at = u32_at(&value(273).3, 0) as usize;
        let samples: Vec<u16> = (0..6).map(|i| u16_at(&bytes, at + i * 2)).collect();
        assert_eq!(samples, vec![65_535, 0, 0, 0, 65_535, 0], "red pixel then green pixel");
    }

    #[test]
    fn an_uncovered_pixel_is_zero_and_the_file_says_so() {
        // TIFF has no NaN. Writing one as zero is the only option, so the file
        // has to carry the warning that a black corner may be an unmeasured
        // one - the FITS is where the difference survives.
        let plane = vec![f32::NAN, 500.0, f32::INFINITY, -100.0];
        let bytes = written(
            "uncovered",
            &Image { width: 4, height: 1, planes: &[&plane] },
            &Mapping::Linear { full: 1000.0 },
        );
        let found = tags(&bytes);
        let description = found.iter().find(|(tag, ..)| *tag == 270).expect("a description");
        let text = String::from_utf8_lossy(&description.3);
        assert!(text.contains("Uncovered pixels are written as zero"), "got {text}");
        assert!(text.contains("65535"), "the scale must be recoverable: {text}");

        let at = u32_at(&found.iter().find(|(t, ..)| *t == 273).unwrap().3, 0) as usize;
        assert_eq!(u16_at(&bytes, at), 0, "NaN");
        assert_eq!(u16_at(&bytes, at + 2), 32_768, "half scale");
        assert_eq!(u16_at(&bytes, at + 4), 0, "infinity is not a measurement either");
        assert_eq!(u16_at(&bytes, at + 6), 0, "negative clips at black");
    }

    #[test]
    fn the_linear_mapping_can_be_undone() {
        // The whole reason linear is the default: another program must be able
        // to get the ADU back out.
        let mapping = Mapping::Linear { full: 20_000.0 };
        for adu in [0.0f32, 1.0, 137.0, 2047.0, 9999.0, 20_000.0] {
            let sample = mapping.apply(adu);
            let recovered = f32::from(sample) / 65_535.0 * 20_000.0;
            assert!((recovered - adu).abs() < 0.2, "{adu} came back as {recovered}");
        }
    }

    #[test]
    fn the_stretch_lifts_the_faint_end_and_says_it_is_not_linear() {
        // A stretched file is for the eye. Someone measuring from it would be
        // wrong, so the file has to admit what was done to it.
        let mapping = Mapping::Asinh { black: 300.0, white: 5000.0, softening: 50.0 };
        let faint = mapping.apply(400.0);
        let linear = Mapping::Linear { full: 5000.0 }.apply(400.0);
        // A hundred ADU above the sky is where a faint outer arm lives, and
        // linearly it is 8% of full scale - indistinguishable from black on a
        // screen. The stretch has to put it somewhere a person can see.
        assert!(
            f32::from(faint) > f32::from(linear) * 2.0,
            "the faint end must be lifted: {faint} against {linear}"
        );
        assert_eq!(mapping.apply(300.0), 0, "black stays black");
        assert_eq!(mapping.apply(5000.0), 65_535, "white stays white");
        assert!(mapping.describe().contains("do not measure"));
    }

    #[test]
    fn a_tall_image_is_written_in_several_strips() {
        // A reader should not have to hold a 250 MB image to decode a band of
        // it, and the offsets and counts have to agree with where the rows
        // really are.
        let height = 9000;
        let plane = vec![1000f32; 1024 * height];
        let bytes = written(
            "strips",
            &Image { width: 1024, height, planes: &[&plane] },
            &Mapping::Linear { full: 1000.0 },
        );
        let found = tags(&bytes);
        let value = |tag: u16| found.iter().find(|(t, ..)| *t == tag).expect("tag present").clone();
        let rows_per_strip = u32_at(&value(278).3, 0) as usize;
        let (_, _, strips, offsets) = value(273);
        let counts = value(279).3;
        assert!(strips > 1, "a 8 MB image should not be one strip");
        assert_eq!(strips as usize, height.div_ceil(rows_per_strip));

        let mut expected = 8usize;
        for strip in 0..strips as usize {
            assert_eq!(u32_at(&offsets, strip * 4) as usize, expected, "strip {strip} starts here");
            let rows = rows_per_strip.min(height - strip * rows_per_strip);
            assert_eq!(u32_at(&counts, strip * 4) as usize, rows * 1024 * 2);
            expected += rows * 1024 * 2;
        }
        assert_eq!(expected, 8 + 1024 * height * 2, "the strips must cover the image exactly");
    }
}
