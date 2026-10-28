// SPDX-License-Identifier: CC0-1.0

//! What `DescribeAccountSettings` REPORTS, end to end over the wire.
//!
//! RULE: `DescribeAccountSettings` answers the minimum throughput billing commitment this
//! account holds AT THE INSTANT OF THE CALL, derived from the stored record alone: the
//! `Status` it carries then (`ENABLED` while no end is scheduled,
//! `ENABLED_UNTIL_EARLIEST_ALLOWED_END` while a scheduled end is still ahead, `DISABLED`
//! once that end has been reached and for an account that never asked for a commitment),
//! the instant it started, the earliest instant it may end at, and an `EndedAt` only once
//! the end has actually been reached. It is a reporting operation, so the end the clock
//! already owes a scheduled disable is reported as reached with no further call needed, and
//! the read itself holds, ends or moves nothing: repeating it reports the same answer and
//! leaves the stored record exactly as it stands.
//!
//! The documented sentences that rule comes from are the operation's own ("Describes the
//! account-level settings for Amazon Kinesis Data Streams") and
//! `MinimumThroughputBillingCommitmentOutput`'s members: "The current status of the minimum
//! throughput billing commitment", "The timestamp when the commitment was started", "The
//! timestamp when the commitment was ended" and "The earliest timestamp when the commitment
//! can be ended"; the published `Status` catalog is `ENABLED | DISABLED |
//! ENABLED_UNTIL_EARLIEST_ALLOWED_END`
//! (`smithy_query(query="shape",shape="MinimumThroughputBillingCommitmentOutputStatus")`).
//!
//! The recorded flow this was derived from reads an account whose commitment was just
//! enabled, which is one row of the table below. The rows differ along what the rule
//! generalises over: whether the account holds a commitment at all, how long ago it started
//! (either side of the minimum duration it has to honour), whether a disable scheduled an
//! end, and where that end sits relative to the instant of the call — ahead of it, exactly
//! at it (the bound at which an end counts as reached) and past it.
//!
//! [`crate::account_settings`] drives the same commitment through `UpdateAccountSettings`;
//! this suite pins what the READ derives from a stored record the clock has already moved
//! past, and reuses that suite's wire helpers rather than copying them.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    Commitment, COMMITMENT_DISABLED, COMMITMENT_ENABLED,
    COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END, MINIMUM_COMMITMENT_DURATION,
};

use crate::account_settings::{
    described, epoch_seconds, instant_of, status_of, TIMESTAMP_TOLERANCE,
};
use crate::stream_tags::Emulator;

/// Where the end a disable scheduled for the stored commitment sits relative to the instant
/// of the call.
#[derive(Clone, Copy, Debug)]
enum End {
    /// No disable has been accepted, so there is no end to reach.
    Unscheduled,
    /// A scheduled end still this far ahead of the call.
    Ahead(Duration),
    /// An end reached this long before the call. `ZERO` is the bound itself: an end AT the
    /// instant of the call has been reached.
    Reached(Duration),
}

/// One row of the reporting table: the commitment the store holds when the read lands, and
/// the status that read has to derive from it.
struct Row {
    label: &'static str,
    /// How long before the call the stored commitment started; `None` stores none at all,
    /// which is the account that never asked for one.
    started_ago: Option<Duration>,
    end: End,
    expected: &'static str,
}

/// A minute, the span rows sit just inside or just past a bound by.
const A_MINUTE: Duration = Duration::from_secs(60);

/// The table the reporting rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an account that never asked for a commitment",
            started_ago: None,
            end: End::Unscheduled,
            expected: COMMITMENT_DISABLED,
        },
        Row {
            label: "a commitment that started at the instant of the call",
            started_ago: Some(Duration::ZERO),
            end: End::Unscheduled,
            expected: COMMITMENT_ENABLED,
        },
        Row {
            label: "a commitment standing just inside its minimum duration",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION - A_MINUTE),
            end: End::Unscheduled,
            expected: COMMITMENT_ENABLED,
        },
        Row {
            label: "a commitment standing past its minimum duration with no disable asked for",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION + A_MINUTE),
            end: End::Unscheduled,
            expected: COMMITMENT_ENABLED,
        },
        Row {
            label: "a disable scheduled just under a day out",
            started_ago: Some(A_MINUTE),
            end: End::Ahead(MINIMUM_COMMITMENT_DURATION - A_MINUTE),
            expected: COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
        },
        Row {
            label: "a disable scheduled a minute out",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION - A_MINUTE),
            end: End::Ahead(A_MINUTE),
            expected: COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
        },
        Row {
            label: "an end falling due at the instant of the call",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION),
            end: End::Reached(Duration::ZERO),
            expected: COMMITMENT_DISABLED,
        },
        Row {
            label: "an end reached a minute ago",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION + A_MINUTE),
            end: End::Reached(A_MINUTE),
            expected: COMMITMENT_DISABLED,
        },
        Row {
            label: "an end reached a week ago",
            started_ago: Some(MINIMUM_COMMITMENT_DURATION + Duration::from_secs(7 * 24 * 60 * 60)),
            end: End::Reached(Duration::from_secs(7 * 24 * 60 * 60)),
            expected: COMMITMENT_DISABLED,
        },
    ]
}

