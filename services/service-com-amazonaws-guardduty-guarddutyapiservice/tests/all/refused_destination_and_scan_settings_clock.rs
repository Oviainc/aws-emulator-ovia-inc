// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED `DeletePublishingDestination` or `UpdateMalwareScanSettings`
//! leaves behind, over EVERY way each of the two is refused, end to end over the
//! wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test is the one the `refused_create_clock` and
//! `refused_mutation_clock` suites assert over the SET of creates and over the SET of
//! detector-scoped mutations; this suite asserts it for these two handlers over the
//! SET of REFUSALS each of them answers, which is the dimension those two suites
//! sample only one row of per operation: a mutating operation reads the service's
//! monotonic "this resource changed now" clock only on the path that actually writes,
//! so however a call is refused it changes nothing at all — it neither moves a
//! resource nor consumes an instant — and the `CreatedAt`/`UpdatedAt` a LATER accepted
//! call stamps therefore still reports the wall-clock instant of that change rather
//! than one pushed into the future by the refusals before it.
//!
//! Observable per row: the clock these two handlers read is what the detector's own
//! `UpdatedAt` records (`smithy_query(operation="GetDetector")` models it as a
//! String), so a burst of refused calls between two accepted mutations shows whether
//! the refusals consumed instants — the clock may advance across the burst only by
//! the wall-clock time that really passed, and the instant the accepted call that
//! closes the burst stamps may not sit in the future of the wall clock read just
//! after it. The accepted call that closes each row is the operation the row refuses,
//! in the form that writes, so the timestamp measured is that handler's own.
//!
//! The rows span what the rule generalises over: the OPERATION, the member at fault
//! (`detectorId`, `destinationId`, `scanResourceCriteria`), the FORM of the offending
//! value (well formed but unheld, or a value that could never have been minted), the
//! Region the request is signed for, and whether the refusal is decided from the
//! request before the store is reached at all or from the store under the lock.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::ops::types::shared_types::SCAN_CRITERION_KEY_VALUES;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    now_millis, DESTINATION_ABSENT_MESSAGE, DETECTOR_ID_MEMBER, RESERVED_TAG_PREFIX,
    SCAN_RESOURCE_CRITERIA_MEMBER,
};

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::refused_create_clock::{
    as_millis, detector_updated_at, READING_SLACK_MILLIS, REFUSALS_PER_BURST,
};

/// The Region the shared harness signs for, and so the one the detector these rows
/// create is reachable through (a detector is a REGIONAL resource).
const HOME_REGION: &str = "us-east-1";

/// Another Region, for the rows whose refusal is that the detector the request names
/// is not held under the Region the request is signed for.
const AWAY_REGION: &str = "eu-west-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator (see `client_rejections.rs`).
const INVALID_INPUT: &str = "InvalidInputException";

/// The only member of the `DestinationType` enum
/// (`smithy_query(shape="DestinationType")`).
const S3: &str = "S3";

/// A KMS key ARN of the form `DestinationProperties.KmsKeyArn` carries
/// (`smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// An S3 ARN of the form `DestinationProperties.DestinationArn` carries
/// (`smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`).
const DESTINATION_ARN: &str = "arn:aws:s3:::guardduty-findings-clock/exports/";

/// A well-formed id of the shape this service mints (32 hexadecimal characters) that
/// no create ever minted.
const ABSENT_DESTINATION_ID: &str = "0123456789abcdef0123456789abcdef";

/// A value that could never be one of this service's minted ids at all, so the rows
/// carrying it are refused on the FORM of the id rather than on the store's contents.
const MALFORMED_ID: &str = "not-an-id-this-service-mints";

/// A `ScanCriterion` map key outside the `ScanCriterionKey` members the model
/// declares (the test asserts it is outside [`SCAN_CRITERION_KEY_VALUES`] before it
/// uses it), so the row carrying it is refused on the criteria it carries.
const UNSUPPORTED_CRITERION_KEY: &str = "EC2_INSTANCE_ID";

/// The setting stored before every burst, and the one an accepted update writes
/// afterwards: two different `EbsSnapshotPreservation` members
/// (`smithy_query(shape="EbsSnapshotPreservation")`), so "the refusals wrote nothing"
/// and "the accepted call wrote" are each observable through a read.
const SETTLED_PRESERVATION: &str = "RETENTION_WITH_FINDING";
const PROBE_PRESERVATION: &str = "NO_RETENTION";

