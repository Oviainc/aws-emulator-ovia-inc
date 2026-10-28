// SPDX-License-Identifier: CC0-1.0

//! This account's ACCOUNT-LEVEL SETTINGS, end to end over the wire: `UpdateAccountSettings`
//! -> `DescribeAccountSettings`.
//!
//! RULE: `UpdateAccountSettings` takes the stored minimum throughput billing commitment of
//! the account to the status the request asks for, and both it and `DescribeAccountSettings`
//! report that stored commitment. Asking for `ENABLED`
//! holds one: it starts a commitment when none stands — stamping the instant it started and
//! the earliest instant it may end, one minimum commitment duration later — and otherwise
//! cancels the end a disable had scheduled, leaving the window it already carries. Asking for
//! `DISABLED` ends one: at the earliest allowed end while that is still ahead, which is the
//! `ENABLED_UNTIL_EARLIEST_ALLOWED_END` the commitment stands in until then, and at the
//! instant of the call once the minimum duration has been honoured. So asking for the status
//! the account already carries changes nothing, an account that never asked for a commitment
//! holds none, a commitment reports an end only once it has actually ended, and whatever an
//! update stored is what the next read reports.
//!
//! The documented sentences that rule comes from are on
//! `MinimumThroughputBillingCommitmentInput`: "Commitments have a minimum duration of 24 hours
//! that must be honored before they can be disabled", "If you attempt to disable a commitment
//! before the minimum commitment period ends, the commitment will be scheduled for automatic
//! disable at the earliest allowed end time", and "You can cancel a pending disable by
//! enabling the commitment again before the earliest allowed end time".
//!
//! The recorded flow this was derived from enables the commitment once and reads it back,
//! which is one row of the table below. The rows differ along what the rule generalises over:
//! the status the update asks for, the status the account already carries when it lands, how
//! many updates the account takes and in which order, whether the minimum commitment duration
//! has been honoured yet, and the endpoint the request is signed for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness is [`crate::stream_tags`]', reused rather than copied.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    Commitment, COMMITMENT_DISABLED, COMMITMENT_ENABLED,
    COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END, MINIMUM_COMMITMENT_DURATION,
};

use crate::stream_tags::{error_code, error_message, Emulator, REGION};

/// The member every account-settings response carries the commitment under.
const COMMITMENT: &str = "MinimumThroughputBillingCommitment";

/// How close two instants have to be to count as the same epoch-seconds timestamp, which a
/// `Timestamp` travels as on the wire.
pub(crate) const TIMESTAMP_TOLERANCE: f64 = 1.0;

/// One row of the transition table: the statuses a sequence of updates asks for, in order,
/// and what the commitment carries once the last of them is stored.
struct Row {
    label: &'static str,
    /// The `MinimumThroughputBillingCommitment.Status` each update of the sequence asks for.
    requested: &'static [&'static str],
    /// The `MinimumThroughputBillingCommitmentOutputStatus` the account carries afterwards.
    expected: &'static str,
    /// Whether the commitment reports a start and an earliest allowed end: an account that
    /// never asked for one holds no window to report.
    holds_window: bool,
}

/// The table the transition rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The account default: nothing ever asked for a commitment, so none stands.
        Row {
            label: "an account that never asked for a commitment",
            requested: &[],
            expected: COMMITMENT_DISABLED,
            holds_window: false,
        },
        // The recorded shape: one enable holds a commitment.
        Row {
            label: "one enable",
            requested: &[COMMITMENT_ENABLED],
            expected: COMMITMENT_ENABLED,
            holds_window: true,
        },
        // Asking for the status the account already carries changes nothing.
        Row {
            label: "two enables",
            requested: &[COMMITMENT_ENABLED, COMMITMENT_ENABLED],
            expected: COMMITMENT_ENABLED,
            holds_window: true,
        },
        // A disable inside the minimum commitment duration only SCHEDULES the end.
        Row {
            label: "a disable inside the minimum commitment duration",
            requested: &[COMMITMENT_ENABLED, COMMITMENT_DISABLED],
            expected: COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
            holds_window: true,
        },
        // And a second disable brings that end no closer.
        Row {
            label: "two disables inside the minimum commitment duration",
            requested: &[
                COMMITMENT_ENABLED,
                COMMITMENT_DISABLED,
                COMMITMENT_DISABLED,
            ],
            expected: COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
            holds_window: true,
        },
        // "You can cancel a pending disable by enabling the commitment again before the
        // earliest allowed end time".
        Row {
            label: "an enable that cancels a pending disable",
            requested: &[COMMITMENT_ENABLED, COMMITMENT_DISABLED, COMMITMENT_ENABLED],
            expected: COMMITMENT_ENABLED,
            holds_window: true,
        },
        // ...and the cancelled disable can be asked for again.
        Row {
            label: "a disable after a cancelled disable",
            requested: &[
                COMMITMENT_ENABLED,
                COMMITMENT_DISABLED,
                COMMITMENT_ENABLED,
                COMMITMENT_DISABLED,
            ],
            expected: COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
            holds_window: true,
        },
        // A disable on an account holding no commitment ends nothing and starts nothing.
        Row {
            label: "a disable with no commitment standing",
            requested: &[COMMITMENT_DISABLED],
            expected: COMMITMENT_DISABLED,
            holds_window: false,
        },
        Row {
            label: "two disables with no commitment standing",
            requested: &[COMMITMENT_DISABLED, COMMITMENT_DISABLED],
            expected: COMMITMENT_DISABLED,
            holds_window: false,
        },
    ]
}

