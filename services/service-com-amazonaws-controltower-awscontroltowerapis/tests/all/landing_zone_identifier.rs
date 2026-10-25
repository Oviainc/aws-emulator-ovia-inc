// SPDX-License-Identifier: CC0-1.0

//! What a `landingZoneIdentifier` may be, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: a landing
//! zone is addressed by the ARN the service reports for it — the only identifier a caller
//! is ever handed for one (`CreateLandingZoneOutput$arn` and `LandingZoneSummary$arn`,
//! both typed `Arn`, length 20..2048, pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`, from
//! `smithy_query(query="operation", operation="CreateLandingZone,ListLandingZones")`) —
//! so a `landingZoneIdentifier` that is not such an ARN is a malformed input, refused
//! with the modelled `ValidationException` (400), while an ARN of that shape naming no
//! stored landing zone is the missing resource `ResourceNotFoundException` (404) reports.
//! `landingZoneIdentifier` is modelled as a plain `String` with no pattern, so the front
//! door passes every value and both answers come from the handlers.
//!
//! The rule is a property of the identifier rather than of one call, so every row is
//! asserted against EVERY operation that takes one: `GetLandingZone`,
//! `UpdateLandingZone`, `ResetLandingZone` and `DeleteLandingZone`. A refused call must
//! also leave the account's landing zone exactly as it was, so no refusal — of a read or
//! of a mutation — can be the thing that changes it.

use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, past_the_settle_span, simple_manifest, Answer, SIGNED_REGION,
};

use smithy_server_runtime::MultiServiceRouter;

/// The catalog's newest landing zone version, so the update rows carry a `version` the
/// service offers and are never refused for that member instead of the identifier.
const CATALOG: &str =
    service_com_amazonaws_controltower_awscontroltowerapis::state::CATALOG_LANDING_ZONE_VERSION;

/// Every operation that takes a `landingZoneIdentifier`, with the other members its
/// request needs. `UpdateLandingZone` is the only one with a second required member.
fn operations() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("/get-landingzone", serde_json::json!({})),
        (
            "/update-landingzone",
            serde_json::json!({ "version": CATALOG }),
        ),
        ("/reset-landingzone", serde_json::json!({})),
        ("/delete-landingzone", serde_json::json!({})),
    ]
}

/// A request body for one of [`operations`], addressing `identifier`.
fn body(other_members: &serde_json::Value, identifier: &str) -> serde_json::Value {
    let mut body = other_members.clone();
    body["landingZoneIdentifier"] = serde_json::json!(identifier);
    body
}

