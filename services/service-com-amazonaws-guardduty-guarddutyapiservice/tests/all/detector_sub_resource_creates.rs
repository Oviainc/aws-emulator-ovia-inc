// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when a
//! sub-resource is added to it or taken off it.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over the SET of sub-resource families a detector
//! holds rather than over one of them: a call that changes the set of sub-resources a
//! detector holds changes the detector, so `GetDetector`'s `UpdatedAt` moves to the
//! instant of that change — and a call that changes nothing (an idempotent replay, a
//! refused request, a read) moves it nowhere. The two halves of one sub-resource
//! lifecycle therefore agree: the create and the delete both move it.
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these
//! assertions order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};

/// A KMS key ARN, the form `DestinationProperties.KmsKeyArn` carries
/// (`smithy_query(shape="DestinationProperties")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One family of sub-resource that hangs off a detector: where its create POSTs,
/// what its output calls the minted id, and a read of what it minted.
struct Family {
    case: &'static str,
    /// The collection segment under `/detector/{detectorId}`, from each create's
    /// `http` binding (`smithy_query(operation="CreateIPSet,CreateThreatIntelSet,\
    /// CreateThreatEntitySet,CreateTrustedEntitySet,CreatePublishingDestination")`).
    segment: &'static str,
    /// The create output member carrying the minted id, from the same query.
    id_member: &'static str,
    /// A create this service accepts.
    create: Value,
    /// The same create with ONE member outside the value set its enum declares, so
    /// the request is refused and nothing is minted.
    refused: Value,
}