/// The ACCEPTED call that closes a row's measurement: the operation the row refuses,
/// in the form that writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    DeleteTheDestination,
    UpdateTheSettings,
}

/// One row: one of the two operations, one way of refusing it, and the accepted call
/// afterwards whose timestamp shows what the refusals did to the clock.
struct Row {
    case: &'static str,
    /// The Region the refused calls are signed for.
    region: &'static str,
    method: &'static str,
    /// The path the refused calls address, built from the detector and the
    /// destination the row created — so a row whose only fault is one member still
    /// names resources that exist for the others.
    path: fn(&str, &str) -> String,
    body: Option<Value>,
    /// The message the refusal carries, built from this service's own classified
    /// wording rather than spelled out here.
    message: String,
    probe: Probe,
}

fn settings_path(detector_id: &str) -> String {
    format!("/detector/{detector_id}/malware-scan-settings")
}

fn destination_path(detector_id: &str, destination_id: &str) -> String {
    format!("/detector/{detector_id}/publishingDestination/{destination_id}")
}

/// A criteria body naming `key` under `kind`, carrying one tag pair.
fn criteria_body(kind: &str, key: &str, tag_key: &str) -> Value {
    json!({
        "scanResourceCriteria": {
            kind: {key: {"mapEquals": [{"key": tag_key, "value": "prod"}]}},
        }
    })
}

/// Every refusal `DeletePublishingDestination` and `UpdateMalwareScanSettings`
/// answer: an unresolvable `DetectorId` in both of the forms this service classifies
/// (well formed but unheld, and not an id at all), the home detector addressed
/// through another Region, a `DestinationId` the detector does not hold in both of
/// those forms, and a `ScanResourceCriteria` outside what the model accepts in each
/// of the two ways it can be.
fn rows() -> Vec<Row> {
    let settings_body = Some(json!({"ebsSnapshotPreservation": PROBE_PRESERVATION}));
    let supported_criterion_key = SCAN_CRITERION_KEY_VALUES[0];
    vec![
        Row {
            case: "delete publishing destination, a detectorId the store does not hold",
            region: HOME_REGION,
            method: "DELETE",
            path: |_, destination_id| destination_path(ABSENT_DETECTOR_ID, destination_id),
            body: None,
            message: not_owned(DETECTOR_ID_MEMBER),
            probe: Probe::DeleteTheDestination,
        },
        Row {
            case: "delete publishing destination, a detectorId that could never have been minted",
            region: HOME_REGION,
            method: "DELETE",
            path: |_, destination_id| destination_path(MALFORMED_ID, destination_id),
            body: None,
            message: invalid_parameter(DETECTOR_ID_MEMBER),
            probe: Probe::DeleteTheDestination,
        },
        Row {
            case: "delete publishing destination, the home detector through another Region",
            region: AWAY_REGION,
            method: "DELETE",
            path: destination_path,
            body: None,
            message: not_owned(DETECTOR_ID_MEMBER),
            probe: Probe::DeleteTheDestination,
        },
        Row {
            case: "delete publishing destination, a destinationId the detector does not hold",
            region: HOME_REGION,
            method: "DELETE",
            path: |detector_id, _| destination_path(detector_id, ABSENT_DESTINATION_ID),
            body: None,
            message: DESTINATION_ABSENT_MESSAGE.to_string(),
            probe: Probe::DeleteTheDestination,
        },
        Row {
            case: "delete publishing destination, a destinationId that could never have been minted",
            region: HOME_REGION,
            method: "DELETE",
            path: |detector_id, _| destination_path(detector_id, MALFORMED_ID),
            body: None,
            message: DESTINATION_ABSENT_MESSAGE.to_string(),
            probe: Probe::DeleteTheDestination,
        },
        Row {
            case: "update malware scan settings, a detectorId the store does not hold",
            region: HOME_REGION,
            method: "POST",
            path: |_, _| settings_path(ABSENT_DETECTOR_ID),
            body: settings_body.clone(),
            message: not_owned(DETECTOR_ID_MEMBER),
            probe: Probe::UpdateTheSettings,
        },
        Row {
            case: "update malware scan settings, a detectorId that could never have been minted",
            region: HOME_REGION,
            method: "POST",
            path: |_, _| settings_path(MALFORMED_ID),
            body: settings_body.clone(),
            message: invalid_parameter(DETECTOR_ID_MEMBER),
            probe: Probe::UpdateTheSettings,
        },
        Row {
            case: "update malware scan settings, the home detector through another Region",
            region: AWAY_REGION,
            method: "POST",
            path: |detector_id, _| settings_path(detector_id),
            body: settings_body.clone(),
            message: not_owned(DETECTOR_ID_MEMBER),
            probe: Probe::UpdateTheSettings,
        },
        Row {
            case: "update malware scan settings, a criterion key outside the enum",
            region: HOME_REGION,
            method: "POST",
            path: |detector_id, _| settings_path(detector_id),
            body: Some(criteria_body(
                "include",
                UNSUPPORTED_CRITERION_KEY,
                "environment",
            )),
            message: format!(
                "{SCAN_RESOURCE_CRITERIA_MEMBER}.include: '{UNSUPPORTED_CRITERION_KEY}' is not a \
                 supported scan criterion key. Supported keys are {}.",
                SCAN_CRITERION_KEY_VALUES.join(", ")
            ),
            probe: Probe::UpdateTheSettings,
        },
        Row {
            case: "update malware scan settings, a condition tag key under the reserved prefix",
            region: HOME_REGION,
            method: "POST",
            path: |detector_id, _| settings_path(detector_id),
            body: Some(criteria_body(
                "exclude",
                supported_criterion_key,
                &format!("{RESERVED_TAG_PREFIX}cloudformation:stack-name"),
            )),
            message: format!(
                "{SCAN_RESOURCE_CRITERIA_MEMBER}.exclude: a tag key must not begin with the \
                 reserved prefix '{RESERVED_TAG_PREFIX}'."
            ),
            probe: Probe::UpdateTheSettings,
        },
    ]
}

