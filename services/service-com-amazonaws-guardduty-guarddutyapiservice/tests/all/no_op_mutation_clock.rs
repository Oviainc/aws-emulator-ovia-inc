// SPDX-License-Identifier: CC0-1.0

//! What an ACCEPTED call that changes nothing leaves behind, end to end over the
//! wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test is the one the `refused_create_clock` and
//! `refused_mutation_clock` suites assert for the calls that are REFUSED, here over
//! the SET of calls that are ACCEPTED and still write nothing: a mutating operation
//! reads the service's monotonic "this resource changed now" clock only on the path
//! that actually writes, so a call which leaves the store exactly as it found it
//! consumes no instant either — and the `CreatedAt`/`UpdatedAt` a LATER accepted call
//! stamps therefore still reports the wall-clock instant of that change rather than
//! one pushed into the future by the no-ops before it.
//!
//! Observable per request: the clock a mutation reads is what the detector's own
//! `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String), so a burst of
//! no-op calls between two accepted mutations is enough to see whether the no-ops
//! consumed instants: the clock may advance across the burst only by the wall-clock
//! time that really passed over the same span, whereas a handler that reads the clock
//! before it knows it is going to write spends an instant per call on top of that.
//!
//! The rows differ along what the rule generalises over: the operation and the
//! resource family, which member the request writes back, whether it carries the
//! activation the resource already has, and whether the resource is a sub-resource or
//! the detector's own configuration. A delete repeating one already served is not
//! among them: it is REFUSED rather than accepted, so it is a row of
//! `refused_mutation_clock` instead.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router};
use crate::refused_create_clock::{as_millis, detector_updated_at, READING_SLACK_MILLIS};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

/// A KMS key ARN, the form `DestinationProperties.KmsKeyArn` carries
/// (`smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// An S3 ARN, the form `DestinationProperties.DestinationArn` carries for the one
/// `DestinationType` the model declares (`smithy_query(shape="DestinationType")`).
const BUCKET_ARN: &str = "arn:aws:s3:::guardduty-findings/no-op/";

/// An `ExpectedBucketOwner`: 12 digits (`smithy_query(shape="ExpectedBucketOwner")`).
const BUCKET_OWNER: &str = "111122223333";

/// How many no-op calls one burst sends. A call that writes nothing consumes no
/// instant, so the burst may be any size; it is this large because a handler that
/// reads the clock BEFORE it knows whether it writes spends at least one millisecond
/// of it per call, and the burst must advance that clock by more than
/// [`READING_SLACK_MILLIS`] past the wall-clock time the burst itself takes for the
/// difference to be an assertion.
const NO_OPS_PER_BURST: usize = 300;

