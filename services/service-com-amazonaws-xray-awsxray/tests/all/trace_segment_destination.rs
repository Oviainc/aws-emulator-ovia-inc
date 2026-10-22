// SPDX-License-Identifier: CC0-1.0

//! The trace segment destination (Transaction Search) configuration, end to end
//! over the wire.
//!
//! Every case drives the same front door the production router does (the `Xray`
//! harness in [`super::trace_ingest_and_groups`]), so the assertions below are
//! about HTTP responses, not internal state.
//!
//! The rules under test:
//!
//! * `GetTraceSegmentDestination` reports the destination the account last
//!   configured, and the account default (`XRay`) until something configures
//!   one; its input models no members, so nothing in its answer can come from
//!   the request;
//! * a configured destination is `PENDING` until the switch becomes effective
//!   and `ACTIVE` from then on, and reading it neither starts nor skips that
//!   move: repeated reads at the same instant answer the same thing;
//! * configuring the destination already in effect applies no switch, so it
//!   stays `ACTIVE`;
//! * a destination outside the model's `TraceSegmentDestination` value set, an
//!   absent destination, a `CloudWatchLogsConfiguration` beside a destination
//!   other than `CloudWatchLogs`, and a `SpansLogGroupKmsKeyId` that is not a
//!   KMS key ARN are each refused at HTTP 400 — a value outside the MODELLED
//!   enum by the request layer as `ValidationException`, a rule this service
//!   enforces itself by the handler as the modelled `InvalidRequestException` —
//!   and a refused call changes nothing.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::types::TRACE_SEGMENT_DESTINATION_VALUES;
use service_com_amazonaws_xray_awsxray::state::{
    DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_ACTIVATION_SPAN, DESTINATION_STATUS_ACTIVE,
    DESTINATION_STATUS_PENDING,
};

use super::xray_wire::{Answer, Xray};

/// Long enough that the `PENDING` -> `ACTIVE` move the clock owes is certainly
/// due, without encoding a second duration of its own.
const PAST_ACTIVATION: Duration =
    DESTINATION_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// A valid KMS key ARN, the only form `SpansLogGroupKmsKeyId` accepts per that
/// member's documentation ("Key IDs and alias ARNs are not supported").
const KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// A member of an answer's body as a string, or `None`.
fn member<'a>(answer: &'a Answer, name: &str) -> Option<&'a str> {
    answer.get(name).as_str()
}

/// `(Destination, Status)`: the pair both operations here answer with.
fn destination_and_status(answer: &Answer) -> (Option<&str>, Option<&str>) {
    (member(answer, "Destination"), member(answer, "Status"))
}

async fn get_destination(xray: &Xray) -> Answer {
    xray.call("/GetTraceSegmentDestination", &json!({})).await
}

async fn update_destination(xray: &Xray, body: &Value) -> Answer {
    xray.call("/UpdateTraceSegmentDestination", body).await
}

/// Configure `destination` and leave it in effect.
async fn settled_destination(xray: &Xray, destination: &str) {
    let updated = update_destination(xray, &json!({ "Destination": destination })).await;
    assert_eq!(
        updated.status, 200,
        "seeding destination {destination:?} must succeed, got {updated:?}"
    );
    tokio::time::sleep(PAST_ACTIVATION).await;
}

// =============================================================================
// 1. The default destination is reported until something configures one
// =============================================================================

#[tokio::test]
async fn the_account_default_destination_is_reported_and_reading_it_changes_nothing() {
    let xray = Xray::new();

    // Three reads in a row: a reporting operation must answer the same thing
    // each time, and the first read must not configure anything.
    for attempt in 1..=3 {
        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (
                    Some(DEFAULT_TRACE_SEGMENT_DESTINATION),
                    Some(DESTINATION_STATUS_ACTIVE)
                )
            ),
            "case read #{attempt} of a never-configured account: expected 200 \
             {DEFAULT_TRACE_SEGMENT_DESTINATION}/{DESTINATION_STATUS_ACTIVE}, got {read:?}"
        );
    }

    // The default is one of the modelled values, not an invention.
    assert!(
        TRACE_SEGMENT_DESTINATION_VALUES.contains(&DEFAULT_TRACE_SEGMENT_DESTINATION),
        "case default value: {DEFAULT_TRACE_SEGMENT_DESTINATION:?} must be one of \
         {TRACE_SEGMENT_DESTINATION_VALUES:?}"
    );
}

// =============================================================================
// 2. A switch is PENDING until the clock makes it ACTIVE, and Get reports it
// =============================================================================

/// One destination switch: from the destination in effect, to the one asked
/// for. The rows span every ordered pair of distinct values the model's
/// `TraceSegmentDestination` enum publishes
/// (`smithy_query(query="shape",shape="TraceSegmentDestination")`), so each
/// value is exercised as both the source and the target of a switch.
fn switch_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for from in TRACE_SEGMENT_DESTINATION_VALUES {
        for to in TRACE_SEGMENT_DESTINATION_VALUES {
            if from != to {
                rows.push(((*from).to_string(), (*to).to_string()));
            }
        }
    }
    rows
}

