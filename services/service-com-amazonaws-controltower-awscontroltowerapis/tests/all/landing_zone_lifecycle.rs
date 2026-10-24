// SPDX-License-Identifier: CC0-1.0

//! The landing zone lifecycle, end to end over HTTP.
//!
//! Drives `CreateLandingZone`, `GetLandingZoneOperation`, `GetLandingZone`,
//! `ListLandingZones` and `DeleteLandingZone` through the same
//! [`MultiServiceRouter`] the production emulator dispatches with, so what these
//! tests exercise is the wire behaviour a client sees — status codes, error codes
//! and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * what a mutating call persists is what a later reporting call returns, for any
//!   `Manifest` document and any `remediationTypes` list;
//! * `latestAvailableVersion` is the NUMERICALLY later of the deployed version and the
//!   catalog's newest version, so it is never older than what is deployed;
//! * an identifier a mutating call returns resolves, and an identifier no call returned
//!   is `ResourceNotFoundException` (404);
//! * the account holds at most one landing zone.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// How long these tests wait for the clock to make an operation due: the service's own
/// settle span plus slack, so retuning `OPERATION_SETTLE_AFTER` retunes the waits too.
pub(crate) fn past_the_settle_span() -> std::time::Duration {
    service_com_amazonaws_controltower_awscontroltowerapis::state::OPERATION_SETTLE_AFTER
        + std::time::Duration::from_millis(400)
}

/// A fresh emulator: one engine, one Control Tower service, authentication off (these
/// tests are about the service's behaviour, not SigV4). Shared with the sibling suites
/// in this test target.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// As [`emulator`], also handing back the shared engine, for the one case whose subject
/// is the store's retention bound rather than a wire behaviour.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_controltower_awscontroltowerapis::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// A dispatched call's answer: the HTTP status, the `x-amzn-errortype` error code
/// restJson1 names a modeled failure with, and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_code: Option<String>,
    pub(crate) body: serde_json::Value,
}

impl Answer {
    /// The body as a borrowed JSON object member, or `Value::Null` when absent.
    pub(crate) fn get(&self, member: &str) -> &serde_json::Value {
        self.body.get(member).unwrap_or(&serde_json::Value::Null)
    }

    /// A member that must be a string, for assertion messages that show what came back.
    pub(crate) fn string(&self, member: &str) -> String {
        self.get(member)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// POST `path` with `body` as a signed restJson1 request. The SigV4 credential scope is
/// what routes the request to this service and names its region.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://controltower.us-west-2.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-west-2/controltower/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            // restJson1 may qualify the shape with a namespace or a URI.
            value
                .split('/')
                .next()
                .unwrap_or(value)
                .rsplit('#')
                .next()
                .unwrap_or(value)
                .to_string()
        });
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    Answer {
        status,
        error_code,
        body,
    }
}

/// A minimal well-formed manifest, for the cases whose subject is not the manifest.
fn simple_manifest() -> serde_json::Value {
    serde_json::json!({
        "governedRegions": ["us-west-2"],
        "organizationStructure": { "security": { "name": "Security" } }
    })
}

/// Create one landing zone with `version` and `manifest`, asserting it succeeded.
async fn create(
    router: &MultiServiceRouter,
    version: &str,
    manifest: serde_json::Value,
) -> Answer {
    let answer = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": version, "manifest": manifest }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "create version {version:?}: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
}

/// The modeled `OperationIdentifier` pattern:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, length 36.
pub(crate) fn is_operation_identifier(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        && value.len() == 36
}

