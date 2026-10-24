// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePublishingDestination`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. An update is PARTIAL: a `DestinationProperties` member the request carries
//!    replaces the stored one, and a member it omits — up to the whole
//!    `destinationProperties` object — leaves the stored one exactly where it was.
//!    `DescribePublishingDestination` reports what the update wrote back.
//! 2. A destination whose properties CHANGED has not been verified for the
//!    properties it now names, so it re-enters `PENDING_VERIFICATION` and the CLOCK
//!    moves it back to `PUBLISHING`; an update that writes the values the
//!    destination already holds changes nothing and leaves the settled status alone.
//! 3. A request GuardDuty cannot accept is refused with HTTP 400 under the
//!    `InvalidInputException` class, naming the member at fault, and leaves the
//!    stored destination untouched.
//! 4. A destination belongs to the detector, and the Region, it was created under,
//!    so an update addressed anywhere else does not reach it.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every GuardDuty client rejection is named by on the wire (see
/// `client_rejections.rs`).
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in.
const ENVELOPE: &str = "BadRequestException";

/// The only member of the `DestinationType` enum
/// (`smithy_query(shape="DestinationType")`).
const S3: &str = "S3";

/// A well-formed `DestinationId` (32 hex characters, the form every id this service
/// mints has) that no create ever minted.
const ABSENT_DESTINATION_ID: &str = "0123456789abcdef0123456789abcdef";

/// The destination ARN every destination in these tests is created with.
const BASE_ARN: &str = "arn:aws:s3:::guardduty-findings-origin";

/// The KMS key ARN every destination in these tests is created with ("The ARN of the
/// KMS key to use for encryption" — `smithy_query(shape="DestinationProperties")`).
const BASE_KEY: &str =
    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// A second KMS key ARN, in the other resource form the service accepts (an alias).
const OTHER_KEY: &str = "arn:aws:kms:us-east-1:111122223333:alias/guardduty-exports";

/// How long to wait for a verification the clock owes to come due: the span itself
/// plus a margin, so the wait is derived from the emulator's own constant rather than
/// from a number written twice.
fn settle_wait() -> std::time::Duration {
    std::time::Duration::from_millis(state::VERIFICATION_DELAY_MILLIS as u64 + 400)
}

/// The `destinationProperties` every destination here starts out with.
fn base_properties() -> Value {
    json!({"destinationArn": BASE_ARN, "kmsKeyArn": BASE_KEY})
}

async fn create_destination(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(body),
    )
    .await
}

async fn describe_destination(
    router: &Router,
    detector_id: &str,
    destination_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await
}

async fn update_destination(
    router: &Router,
    detector_id: &str,
    destination_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        Some(body),
    )
    .await
}

/// Create a destination carrying [`base_properties`] and return its id.
async fn base_destination(router: &Router, detector_id: &str) -> String {
    let (status, response) = create_destination(
        router,
        detector_id,
        json!({"destinationType": S3, "destinationProperties": base_properties()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-publishing-destination: expected 200, got {status} {response:?}"
    );
    response["destinationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-publishing-destination: expected a destinationId, got {response:?}")
        })
        .to_string()
}