#[tokio::test]
async fn a_destination_switch_is_pending_then_the_clock_makes_it_active() {
    for (from, to) in switch_rows() {
        let case = format!("{from} -> {to}");
        let xray = Xray::new();
        if from != DEFAULT_TRACE_SEGMENT_DESTINATION {
            settled_destination(&xray, &from).await;
        }
        let before = get_destination(&xray).await;
        assert_eq!(
            destination_and_status(&before),
            (Some(from.as_str()), Some(DESTINATION_STATUS_ACTIVE)),
            "case {case:?}: the starting destination must be in effect, got {before:?}"
        );

        let updated = update_destination(&xray, &json!({ "Destination": to.clone() })).await;
        assert_eq!(
            (updated.status, destination_and_status(&updated)),
            (200, (Some(to.as_str()), Some(DESTINATION_STATUS_PENDING))),
            "case {case:?}: an accepted switch answers the new destination as \
             {DESTINATION_STATUS_PENDING}, got {updated:?}"
        );

        // The transient state is observable: a caller reading straight back
        // sees PENDING, twice, because a read does not settle the move early.
        for attempt in 1..=2 {
            let pending = get_destination(&xray).await;
            assert_eq!(
                (pending.status, destination_and_status(&pending)),
                (200, (Some(to.as_str()), Some(DESTINATION_STATUS_PENDING))),
                "case {case:?} read #{attempt} before the switch is due: expected 200 \
                 {to}/{DESTINATION_STATUS_PENDING}, got {pending:?}"
            );
        }

        tokio::time::sleep(PAST_ACTIVATION).await;

        for attempt in 1..=2 {
            let active = get_destination(&xray).await;
            assert_eq!(
                (active.status, destination_and_status(&active)),
                (200, (Some(to.as_str()), Some(DESTINATION_STATUS_ACTIVE))),
                "case {case:?} read #{attempt} after the switch is due: expected 200 \
                 {to}/{DESTINATION_STATUS_ACTIVE}, got {active:?}"
            );
        }
    }
}

// =============================================================================
// 3. Configuring the destination already in effect applies no switch
// =============================================================================

#[tokio::test]
async fn re_configuring_the_destination_in_effect_leaves_it_active() {
    for destination in TRACE_SEGMENT_DESTINATION_VALUES {
        let xray = Xray::new();
        if *destination != DEFAULT_TRACE_SEGMENT_DESTINATION {
            settled_destination(&xray, destination).await;
        }

        let repeated = update_destination(&xray, &json!({ "Destination": destination })).await;
        assert_eq!(
            (repeated.status, destination_and_status(&repeated)),
            (200, (Some(*destination), Some(DESTINATION_STATUS_ACTIVE))),
            "case {destination:?} re-configured: nothing switches, so it stays \
             {DESTINATION_STATUS_ACTIVE}, got {repeated:?}"
        );

        let read = get_destination(&xray).await;
        assert_eq!(
            destination_and_status(&read),
            (Some(*destination), Some(DESTINATION_STATUS_ACTIVE)),
            "case {destination:?} re-configured: the read-back must agree, got {read:?}"
        );
    }
}

// =============================================================================
// 4. A CloudWatchLogsConfiguration is accepted with the CloudWatchLogs
//    destination, and the destination still round-trips through Get
// =============================================================================

#[tokio::test]
async fn a_cloud_watch_logs_configuration_round_trips_with_its_destination() {
    // Rows differ in whether a configuration is supplied at all, and in the
    // key ARN supplied with it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no CloudWatchLogsConfiguration",
            json!({"Destination": "CloudWatchLogs"}),
        ),
        (
            "an empty CloudWatchLogsConfiguration",
            json!({"Destination": "CloudWatchLogs", "CloudWatchLogsConfiguration": {}}),
        ),
        (
            "a customer-managed key ARN",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN},
            }),
        ),
        (
            "a key ARN in another partition and region",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId":
                        "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/abcd-ef01",
                },
            }),
        ),
    ];

    for (case, body) in rows {
        let xray = Xray::new();
        let updated = update_destination(&xray, &body).await;
        assert_eq!(
            (updated.status, destination_and_status(&updated)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_PENDING))
            ),
            "case {case:?}: expected 200 CloudWatchLogs/{DESTINATION_STATUS_PENDING}, \
             got {updated:?}"
        );

        tokio::time::sleep(PAST_ACTIVATION).await;
        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_ACTIVE))
            ),
            "case {case:?}: the configured destination must read back as \
             {DESTINATION_STATUS_ACTIVE}, got {read:?}"
        );
    }
}

// =============================================================================
// 5. The stored CloudWatchLogsConfiguration belongs to its destination
// =============================================================================