/// The whole lifecycle: every identifier a mutating call returns resolves against the
/// store, and every field a reporting call returns comes from what was persisted.
#[tokio::test]
async fn the_landing_zone_lifecycle_is_served_from_the_store() {
    let router = emulator();
    let manifest = simple_manifest();

    // Before any create the account holds no landing zone.
    let empty = call(&router, "/list-landingzones", serde_json::json!({})).await;
    assert_eq!(
        empty.status, 200,
        "list before create: expected 200, got {} body {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.get("landingZones"),
        &serde_json::json!([]),
        "list before create: expected an empty landingZones list, got {:?}",
        empty.get("landingZones")
    );

    let created = create(&router, "3.2", manifest.clone()).await;
    let arn = created.string("arn");
    let create_operation = created.string("operationIdentifier");
    assert!(
        arn.starts_with("arn:aws:controltower:us-west-2:")
            && arn.contains(":landingzone/")
            && arn.len() >= 20,
        "create: expected a controltower landingzone ARN of at least 20 chars, got {arn:?}"
    );
    assert!(
        is_operation_identifier(&create_operation),
        "create: expected a 36-char lowercase-hex operationIdentifier, got {create_operation:?}"
    );

    // The CREATE operation resolves, and it is in flight: CreateLandingZone starts an
    // asynchronous operation, so the transient state is observable.
    let in_flight = call(
        &router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": create_operation }),
    )
    .await;
    let details = in_flight.get("operationDetails").clone();
    assert_eq!(
        details.get("operationType").and_then(|v| v.as_str()),
        Some("CREATE"),
        "get-landingzone-operation while in flight: expected operationType CREATE, got {details:?}"
    );
    assert_eq!(
        details.get("operationIdentifier").and_then(|v| v.as_str()),
        Some(create_operation.as_str()),
        "get-landingzone-operation: expected the identifier to be echoed from state, got {details:?}"
    );
    assert_eq!(
        details.get("status").and_then(|v| v.as_str()),
        Some("IN_PROGRESS"),
        "get-landingzone-operation while in flight: expected IN_PROGRESS, got {details:?}"
    );
    assert!(
        details.get("startTime").is_some(),
        "get-landingzone-operation: expected a startTime, got {details:?}"
    );
    assert!(
        details.get("endTime").is_none(),
        "get-landingzone-operation while in flight: expected no endTime, got {details:?}"
    );

    // While the CREATE is in flight the landing zone is PROCESSING, and deleting it
    // would leave it inconsistent.
    let processing = call(
        &router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        processing
            .get("landingZone")
            .get("status")
            .and_then(|v| v.as_str()),
        Some("PROCESSING"),
        "get-landingzone while the create is in flight: expected PROCESSING, got {:?}",
        processing.get("landingZone")
    );
    let conflict = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        (conflict.status, conflict.error_code.as_deref()),
        (409, Some("ConflictException")),
        "delete while the create is in flight: expected 409 ConflictException, got {:?} body {:?}",
        (conflict.status, conflict.error_code.as_deref()),
        conflict.body
    );

    // A second create collides with the one-landing-zone-per-account rule.
    let second = call(
        &router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.2", "manifest": manifest }),
    )
    .await;
    assert_eq!(
        (second.status, second.error_code.as_deref()),
        (400, Some("ValidationException")),
        "second create: expected 400 ValidationException, got {:?} body {:?}",
        (second.status, second.error_code.as_deref()),
        second.body
    );

    // The clock makes the CREATE due; nothing sleeps server-side, the wait is the
    // caller's.
    tokio::time::sleep(past_the_settle_span()).await;

    let settled = call(
        &router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": create_operation }),
    )
    .await;
    let settled_details = settled.get("operationDetails").clone();
    assert_eq!(
        settled_details.get("status").and_then(|v| v.as_str()),
        Some("SUCCEEDED"),
        "get-landingzone-operation after the span: expected SUCCEEDED, got {settled_details:?}"
    );
    let end_time = settled_details.get("endTime").cloned();
    assert!(
        end_time.is_some(),
        "get-landingzone-operation after the span: expected an endTime, got {settled_details:?}"
    );

    // A terminal record is stable: a second read reports the same endTime, it is not
    // re-stamped per read.
    let reread = call(
        &router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": create_operation }),
    )
    .await;
    assert_eq!(
        reread.get("operationDetails").get("endTime").cloned(),
        end_time,
        "re-reading a terminal operation must report the same endTime, got {:?}",
        reread.get("operationDetails")
    );

    // The landing zone now reports ACTIVE, with every member read from the store.
    let active = call(
        &router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    let detail = active.get("landingZone").clone();
    assert_eq!(
        detail.get("status").and_then(|v| v.as_str()),
        Some("ACTIVE"),
        "get-landingzone after the span: expected ACTIVE, got {detail:?}"
    );
    assert_eq!(
        detail.get("version").and_then(|v| v.as_str()),
        Some("3.2"),
        "get-landingzone: expected the version CreateLandingZone persisted, got {detail:?}"
    );
    assert_eq!(
        detail.get("arn").and_then(|v| v.as_str()),
        Some(arn.as_str()),
        "get-landingzone: expected the ARN the create returned, got {detail:?}"
    );
    assert_eq!(
        detail.get("manifest"),
        Some(&simple_manifest()),
        "get-landingzone: expected the manifest CreateLandingZone was given, got {detail:?}"
    );

    // The created landing zone is the one entry the list reports.
    let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
    assert_eq!(
        listed.get("landingZones"),
        &serde_json::json!([{ "arn": arn }]),
        "list after create: expected exactly the created ARN, got {:?}",
        listed.get("landingZones")
    );

    // Deleting it removes it from state and logs a DELETE operation.
    let deleted = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "delete: expected 200, got {} body {:?}",
        deleted.status, deleted.body
    );
    let delete_operation = deleted.string("operationIdentifier");
    assert!(
        is_operation_identifier(&delete_operation),
        "delete: expected a 36-char lowercase-hex operationIdentifier, got {delete_operation:?}"
    );
    assert_ne!(
        delete_operation, create_operation,
        "delete: expected a new operationIdentifier, got the create's {create_operation:?}"
    );

    let gone = call(
        &router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "get-landingzone after delete: expected 404 ResourceNotFoundException, got {:?} body {:?}",
        (gone.status, gone.error_code.as_deref()),
        gone.body
    );
    let emptied = call(&router, "/list-landingzones", serde_json::json!({})).await;
    assert_eq!(
        emptied.get("landingZones"),
        &serde_json::json!([]),
        "list after delete: expected an empty landingZones list, got {:?}",
        emptied.get("landingZones")
    );

    // The operation log outlives the landing zone: both identifiers still resolve.
    for (identifier, expected_type) in [
        (&create_operation, "CREATE"),
        (&delete_operation, "DELETE"),
    ] {
        let logged = call(
            &router,
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            logged
                .get("operationDetails")
                .get("operationType")
                .and_then(|v| v.as_str()),
            Some(expected_type),
            "case {identifier:?}: expected operationType {expected_type}, got {:?}",
            logged.get("operationDetails")
        );
    }

    // While the DELETE is still in flight the account is occupied by it, so a create
    // collides with that operation rather than minting a second landing zone.
    let too_soon = call(
        &router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.2", "manifest": simple_manifest() }),
    )
    .await;
    assert_eq!(
        (too_soon.status, too_soon.error_code.as_deref()),
        (409, Some("ConflictException")),
        "create while the DELETE is in flight: expected 409 ConflictException, got {:?} \
         body {:?}",
        (too_soon.status, too_soon.error_code.as_deref()),
        too_soon.body
    );

    // And a create succeeds again once that operation is due and the account holds no
    // landing zone.
    tokio::time::sleep(past_the_settle_span()).await;
    let recreated = create(&router, "3.2", simple_manifest()).await;
    assert_ne!(
        recreated.string("arn"),
        arn,
        "re-create after delete: expected a new ARN, got the deleted {arn:?}"
    );
}