/// Ask for `status` and answer the commitment the update reports.
async fn update(emulator: &Emulator, case: &str, status: &str) -> Value {
    let answered = emulator
        .ok(
            case,
            "UpdateAccountSettings",
            &json!({ COMMITMENT: { "Status": status } }),
        )
        .await;
    commitment_of(case, "UpdateAccountSettings", &answered)
}

/// The commitment `DescribeAccountSettings` reports in `region`.
async fn described_in(emulator: &Emulator, case: &str, region: &str) -> Value {
    let answered = emulator
        .ok_in(case, region, "DescribeAccountSettings", &json!({}))
        .await;
    commitment_of(case, "DescribeAccountSettings", &answered)
}

/// The commitment `DescribeAccountSettings` reports in the default test region.
pub(crate) async fn described(emulator: &Emulator, case: &str) -> Value {
    described_in(emulator, case, REGION).await
}

/// The commitment member of a response, which every account-settings answer carries.
fn commitment_of(case: &str, operation: &str, response: &Value) -> Value {
    response.get(COMMITMENT).cloned().unwrap_or_else(|| {
        panic!("case {case}: {operation} must carry {COMMITMENT}, got {response}")
    })
}

/// The `Status` a reported commitment carries.
pub(crate) fn status_of(commitment: &Value) -> String {
    commitment
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or("<no Status>")
        .to_string()
}

/// The epoch seconds a reported commitment carries under `member`, `None` when it carries
/// none.
pub(crate) fn instant_of(commitment: &Value, member: &str) -> Option<f64> {
    commitment.get(member).and_then(Value::as_f64)
}

/// Epoch seconds at `at`, the unit a `Timestamp` travels as on the wire.
pub(crate) fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// Whatever a sequence of updates stored is what the last update answered and what the next
/// read reports, for every transition the two input statuses can make.
#[tokio::test]
async fn an_update_stores_the_commitment_the_next_read_reports() {
    for row in rows() {
        let case = row.label;
        let emulator = Emulator::new();

        // The instant the FIRST enable of the sequence lands in: the start every later update
        // of the same commitment has to keep.
        let before = SystemTime::now();
        let mut answered: Option<Value> = None;
        for status in row.requested {
            answered = Some(update(&emulator, case, status).await);
        }
        let after = SystemTime::now();

        let read = described(&emulator, case).await;
        if let Some(answered) = &answered {
            assert_eq!(
                answered, &read,
                "case {case}: the update answers the commitment the next read reports, got \
                 {answered} then {read}"
            );
        }

        assert_eq!(
            status_of(&read),
            row.expected,
            "case {case}: expected status {}, got {read}",
            row.expected
        );

        let started_at = instant_of(&read, "StartedAt");
        let earliest = instant_of(&read, "EarliestAllowedEndAt");
        assert_eq!(
            (started_at.is_some(), earliest.is_some()),
            (row.holds_window, row.holds_window),
            "case {case}: expected a commitment window reported: {}, got {read}",
            row.holds_window
        );

        if row.holds_window {
            let started_at = started_at.unwrap_or_default();
            assert!(
                started_at >= epoch_seconds(before) - TIMESTAMP_TOLERANCE
                    && started_at <= epoch_seconds(after) + TIMESTAMP_TOLERANCE,
                "case {case}: StartedAt is the instant the FIRST enable landed in \
                 ({}..={}), got {started_at}",
                epoch_seconds(before),
                epoch_seconds(after)
            );
            // "Commitments have a minimum duration of 24 hours that must be honored before
            // they can be disabled": the earliest allowed end is exactly that far past the
            // start.
            let expected_earliest = started_at + MINIMUM_COMMITMENT_DURATION.as_secs_f64();
            let earliest = earliest.unwrap_or_default();
            assert!(
                (earliest - expected_earliest).abs() <= TIMESTAMP_TOLERANCE,
                "case {case}: expected EarliestAllowedEndAt {expected_earliest} (one minimum \
                 commitment duration past StartedAt), got {earliest}"
            );
        }

        // "The timestamp when the commitment was ended": no row of this table honours the
        // minimum duration, so no commitment of it has ended yet.
        assert_eq!(
            instant_of(&read, "EndedAt"),
            None,
            "case {case}: a commitment that has not ended reports no EndedAt, got {read}"
        );

        // A read changes nothing: a second one reports exactly what the first did.
        let again = described(&emulator, case).await;
        assert_eq!(
            again, read,
            "case {case}: a read leaves the stored commitment as it was, got {again} after \
             {read}"
        );
    }
}