/// A landing zone ARN of exactly `length` characters, for the rows that sit on `Arn`'s
/// published `@length` bounds (min 20, max 2048 — the type of the `arn` members a caller
/// takes this identifier from). The shortest landing zone ARN is already longer than the
/// minimum, so the min bound is unreachable for a value of this shape; the shortest one
/// there is (a single-character resource id) is a row of its own below.
fn landing_zone_arn_of_length(length: usize) -> String {
    let prefix = "arn:aws:controltower:us-west-2:123456789012:landingzone/";
    assert!(
        length > prefix.len(),
        "a landing zone ARN needs at least {} characters, asked for {length}",
        prefix.len() + 1
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// Create the account's one landing zone and wait for its `CREATE` operation to become
/// due, so the snapshot every row is compared against is of a settled resource. Answers
/// its ARN.
async fn create_settled_landing_zone(router: &MultiServiceRouter) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": CATALOG, "manifest": simple_manifest() }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create these rows start from: expected 200, got {} body {:?}",
        created.status, created.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// `GetLandingZone`'s `landingZone` member for `arn`, asserting it resolved.
async fn detail(router: &MultiServiceRouter, case: &str, arn: &str) -> serde_json::Value {
    let answer = call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the stored ARN to resolve, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("landingZone").clone()
}

/// Assert `answer` started no operation: a refusal must not have mutated anything, and an
/// `operationIdentifier` in the body is what a started one looks like.
fn started_nothing(case: &str, path: &str, answer: &Answer) {
    assert!(
        answer.body.get("operationIdentifier").is_none(),
        "case {case:?} on {path}: expected the refusal to start no operation, got {:?}",
        answer.body
    );
}

/// Assert the account still holds exactly `arn`, reporting `expected`.
async fn unchanged(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    expected: &serde_json::Value,
) {
    let listed = call(router, "/list-landingzones", serde_json::json!({})).await;
    assert_eq!(
        listed.get("landingZones"),
        &serde_json::json!([{ "arn": arn }]),
        "case {case:?}: expected the refused calls to leave the one landing zone in place, \
         got {:?}",
        listed.get("landingZones")
    );
    let now = detail(router, case, arn).await;
    assert_eq!(
        &now, expected,
        "case {case:?}: expected the refused calls to change nothing, got {now:?}"
    );
}

/// RULE: a `landingZoneIdentifier` that is not the ARN the service reports for a landing
/// zone is a malformed input — `ValidationException` (400), naming the member — for every
/// operation that takes one, and it changes nothing.
///
/// Rows differ along what the rule generalises over: a bare resource id (the stored
/// landing zone's own, and ids of the two forms AWS and this emulator mint), a free-form
/// name, a single character, the empty string, an ARN of another service, an ARN of
/// another resource type of THIS service, an ARN with no resource id, an ARN whose
/// account or region segment is not one an ARN carries, and an ARN one character past
/// `Arn`'s published length maximum of 2048.
#[tokio::test]
async fn an_identifier_that_is_not_a_landing_zone_arn_is_refused() {
    let router = emulator();
    let arn = create_settled_landing_zone(&router).await;
    let stored_bare_id = arn
        .rsplit('/')
        .next()
        .expect("an ARN ends in its resource segment")
        .to_string();
    // The configuration as deployed before any refused call, so every row asserts
    // "unchanged" rather than a copy of what the create asked for.
    let deployed = detail(&router, "the deployed landing zone", &arn).await;

    let mut rows: Vec<(String, String)> = vec![
        (
            "a bare resource id of the form AWS mints".to_string(),
            "7Q2WXYZ1ABCD3456".to_string(),
        ),
        (
            "a bare resource id of the form this service mints".to_string(),
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("a free-form name".to_string(), "my-landing-zone".to_string()),
        ("a single character".to_string(), "x".to_string()),
        ("the empty string".to_string(), String::new()),
        ("the bare word arn".to_string(), "arn".to_string()),
        (
            "an ARN of another AWS service".to_string(),
            "arn:aws:organizations::123456789012:ou/o-abc123/ou-abc1-22222222".to_string(),
        ),
        (
            "an ARN of another resource type of this service".to_string(),
            "arn:aws:controltower:us-west-2:123456789012:enabledcontrol/abc123".to_string(),
        ),
        (
            "a landing zone ARN with no resource id".to_string(),
            "arn:aws:controltower:us-west-2:123456789012:landingzone".to_string(),
        ),
        (
            "a landing zone ARN whose resource id is empty".to_string(),
            "arn:aws:controltower:us-west-2:123456789012:landingzone/".to_string(),
        ),
        (
            "an ARN whose account is one digit short of an account id".to_string(),
            "arn:aws:controltower:us-west-2:12345678901:landingzone/abc123".to_string(),
        ),
        (
            "an ARN whose account is one digit too long".to_string(),
            "arn:aws:controltower:us-west-2:1234567890123:landingzone/abc123".to_string(),
        ),
        (
            "an ARN whose account is not digits".to_string(),
            "arn:aws:controltower:us-west-2:abcdefghijkl:landingzone/abc123".to_string(),
        ),
        (
            "an ARN with no region".to_string(),
            "arn:aws:controltower::123456789012:landingzone/abc123".to_string(),
        ),
        (
            "an ARN of a partition outside aws".to_string(),
            "arn:azure:controltower:us-west-2:123456789012:landingzone/abc123".to_string(),
        ),
        (
            "an ARN one character past Arn's length max of 2048".to_string(),
            landing_zone_arn_of_length(2049),
        ),
    ];
    // The issue's own case, computed rather than written: the resource id the stored
    // landing zone's ARN ends in is NOT an identifier the service handed the caller.
    rows.push((
        "the stored landing zone's own bare resource id".to_string(),
        stored_bare_id,
    ));

    for (case, identifier) in rows {
        for (path, other_members) in operations() {
            let refused = call(&router, path, body(&other_members, &identifier)).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} ({identifier:?}) on {path}: expected 400 ValidationException, \
                 got {:?} body {:?}",
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            let message = refused.string("message");
            assert!(
                message.contains("landingZoneIdentifier"),
                "case {case:?} on {path}: expected the refusal to name the member it refused, \
                 got {message:?}"
            );
            started_nothing(&case, path, &refused);
        }
        unchanged(&router, &case, &arn, &deployed).await;
    }
}

/// RULE (the other side of it): a landing zone ARN that names no stored landing zone is a
/// missing resource — `ResourceNotFoundException` (404) — so the refusal above does not
/// swallow the answer a well-formed reference gets.
///
/// Rows differ along the segments a landing zone ARN carries: another account, another
/// region, another partition, the shortest resource id there is, a resource id of each
/// form the service and AWS mint, one carrying the underscore `Arn`'s character class
/// admits, and one sitting exactly ON `Arn`'s published length maximum of 2048.
#[tokio::test]
async fn a_landing_zone_arn_that_matches_no_landing_zone_is_not_found() {
    let router = emulator();
    let arn = create_settled_landing_zone(&router).await;
    let deployed = detail(&router, "the deployed landing zone", &arn).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an ARN naming another account",
            "arn:aws:controltower:us-west-2:000000000000:landingzone/abc123".to_string(),
        ),
        (
            "an ARN naming another region",
            "arn:aws:controltower:eu-west-1:123456789012:landingzone/abc123".to_string(),
        ),
        (
            "an ARN of another partition",
            "arn:aws-cn:controltower:cn-north-1:123456789012:landingzone/abc123".to_string(),
        ),
        (
            "an ARN whose resource id is a single character",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/x".to_string(),
        ),
        (
            "an ARN with a resource id of the form AWS mints",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/7Q2WXYZ1ABCD3456".to_string(),
        ),
        (
            "an ARN with a resource id of the form this service mints",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/\
             11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "an ARN whose resource id carries an underscore",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/abc_123".to_string(),
        ),
        (
            "an ARN exactly on Arn's length max of 2048",
            landing_zone_arn_of_length(2048),
        ),
    ];

    for (case, identifier) in rows {
        for (path, other_members) in operations() {
            let refused = call(&router, path, body(&other_members, &identifier)).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} ({identifier:?}) on {path}: expected 404 \
                 ResourceNotFoundException, got {:?} body {:?}",
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            started_nothing(case, path, &refused);
        }
        unchanged(&router, case, &arn, &deployed).await;
    }

    // The control: the ARN the service DID report for this landing zone is accepted by
    // the very operations that refused every row above, so the two refusals are about the
    // identifier rather than about these operations.
    let deleted = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the stored ARN: expected the delete to be accepted, got {} body {:?}",
        deleted.status, deleted.body
    );
}