/// No modelled output returns `SpansLogGroupKmsKeyId`, so this case reads the
/// stored record directly: the supplied key is kept while the service points at
/// `CloudWatchLogs` and dropped when it points elsewhere.
#[tokio::test]
async fn the_stored_spans_log_group_key_follows_the_cloud_watch_logs_destination() {
    use service_com_amazonaws_xray_awsxray::state::{
        ComAmazonawsXrayAwsxrayState, CLOUD_WATCH_LOGS_DESTINATION,
    };

    let mut store = ComAmazonawsXrayAwsxrayState::default();
    let now = std::time::Instant::now();

    // Rows: a key supplied with CloudWatchLogs is stored; the same destination
    // re-configured without one keeps it; pointing elsewhere drops it.
    let stored = store.set_trace_segment_destination(CLOUD_WATCH_LOGS_DESTINATION, Some(KEY_ARN), now);
    assert_eq!(
        stored.spans_log_group_kms_key_id.as_deref(),
        Some(KEY_ARN),
        "case a key supplied with {CLOUD_WATCH_LOGS_DESTINATION}: expected it stored, \
         got {stored:?}"
    );

    let kept = store.set_trace_segment_destination(CLOUD_WATCH_LOGS_DESTINATION, None, now);
    assert_eq!(
        kept.spans_log_group_kms_key_id.as_deref(),
        Some(KEY_ARN),
        "case {CLOUD_WATCH_LOGS_DESTINATION} re-configured without a key: expected the \
         stored key kept, got {kept:?}"
    );

    let dropped =
        store.set_trace_segment_destination(DEFAULT_TRACE_SEGMENT_DESTINATION, None, now);
    assert_eq!(
        dropped.spans_log_group_kms_key_id, None,
        "case switched to {DEFAULT_TRACE_SEGMENT_DESTINATION}: the configuration applies \
         only to {CLOUD_WATCH_LOGS_DESTINATION}, so expected no stored key, got {dropped:?}"
    );
    assert_eq!(
        dropped.status(),
        DESTINATION_STATUS_PENDING,
        "case switched to {DEFAULT_TRACE_SEGMENT_DESTINATION}: a real switch is \
         {DESTINATION_STATUS_PENDING}, got {dropped:?}"
    );
}

// =============================================================================
// 6. An invalid configuration request is refused, and changes nothing
// =============================================================================

#[tokio::test]
async fn an_invalid_destination_request_is_refused_and_stores_nothing() {
    // Rows span the ways the request can be invalid: no destination at all, a
    // value outside the model's enum (unknown, miscased, empty), a
    // configuration beside the wrong destination, and every
    // `SpansLogGroupKmsKeyId` form the member's documentation excludes.
    //
    // The third column is the code that class of refusal carries: a value
    // outside the MODELLED enum is refused by the request layer, which answers
    // the `ValidationException` envelope, while a rule this service enforces
    // itself is refused by the handler as `InvalidRequestException`.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("Destination absent", json!({}), "InvalidRequestException"),
        (
            "Destination absent but a configuration supplied",
            json!({"CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN}}),
            "InvalidRequestException",
        ),
        (
            "Destination empty",
            json!({"Destination": ""}),
            "ValidationException",
        ),
        (
            "Destination outside the model's value set",
            json!({"Destination": "S3"}),
            "ValidationException",
        ),
        (
            "Destination miscased",
            json!({"Destination": "cloudwatchlogs"}),
            "ValidationException",
        ),
        (
            "Destination with surrounding whitespace",
            json!({"Destination": " CloudWatchLogs "}),
            "ValidationException",
        ),
        (
            "a configuration beside the XRay destination",
            json!({
                "Destination": "XRay",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": KEY_ARN},
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as a bare key id",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                },
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as an alias name",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {"SpansLogGroupKmsKeyId": "alias/spans"},
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId as an alias ARN",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "arn:aws:kms:us-east-1:123456789012:alias/spans",
                },
            }),
            "InvalidRequestException",
        ),
        (
            "SpansLogGroupKmsKeyId naming another service's resource",
            json!({
                "Destination": "CloudWatchLogs",
                "CloudWatchLogsConfiguration": {
                    "SpansLogGroupKmsKeyId": "arn:aws:logs:us-east-1:123456789012:key/1234abcd",
                },
            }),
            "InvalidRequestException",
        ),
    ];

    for (case, body, code) in rows {
        // Start from a destination that is NOT the default, so a refusal that
        // wrote anything — including a reset to the default — would show.
        let xray = Xray::new();
        settled_destination(&xray, "CloudWatchLogs").await;

        let refused = update_destination(&xray, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(code)),
            "case {case:?}: expected 400 {code}, got {refused:?}"
        );
        // A framework refusal renders `message`; a modelled error shape renders
        // its member's own casing (`Message`).
        let message = member(&refused, "message").or_else(|| member(&refused, "Message"));
        assert!(
            message.is_some_and(|message| !message.is_empty()),
            "case {case:?}: a refusal must say what was wrong, got {refused:?}"
        );

        let read = get_destination(&xray).await;
        assert_eq!(
            (read.status, destination_and_status(&read)),
            (
                200,
                (Some("CloudWatchLogs"), Some(DESTINATION_STATUS_ACTIVE))
            ),
            "case {case:?}: a refused request must change nothing, got {read:?}"
        );
    }
}