/// RULE: `GetLandingZone` returns the `Manifest` document `CreateLandingZone` was given,
/// for ANY shape of document — the manifest is opaque to the service.
///
/// The rows differ along the document's own structure: nesting, arrays, scalars of each
/// JSON type, an empty object, and a key order the service must not reorder away.
#[tokio::test]
async fn any_manifest_document_round_trips_unchanged() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("empty object", serde_json::json!({})),
        ("flat scalars", serde_json::json!({ "a": 1, "b": "two", "c": true })),
        ("null member", serde_json::json!({ "retain": serde_json::Value::Null })),
        (
            "nested arrays and objects",
            serde_json::json!({
                "governedRegions": ["us-west-2", "eu-west-1"],
                "organizationStructure": {
                    "security": { "name": "Security" },
                    "sandbox": { "name": "Sandbox" }
                },
                "accessManagement": { "enabled": false }
            }),
        ),
        (
            "deeply nested",
            serde_json::json!({ "a": { "b": { "c": { "d": { "e": [1, [2, [3]]] } } } } }),
        ),
        (
            "numbers of each width",
            serde_json::json!({ "int": -7, "big": 9007199254740991i64, "frac": 1.5 }),
        ),
        ("array at the root", serde_json::json!([1, "two", { "three": 3 }])),
        ("string at the root", serde_json::json!("a manifest can be any document")),
    ];

    for (case, manifest) in cases {
        // One landing zone per account, so each row gets its own emulator.
        let router = emulator();
        let created = create(&router, "3.3", manifest.clone()).await;
        let arn = created.string("arn");

        let read_back = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        assert_eq!(
            read_back.get("landingZone").get("manifest"),
            Some(&manifest),
            "case {case:?}: expected the manifest to round-trip unchanged, got {:?}",
            read_back.get("landingZone").get("manifest")
        );
    }
}