/// Once the minimum duration a commitment has to honour has passed, a disable ends it at the
/// instant of the call — and an end that was SCHEDULED is reported as reached as soon as it
/// falls due, with no further call needed.
#[tokio::test]
async fn a_commitment_past_its_minimum_duration_ends() {
    // (case, how long ago the commitment started, whether a disable was already accepted,
    // the status the account carries now)
    let cases = [
        (
            "a standing commitment whose minimum duration has passed",
            MINIMUM_COMMITMENT_DURATION + Duration::from_secs(60),
            false,
            COMMITMENT_ENABLED,
        ),
        (
            "a scheduled end that has fallen due",
            MINIMUM_COMMITMENT_DURATION + Duration::from_secs(60),
            true,
            COMMITMENT_DISABLED,
        ),
    ];

    for (case, started_ago, end_scheduled, expected_now) in cases {
        let emulator = Emulator::new();
        let now = SystemTime::now();
        let started_at = now - started_ago;
        let earliest_allowed_end_at = started_at + MINIMUM_COMMITMENT_DURATION;

        // A commitment this old cannot be reached over the wire without waiting a day for it,
        // so the stored record is laid down in the very store every handler reads.
        emulator.store().lock().commitment = Some(Commitment {
            started_at,
            earliest_allowed_end_at,
            ends_at: end_scheduled.then_some(earliest_allowed_end_at),
        });

        let read = described(&emulator, case).await;
        assert_eq!(
            status_of(&read),
            expected_now,
            "case {case}: expected status {expected_now}, got {read}"
        );
        let ended_at = instant_of(&read, "EndedAt");
        if end_scheduled {
            // The end the clock owed is reported as reached, at the instant it was scheduled
            // for, by a read that wrote nothing.
            let expected = epoch_seconds(earliest_allowed_end_at);
            assert!(
                ended_at.is_some_and(|at| (at - expected).abs() <= TIMESTAMP_TOLERANCE),
                "case {case}: expected EndedAt {expected}, got {read}"
            );
        } else {
            assert_eq!(
                ended_at, None,
                "case {case}: a standing commitment reports no EndedAt, got {read}"
            );
        }

        // A disable now has no minimum duration left to honour, so the commitment ends at the
        // instant of the call rather than later...
        let before = SystemTime::now();
        let answered = update(&emulator, case, COMMITMENT_DISABLED).await;
        let after = SystemTime::now();
        assert_eq!(
            status_of(&answered),
            COMMITMENT_DISABLED,
            "case {case}: a disable past the minimum duration ends the commitment, got \
             {answered}"
        );
        let ended_at = instant_of(&answered, "EndedAt").unwrap_or_default();
        let (lower, upper) = if end_scheduled {
            // ...except where an end was already scheduled: that end stands, and this call
            // brings it no closer.
            (
                epoch_seconds(earliest_allowed_end_at) - TIMESTAMP_TOLERANCE,
                epoch_seconds(earliest_allowed_end_at) + TIMESTAMP_TOLERANCE,
            )
        } else {
            (
                epoch_seconds(before) - TIMESTAMP_TOLERANCE,
                epoch_seconds(after) + TIMESTAMP_TOLERANCE,
            )
        };
        assert!(
            ended_at >= lower && ended_at <= upper,
            "case {case}: expected EndedAt in {lower}..={upper}, got {answered}"
        );
        assert_eq!(
            described(&emulator, case).await,
            answered,
            "case {case}: the read reports the commitment the disable stored"
        );

        // An enable after the end starts a NEW commitment, whose own minimum duration runs
        // from this instant: the spent window is not resurrected.
        let before = SystemTime::now();
        let restarted = update(&emulator, case, COMMITMENT_ENABLED).await;
        assert_eq!(
            status_of(&restarted),
            COMMITMENT_ENABLED,
            "case {case}: an enable after the end holds a commitment again, got {restarted}"
        );
        let restarted_start = instant_of(&restarted, "StartedAt").unwrap_or_default();
        assert!(
            restarted_start >= epoch_seconds(before) - TIMESTAMP_TOLERANCE,
            "case {case}: a new commitment starts at the instant of the enable (>= {}), got \
             {restarted_start}",
            epoch_seconds(before)
        );
        assert_eq!(
            instant_of(&restarted, "EndedAt"),
            None,
            "case {case}: a commitment that has just started has not ended, got {restarted}"
        );
    }
}

