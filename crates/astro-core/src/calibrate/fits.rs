//! Writing a master as 32-bit floating point FITS.
//!
//! Hand-rolled rather than taken from a crate, because the part of FITS a master
//! frame needs is genuinely small — 80-character cards in 2880-byte blocks, then
//! big-endian samples — and a dependency that reads the whole standard would be
//! more surface than the format it hides.
//!
//! Two choices worth defending.
//!
//! **Values are in ADU, exactly as the sensor read them.** Some readers rescale
//! a float image they think is meant to be in `[0, 1]`, and a master bias near
//! 2050 will surprise them. Writing a made-up scale to suit one reader would put
//! a number in the file that the sensor never produced, and the whole point of a
//! master is to be the measurement.
//!
//! **Rows go out in sensor readout order, top first**, with `ROWORDER` saying so.
//! FITS itself puts the first row at the bottom, which is a display convention;
//! the samples here are indexed against a mosaic and a black-level grid that are
//! both anchored to the sensor's own origin, and flipping them silently would
//! move every photosite off its colour.

use std::io::{BufWriter, Write};
use std::path::Path;

const BLOCK: usize = 2880;
const CARD: usize = 80;

/// A keyword and its value, already rendered.
struct Card(String);

impl Card {
    fn logical(key: &str, value: bool) -> Self {
        Self::fixed(key, if value { "T" } else { "F" })
    }

    fn integer(key: &str, value: i64) -> Self {
        Self::fixed(key, &value.to_string())
    }

    fn real(key: &str, value: f64) -> Self {
        Self::fixed(key, &format!("{value:.6}"))
    }

    fn text(key: &str, value: &str) -> Self {
        // FITS strings are single-quoted with any embedded quote doubled, and
        // are padded to at least eight characters.
        let escaped = value.replace('\'', "''");
        let padded = format!("{escaped:<8}");
        Self(format!("{key:<8}= '{padded}'"))
    }

    fn comment(text: &str) -> Self {
        Self(format!("COMMENT {text}"))
    }

    /// Numbers are right-justified into columns 11..30, which is where every
    /// reader expects a fixed-format value.
    fn fixed(key: &str, value: &str) -> Self {
        Self(format!("{key:<8}= {value:>20}"))
    }

    fn write(&self, out: &mut impl Write) -> std::io::Result<()> {
        let mut card = self.0.clone();
        card.truncate(CARD);
        write!(out, "{card:<CARD$}")
    }
}

/// What a master needs to carry so that it can be trusted a month later.
#[derive(Debug, Clone, Default)]
pub struct Header {
    pub image_type: String,
    pub instrument: String,
    /// Seconds. `None` where it does not apply or was not recorded.
    pub exposure: Option<f64>,
    pub iso: Option<f64>,
    /// How many frames went into it.
    pub frames: usize,
    pub combination: String,
    /// `RGGB` and the like, or `None` for a frame that is not mosaiced.
    pub bayer_pattern: Option<String>,
    /// Free text: what was subtracted, how much was rejected, what was odd.
    pub notes: Vec<String>,
}

/// Writes one 2-dimensional 32-bit float image.
///
/// `pixels` is `width * height` in sensor readout order.
pub fn write(
    path: &Path,
    pixels: &[f32],
    width: usize,
    height: usize,
    header: &Header,
) -> std::io::Result<()> {
    if pixels.len() != width * height {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} samples for a {width}x{height} image", pixels.len()),
        ));
    }

    let file = std::fs::File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);

    let mut cards = vec![
        Card::logical("SIMPLE", true),
        Card::integer("BITPIX", -32),
        Card::integer("NAXIS", 2),
        Card::integer("NAXIS1", width as i64),
        Card::integer("NAXIS2", height as i64),
        Card::real("BZERO", 0.0),
        Card::real("BSCALE", 1.0),
        // Sensor readout order, not the FITS display convention. See the module
        // documentation: the mosaic is anchored to the sensor origin.
        Card::text("ROWORDER", "TOP-DOWN"),
    ];

    if !header.image_type.is_empty() {
        cards.push(Card::text("IMAGETYP", &header.image_type));
    }
    if !header.instrument.is_empty() {
        cards.push(Card::text("INSTRUME", &header.instrument));
    }
    if let Some(exposure) = header.exposure {
        cards.push(Card::real("EXPTIME", exposure));
    }
    if let Some(iso) = header.iso {
        cards.push(Card::real("ISOSPEED", iso));
    }
    cards.push(Card::integer("STACKCNT", header.frames as i64));
    if !header.combination.is_empty() {
        cards.push(Card::text("COMBINE", &header.combination));
    }
    if let Some(pattern) = &header.bayer_pattern {
        cards.push(Card::text("BAYERPAT", pattern));
        cards.push(Card::integer("XBAYROFF", 0));
        cards.push(Card::integer("YBAYROFF", 0));
    }
    cards.push(Card::text("CREATOR", concat!("astro-stacker ", env!("CARGO_PKG_VERSION"))));
    cards.push(Card::comment("Values are in ADU as the sensor read them, not normalised."));
    for note in &header.notes {
        // A note longer than a card is truncated rather than wrapped: a header
        // is not the place for prose, and the report says it in full.
        cards.push(Card::comment(note));
    }
    cards.push(Card(String::from("END")));

    for card in &cards {
        card.write(&mut out)?;
    }
    pad_to_block(&mut out, cards.len() * CARD, b' ')?;

    // Big-endian, which is the only byte order FITS has.
    let mut buffer = Vec::with_capacity(1 << 16);
    for value in pixels {
        buffer.extend_from_slice(&value.to_be_bytes());
        if buffer.len() >= (1 << 16) {
            out.write_all(&buffer)?;
            buffer.clear();
        }
    }
    out.write_all(&buffer)?;
    pad_to_block(&mut out, size_of_val(pixels), 0)?;

    out.flush()
}