/// RULE: `latestAvailableVersion` is the NUMERICALLY later of the deployed version and
/// the catalog's newest version, so it is never older than what is deployed.
///
/// The catalog's newest version is `3.3` (`crate::state::CATALOG_LANDING_ZONE_VERSION`).
/// Rows span: below the catalog, equal to it, above it, a minor component that is larger
/// numerically but smaller lexicographically (`3.10`), and both published length bounds
/// of `LandingZoneVersion` (len 3..10).
#[tokio::test]
async fn latest_available_version_is_the_numerically_later_of_deployed_and_catalog() {
    let cases = [
        ("below the catalog", "2.0", "3.3"),
        ("one minor below", "3.2", "3.3"),
        ("equal to the catalog", "3.3", "3.3"),
        ("above the catalog", "4.0", "4.0"),
        // 3.10 is numerically newer than 3.3 but lexicographically smaller.
        ("minor compared numerically", "3.10", "3.10"),
        // 10.0 is numerically newer than 3.3 but lexicographically smaller.
        ("major compared numerically", "10.0", "10.0"),
        // The published length bounds of LandingZoneVersion: min 3 and max 10.
        ("shortest accepted version", "1.0", "3.3"),
        ("longest accepted version", "1234.56789", "1234.56789"),
    ];

    for (case, deployed, expected_latest) in cases {
        let router = emulator();
        let created = create(&router, deployed, simple_manifest()).await;
        let arn = created.string("arn");

        let detail = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        let landing_zone = detail.get("landingZone").clone();
        assert_eq!(
            landing_zone.get("version").and_then(|v| v.as_str()),
            Some(deployed),
            "case {case:?}: expected the deployed version {deployed:?}, got {landing_zone:?}"
        );
        assert_eq!(
            landing_zone
                .get("latestAvailableVersion")
                .and_then(|v| v.as_str()),
            Some(expected_latest),
            "case {case:?}: expected latestAvailableVersion {expected_latest:?}, got {landing_zone:?}"
        );
    }
}

/// RULE: a request that does not satisfy the model's constraints is refused with 400
/// and nothing is persisted; a request the FRONT DOOR refuses (before any handler ran)
/// carries the generic `BadRequestException`, while a request that satisfied every
/// modeled constraint and was then refused by the handler carries the operation's own
/// `ValidationException`.
///
/// Rows sit one step OUTSIDE each published bound: `version` shorter than 3 and longer
/// than 10 characters, `remediationTypes` longer than its max of 1, and `maxResults`
/// below and above its range of 1..1 — plus one row whose value satisfies every
/// modeled constraint and is refused by the handler instead.
#[tokio::test]
async fn values_outside_a_modeled_bound_are_refused_and_persist_nothing() {
    // (case, path, body, the error code the refusal must carry)
    let cases: Vec<(&str, &str, serde_json::Value, &str)> = vec![
        (
            "version shorter than the min of 3",
            "/create-landingzone",
            serde_json::json!({ "version": "3", "manifest": simple_manifest() }),
            "BadRequestException",
        ),
        (
            "version longer than the max of 10",
            "/create-landingzone",
            serde_json::json!({ "version": "12345.67890", "manifest": simple_manifest() }),
            "BadRequestException",
        ),
        (
            "remediationTypes longer than the max of 1",
            "/create-landingzone",
            serde_json::json!({
                "version": "3.3",
                "manifest": simple_manifest(),
                "remediationTypes": ["INHERITANCE_DRIFT", "INHERITANCE_DRIFT"]
            }),
            "BadRequestException",
        ),
        (
            "maxResults below the range min of 1",
            "/list-landingzones",
            serde_json::json!({ "maxResults": 0 }),
            "BadRequestException",
        ),
        (
            "maxResults above the range max of 1",
            "/list-landingzones",
            serde_json::json!({ "maxResults": 2 }),
            "BadRequestException",
        ),
        (
            // Satisfies `ListLandingZonesNextToken`'s pattern, so the front door passes
            // it and the handler is the one that refuses it.
            "a nextToken no previous call returned",
            "/list-landingzones",
            serde_json::json!({ "nextToken": "not-a-token" }),
            "ValidationException",
        ),
        (
            // `manifest` carries no @required trait in this model, so the front door
            // passes the request and the handler is the one that refuses it.
            "no manifest to describe the landing zone with",
            "/create-landingzone",
            serde_json::json!({ "version": "3.3" }),
            "ValidationException",
        ),
    ];

    for (case, path, body, expected_code) in cases {
        let router = emulator();
        let refused = call(&router, path, body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(expected_code)),
            "case {case:?}: expected 400 {expected_code}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );

        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([]),
            "case {case:?}: a refused request must persist nothing, got {:?}",
            listed.get("landingZones")
        );
    }
}