/// The settings are the ACCOUNT's: "the minimum throughput billing commitment for your Amazon
/// Web Services account" (`DescribeAccountSettingsOutput$MinimumThroughputBillingCommitment`),
/// so an update signed for one endpoint is what every endpoint of the account then reports.
#[tokio::test]
async fn the_settings_are_the_account_s_whatever_endpoint_reads_them() {
    let emulator = Emulator::new();
    let case = "account-wide";
    let elsewhere = "eu-west-1";

    let before_any_update = described_in(&emulator, case, elsewhere).await;
    assert_eq!(
        status_of(&before_any_update),
        COMMITMENT_DISABLED,
        "case {case}: an account that never asked for a commitment holds none, got \
         {before_any_update}"
    );

    let answered = update(&emulator, case, COMMITMENT_ENABLED).await;
    assert_eq!(
        described_in(&emulator, case, elsewhere).await,
        answered,
        "case {case}: an update signed for {REGION} is what a read signed for {elsewhere} \
         reports"
    );

    // And a disable signed for the other endpoint lands on that same commitment, keeping the
    // start and the earliest allowed end the enable stamped.
    let (status, response) = emulator
        .call_in(
            elsewhere,
            "UpdateAccountSettings",
            &json!({ COMMITMENT: { "Status": COMMITMENT_DISABLED } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: a disable signed for {elsewhere} is served, got {status} with body \
         {response}"
    );
    let after = described(&emulator, case).await;
    assert_eq!(
        status_of(&after),
        COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
        "case {case}: the disable landed on the commitment the other endpoint enabled, got \
         {after}"
    );
    assert_eq!(
        instant_of(&after, "StartedAt"),
        instant_of(&answered, "StartedAt"),
        "case {case}: the commitment keeps the start the enable stamped, got {after}"
    );
}

/// `MinimumThroughputBillingCommitment.Status` is a modelled enum and the member itself is
/// required, so a request naming a value outside the published catalog, or naming none at
/// all, is refused with nothing stored.
#[tokio::test]
async fn a_request_outside_the_published_catalog_is_refused() {
    let emulator = Emulator::new();
    let case = "refusals";

    // The published catalog of `MinimumThroughputBillingCommitmentInputStatus`
    // (`smithy_query(query="shape",shape="MinimumThroughputBillingCommitmentInputStatus")`)
    // is ENABLED | DISABLED, so each of these names no status the account could carry, and
    // the modelled enum constraint refuses it before the handler runs.
    let refused = [
        ("the output-only status", COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END),
        ("a lowercased status", "enabled"),
        ("an empty status", ""),
        ("a status nothing models", "SUSPENDED"),
    ];
    for (label, status) in refused {
        let (http_status, response) = emulator
            .call(
                "UpdateAccountSettings",
                &json!({ COMMITMENT: { "Status": status } }),
            )
            .await;
        assert_eq!(
            (http_status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}/{label}: a status outside the published catalog is refused, got \
             {http_status} with body {response}"
        );
        let message = error_message(&response);
        // The sentence names the catalog the value is outside...
        assert!(
            message.contains(COMMITMENT_ENABLED) && message.contains(COMMITMENT_DISABLED),
            "case {case}/{label}: the refusal names the catalog the value is outside, got \
             {response}"
        );
        // ...and the value at fault, where the request named one at all.
        assert!(
            status.is_empty() || message.contains(status),
            "case {case}/{label}: the refusal names the value at fault, got {response}"
        );
    }

    // A request naming the required member not at all names nothing to store either.
    for (label, body) in [
        ("no Status", json!({ COMMITMENT: {} })),
        ("no commitment at all", json!({})),
    ] {
        let (http_status, response) = emulator.call("UpdateAccountSettings", &body).await;
        assert_eq!(
            http_status, 400,
            "case {case}/{label}: the required member must be named, got {http_status} with \
             body {response}"
        );
    }

    // Nothing any of the refusals named was stored.
    let read = described(&emulator, case).await;
    assert_eq!(
        status_of(&read),
        COMMITMENT_DISABLED,
        "case {case}: a refused update stores nothing, got {read}"
    );
}