fn pad_to_block(out: &mut impl Write, written: usize, fill: u8) -> std::io::Result<()> {
    let remainder = written % BLOCK;
    if remainder == 0 {
        return Ok(());
    }
    let padding = vec![fill; BLOCK - remainder];
    out.write_all(&padding)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Named per test: these run in parallel, and three of them write a two
    /// by two image.
    fn written(name: &str, pixels: &[f32], width: usize, height: usize) -> Vec<u8> {
        let dir = std::env::temp_dir().join("astro-stacker-fits-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.fits"));
        let header = Header {
            image_type: "Dark".to_owned(),
            instrument: "Canon EOS 60D".to_owned(),
            exposure: Some(30.0),
            iso: Some(6400.0),
            frames: 11,
            combination: "median".to_owned(),
            bayer_pattern: Some("GBRG".to_owned()),
            notes: vec!["nothing rejected".to_owned()],
        };
        write(&path, pixels, width, height, &header).unwrap();
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn the_file_is_a_whole_number_of_blocks() {
        // Every reader in this format assumes it, and a short final block is
        // the classic way to produce a file that opens in one tool and not
        // another.
        let bytes = written("blocks", &vec![2048.0; 17 * 13], 17, 13);
        assert_eq!(bytes.len() % BLOCK, 0);
        assert!(bytes.len() >= 2 * BLOCK, "a header block and a data block at least");
    }

    #[test]
    fn the_required_cards_come_first_and_in_order() {
        let bytes = written("cards", &[1.0; 4], 2, 2);
        let header = String::from_utf8_lossy(&bytes[..BLOCK]).into_owned();
        let cards: Vec<&str> = header.as_bytes().chunks(CARD).map(|c| std::str::from_utf8(c).unwrap()).collect();

        assert!(cards[0].starts_with("SIMPLE  =                    T"));
        assert!(cards[1].starts_with("BITPIX  =                  -32"));
        assert!(cards[2].starts_with("NAXIS   =                    2"));
        assert!(cards[3].starts_with("NAXIS1  =                    2"));
        assert!(cards[4].starts_with("NAXIS2  =                    2"));
        assert!(header.contains("END"));
    }

    #[test]
    fn samples_are_big_endian_in_sensor_order() {
        // A reader that took these little-endian would see garbage, and one
        // that flipped the rows would move every photosite off its colour.
        let pixels = [1.0f32, 2.0, 3.0, 4.0];
        let bytes = written("endian", &pixels, 2, 2);
        let data = &bytes[BLOCK..BLOCK + 16];
        for (index, expected) in pixels.iter().enumerate() {
            let mut four = [0u8; 4];
            four.copy_from_slice(&data[index * 4..index * 4 + 4]);
            assert_eq!(f32::from_be_bytes(four), *expected, "sample {index}");
        }
    }

    #[test]
    fn a_negative_value_survives_the_round_trip() {
        // Calibration goes negative the moment anything is subtracted, and a
        // master that clamped at zero would hide exactly the over-subtraction
        // the user needs to see.
        let bytes = written("negative", &[-12.5f32, 0.0, 3.25, -0.5], 2, 2);
        let mut four = [0u8; 4];
        four.copy_from_slice(&bytes[BLOCK..BLOCK + 4]);
        assert_eq!(f32::from_be_bytes(four), -12.5);
    }

    #[test]
    fn a_quote_in_a_value_does_not_break_the_card() {
        let card = Card::text("OBJECT", "Barnard's Loop");
        assert!(card.0.contains("'Barnard''s Loop'"), "{}", card.0);
    }
}