/// RULE: an identifier no call returned resolves to nothing, so the operation answers
/// `ResourceNotFoundException` (404) — never a synthesized success.
///
/// Rows span both identifier kinds and both reporting and mutating operations, on an
/// empty account AND on an account that holds a different landing zone.
#[tokio::test]
async fn an_identifier_no_call_returned_is_not_found() {
    // Well-formed per OperationIdentifier's pattern, but never minted here.
    let unknown_operation = "00000000-0000-4000-8000-000000000000";
    let unknown_arn = "arn:aws:controltower:us-west-2:000000000000:landingzone/absent";

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "get-landingzone-operation on an unminted identifier",
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": unknown_operation }),
        ),
        (
            "get-landingzone on an unknown ARN",
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": unknown_arn }),
        ),
        (
            "get-landingzone on an unknown bare id",
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": "absent" }),
        ),
        (
            "delete-landingzone on an unknown ARN",
            "/delete-landingzone",
            serde_json::json!({ "landingZoneIdentifier": unknown_arn }),
        ),
    ];

    for populated in [false, true] {
        for (case, path, body) in &cases {
            let router = emulator();
            if populated {
                create(&router, "3.3", simple_manifest()).await;
            }
            let answer = call(&router, path, body.clone()).await;
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} (account populated: {populated}): expected 404 \
                 ResourceNotFoundException, got {:?} body {:?}",
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
        }
    }
}

/// RULE: an optional member is reported exactly as it was supplied — present with the
/// stored value, absent when it was never given.
///
/// `remediationTypes` is the one such list on `LandingZoneDetail`; its only modeled
/// value is `INHERITANCE_DRIFT` (`RemediationType`), and the list's length is 1..1.
#[tokio::test]
async fn optional_members_are_reported_as_they_were_supplied() {
    let cases: Vec<(&str, Option<Vec<&str>>)> = vec![
        ("absent", None),
        ("the one modeled remediation type", Some(vec!["INHERITANCE_DRIFT"])),
    ];

    for (case, remediation_types) in cases {
        let router = emulator();
        let mut body = serde_json::json!({ "version": "3.3", "manifest": simple_manifest() });
        if let Some(types) = &remediation_types {
            body["remediationTypes"] = serde_json::json!(types);
        }
        // `tags` is accepted on every row (0..200 entries) and must not disturb the rest.
        body["tags"] = serde_json::json!({ "Team": "platform" });

        let created = call(&router, "/create-landingzone", body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            created.status, created.body
        );
        let arn = created.string("arn");

        let detail = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        let landing_zone = detail.get("landingZone").clone();
        let expected = remediation_types
            .as_ref()
            .map(|types| serde_json::json!(types));
        assert_eq!(
            landing_zone.get("remediationTypes").cloned(),
            expected,
            "case {case:?}: expected remediationTypes {expected:?}, got {landing_zone:?}"
        );
    }
}

/// RULE: a landing zone resolves by either form of its identifier — the ARN it is stored
/// under, or the bare resource id that ARN ends in.
#[tokio::test]
async fn a_landing_zone_resolves_by_arn_or_by_bare_id() {
    let router = emulator();
    let created = create(&router, "3.3", simple_manifest()).await;
    let arn = created.string("arn");
    let bare_id = arn
        .split('/')
        .next_back()
        .expect("the ARN carries a resource id")
        .to_string();

    for (case, identifier) in [("full ARN", arn.clone()), ("bare resource id", bare_id)] {
        let detail = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            detail.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            detail.status, detail.body
        );
        assert_eq!(
            detail
                .get("landingZone")
                .get("arn")
                .and_then(|v| v.as_str()),
            Some(arn.as_str()),
            "case {case:?}: expected the stored ARN {arn:?}, got {:?}",
            detail.get("landingZone")
        );
    }
}