/// The commitment a row stores, placed around `now`.
fn stored_commitment(row: &Row, now: SystemTime) -> Option<Commitment> {
    row.started_ago.map(|ago| {
        let started_at = now - ago;
        Commitment {
            started_at,
            earliest_allowed_end_at: started_at + MINIMUM_COMMITMENT_DURATION,
            ends_at: match row.end {
                End::Unscheduled => None,
                End::Ahead(by) => Some(now + by),
                End::Reached(ago) => Some(now - ago),
            },
        }
    })
}

/// The three instants a stored commitment is identified by, for asserting a read left it as
/// it stands ([`Commitment`] carries no equality of its own).
fn instants(commitment: &Commitment) -> (SystemTime, SystemTime, Option<SystemTime>) {
    (
        commitment.started_at,
        commitment.earliest_allowed_end_at,
        commitment.ends_at,
    )
}

/// Assert the instant a response reports under `member` is the expected one, to the
/// resolution a `Timestamp` travels at on the wire.
fn assert_instant(
    case: &str,
    member: &str,
    actual: Option<f64>,
    expected: Option<f64>,
    response: &Value,
) {
    match (actual, expected) {
        (None, None) => {}
        (Some(actual), Some(expected)) if (actual - expected).abs() <= TIMESTAMP_TOLERANCE => {}
        (actual, expected) => panic!(
            "case {case}: expected {member} {expected:?}, got {actual:?} in {response}"
        ),
    }
}

/// Every row of the table: the read derives the status and the instants from the stored
/// record at the instant of the call, and writes nothing.
#[tokio::test]
async fn a_read_reports_the_stored_commitment_at_the_instant_of_the_call() {
    for row in rows() {
        let case = row.label;
        let emulator = Emulator::new();

        // A commitment this old cannot be reached over the wire without waiting a day for
        // it, so the record is laid down in the very store every handler reads.
        let now = SystemTime::now();
        let stored = stored_commitment(&row, now);
        emulator.store().lock().commitment = stored.clone();

        let read = described(&emulator, case).await;
        assert_eq!(
            status_of(&read),
            row.expected,
            "case {case}: expected status {}, got {read}",
            row.expected
        );

        let window = stored.as_ref().map(instants);
        assert_instant(
            case,
            "StartedAt",
            instant_of(&read, "StartedAt"),
            window.map(|(started_at, _, _)| epoch_seconds(started_at)),
            &read,
        );
        assert_instant(
            case,
            "EarliestAllowedEndAt",
            instant_of(&read, "EarliestAllowedEndAt"),
            window.map(|(_, earliest, _)| epoch_seconds(earliest)),
            &read,
        );
        // "The timestamp when the commitment was ended": only an end that has been REACHED
        // is an end the commitment was ended at.
        let expected_ended = match row.end {
            End::Reached(_) => window
                .and_then(|(_, _, ends_at)| ends_at)
                .map(epoch_seconds),
            End::Unscheduled | End::Ahead(_) => None,
        };
        assert_instant(
            case,
            "EndedAt",
            instant_of(&read, "EndedAt"),
            expected_ended,
            &read,
        );

        // The read holds, ends and moves nothing: a second one answers the same, and the
        // stored record still carries the instants it was laid down with.
        let again = described(&emulator, case).await;
        assert_eq!(
            again, read,
            "case {case}: a second read reports what the first did, got {again} after {read}"
        );
        let after = emulator.store().lock().commitment.clone();
        assert_eq!(
            after.as_ref().map(instants),
            stored.as_ref().map(instants),
            "case {case}: a read leaves the stored commitment as it stands, got {after:?} \
             after {stored:?}"
        );
    }
}

/// An end a disable scheduled is reported as reached the moment the clock passes it, with no
/// call in between and nothing written to bring it about: the same stored record reads as
/// standing before that instant and as ended after it.
#[tokio::test]
async fn an_end_the_clock_reaches_needs_no_further_call() {
    let case = "an end the clock reaches";
    let emulator = Emulator::new();

    // A commitment past its minimum duration whose scheduled end is a moment away, so the
    // instant it falls due is reachable inside a test — with enough head-room that the read
    // taken BEFORE it lands before it even on a loaded machine.
    let ends_in = Duration::from_secs(2);
    let now = SystemTime::now();
    let started_at = now - (MINIMUM_COMMITMENT_DURATION + A_MINUTE);
    let ends_at = now + ends_in;
    let stored = Commitment {
        started_at,
        earliest_allowed_end_at: started_at + MINIMUM_COMMITMENT_DURATION,
        ends_at: Some(ends_at),
    };
    emulator.store().lock().commitment = Some(stored.clone());

    let before = described(&emulator, case).await;
    assert_eq!(
        status_of(&before),
        COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
        "case {case}: a scheduled end still ahead leaves the commitment standing, got {before}"
    );
    assert_instant(case, "EndedAt", instant_of(&before, "EndedAt"), None, &before);

    tokio::time::sleep(ends_in + Duration::from_millis(400)).await;

    let after = described(&emulator, case).await;
    assert_eq!(
        status_of(&after),
        COMMITMENT_DISABLED,
        "case {case}: the end the clock reached is reported, got {after}"
    );
    assert_instant(
        case,
        "EndedAt",
        instant_of(&after, "EndedAt"),
        Some(epoch_seconds(ends_at)),
        &after,
    );
    // The end was reported, not written: the record still carries the instants it was laid
    // down with.
    let held = emulator.store().lock().commitment.clone();
    assert_eq!(
        held.as_ref().map(instants),
        Some(instants(&stored)),
        "case {case}: the read reported the end without rewriting the record, got {held:?}"
    );
}