/// One row: a resource to seed, the accepted call that changes nothing about it, and
/// an accepted create afterwards whose timestamp shows what the burst did to the
/// clock.
struct Row {
    case: &'static str,
    /// The segment under `/detector/{detectorId}` the resource lives at, from each
    /// operation's `http` binding
    /// (`smithy_query(operation="UpdateIPSet,UpdateTrustedEntitySet,\
    /// UpdateThreatEntitySet,UpdateThreatIntelSet,UpdatePublishingDestination,\
    /// UpdateMalwareScanSettings")`).
    segment: &'static str,
    /// The create that seeds the resource, with the output member carrying the id it
    /// mints. `None` for the detector's own scan settings, which no create mints:
    /// that row addresses the segment itself.
    create: Option<(Value, &'static str)>,
    /// The no-op call: its method and the body it carries, which repeats what the
    /// seeded resource already holds.
    method: &'static str,
    body: Option<Value>,
    /// Read the resource back before and after the burst and require the two readings
    /// to be identical. Off where the read reports a status the CLOCK is owed, which
    /// moves on its own whatever the burst did.
    compare_record: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "update trusted entity set, writing back the stored name and location",
            segment: "trustedentityset",
            create: Some((
                json!({
                    "name": "trusted-entities",
                    "format": "TXT",
                    "location": "s3://guardduty-lists/trusted-entities.txt",
                    "activate": false,
                }),
                "trustedEntitySetId",
            )),
            method: "POST",
            body: Some(json!({
                "name": "trusted-entities",
                "location": "s3://guardduty-lists/trusted-entities.txt",
            })),
            compare_record: true,
        },
        Row {
            case: "update trusted entity set, repeating the activation it already has",
            segment: "trustedentityset",
            create: Some((
                json!({
                    "name": "trusted_entities_2",
                    "format": "OTX_CSV",
                    "location": "s3://guardduty-lists/trusted-entities.csv",
                    "activate": false,
                }),
                "trustedEntitySetId",
            )),
            method: "POST",
            // `Activate` asks for the status the set has already settled on, so it
            // starts no transition — the one member this row changes along.
            body: Some(json!({"activate": false})),
            compare_record: true,
        },
        Row {
            case: "update threat entity set, writing back the stored expectedBucketOwner",
            segment: "threatentityset",
            create: Some((
                json!({
                    "name": "threat-entities",
                    "format": "TXT",
                    "location": "s3://guardduty-lists/threat-entities.txt",
                    "expectedBucketOwner": BUCKET_OWNER,
                    "activate": false,
                }),
                "threatEntitySetId",
            )),
            method: "POST",
            body: Some(json!({"expectedBucketOwner": BUCKET_OWNER})),
            compare_record: true,
        },
        Row {
            case: "update ip set, writing back the stored name",
            segment: "ipset",
            create: Some((
                json!({
                    "name": "trusted-addresses",
                    "format": "TXT",
                    "location": "s3://guardduty-lists/trusted-addresses.txt",
                    "activate": false,
                }),
                "ipSetId",
            )),
            method: "POST",
            body: Some(json!({
                "name": "trusted-addresses",
                "activate": false,
            })),
            compare_record: true,
        },
        Row {
            case: "update threat intel set, writing back the stored location",
            segment: "threatintelset",
            create: Some((
                json!({
                    "name": "threat-list",
                    "format": "STIX",
                    "location": "s3://guardduty-lists/threat-list.xml",
                    "activate": false,
                }),
                "threatIntelSetId",
            )),
            method: "POST",
            body: Some(json!({
                "location": "s3://guardduty-lists/threat-list.xml",
                "activate": false,
            })),
            compare_record: true,
        },
        Row {
            case: "update publishing destination, writing back the stored properties",
            segment: "publishingDestination",
            create: Some((
                json!({
                    "destinationType": "S3",
                    "destinationProperties": {
                        "destinationArn": BUCKET_ARN,
                        "kmsKeyArn": KEY_ARN,
                    },
                }),
                "destinationId",
            )),
            method: "POST",
            body: Some(json!({
                "destinationProperties": {
                    "destinationArn": BUCKET_ARN,
                    "kmsKeyArn": KEY_ARN,
                },
            })),
            // A destination the create left PENDING_VERIFICATION reaches PUBLISHING on
            // the clock alone, so the two readings need not agree.
            compare_record: false,
        },
        Row {
            case: "update malware scan settings, carrying no setting at all",
            segment: "malware-scan-settings",
            create: None,
            method: "POST",
            body: Some(json!({})),
            compare_record: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// The rule: an accepted call that writes nothing consumes no instant, so the next
// accepted mutation's timestamp is still a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepted_no_op_calls_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        // The resource the no-op acts on, and the path that addresses it.
        let collection = format!("/detector/{detector_id}/{}", row.segment);
        let target = match row.create.as_ref() {
            Some((create_body, id_member)) => {
                let (status, created) =
                    call(&router, "POST", &collection, Some(create_body.clone())).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the seeding create to answer 200, got {status} \
                     {created:?}"
                );
                let minted = created[*id_member].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a {id_member}, got {created:?}")
                });
                format!("{collection}/{minted}")
            }
            // The detector's own scan settings are addressed by the segment itself.
            None => collection.clone(),
        };

        // What the resource reports before the burst, where that reading is one the
        // clock cannot move on its own.
        let before_record = if row.compare_record {
            let (status, read) = call(&router, "GET", &target, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a read of the seeded resource to answer 200, got \
                 {status} {read:?}"
            );
            Some(read)
        } else {
            None
        };

        // The instant the last ACCEPTED write stamped, and the wall clock just after
        // it: between these two readings and the accepted create below, every call the
        // service sees changes nothing, so the clock may advance only by the time that
        // really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        for attempt in 0..NO_OPS_PER_BURST {
            let (status, answered) =
                call(&router, row.method, &target, row.body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: no-op call {attempt} must answer 200, got {status} {answered:?}"
            );
        }

        // A call that changes nothing changes nothing about the resource either.
        if let Some(before_record) = before_record {
            let (status, after_record) = call(&router, "GET", &target, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a read after the burst to answer 200, got {status} \
                 {after_record:?}"
            );
            assert_eq!(
                after_record, before_record,
                "case {case:?}: {NO_OPS_PER_BURST} no-op calls must leave the resource exactly \
                 as it was, expected {before_record:?}, got {after_record:?}"
            );
        }

        // Nor about the detector that holds it: the set of sub-resources it holds and
        // its configuration are both unchanged.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {NO_OPS_PER_BURST} no-op calls must not move the detector's \
             updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );

        // The accepted create right after the burst: it writes, so it stamps the
        // instant it happened — and that instant is a WALL-CLOCK instant, not one the
        // no-ops pushed into the future.
        let probe_collection = format!("/detector/{detector_id}/ipset");
        let (status, created) = call(
            &router,
            "POST",
            &probe_collection,
            Some(json!({
                "name": "probe-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-ip-set.txt",
                "activate": false,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {created:?}"
        );
        let minted = created["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {created:?}"))
            .to_string();

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted create changed the detector, so its updatedAt must move \
             past {after_burst:?}, got {stamped:?}"
        );
        // The measurement: how far the clock moved across the burst, against how far
        // the WALL clock moved across the same span. A call that writes nothing
        // consumes no instant, so the two agree to within the slack above however long
        // the burst took — where a handler that read the clock before knowing whether
        // it writes would have spent an instant per call on top of it.
        let elapsed = wall_after - wall_before;
        let advance = as_millis(&stamped, case) - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {NO_OPS_PER_BURST} no-op calls consume no instant, so the clock may \
             advance only by the {elapsed} ms that really passed (plus {READING_SLACK_MILLIS} ms \
             of slack), got {advance} ms (from {before_burst:?} to {stamped:?})"
        );

        // Round trip: the IPSet the probing create minted is one a read resolves, so
        // the timestamp asserted above records a change that really landed.
        let (status, read_back) =
            call(&router, "GET", &format!("{probe_collection}/{minted}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted ipSetId to answer 200, got {status} \
             {read_back:?}"
        );
    }
}