/// RULE (that it is the ARN the service MINTED, wherever it minted it): the identifier a
/// create hands back resolves in the region it was called for, and the resource id that
/// ARN ends in does not — for every region, not only the one the other rows sign for.
#[tokio::test]
async fn only_the_minted_arn_resolves_in_any_region() {
    for region in [SIGNED_REGION, "eu-west-1", "ap-south-1"] {
        let router = emulator();
        let created = call_in_region(
            &router,
            region,
            "/create-landingzone",
            serde_json::json!({ "version": CATALOG, "manifest": simple_manifest() }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "region {region:?}: expected the create to return 200, got {} body {:?}",
            created.status, created.body
        );
        let arn = created.string("arn");
        assert!(
            arn.contains(region),
            "region {region:?}: expected the minted ARN to address that region, got {arn:?}"
        );

        let resolved = call_in_region(
            &router,
            region,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            resolved.status, 200,
            "region {region:?}: expected the minted ARN to resolve, got {} body {:?}",
            resolved.status, resolved.body
        );

        let bare_id = arn
            .rsplit('/')
            .next()
            .expect("an ARN ends in its resource segment")
            .to_string();
        let refused = call_in_region(
            &router,
            region,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": bare_id }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "region {region:?} ({bare_id:?}): expected 400 ValidationException for the bare \
             resource id, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}
