// SPDX-License-Identifier: CC0-1.0

//! The clock helpers every property-value suite in this crate shares.
//!
//! `BatchPutAssetPropertyValue` ingests only TQVs inside
//! `[now - 7 days, now + 10 minutes]`, so no suite can seed a series at a fixed
//! calendar second. A suite either anchors its timestamps on [`now_seconds`]
//! directly, or — when its tables read better with the fixed timestamps they were
//! written against — keeps them and converts at the wire boundary with
//! [`anchored`] / [`relative`], which translate a table second into the accepted
//! window and back. The anchor is computed ONCE per test binary, so two suites
//! cannot drift about where "now" is.
//!
//! The wire harness itself (`router`, `call`, `create_model`, …) lives in
//! `super::asset_lifecycle`, which is this crate's harness module.

/// The current second in the Unix epoch.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the Unix epoch")
        .as_secs() as i64
}

/// How far back of `now_seconds()` an anchored table lands.
///
/// Far enough inside the accepted window that a run taking several seconds cannot
/// drift out of it, and close enough that a table spanning a few thousand seconds
/// forward of the anchor stays in the past.
const ANCHOR_AGE: i64 = 10_000;

/// Where an anchored table's own epoch lands, fixed once for the test binary.
pub fn anchor() -> i64 {
    static ANCHOR: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ANCHOR.get_or_init(|| now_seconds() - ANCHOR_AGE)
}

/// A table second, measured against `row_epoch`, as the absolute `timeInSeconds`
/// the wire carries.
pub fn anchored(row_epoch: i64, seconds: i64) -> i64 {
    anchor() + (seconds - row_epoch)
}

/// The inverse of [`anchored`]: an absolute `timeInSeconds` as the table second it
/// was written from.
pub fn relative(row_epoch: i64, seconds: i64) -> i64 {
    row_epoch + (seconds - anchor())
}
