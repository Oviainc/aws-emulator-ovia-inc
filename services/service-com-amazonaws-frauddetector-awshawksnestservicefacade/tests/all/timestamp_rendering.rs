// SPDX-License-Identifier: CC0-1.0

//! The `createdTime`/`lastUpdatedTime` rendering: a FIXED-WIDTH ISO-8601 string, so the
//! order of two of this service's timestamps as strings is their order in time.
//!
//! The rule under test: for any instant, `crate::state::timestamp_of_millis` renders
//! `YYYY-MM-DDTHH:MM:SS.mmmZ` — same length, always three fractional digits — and so
//! `a <= b` as strings exactly when `a <= b` as instants. The case that made this a rule
//! is an instant landing on a whole second, which a conditional RFC-3339 writer renders
//! without a fractional part at all (`...:52Z`), sorting it AFTER `...:52.100Z`.

use service_com_amazonaws_frauddetector_awshawksnestservicefacade::state::timestamp_of_millis;

/// The modelled `time` shape is `@length(11..30)`; this rendering is 24 characters.
const RENDERED_WIDTH: usize = 24;

/// Instants spanning what a clock produces: the epoch, a whole second, each end of the
/// fractional range, a second boundary from both sides, and a far-future instant.
fn cases() -> Vec<i64> {
    vec![
        0,
        1,
        999,
        1_000,
        1_001,
        1_999,
        2_000,
        1_761_000_000_000,
        1_761_000_000_001,
        1_761_000_000_999,
        1_761_000_001_000,
        4_102_444_800_000,
    ]
}

#[test]
fn every_timestamp_is_rendered_at_the_same_width() {
    for epoch_millis in cases() {
        let rendered = timestamp_of_millis(epoch_millis);
        assert_eq!(
            rendered.len(),
            RENDERED_WIDTH,
            "case {epoch_millis}: expected a {RENDERED_WIDTH}-character timestamp, got {rendered:?}"
        );
        let fraction = rendered
            .split('.')
            .next_back()
            .unwrap_or_default()
            .trim_end_matches('Z');
        assert_eq!(
            fraction.len(),
            3,
            "case {epoch_millis}: expected three fractional digits, got {rendered:?}"
        );
        assert!(
            fraction.bytes().all(|byte| byte.is_ascii_digit()),
            "case {epoch_millis}: the fractional part must be digits, got {rendered:?}"
        );
        assert!(
            rendered.ends_with('Z'),
            "case {epoch_millis}: an ISO-8601 UTC timestamp ends in Z, got {rendered:?}"
        );
    }
}

#[test]
fn string_order_is_time_order() {
    let instants = cases();
    for (index, earlier) in instants.iter().enumerate() {
        for later in &instants[index..] {
            let (rendered_earlier, rendered_later) =
                (timestamp_of_millis(*earlier), timestamp_of_millis(*later));
            assert!(
                rendered_earlier <= rendered_later,
                "case ({earlier}, {later}): {rendered_earlier:?} must not sort after \
                 {rendered_later:?}"
            );
            assert_eq!(
                rendered_earlier == rendered_later,
                earlier == later,
                "case ({earlier}, {later}): two distinct instants must render differently, \
                 got {rendered_earlier:?} and {rendered_later:?}"
            );
        }
    }
}

/// The known instants, so the rendering is pinned and not merely self-consistent.
#[test]
fn known_instants_render_as_documented() {
    // UNIX epoch, and 2025-10-20T22:40:00.123Z — both computed from the epoch
    // milliseconds, not from a clock.
    let cases: &[(i64, &str)] = &[
        (0, "1970-01-01T00:00:00.000Z"),
        (1, "1970-01-01T00:00:00.001Z"),
        (999, "1970-01-01T00:00:00.999Z"),
        (1_000, "1970-01-01T00:00:01.000Z"),
        (1_761_000_000_123, "2025-10-20T22:40:00.123Z"),
    ];
    for (epoch_millis, expected) in cases {
        let rendered = timestamp_of_millis(*epoch_millis);
        assert_eq!(
            rendered, *expected,
            "case {epoch_millis}: expected {expected:?}, got {rendered:?}"
        );
    }
}
