// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the documentation-version family: `CreateDocumentationVersion`
//! and `GetDocumentationVersion`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], [`Answer`], `create_api`,
//! `string_member` and `assert_failure` are this crate's one wire-test harness, reused
//! from `rest_api_lifecycle`, and the stage a snapshot is associated with is deployed
//! through the `stage_lifecycle` suite's `create_deployed_stage`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a snapshot is persisted under the version identifier the request names and read
//!   back member for member, over a TABLE of identifier forms and of the optional
//!   members a request may carry;
//! * a second snapshot under an identifier the API already has is refused as a conflict
//!   and changes nothing, over that same table;
//! * an identifier that names no API and one that names no snapshot of a known API are
//!   not found, over a table of both;
//! * an identifier that could not address a snapshot again (blank, or past the store's
//!   character bound) is refused, with the bound itself and one character past it as the
//!   rows;
//! * `stageName` associates the new snapshot with THAT stage, over a table of stages,
//!   and a stage name the API does not have is not found.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "NotFoundException";

/// The rows of the round-trip and conflict tables: a case name, the version identifier
/// and the description the request carries (`None` for a request that sets none).
///
/// The identifiers differ in form — a bare name, a dotted prerelease, a date, an
/// underscored name, a single character — because the rule is about the identifier being
/// the record's key, not about any one spelling of it. `documentationVersion` is a plain
/// `String` in this service's model with no pattern and no length constraint
/// (`smithy_query(query="operation", operation="CreateDocumentationVersion")`), so every
/// row below is a value the model admits.
const SNAPSHOTS: &[(&str, &str, Option<&str>)] = &[
    ("a bare identifier, no description", "v1", None),
    (
        "a dotted prerelease identifier",
        "1.0.0-beta.1",
        Some("the first beta snapshot"),
    ),
    ("a dated identifier", "2026-01-01", Some("the January cut")),
    ("an underscored identifier", "release_2026_1", None),
    ("a single character identifier", "7", Some("the seventh")),
];

/// `CreateDocumentationVersion` for `api`, with the request body sent as given.
async fn create_version(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/versions"),
            Some(request),
        )
        .await
}

/// `GetDocumentationVersion` for one snapshot of `api`.
async fn get_version(gateway: &Gateway, api: &str, version: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/versions/{version}"),
            None,
        )
        .await
}

/// The create request one table row sends.
fn snapshot_request(version: &str, description: Option<&str>) -> Value {
    let mut request = json!({ "documentationVersion": version });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    request
}

/// Assert `body` is the `DocumentationVersion` of `version`, with the description the
/// request carried and a creation date.
fn assert_snapshot(body: &Value, version: &str, description: Option<&str>, case: &str) {
    assert_eq!(
        body["version"].as_str(),
        Some(version),
        "case {case}: expected version {version:?}, got {body:?}"
    );
    match description {
        Some(expected) => assert_eq!(
            body["description"].as_str(),
            Some(expected),
            "case {case}: expected description {expected:?}, got {body:?}"
        ),
        None => assert!(
            body["description"].is_null(),
            "case {case}: a request that set no description must report none, got {body:?}"
        ),
    }
    assert!(
        body["createdDate"].is_number(),
        "case {case}: expected a createdDate epoch timestamp, got {:?}",
        body["createdDate"]
    );
}

#[tokio::test]
async fn a_created_documentation_version_is_persisted_and_read_back_member_for_member() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "petstore").await;

    for (case, version, description) in SNAPSHOTS {
        let created = create_version(&gateway, &api, snapshot_request(version, *description)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDocumentationVersion, got {} with {:?}",
            created.status, created.body
        );
        assert_snapshot(&created.body, version, *description, case);

        // The read reports the PERSISTED record, not a body synthesised from the
        // request: every member the create answered comes back identical.
        let read = get_version(&gateway, &api, version).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDocumentationVersion, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: the read must report what the create stored, got {:?} after {:?}",
            read.body, created.body
        );

        // And reading changes nothing a later caller sees — the creation date included,
        // which is stamped once rather than per read.
        let again = get_version(&gateway, &api, version).await;
        assert_eq!(
            again.body, read.body,
            "case {case}: a second read must report the same snapshot, got {:?} after {:?}",
            again.body, read.body
        );
    }

    // Each row is its own record: every snapshot created above is still readable, and
    // no two of them collapsed onto one identifier.
    for (case, version, description) in SNAPSHOTS {
        let read = get_version(&gateway, &api, version).await;
        assert_eq!(
            read.status, 200,
            "case {case}: every created snapshot stays readable, got {} with {:?}",
            read.status, read.body
        );
        assert_snapshot(&read.body, version, *description, case);
    }
}

