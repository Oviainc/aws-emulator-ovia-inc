// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED create leaves behind, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, asserted over the SET of detector sub-resource creates and
//! over the SET of ways one of them is refused rather than over a single call: a
//! mutating operation reads the service's monotonic "this resource changed now"
//! clock only on the path that actually writes, so a refused create changes nothing
//! at all — it neither mints a resource nor consumes an instant — and the
//! `CreatedAt`/`UpdatedAt` a LATER accepted call stamps therefore still reports the
//! wall-clock instant of that change rather than one pushed into the future by the
//! refusals before it.
//!
//! Observable per request: the clock a create reads is what the detector's own
//! `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String), so a burst of
//! refused creates between two accepted mutations is enough to see whether the
//! refusals consumed instants: the clock may move across the burst only by the
//! wall-clock time that really passed over the same span, whereas a create that
//! reads the clock before it refuses spends an instant per refused call on top of
//! that.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    now_millis, IP_SETS_PER_DETECTOR,
};

/// A KMS key ARN, the form `DestinationProperties.KmsKeyArn` carries
/// (`smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// How many refused creates one burst sends. A refused create consumes no instant,
/// so the burst may be any size; it is this large because a handler that reads the
/// clock BEFORE it refuses spends at least one millisecond of it per refused call,
/// and the burst must advance that clock by more than [`READING_SLACK_MILLIS`] past
/// the wall-clock time the burst itself takes for the difference to be an assertion.
const REFUSALS_PER_BURST: usize = 300;

/// What the measurement below allows on top of the wall-clock time that really
/// passed: the clock is strictly monotonic, so the accepted create that closes the
/// measurement sits one millisecond past the instant that opened it even when no
/// time passed at all, and the wall-clock reading that opens the measurement is
/// taken a request later than the mutation that stamped that instant.
const READING_SLACK_MILLIS: i64 = 25;

/// One row: a family of detector sub-resource create, one way of refusing it, and an
/// accepted create whose timestamp shows what the refusals did to the clock.
struct Row {
    case: &'static str,
    /// IPSets created on the detector before the burst, so the burst is refused by
    /// the per-detector ceiling rather than by the id it names.
    fill_ip_sets: usize,
    /// `true` when the refused creates name a `DetectorId` the store does not hold,
    /// `false` when they are addressed to the real detector and refused by what they
    /// carry or by the ceiling they hit.
    absent_detector: bool,
    /// The collection segment under `/detector/{detectorId}` the refused creates POST
    /// to, from each create's `http` binding
    /// (`smithy_query(operation="CreateIPSet,CreateThreatIntelSet,\
    /// CreateThreatEntitySet,CreateTrustedEntitySet,CreatePublishingDestination")`).
    refused_segment: &'static str,
    refused_body: Value,
    /// The accepted create that probes the clock afterwards: its segment, the output
    /// member carrying the minted id (same query), and its body.
    probe_segment: &'static str,
    probe_id_member: &'static str,
    probe_body: Value,
}

/// A create of each sub-resource family this detector holds, refused in each of the
/// ways these handlers refuse one: an unresolvable `DetectorId`, a per-detector
/// ceiling, and a member outside the value set its enum declares. The rows differ
/// along what the rule generalises over — the family, the refusal, and whether the
/// refused call ever reaches the store at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ip set, refused by a DetectorId the store does not hold",
            fill_ip_sets: 0,
            absent_detector: true,
            refused_segment: "ipset",
            refused_body: json!({
                "name": "refused-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/refused-ip-set.txt",
                "activate": true,
            }),
            probe_segment: "ipset",
            probe_id_member: "ipSetId",
            probe_body: json!({
                "name": "probe-ip-set",
                "format": "STIX",
                "location": "s3://guardduty-lists/probe-ip-set.xml",
                "activate": false,
            }),
        },
        Row {
            case: "ip set, refused by the per-detector ceiling",
            fill_ip_sets: IP_SETS_PER_DETECTOR,
            absent_detector: false,
            refused_segment: "ipset",
            refused_body: json!({
                "name": "past-the-ceiling",
                "format": "OTX_CSV",
                "location": "s3://guardduty-lists/past-the-ceiling.csv",
                "activate": true,
            }),
            // The IPSet collection is full, so the probe is another family's create.
            probe_segment: "trustedentityset",
            probe_id_member: "trustedEntitySetId",
            probe_body: json!({
                "name": "probe-trusted-entities",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-trusted-entities.txt",
                "activate": true,
            }),
        },
        Row {
            case: "publishing destination, refused by a DetectorId the store does not hold",
            fill_ip_sets: 0,
            absent_detector: true,
            refused_segment: "publishingDestination",
            refused_body: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/refused/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            probe_segment: "publishingDestination",
            probe_id_member: "destinationId",
            probe_body: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/probe/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
        },
        Row {
            // `DestinationType` declares exactly one member, S3
            // (`smithy_query(shape="DestinationType")`), so this names a value
            // outside it.
            case: "publishing destination, refused by a destinationType outside the enum",
            fill_ip_sets: 0,
            absent_detector: false,
            refused_segment: "publishingDestination",
            refused_body: json!({
                "destinationType": "KINESIS",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/refused/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            probe_segment: "publishingDestination",
            probe_id_member: "destinationId",
            probe_body: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/probe/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
        },
        Row {
            case: "threat intel set, refused by a DetectorId the store does not hold",
            fill_ip_sets: 0,
            absent_detector: true,
            refused_segment: "threatintelset",
            refused_body: json!({
                "name": "refused-threat-list",
                "format": "STIX",
                "location": "s3://guardduty-lists/refused-threat-list.xml",
                "activate": false,
            }),
            probe_segment: "threatintelset",
            probe_id_member: "threatIntelSetId",
            probe_body: json!({
                "name": "probe-threat-list",
                "format": "PROOF_POINT",
                "location": "s3://guardduty-lists/probe-threat-list.txt",
                "activate": true,
            }),
        },
        Row {
            case: "threat entity set, refused by a DetectorId the store does not hold",
            fill_ip_sets: 0,
            absent_detector: true,
            refused_segment: "threatentityset",
            refused_body: json!({
                "name": "refused-threat-entities",
                "format": "ALIEN_VAULT",
                "location": "s3://guardduty-lists/refused-threat-entities.txt",
                "activate": true,
            }),
            probe_segment: "threatentityset",
            probe_id_member: "threatEntitySetId",
            probe_body: json!({
                "name": "probe-threat-entities",
                "format": "FIRE_EYE",
                "location": "s3://guardduty-lists/probe-threat-entities.txt",
                "activate": false,
            }),
        },
        Row {
            case: "trusted entity set, refused by a DetectorId the store does not hold",
            fill_ip_sets: 0,
            absent_detector: true,
            refused_segment: "trustedentityset",
            refused_body: json!({
                "name": "refused-trusted-entities",
                "format": "TXT",
                "location": "s3://guardduty-lists/refused-trusted-entities.txt",
                "activate": false,
            }),
            probe_segment: "trustedentityset",
            probe_id_member: "trustedEntitySetId",
            probe_body: json!({
                "name": "probe-trusted-entities",
                "format": "OTX_CSV",
                "location": "s3://guardduty-lists/probe-trusted-entities.csv",
                "activate": true,
            }),
        },
    ]
}

