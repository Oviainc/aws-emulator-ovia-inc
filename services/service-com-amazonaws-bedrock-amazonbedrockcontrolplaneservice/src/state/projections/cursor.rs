// SPDX-License-Identifier: CC0-1.0

//! The CURSOR every paginated listing of this service pages with, and the instants a
//! record reports.
//!
//! A cursor is the row a listing resumes at, rendered so byte order over cursors IS the
//! order the listing is sorted in — which is what lets a page resume at "the first row at
//! or after this cursor" even when the row the previous page ended on has since been
//! deleted. It lives in its own file because every listing of this service pages with the
//! same one, and because what it renders has to stay inside `PaginationToken`'s own
//! constraint (`^\S*$`).

use super::*;

/// A `SystemTime` as the wire timestamp type the response shapes carry.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Whether the `lastUpdatedAt` concurrency token a request carried names the write the
/// addressed resource holds at `held`.
///
/// Stated once here because every resource of this service that is written under such a
/// token is held against it the same way, so two renderings of the comparison could drift
/// and one operation would then accept a token another refuses.
///
/// The comparison is at MILLISECOND resolution, and not finer: the token is a value the
/// caller echoes back from a read of the same resource, and a `date-time` instant travels
/// through clients that carry milliseconds, so holding the echoed value to the nanosecond
/// the store minted would refuse a caller naming the very write it just read. A millisecond
/// still distinguishes two writes, which is what the token is for.
#[must_use]
pub fn names_write_at(token: &::aws_smithy_types::DateTime, held: SystemTime) -> bool {
    epoch_millis(token) == epoch_millis(&date_time(held))
}

/// The instant a wire timestamp names, in whole milliseconds since the epoch.
pub(crate) fn epoch_millis(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000 + i128::from(at.subsec_nanos() / 1_000_000)
}

// =============================================================================
// The cursor every creation-time listing of this service pages with
// =============================================================================
//
// `ops/list_provisioned_model_throughputs.rs` and `ops/list_custom_models.rs` sort by the
// one instant their `sortBy` admits and page with a `nextToken`, and
// `ops/list_inference_profiles.rs` pages the same way over the instant its profiles were
// created, so they need the SAME cursor: stated once here, because two renderings of it
// could drift and a token issued by one shape would then not resume the listing it came
// from.

/// The number of decimal digits a listing cursor's instant half carries.
///
/// Fixed-width so BYTE order over cursors IS the listing's order: nanoseconds since the
/// epoch need 19 digits for any instant this emulator can mint, and 20 leaves the width
/// stable past them.
pub const CURSOR_INSTANT_DIGITS: usize = 20;

/// The cursor of the row a listing resumes at: the instant the resource was created, then
/// its ARN.
///
/// Rendered so byte order over cursors is the `(creationTime, arn)` order such a listing is
/// sorted by — the instant half is fixed-width and `:` sorts below every character the rest
/// of an ARN can carry after it — which is what lets a page resume at "the first row at or
/// after this cursor" even when the row the previous page ended on has since been deleted.
/// It carries no whitespace, as `PaginationToken` (`^\S*$`) requires.
#[must_use]
pub fn listing_cursor(created_at: SystemTime, arn: &str) -> String {
    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:0width$}:{arn}", width = CURSOR_INSTANT_DIGITS)
}

/// Whether `token` is a cursor [`listing_cursor`] renders, so a token from somewhere else
/// is refused rather than silently read as an empty or wrong page.
#[must_use]
pub fn cursor_belongs(token: &str) -> bool {
    let Some((instant, arn)) = token.split_once(':') else {
        return false;
    };
    instant.len() == CURSOR_INSTANT_DIGITS
        && instant.bytes().all(|byte| byte.is_ascii_digit())
        && !arn.is_empty()
}

/// The trailing half of the cursor of a PUBLISHED row of a listing: its position in the
/// order the published catalogue is reported in.
///
/// A published row is created by no call, so there is no instant and no minted ARN to
/// order it by — it is reported in the order its own catalogue publishes it, which for the
/// cross-region inference profiles is the name Amazon Bedrock publishes each one under
/// ([`system_defined_inference_profiles`]). A name carries whitespace and
/// `PaginationToken` is `^\S*$`, so the POSITION in that order is what the cursor carries:
/// fixed-width, so byte order over cursors is still the listing's order, and
/// whitespace-free, so the token a page hands back is one the next request may send.
#[must_use]
pub fn published_row_cursor_key(position: usize) -> String {
    format!("published-{position:0width$}", width = CURSOR_INSTANT_DIGITS)
}

/// `at` as epoch seconds, the scale a `creationTime` filter carries on the wire
/// (`@timestampFormat("date-time")`), so a filter and a record are compared on one scale.
#[must_use]
pub fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}