#[tokio::test]
async fn a_second_snapshot_under_one_identifier_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "petstore").await;

    for (case, version, description) in SNAPSHOTS {
        let created = create_version(&gateway, &api, snapshot_request(version, *description)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from the first CreateDocumentationVersion, got {} with {:?}",
            created.status, created.body
        );

        // The identifier addresses one snapshot of one API, so a second create under it
        // is a conflict — whatever the rest of the request says.
        let again = create_version(
            &gateway,
            &api,
            snapshot_request(version, Some("a different description")),
        )
        .await;
        assert_failure(&again, 409, CONFLICT, &format!("{case}: recreated"));

        // The refused request did not overwrite the stored snapshot.
        let read = get_version(&gateway, &api, version).await;
        assert_eq!(
            read.body, created.body,
            "case {case}: a refused recreate must leave the snapshot as it was, got {:?} after {:?}",
            read.body, created.body
        );
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_api_or_no_snapshot_is_not_found() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "petstore").await;
    let (other, _other_root) = create_api(&gateway, "bookstore").await;
    let created = create_version(&gateway, &api, snapshot_request("v1", None)).await;
    assert_eq!(
        created.status, 201,
        "case setup: expected 201 from CreateDocumentationVersion, got {} with {:?}",
        created.status, created.body
    );

    // Rows differ along WHICH identifier of the pair names nothing: the API, the
    // snapshot, or a snapshot that exists but under a different API.
    let missing_api = "zzzzzzzzzz";
    let cases: Vec<(&str, String)> = vec![
        (
            "read a snapshot of an unknown API",
            format!("/restapis/{missing_api}/documentation/versions/v1"),
        ),
        (
            "read an unknown snapshot",
            format!("/restapis/{api}/documentation/versions/v2"),
        ),
        (
            "read a snapshot whose identifier differs only in case",
            format!("/restapis/{api}/documentation/versions/V1"),
        ),
        (
            "read another API's snapshot",
            format!("/restapis/{other}/documentation/versions/v1"),
        ),
    ];
    for (case, uri) in &cases {
        let answer = gateway.call("GET", uri, None).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
    }

    // Creating under an unknown API is the same refusal: there is no API to snapshot.
    let created = create_version(&gateway, missing_api, snapshot_request("v1", None)).await;
    assert_failure(&created, 404, NOT_FOUND, "create under an unknown API");
}

#[tokio::test]
async fn a_version_identifier_that_could_not_address_a_snapshot_is_refused() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "petstore").await;

    // The identifier is the record's key and the path label of every later read of it, so
    // it has to name something and has to fit the store's bound. The bound is this
    // crate's own `MAX_STRING_CHARS` (API Gateway publishes no length for
    // `documentationVersion`), tested AT the bound and one character past it.
    let at_bound = "v".repeat(MAX_STRING_CHARS);
    let past_bound = "v".repeat(MAX_STRING_CHARS + 1);
    let refused: Vec<(&str, &str)> = vec![
        ("an empty identifier", ""),
        ("a whitespace identifier", "   "),
        ("one character past the bound", past_bound.as_str()),
    ];
    for (case, version) in &refused {
        let answer = create_version(&gateway, &api, snapshot_request(version, None)).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
    }

    // At the bound it is accepted and round-trips, so the refusal above is the bound and
    // not a blanket refusal of long identifiers.
    let created = create_version(&gateway, &api, snapshot_request(&at_bound, None)).await;
    assert_eq!(
        created.status, 201,
        "case at the bound: expected 201, got {} with {:?}",
        created.status, created.body
    );
    let read = get_version(&gateway, &api, &at_bound).await;
    assert_eq!(
        read.status, 200,
        "case at the bound: expected 200 from GetDocumentationVersion, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["version"].as_str(),
        Some(at_bound.as_str()),
        "case at the bound: expected the identifier back, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_snapshot_created_for_a_stage_is_associated_with_that_stage_only() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "petstore").await;

    // Rows differ along which stage the snapshot is for, so the association is asserted
    // to follow `stageName` rather than to land on whichever stage exists.
    let cases: &[(&str, &str)] = &[("prod", "v1"), ("beta", "1.0.0-beta.1")];
    for (stage, _version) in cases {
        create_deployed_stage(&gateway, &api, stage).await;
    }
    for (stage, version) in cases {
        let created = create_version(
            &gateway,
            &api,
            json!({ "documentationVersion": version, "stageName": stage }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case stage {stage}: expected 201, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            string_member(&created.body, "version", stage),
            *version,
            "case stage {stage}: expected the snapshot identifier back, got {:?}",
            created.body
        );
    }

    // Each stage serves the version created FOR it, which is what a read of the stage
    // reports.
    for (stage, version) in cases {
        let read = gateway
            .call("GET", &format!("/restapis/{api}/stages/{stage}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case stage {stage}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["documentationVersion"].as_str(),
            Some(*version),
            "case stage {stage}: expected the associated documentation version, got {:?}",
            read.body
        );
    }

    // A stage name the API does not have names nothing to associate, and the refused
    // request stores no snapshot.
    let answer = create_version(
        &gateway,
        &api,
        json!({ "documentationVersion": "v2", "stageName": "nowhere" }),
    )
    .await;
    assert_failure(&answer, 404, NOT_FOUND, "an unknown stage");
    let read = get_version(&gateway, &api, "v2").await;
    assert_failure(
        &read,
        404,
        NOT_FOUND,
        "a snapshot the refused request did not store",
    );
}