/// The epoch milliseconds an ISO-8601 instant names, so two of the strings a client
/// is given can be subtracted. Read through the same date-time implementation the
/// service renders them with, rather than by picking the text apart here.
fn as_millis(text: &str, case: &str) -> i64 {
    let seconds = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("case {case:?}: {text:?} is not an ISO-8601 instant: {error}"))
    .as_secs_f64();
    (seconds * 1000.0).round() as i64
}

/// The detector's own `UpdatedAt`, as the string a client sees.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The rule: refused creates consume no instant, so the next accepted create's
// timestamp is still a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refused_creates_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        // The IPSets a ceiling row needs before the burst can hit it.
        for index in 0..row.fill_ip_sets {
            let (status, filled) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/ipset"),
                Some(json!({
                    "name": format!("filler-ip-set-{index}"),
                    "format": "TXT",
                    "location": format!("s3://guardduty-lists/filler-{index}.txt"),
                    "activate": false,
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected filler ip set {index} to be accepted, got {status} \
                 {filled:?}"
            );
        }

        let refused_collection = if row.absent_detector {
            format!("/detector/{ABSENT_DETECTOR_ID}/{}", row.refused_segment)
        } else {
            format!("/detector/{detector_id}/{}", row.refused_segment)
        };
        // The instant the last ACCEPTED mutation stamped, and the wall clock just
        // after it: between these two readings and the accepted create below, the
        // only calls the service sees are refused ones, so the clock may advance
        // only by the time that really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        for attempt in 0..REFUSALS_PER_BURST {
            let (status, refusal) = call(
                &router,
                "POST",
                &refused_collection,
                Some(row.refused_body.clone()),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: refused create {attempt} must answer 400, got {status} {refusal:?}"
            );
        }

        // A refused create changes nothing, which the detector that was supposed to
        // be left alone reports first of all.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {REFUSALS_PER_BURST} refused creates must not move the detector's \
             updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );

        // The accepted create right after the burst: it writes, so it stamps the
        // instant it happened — and that instant is a WALL-CLOCK instant, not one the
        // refusals pushed into the future.
        let probe_collection = format!("/detector/{detector_id}/{}", row.probe_segment);
        let (status, created) = call(
            &router,
            "POST",
            &probe_collection,
            Some(row.probe_body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {created:?}"
        );
        let minted = created[row.probe_id_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {}, got {created:?}",
                    row.probe_id_member
                )
            })
            .to_string();

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted create changed the detector, so its updatedAt must move \
             past {after_burst:?}, got {stamped:?}"
        );
        // The measurement: how far the clock moved across the burst, against how far
        // the WALL clock moved across the same span. A refused create consumes no
        // instant, so the two agree to within the slack above however long the burst
        // took — where a create that read the clock before refusing would have spent
        // an instant per refused call on top of it.
        let elapsed = wall_after - wall_before;
        let advance = as_millis(&stamped, case) - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {REFUSALS_PER_BURST} refused creates consume no instant, so the clock \
             may advance only by the {elapsed} ms that really passed (plus {READING_SLACK_MILLIS} \
             ms of slack), got {advance} ms (from {before_burst:?} to {stamped:?})"
        );

        // Round trip: the resource the probing create minted is one a read resolves,
        // so the timestamp asserted above records a change that really landed.
        let (status, read_back) = call(
            &router,
            "GET",
            &format!("{probe_collection}/{minted}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted {} to answer 200, got {status} \
             {read_back:?}",
            row.probe_id_member
        );
    }
}