/// Every sub-resource family whose create is implemented, so the rule is asserted on
/// the whole class and not on the one family a recorded call cited. The rows differ
/// along what the rule generalises over: the family, the `Format` (four members of
/// the six-value enum — `smithy_query(shape="IpSetFormat")` and its three siblings
/// declare TXT | STIX | OTX_CSV | ALIEN_VAULT | PROOF_POINT | FIRE_EYE), whether the
/// caller asked to activate, whether tags and an expected bucket owner are carried,
/// and a family whose create takes a nested structure instead of a location.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "ip set, TXT, activated",
            segment: "ipset",
            id_member: "ipSetId",
            create: json!({
                "name": "ip-set-one",
                "format": "TXT",
                "location": "s3://guardduty-lists/ip-set-one.txt",
                "activate": true,
            }),
            refused: json!({
                "name": "ip-set-refused",
                "format": "NOT_A_FORMAT",
                "location": "s3://guardduty-lists/refused.txt",
                "activate": true,
            }),
        },
        Family {
            case: "threat intel set, STIX, not activated, tagged, with an expected bucket owner",
            segment: "threatintelset",
            id_member: "threatIntelSetId",
            create: json!({
                "name": "threat-list-one",
                "format": "STIX",
                "location": "s3://guardduty-lists/threat-list-one.xml",
                "activate": false,
                "expectedBucketOwner": "111122223333",
                "tags": {"team": "secops"},
            }),
            refused: json!({
                "name": "threat-list-refused",
                "format": "NOT_A_FORMAT",
                "location": "s3://guardduty-lists/refused.xml",
                "activate": false,
            }),
        },
        Family {
            case: "threat entity set, OTX_CSV, activated",
            segment: "threatentityset",
            id_member: "threatEntitySetId",
            create: json!({
                "name": "threat-entities-one",
                "format": "OTX_CSV",
                "location": "s3://guardduty-lists/threat-entities-one.csv",
                "activate": true,
            }),
            refused: json!({
                "name": "threat-entities-refused",
                "format": "NOT_A_FORMAT",
                "location": "s3://guardduty-lists/refused.csv",
                "activate": true,
            }),
        },
        Family {
            case: "trusted entity set, FIRE_EYE, not activated, tagged",
            segment: "trustedentityset",
            id_member: "trustedEntitySetId",
            create: json!({
                "name": "trusted-entities-one",
                "format": "FIRE_EYE",
                "location": "s3://guardduty-lists/trusted-entities-one.txt",
                "activate": false,
                "tags": {"env": "test", "owner": "team-a"},
            }),
            refused: json!({
                "name": "trusted-entities-refused",
                "format": "NOT_A_FORMAT",
                "location": "s3://guardduty-lists/refused.txt",
                "activate": false,
            }),
        },
        Family {
            case: "publishing destination, S3",
            segment: "publishingDestination",
            id_member: "destinationId",
            // `DestinationType` declares exactly one member, S3
            // (`smithy_query(shape="DestinationType")`), so the refused row names a
            // value outside it.
            create: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/exports/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            refused: json!({
                "destinationType": "KINESIS",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings/exports/",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
        },
    ]
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// The detector's `UpdatedAt` as the string a client sees.
async fn detector_updated_at(router: &Router, detector_id: &str, case: &str) -> String {
    let got = read_detector(router, detector_id, case).await;
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

/// The minted id a create returned, failing the test if the create did not succeed or
/// did not name one.
fn minted_id(case: &str, member: &str, status: u16, got: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer 200, got {status} {got:?}"
    );
    let id = got[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {got:?}"));
    assert!(
        id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "case {case:?}: expected a 32-character hexadecimal {member}, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// The rule, over every sub-resource family: a create moves the detector's
// UpdatedAt, and a call that changes nothing moves nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adding_a_sub_resource_moves_the_detectors_updated_at() {
    for family in families() {
        let case = family.case;
        // A router and a detector per row, so a row cannot pass on another row's
        // write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let collection = format!("/detector/{detector_id}/{}", family.segment);

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A refused create mints nothing, so the detector is unchanged by it.
        let (status, refusal) =
            call(&router, "POST", &collection, Some(family.refused.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the modelled enum must be refused, got {status} {refusal:?}"
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused create must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        // A read is not a change either.
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, updated_before,
            "case {case:?}: reading the detector must not move its updatedAt, expected \
             {updated_before:?}, got {after_read:?}"
        );

        // The accepted create: the detector now holds one more sub-resource, so its
        // UpdatedAt moves.
        let mut body = family.create.clone();
        body["clientToken"] = json!(format!("token-{}", family.segment));
        let (status, created) = call(&router, "POST", &collection, Some(body.clone())).await;
        let id = minted_id(case, family.id_member, status, &created);

        let after_create = read_detector(&router, &detector_id, case).await;
        let updated_after = after_create["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {after_create:?}"))
            .to_string();
        assert!(
            updated_after > updated_before,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {updated_before:?}, got {updated_after:?}"
        );
        assert_eq!(
            after_create["createdAt"], created_at,
            "case {case:?}: a sub-resource create must not move the detector's createdAt, \
             expected {created_at:?}, got {:?}",
            after_create["createdAt"]
        );

        // Round trip: the id the create minted is a resource a read resolves, so the
        // moved UpdatedAt records a change that really landed in the store.
        let (status, read_back) = call(
            &router,
            "GET",
            &format!("{collection}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted {} to answer 200, got {status} \
             {read_back:?}",
            family.id_member
        );

        // An idempotent replay mints nothing, so it moves nothing.
        let (status, replayed) = call(&router, "POST", &collection, Some(body)).await;
        let replayed_id = minted_id(case, family.id_member, status, &replayed);
        assert_eq!(
            replayed_id, id,
            "case {case:?}: a replayed clientToken must return the first call's id, expected \
             {id:?}, got {replayed_id:?}"
        );
        let after_replay = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_replay, updated_after,
            "case {case:?}: a replayed create mints nothing, so the detector's updatedAt must \
             not move, expected {updated_after:?}, got {after_replay:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The two halves of one lifecycle agree: the create and the delete of the same
// sub-resource both move the detector's UpdatedAt.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_halves_of_a_sub_resource_lifecycle_move_the_detectors_updated_at() {
    // The families whose create AND delete are both implemented: the lists and the
    // entity sets a detector holds.
    let rows: [(&str, &str, &str, Value); 3] = [
        (
            "ip set",
            "ipset",
            "ipSetId",
            json!({
                "name": "lifecycle-ip-set",
                "format": "ALIEN_VAULT",
                "location": "s3://guardduty-lists/lifecycle-ip-set.txt",
                "activate": true,
            }),
        ),
        (
            "threat intel set",
            "threatintelset",
            "threatIntelSetId",
            json!({
                "name": "lifecycle-threat-list",
                "format": "PROOF_POINT",
                "location": "s3://guardduty-lists/lifecycle-threat-list.txt",
                "activate": false,
            }),
        ),
        (
            "trusted entity set",
            "trustedentityset",
            "trustedEntitySetId",
            json!({
                "name": "lifecycle-trusted-entities",
                "format": "TXT",
                "location": "s3://guardduty-lists/lifecycle-trusted-entities.txt",
                "activate": true,
            }),
        ),
    ];

    for (case, segment, id_member, create) in rows {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let collection = format!("/detector/{detector_id}/{segment}");

        let before_create = detector_updated_at(&router, &detector_id, case).await;
        let (status, created) = call(&router, "POST", &collection, Some(create)).await;
        let id = minted_id(case, id_member, status, &created);
        let after_create = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            after_create > before_create,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {before_create:?}, got {after_create:?}"
        );

        let (status, deleted) = call(&router, "DELETE", &format!("{collection}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
        );
        let after_delete = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            after_delete > after_create,
            "case {case:?}: the delete changed the detector too, so its updatedAt must move past \
             {after_create:?}, got {after_delete:?}"
        );
    }
}
