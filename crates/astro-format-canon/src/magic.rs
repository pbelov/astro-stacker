//! Identifying a Canon raw from its first bytes.
//!
//! The extension is not consulted: a `.CR2` that is really a JPEG must not be
//! claimed, and a correctly-formed file renamed to `.raw` should still be read.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonFormat {
    /// TIFF container with a `CR` marker in the header.
    Cr2,
    /// ISO base media container with the `crx ` brand.
    Cr3,
}

pub fn identify(header: &[u8]) -> Option<CanonFormat> {
    if is_cr3(header) {
        Some(CanonFormat::Cr3)
    } else if is_cr2(header) {
        Some(CanonFormat::Cr2)
    } else {
        None
    }
}

/// CR2 is a little-endian TIFF whose header carries `CR` and a major version
/// at offset 8. Canon has never shipped a big-endian CR2.
fn is_cr2(header: &[u8]) -> bool {
    if header.len() < 11 {
        return false;
    }
    &header[0..4] == b"II\x2A\x00" && &header[8..10] == b"CR" && header[10] == 2
}

/// CR3 is ISO base media format: a `ftyp` box whose major brand is `crx `.
fn is_cr3(header: &[u8]) -> bool {
    header.len() >= 12 && &header[4..8] == b"ftyp" && &header[8..12] == b"crx "
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr3_header() -> Vec<u8> {
        let mut header = vec![0x00, 0x00, 0x00, 0x18];
        header.extend_from_slice(b"ftypcrx ");
        header.extend_from_slice(&[0u8; 16]);
        header
    }

    fn cr2_header() -> Vec<u8> {
        let mut header = vec![b'I', b'I', 0x2A, 0x00, 0x10, 0x00, 0x00, 0x00];
        header.extend_from_slice(&[b'C', b'R', 2, 0]);
        header
    }

    #[test]
    fn identifies_both_canon_containers() {
        assert_eq!(identify(&cr3_header()), Some(CanonFormat::Cr3));
        assert_eq!(identify(&cr2_header()), Some(CanonFormat::Cr2));
    }

    #[test]
    fn rejects_neighbouring_formats() {
        // A plain TIFF: right magic, no CR marker.
        let mut tiff = vec![b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        tiff.extend_from_slice(&[0u8; 8]);
        assert_eq!(identify(&tiff), None);

        // An MP4: right box structure, wrong brand.
        let mut mp4 = vec![0x00, 0x00, 0x00, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[0u8; 8]);
        assert_eq!(identify(&mp4), None);

        // A Nikon raw is also a TIFF, and also must not be claimed.
        let mut nef = vec![b'M', b'M', 0x00, 0x2A];
        nef.extend_from_slice(&[0u8; 12]);
        assert_eq!(identify(&nef), None);
    }

    #[test]
    fn a_truncated_header_is_not_a_match() {
        assert_eq!(identify(&[]), None);
        assert_eq!(identify(b"II*\0"), None);
        assert_eq!(identify(&cr3_header()[..8]), None);
        assert_eq!(identify(&cr2_header()[..10]), None);
    }
}