/// Create one publishing destination on `detector_id` and return the id the create
/// minted.
async fn created_destination(router: &Router, detector_id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(json!({
            "destinationType": S3,
            "destinationProperties": {
                "destinationArn": DESTINATION_ARN,
                "kmsKeyArn": KEY_ARN,
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of the destination to answer 200, got {status} \
         {body:?}"
    );
    body["destinationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a destinationId, got {body:?}"))
        .to_string()
}

/// Write one `EbsSnapshotPreservation` setting and fail the test unless it was taken.
async fn accepted_settings_update(
    router: &Router,
    detector_id: &str,
    preservation: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "POST",
        &settings_path(detector_id),
        Some(json!({"ebsSnapshotPreservation": preservation})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the settings update to {preservation} to answer 200, got \
         {status} {body:?}"
    );
}

/// The `EbsSnapshotPreservation` a read reports for the detector.
async fn reported_preservation(router: &Router, detector_id: &str, case: &str) -> String {
    let (status, body) = call(router, "GET", &settings_path(detector_id), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the settings read to answer 200, got {status} {body:?}"
    );
    body["ebsSnapshotPreservation"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an ebsSnapshotPreservation, got {body:?}")
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// The rule: however one of these two writes is refused, the refusal consumes no
// instant, so the next accepted call's timestamp is still a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_refusal_of_these_two_writes_leaves_the_clock_where_the_wall_clock_is() {
    assert!(
        !SCAN_CRITERION_KEY_VALUES.contains(&UNSUPPORTED_CRITERION_KEY),
        "the key the criteria row carries must be outside the modelled set, which declares \
         {SCAN_CRITERION_KEY_VALUES:?}"
    );
    for row in rows() {
        let case = row.case;
        // A router, an engine and a detector per row, so a row cannot pass or fail on
        // another row's writes.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let destination_id = created_destination(&router, &detector_id, case).await;
        // A setting the burst could be seen to overwrite, and that is not the one the
        // accepted call afterwards writes.
        accepted_settings_update(&router, &detector_id, SETTLED_PRESERVATION, case).await;

        // The instant the last ACCEPTED mutation stamped, and the wall clock just
        // after it: between these two readings and the accepted call below, the only
        // calls the service sees are refused ones, so the clock may advance only by
        // the time that really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        let refused_path = (row.path)(&detector_id, &destination_id);
        for attempt in 0..REFUSALS_PER_BURST {
            let (status, refusal) = call_in(
                &router,
                row.region,
                row.method,
                &refused_path,
                row.body.clone(),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: refused call {attempt} must answer 400, got {status} {refusal:?}"
            );
            assert_eq!(
                error_code(&refusal),
                INVALID_INPUT,
                "case {case:?}: refused call {attempt} must carry the {INVALID_INPUT} class, got \
                 {refusal:?}"
            );
            assert_eq!(
                refusal["message"].as_str(),
                Some(row.message.as_str()),
                "case {case:?}: refused call {attempt} must carry the classified message, \
                 expected {:?}, got {refusal:?}",
                row.message
            );
        }

        // A refused call changes nothing, which the records it was addressed to report
        // first of all: the detector's own timestamp, the destination a refused delete
        // was supposed to leave alone, and the setting a refused update was supposed
        // not to write.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {REFUSALS_PER_BURST} refused calls must not move the detector's \
             updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );
        let (status, described) = call(
            &router,
            "GET",
            &destination_path(&detector_id, &destination_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the destination the refusals left alone must still resolve, got \
             {status} {described:?}"
        );
        let preserved = reported_preservation(&router, &detector_id, case).await;
        assert_eq!(
            preserved, SETTLED_PRESERVATION,
            "case {case:?}: {REFUSALS_PER_BURST} refused calls must leave the stored setting \
             alone, expected {SETTLED_PRESERVATION:?}, got {preserved:?}"
        );

        // The accepted call right after the burst — the very operation the row refuses,
        // in the form that writes. It stamps the instant it happened, and that instant
        // is a WALL-CLOCK instant, not one the refusals pushed into the future.
        let (status, accepted) = match row.probe {
            Probe::DeleteTheDestination => {
                call(
                    &router,
                    "DELETE",
                    &destination_path(&detector_id, &destination_id),
                    None,
                )
                .await
            }
            Probe::UpdateTheSettings => {
                call(
                    &router,
                    "POST",
                    &settings_path(&detector_id),
                    Some(json!({"ebsSnapshotPreservation": PROBE_PRESERVATION})),
                )
                .await
            }
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected the accepted {:?} to answer 200, got {status} {accepted:?}",
            row.probe
        );

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted {:?} changed the detector, so its updatedAt must move \
             past {after_burst:?}, got {stamped:?}",
            row.probe
        );
        // The measurement: how far the clock moved across the burst, against how far
        // the WALL clock moved over the same span. A refused call consumes no instant,
        // so the two agree to within the slack the sibling suites publish however long
        // the burst took — where a handler that read the clock before refusing would
        // have spent an instant per refused call on top of it.
        let elapsed = wall_after - wall_before;
        let stamped_millis = as_millis(&stamped, case);
        let advance = stamped_millis - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {REFUSALS_PER_BURST} refused calls consume no instant, so the clock \
             may advance only by the {elapsed} ms that really passed (plus \
             {READING_SLACK_MILLIS} ms of slack), got {advance} ms (from {before_burst:?} to \
             {stamped:?})"
        );
        // The same fact as a client sees it: the timestamp an accepted write stamps
        // reports when that write happened, so it does not sit in the future of the
        // wall clock read right after it.
        assert!(
            stamped_millis <= wall_after + READING_SLACK_MILLIS,
            "case {case:?}: the instant the accepted {:?} stamped must be a wall-clock instant, \
             so at most the {wall_after} ms read just after it (plus {READING_SLACK_MILLIS} ms of \
             slack), got {stamped_millis} ms ({stamped:?})",
            row.probe
        );

        // Round trip: what the accepted call wrote is what a read reports — the
        // destination it deleted resolves nowhere, the setting it wrote is the one
        // reported — so the timestamp asserted above records a change that landed.
        match row.probe {
            Probe::DeleteTheDestination => {
                let (status, gone) = call(
                    &router,
                    "GET",
                    &destination_path(&detector_id, &destination_id),
                    None,
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: the destination the accepted delete dropped must resolve \
                     nowhere, got {status} {gone:?}"
                );
            }
            Probe::UpdateTheSettings => {
                let written = reported_preservation(&router, &detector_id, case).await;
                assert_eq!(
                    written, PROBE_PRESERVATION,
                    "case {case:?}: the accepted update wrote {PROBE_PRESERVATION:?}, so a read \
                     must report it, got {written:?}"
                );
            }
        }
    }
}