/// Read a destination back and return the `destinationProperties` and `status` it
/// reports.
async fn read_back(
    router: &Router,
    detector_id: &str,
    destination_id: &str,
    case: &str,
) -> (Value, String) {
    let (status, body) = describe_destination(router, detector_id, destination_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the destination to be readable, got {status} {body:?}"
    );
    let reported = body["status"].as_str().unwrap_or_default().to_string();
    (body["destinationProperties"].clone(), reported)
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: a partial update, and the verification a change owes.
// ---------------------------------------------------------------------------

/// One update request and what the destination it was applied to must report after.
struct Update {
    case: &'static str,
    /// The whole request body, so a row can leave `destinationProperties` out.
    body: Value,
    expected_arn: &'static str,
    expected_key: &'static str,
    /// The `expectedBucketOwner` the read must report, or `None` for a destination
    /// no request ever supplied one for.
    expected_owner: Option<&'static str>,
    /// Did this update change the destination's properties? A change owes the
    /// verification again; an update that writes what is already stored does not.
    changed: bool,
}

/// The rows differ along every axis the rule generalises over: WHICH members the
/// update carries (one, two, all three, none, and no `destinationProperties` member
/// at all), the FORM of the value written (a bucket, a bucket FOLDER — "To specify an
/// S3 bucket folder use the following format: `arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/`"
/// — and the two KMS resource forms), the PARTITION the ARNs live in, and whether the
/// value written differs from the stored one at all.
fn updates() -> Vec<Update> {
    vec![
        Update {
            case: "destinationArn only",
            body: json!({"destinationProperties": {"destinationArn": "arn:aws:s3:::guardduty-findings-moved"}}),
            expected_arn: "arn:aws:s3:::guardduty-findings-moved",
            expected_key: BASE_KEY,
            expected_owner: None,
            changed: true,
        },
        Update {
            case: "destinationArn naming a bucket folder",
            body: json!({"destinationProperties": {"destinationArn": "arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/"}}),
            expected_arn: "arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/",
            expected_key: BASE_KEY,
            expected_owner: None,
            changed: true,
        },
        Update {
            case: "kmsKeyArn only, as an alias",
            body: json!({"destinationProperties": {"kmsKeyArn": OTHER_KEY}}),
            expected_arn: BASE_ARN,
            expected_key: OTHER_KEY,
            expected_owner: None,
            changed: true,
        },
        Update {
            case: "both ARNs at once",
            body: json!({"destinationProperties": {
                "destinationArn": "arn:aws:s3:::guardduty-findings-both/exports/",
                "kmsKeyArn": OTHER_KEY,
            }}),
            expected_arn: "arn:aws:s3:::guardduty-findings-both/exports/",
            expected_key: OTHER_KEY,
            expected_owner: None,
            changed: true,
        },
        Update {
            case: "expectedBucketOwner only",
            body: json!({"destinationProperties": {"expectedBucketOwner": "111122223333"}}),
            expected_arn: BASE_ARN,
            expected_key: BASE_KEY,
            expected_owner: Some("111122223333"),
            changed: true,
        },
        Update {
            case: "every member at once, in another partition",
            body: json!({"destinationProperties": {
                "destinationArn": "arn:aws-us-gov:s3:::gov-findings-bucket/exports/",
                "kmsKeyArn": "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/abcd",
                "expectedBucketOwner": "444455556666",
            }}),
            expected_arn: "arn:aws-us-gov:s3:::gov-findings-bucket/exports/",
            expected_key: "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/abcd",
            expected_owner: Some("444455556666"),
            changed: true,
        },
        Update {
            case: "the values the destination already holds",
            body: json!({"destinationProperties": {"destinationArn": BASE_ARN, "kmsKeyArn": BASE_KEY}}),
            expected_arn: BASE_ARN,
            expected_key: BASE_KEY,
            expected_owner: None,
            changed: false,
        },
        Update {
            case: "an empty destinationProperties object",
            body: json!({"destinationProperties": {}}),
            expected_arn: BASE_ARN,
            expected_key: BASE_KEY,
            expected_owner: None,
            changed: false,
        },
        Update {
            case: "no destinationProperties member at all",
            body: json!({}),
            expected_arn: BASE_ARN,
            expected_key: BASE_KEY,
            expected_owner: None,
            changed: false,
        },
    ]
}

#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_leaves_the_others_stored() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Every row gets its own destination, created up front so ONE wait settles them
    // all: the assertions below need each destination to start from the verified
    // status, so that re-entering `PENDING_VERIFICATION` is observable.
    let rows = updates();
    let mut destinations = Vec::with_capacity(rows.len());
    for _ in &rows {
        destinations.push(base_destination(&router, &detector_id).await);
    }
    tokio::time::sleep(settle_wait()).await;

    for (row, destination_id) in rows.iter().zip(&destinations) {
        let case = row.case;
        let (_, before) = read_back(&router, &detector_id, destination_id, case).await;
        assert_eq!(
            before, "PUBLISHING",
            "case {case:?}: the destination must be verified before the update, got {before:?}"
        );

        let (status, response) =
            update_destination(&router, &detector_id, destination_id, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        // `UpdatePublishingDestinationResponse` has no members
        // (`smithy_query(operation="UpdatePublishingDestination")`), so success is
        // the modelled 200 and nothing else.
        assert_eq!(
            response,
            json!({}),
            "case {case:?}: expected an empty response body, got {response:?}"
        );

        let (properties, reported) = read_back(&router, &detector_id, destination_id, case).await;
        assert_eq!(
            properties["destinationArn"].as_str(),
            Some(row.expected_arn),
            "case {case:?}: expected destinationArn {:?}, got {:?}",
            row.expected_arn,
            properties["destinationArn"]
        );
        assert_eq!(
            properties["kmsKeyArn"].as_str(),
            Some(row.expected_key),
            "case {case:?}: expected kmsKeyArn {:?}, got {:?}",
            row.expected_key,
            properties["kmsKeyArn"]
        );
        assert_eq!(
            properties["expectedBucketOwner"].as_str(),
            row.expected_owner,
            "case {case:?}: expected expectedBucketOwner {:?}, got {:?}",
            row.expected_owner,
            properties["expectedBucketOwner"]
        );
        // The type of an existing destination is not something an update can move.
        let expected_status = if row.changed {
            "PENDING_VERIFICATION"
        } else {
            "PUBLISHING"
        };
        assert_eq!(
            reported, expected_status,
            "case {case:?}: expected status {expected_status:?} after the update, got {reported:?}"
        );
    }

    // The verification a changed destination owes is settled by the CLOCK, not by
    // being read: every row reports the verified status once the span is up.
    tokio::time::sleep(settle_wait()).await;
    for (row, destination_id) in rows.iter().zip(&destinations) {
        let case = row.case;
        let (_, reported) = read_back(&router, &detector_id, destination_id, case).await;
        assert_eq!(
            reported, "PUBLISHING",
            "case {case:?}: expected the clock to verify the destination again, got {reported:?}"
        );
    }
}

/// A changed destination stays in `PENDING_VERIFICATION` for the whole span the
/// clock owes, however often it is read — reading it is not what verifies it — and a
/// second update restarts that span from its own instant.
#[tokio::test]
async fn a_changed_destination_is_verified_by_the_clock_and_not_by_being_read() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = base_destination(&router, &detector_id).await;
    tokio::time::sleep(settle_wait()).await;

    for (round, arn) in [
        (1, "arn:aws:s3:::guardduty-findings-round-one"),
        (2, "arn:aws:s3:::guardduty-findings-round-two/exports/"),
    ] {
        let case = format!("update round {round}");
        let (status, response) = update_destination(
            &router,
            &detector_id,
            &destination_id,
            json!({"destinationProperties": {"destinationArn": arn}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );

        for read in 1..=3 {
            let (_, reported) = read_back(&router, &detector_id, &destination_id, &case).await;
            assert_eq!(
                reported, "PENDING_VERIFICATION",
                "case {case:?}: read {read} before the verification is due must still report PENDING_VERIFICATION, got {reported:?}"
            );
        }

        tokio::time::sleep(settle_wait()).await;
        let (properties, reported) =
            read_back(&router, &detector_id, &destination_id, &case).await;
        assert_eq!(
            reported, "PUBLISHING",
            "case {case:?}: expected PUBLISHING once the span is up, got {reported:?}"
        );
        assert_eq!(
            properties["destinationArn"].as_str(),
            Some(arn),
            "case {case:?}: expected destinationArn {arn:?}, got {:?}",
            properties["destinationArn"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a request the service cannot accept is refused, and writes nothing.
// ---------------------------------------------------------------------------

/// `expectedBucketOwner` carries `len 12..12`
/// (`smithy_query(shape="DestinationProperties")`), so a value outside that length is
/// refused by the BOUNDARY validator under the modelled `BadRequestException` before
/// any handler runs; every other row is a rule the model cannot express, so the
/// handler refuses it under GuardDuty's own `InvalidInputException` class. Either way
/// the destination keeps the properties it had.
#[tokio::test]
async fn an_update_the_service_cannot_accept_is_refused_and_writes_nothing() {
    // (case, destinationProperties, the member the message must name, the class)
    let cases: [(&str, Value, &str, &str); 9] = [
        (
            "destinationArn is not an ARN at all",
            json!({"destinationArn": "guardduty-findings"}),
            "destinationArn",
            CLASS,
        ),
        (
            "empty destinationArn",
            json!({"destinationArn": ""}),
            "destinationArn",
            CLASS,
        ),
        (
            "destinationArn names another service",
            json!({"destinationArn": "arn:aws:sqs:us-east-1:111122223333:findings"}),
            "destinationArn",
            CLASS,
        ),
        (
            "destinationArn has no resource",
            json!({"destinationArn": "arn:aws:s3:::"}),
            "destinationArn",
            CLASS,
        ),
        (
            "kmsKeyArn names another service",
            json!({"kmsKeyArn": "arn:aws:s3:::bucket"}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "kmsKeyArn is not an ARN at all",
            json!({"kmsKeyArn": "1234abcd-12ab-34cd-56ef-1234567890ab"}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "one bad member beside one good one",
            json!({"destinationArn": "arn:aws:s3:::acceptable-bucket", "kmsKeyArn": "not-an-arn"}),
            "kmsKeyArn",
            CLASS,
        ),
        (
            "expectedBucketOwner is 12 characters but not 12 digits",
            json!({"expectedBucketOwner": "owner1234567"}),
            "expectedBucketOwner",
            CLASS,
        ),
        (
            "expectedBucketOwner is one digit short of the modelled length",
            json!({"expectedBucketOwner": "11112222333"}),
            "expectedBucketOwner",
            ENVELOPE,
        ),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = base_destination(&router, &detector_id).await;

    for (case, properties, member, class) in cases {
        let (status, response) = update_destination(
            &router,
            &detector_id,
            &destination_id,
            json!({"destinationProperties": properties}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            class,
            "case {case:?}: expected the {class} class, got {response:?}"
        );
        // Either way the error SHAPE on the wire is the only client error this
        // service's model declares, never the framework's `ValidationException`.
        assert_eq!(
            error_shape(&response),
            ENVELOPE,
            "case {case:?}: expected the {ENVELOPE} envelope shape, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member}, got {message:?}"
        );

        // A refused update is not a partial one: nothing it carried was written.
        let (stored, _) = read_back(&router, &detector_id, &destination_id, case).await;
        assert_eq!(
            stored,
            json!({"destinationArn": BASE_ARN, "kmsKeyArn": BASE_KEY}),
            "case {case:?}: a refused update must leave the stored properties alone, got {stored:?}"
        );
    }
}

/// An id the store does not resolve is the same classified 400 `DescribePublishing
/// Destination` answers: a value that is not an id of this service at all is a
/// parameter failure, a well-formed one the account does not own is an ownership
/// failure, and neither message quotes the caller's value back at them.
#[tokio::test]
async fn an_update_addressed_to_an_id_the_store_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let destination_id = base_destination(&router, &detector_id).await;

    // (case, path, the member the message must name)
    let cases: [(&str, String, &str); 5] = [
        (
            "an absent detector",
            format!("/detector/{ABSENT_DETECTOR_ID}/publishingDestination/{destination_id}"),
            "detectorId",
        ),
        (
            "a detector id that is not one",
            format!("/detector/not-a-detector-id/publishingDestination/{destination_id}"),
            "detectorId",
        ),
        (
            "an absent destination",
            format!("/detector/{detector_id}/publishingDestination/{ABSENT_DESTINATION_ID}"),
            "destinationId",
        ),
        (
            "a destination id that is not one",
            format!("/detector/{detector_id}/publishingDestination/not-a-destination-id"),
            "destinationId",
        ),
        (
            "a destination id the caller upper-cased",
            format!(
                "/detector/{detector_id}/publishingDestination/{}",
                destination_id.to_uppercase()
            ),
            "destinationId",
        ),
    ];

    for (case, path, member) in cases {
        let (status, response) = call(
            &router,
            "POST",
            &path,
            Some(json!({"destinationProperties": {"destinationArn": "arn:aws:s3:::elsewhere"}})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            CLASS,
            "case {case:?}: expected the {CLASS} class, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member}, got {message:?}"
        );
        assert!(
            !message.contains(ABSENT_DETECTOR_ID) && !message.contains(ABSENT_DESTINATION_ID),
            "case {case:?}: the refusal must not quote the caller's id back, got {message:?}"
        );
    }

    // None of those refusals wrote anything: the destination still reports what the
    // create stored.
    let (stored, _) = read_back(&router, &detector_id, &destination_id, "after the refusals").await;
    assert_eq!(
        stored,
        json!({"destinationArn": BASE_ARN, "kmsKeyArn": BASE_KEY}),
        "a refused update must leave the stored properties alone, got {stored:?}"
    );
}

/// A detector is REGIONAL, so a destination created through one Region's endpoint is
/// not something another Region's endpoint can update — even with the same ids.
#[tokio::test]
async fn an_update_through_another_regions_endpoint_does_not_reach_the_destination() {
    let router = new_router();
    let home = "us-west-2";
    let away = "eu-central-1";

    let (status, created) = call_in(
        &router,
        home,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in {home}: expected 200, got {status} {created:?}"
    );
    let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();
    let (status, created) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(json!({"destinationType": S3, "destinationProperties": base_properties()})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-publishing-destination in {home}: expected 200, got {status} {created:?}"
    );
    let destination_id = created["destinationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, response) = call_in(
        &router,
        away,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        Some(json!({"destinationProperties": {"destinationArn": "arn:aws:s3:::cross-region"}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"update through {away}\": expected 400, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        CLASS,
        "case \"update through {away}\": expected the {CLASS} class, got {response:?}"
    );

    // The home Region's destination is untouched by the refused cross-Region call.
    let (status, read) = call_in(
        &router,
        home,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read through {home}\": expected 200, got {status} {read:?}"
    );
    assert_eq!(
        read["destinationProperties"]["destinationArn"].as_str(),
        Some(BASE_ARN),
        "case \"read through {home}\": expected the stored destinationArn, got {:?}",
        read["destinationProperties"]
    );

    // A detector in the away Region does not resolve the other Region's destination
    // id either: the refusal above is about the destination, not about the detector
    // alone.
    let (status, away_detector) = call_in(
        &router,
        away,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in {away}: expected 200, got {status} {away_detector:?}"
    );
    let away_detector_id = away_detector["detectorId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, response) = call_in(
        &router,
        away,
        "POST",
        &format!("/detector/{away_detector_id}/publishingDestination/{destination_id}"),
        Some(json!({"destinationProperties": {"destinationArn": "arn:aws:s3:::cross-region"}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"update under the {away} detector\": expected 400, got {status} {response:?}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("destinationId"),
        "case \"update under the {away} detector\": the refusal must name destinationId, got {message:?}"
    );
}