/// RULE: a reporting operation must not change what a later caller sees. Repeating each
/// read leaves the store reporting the same thing.
#[tokio::test]
async fn repeating_a_read_changes_nothing() {
    let router = emulator();
    let created = create(&router, "3.3", simple_manifest()).await;
    let arn = created.string("arn");
    let operation = created.string("operationIdentifier");

    // Past the settle span, so every read is of a terminal, fully-settled resource.
    tokio::time::sleep(past_the_settle_span()).await;

    let reads: Vec<(&str, serde_json::Value)> = vec![
        (
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        ),
        (
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": operation }),
        ),
        ("/list-landingzones", serde_json::json!({})),
    ];

    for (path, body) in reads {
        let first = call(&router, path, body.clone()).await;
        let second = call(&router, path, body.clone()).await;
        assert_eq!(
            first.body, second.body,
            "case {path:?}: repeating a read must report the same thing, got {:?} then {:?}",
            first.body, second.body
        );
        // And no read may consume the resource: it is still listed.
        let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([{ "arn": arn }]),
            "case {path:?}: a read must not remove the landing zone, got {:?}",
            listed.get("landingZones")
        );
    }
}

/// RULE: the operation log retains at most `MAX_LANDING_ZONE_OPERATIONS` records, and
/// what it drops to stay inside that bound is the OLDEST record — a terminal one by
/// preference, so an operation a caller is still polling is never dropped out from under
/// it. A record still in the log resolves; an evicted one is `ResourceNotFoundException`.
///
/// Rows span the two eviction orders: a log that holds terminal records, and one whose
/// records are all still in flight.
#[tokio::test]
async fn the_operation_log_keeps_the_newest_records_within_its_bound() {
    use service_com_amazonaws_controltower_awscontroltowerapis::state::{
        service_state, LandingZoneOperationType, MAX_LANDING_ZONE_OPERATIONS,
    };
    use std::time::{Duration, SystemTime};

    for (case, terminal) in [
        ("the oldest terminal record is evicted", true),
        ("with nothing terminal, the oldest record is evicted", false),
    ] {
        let (router, engine) = emulator_with_engine();
        let state = service_state(&*engine).expect("the service's own store");
        let arn = "arn:aws:controltower:us-west-2:000000000000:landingzone/retention";

        // Fill the log to its cap. Distinct, increasing start times order the records
        // totally, so which one is "oldest" is unambiguous.
        let base = if terminal {
            // Already past their due instant, so `settle` makes them terminal below.
            SystemTime::now() - Duration::from_secs(3600)
        } else {
            SystemTime::now()
        };
        let mut recorded = Vec::with_capacity(MAX_LANDING_ZONE_OPERATIONS);
        {
            let mut store = state.lock();
            for step in 0..MAX_LANDING_ZONE_OPERATIONS {
                let at = base + Duration::from_millis(step as u64);
                recorded.push(store.record_operation(
                    LandingZoneOperationType::Update,
                    arn,
                    at,
                ));
            }
            if terminal {
                store.settle(SystemTime::now());
            }
            assert_eq!(
                store.landing_zone_operations.len(),
                MAX_LANDING_ZONE_OPERATIONS,
                "case {case:?}: expected the log to hold its cap of \
                 {MAX_LANDING_ZONE_OPERATIONS}, got {}",
                store.landing_zone_operations.len()
            );
        }

        // One more record pushes the log past its cap.
        let newest = {
            let mut store = state.lock();
            let id = store.record_operation(
                LandingZoneOperationType::Reset,
                arn,
                base + Duration::from_millis(MAX_LANDING_ZONE_OPERATIONS as u64),
            );
            assert_eq!(
                store.landing_zone_operations.len(),
                MAX_LANDING_ZONE_OPERATIONS,
                "case {case:?}: expected the log to stay at its cap of \
                 {MAX_LANDING_ZONE_OPERATIONS}, got {}",
                store.landing_zone_operations.len()
            );
            id
        };

        // The oldest record is the one that went; the newest and the rest still resolve.
        let evicted = call(
            &router,
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": recorded[0] }),
        )
        .await;
        assert_eq!(
            (evicted.status, evicted.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the oldest record to be evicted (404 \
             ResourceNotFoundException), got {:?} body {:?}",
            (evicted.status, evicted.error_code.as_deref()),
            evicted.body
        );
        for (label, identifier) in [("newest", &newest), ("second oldest", &recorded[1])] {
            let kept = call(
                &router,
                "/get-landingzone-operation",
                serde_json::json!({ "operationIdentifier": identifier }),
            )
            .await;
            assert_eq!(
                kept.status, 200,
                "case {case:?} ({label}): expected a retained record to resolve, got {} \
                 body {:?}",
                kept.status, kept.body
            );
        }
    }
}

/// RULE: every identifier this service mints satisfies the modeled `OperationIdentifier`
/// form — 36 characters, lowercase hex in the 8-4-4-4-12 grouping, with RFC 4122's
/// version-4 and variant bits — and no two mints collide.
///
/// Asserted over a BATCH of mints rather than one, because uniqueness is only visible
/// across a set: the rows are the mint ordinals, which span identifiers taken within one
/// clock tick (the counter must separate them) as well as across ticks.
#[tokio::test]
async fn every_minted_identifier_is_well_formed_and_unique() {
    use std::collections::BTreeSet;

    // Bounded, and large enough that same-tick mints occur: a tight loop mints far
    // faster than the clock's resolution.
    const MINTS: usize = 512;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for ordinal in 0..MINTS {
        let identifier =
            service_com_amazonaws_controltower_awscontroltowerapis::state::new_operation_identifier();
        assert!(
            is_operation_identifier(&identifier),
            "mint {ordinal}: expected the modeled OperationIdentifier form, got {identifier:?}"
        );
        // RFC 4122 version 4 (13th hex digit) and variant (17th hex digit in 8..b).
        let digits: Vec<char> = identifier.chars().filter(|c| *c != '-').collect();
        assert_eq!(
            digits[12], '4',
            "mint {ordinal}: expected version nibble 4, got {:?} in {identifier:?}",
            digits[12]
        );
        assert!(
            matches!(digits[16], '8' | '9' | 'a' | 'b'),
            "mint {ordinal}: expected variant nibble in 8..b, got {:?} in {identifier:?}",
            digits[16]
        );
        assert!(
            seen.insert(identifier.clone()),
            "mint {ordinal}: {identifier:?} was already minted — identifiers must be unique"
        );
    }
    assert_eq!(
        seen.len(),
        MINTS,
        "expected {MINTS} distinct identifiers, got {}",
        seen.len()
    );

    // And over the wire: the identifiers two different calls return are distinct and
    // both resolve against the log.
    let router = emulator();
    let created = create(&router, "3.3", simple_manifest()).await;
    let create_operation = created.string("operationIdentifier");
    tokio::time::sleep(past_the_settle_span()).await;
    let deleted = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": created.string("arn") }),
    )
    .await;
    let delete_operation = deleted.string("operationIdentifier");
    assert_ne!(
        create_operation, delete_operation,
        "two calls must mint different identifiers, got {create_operation:?} twice"
    );
    for (case, identifier) in [
        ("create", &create_operation),
        ("delete", &delete_operation),
    ] {
        let resolved = call(
            &router,
            "/get-landingzone-operation",
            serde_json::json!({ "operationIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            resolved.status, 200,
            "case {case:?}: expected {identifier:?} to resolve, got {} body {:?}",
            resolved.status, resolved.body
        );
    }
}

/// RULE: `ListLandingZones` honours `maxResults` and `nextToken`, and the token it hands
/// out resumes where the page stopped. With one landing zone and `maxResults` modeled
/// `1..1` there is exactly one page, so no token is returned.
#[tokio::test]
async fn listing_pages_from_the_store() {
    let router = emulator();
    let created = create(&router, "3.3", simple_manifest()).await;
    let arn = created.string("arn");

    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "no page size",
            serde_json::json!({}),
            serde_json::json!([{ "arn": arn }]),
        ),
        (
            "maxResults at its only legal value",
            serde_json::json!({ "maxResults": 1 }),
            serde_json::json!([{ "arn": arn }]),
        ),
        (
            "an empty nextToken is the first page",
            serde_json::json!({ "nextToken": "" }),
            serde_json::json!([{ "arn": arn }]),
        ),
    ];

    for (case, body, expected) in cases {
        let listed = call(&router, "/list-landingzones", body).await;
        assert_eq!(
            listed.get("landingZones"),
            &expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.get("landingZones")
        );
        assert!(
            listed.get("nextToken").is_null(),
            "case {case:?}: a single-page result must carry no nextToken, got {:?}",
            listed.get("nextToken")
        );
    }
}
