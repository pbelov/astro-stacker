//! Fixtures shared by the tests in this module.
//!
//! Built by hand rather than decoded, so that the rules can be tested against
//! frames nobody has to own: a 45-megapixel Canon R5 light, and the calibration
//! frames that would go with it.

use astro_plugin_abi::abi::{COLOR_BLUE, COLOR_GREEN, COLOR_RED, ImageLayout, SampleFormat};
use astro_plugin_abi::safe::FrameInfo;

use super::{BodyKey, DirId, FrameFingerprint, FrameRecord, FrameRole, FrameSource, GroupId, PluginId};

/// A Canon R5 sensor: RGGB, 14 bit, full frame.
pub fn layout() -> ImageLayout {
    let mut layout = ImageLayout {
        width: 8280,
        height: 5520,
        components: 1,
        bits_per_sample: 14,
        sample_format: SampleFormat::U16,
        cfa_width: 2,
        cfa_height: 2,
        active_x: 0,
        active_y: 0,
        active_width: 8192,
        active_height: 5464,
        black_level_width: 2,
        black_level_height: 2,
        white_level: [16383.0; 4],
        orientation: 1,
        ..Default::default()
    };
    layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
    layout.black_level[..4].copy_from_slice(&[2048.0; 4]);
    layout
}

/// Shooting parameters through a telescope: no lens, so no focal length and no
/// aperture, which is the target use case rather than an edge case.
pub fn info(exposure_seconds: f64, iso: f64) -> FrameInfo {
    FrameInfo {
        camera_make: "Canon".to_owned(),
        camera_model: "Canon EOS R5".to_owned(),
        lens_model: String::new(),
        exposure_seconds: Some(exposure_seconds),
        iso: Some(iso),
        aperture: None,
        focal_length_mm: None,
        capture_time_unix: Some(1_787_834_096),
        // The Canon plugin cannot read this, and says so rather than inventing
        // a number. Every rule that touches temperature has to cope.
        sensor_temperature_c: None,
    }
}

/// A five-minute sub at ISO 1600.
pub fn light() -> FrameInfo {
    info(300.0, 1600.0)
}

/// A flat: deliberately a completely different exposure and gain from the
/// lights, because that is correct and the rules must not fault it.
pub fn flat() -> FrameInfo {
    info(1.0 / 60.0, 400.0)
}

/// A bias: the shortest exposure the body can take.
pub fn bias() -> FrameInfo {
    info(1.0 / 8000.0, 1600.0)
}

pub fn record(file_name: &str, info: FrameInfo) -> FrameRecord {
    record_in(GroupId::MAIN, file_name, info)
}

/// Adds a frame the user has said the kind of.
pub fn assigned(
    session: &mut super::Session,
    kind: super::FrameKind,
    file_name: &str,
    info: FrameInfo,
) -> super::FrameId {
    assigned_in(session, GroupId::MAIN, kind, file_name, info)
}

pub fn assigned_in(
    session: &mut super::Session,
    group: GroupId,
    kind: super::FrameKind,
    file_name: &str,
    info: FrameInfo,
) -> super::FrameId {
    let mut record = record_in(group, file_name, info);
    record.role = FrameRole::Assigned(kind);
    session.insert(record)
}

/// A run of frames shot back to back, so that a set has plausible timestamps.
pub fn run(
    session: &mut super::Session,
    kind: super::FrameKind,
    prefix: &str,
    count: usize,
    info: FrameInfo,
) {
    let cadence = info.exposure_seconds.unwrap_or(1.0).max(1.0) as i64 + 2;
    for index in 0..count {
        let mut frame = info.clone();
        frame.capture_time_unix = info.capture_time_unix.map(|start| start + cadence * index as i64);
        assigned(session, kind, &format!("{prefix}{index:04}.CR3"), frame);
    }
}

pub fn record_in(group: GroupId, file_name: &str, info: FrameInfo) -> FrameRecord {
    FrameRecord {
        source: FrameSource {
            directory: DirId(0),
            file_name: file_name.into(),
            plugin: PluginId(0),
            // Distinct per name, so that duplicate detection has something to
            // work with.
            fingerprint: FrameFingerprint::new(45_000_000, Some(1_787_834_096), file_name.as_bytes()),
        },
        layout: layout(),
        body: BodyKey::from_info(&info),
        info,
        role: FrameRole::unassigned(),
        group,
        exclusion: None,
    }
}
