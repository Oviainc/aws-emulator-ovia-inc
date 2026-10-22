// SPDX-License-Identifier: CC0-1.0

//! What a Redshift Serverless response says about a namespace, over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: RedshiftServerless.<Operation>` and an awsJson1_1 body — so the boundary
//! validator, the protocol codec and the handler all take part, exactly as they do for an
//! SDK client.
//!
//! Four rules are under test, each over a TABLE of inputs rather than one request:
//!
//! 1. A member the SERVICE fills in when the request leaves it unset is stored like any
//!    other member, so every operation that returns the resource reports it:
//!    `adminUsername`, `kmsKeyId` and `lakehouseRegistrationStatus` on a namespace.
//! 2. A timestamp member whose model declares `@timestampFormat("date-time")` is
//!    serialized as an RFC 3339 STRING, never as the protocol's default epoch number.
//! 3. A namespace NAME this service cannot use — one that resolves to nothing, or one
//!    already taken — is refused with HTTP 400 carrying the modelled error shape.
//! 4. A modelled `@pattern` is interpolated into a validation message un-anchored; every
//!    other violation class keeps the framework's own sentence.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to this service: it is what routes the request
/// here. Signatures are not verified in these tests (authentication is off).
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/redshift-serverless/aws4_request, \
     SignedHeaders=host, Signature=deadbeef";

/// One emulator with an empty store: the shared engine the router builds in production,
/// carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(engine),
    );
    router
}

/// Call `operation` with `body`; answer its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RedshiftServerless.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The error shape name out of an awsJson error envelope, with any namespace prefix
/// dropped (`com.amazonaws.redshiftserverless#ValidationException` ->
/// `ValidationException`).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('#')
        .next_back()
        .unwrap_or("")
        .to_string()
}

/// The `message` of an error envelope.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// `value` read as the RFC 3339 `date-time` string the model asks for: the parsed instant,
/// or the reason it is not one. A JSON number — the protocol default this service must NOT
/// fall back to for these members — fails here.
fn as_date_time(value: &Value) -> Result<DateTime, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("expected a date-time string, got {value}"))?;
    DateTime::from_str(text, Format::DateTime)
        .map_err(|error| format!("{text:?} is not an RFC 3339 date-time: {error}"))
}

/// Assert `value` is a date-time string naming an instant around now.
fn assert_recent_date_time(case: &str, value: &Value) {
    let parsed = match as_date_time(value) {
        Ok(parsed) => parsed,
        Err(why) => panic!("case {case:?}: {why}"),
    };
    let skew = (parsed.secs() - now_secs()).abs();
    assert!(
        skew < 300,
        "case {case:?}: expected an instant around now, got {value} ({skew}s away)"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: what the service fills in, it also reports — on every read.
// ---------------------------------------------------------------------------

/// One create case: a label, the members the request sets, and the namespace members every
/// later read must then carry.
type CreateCase = (&'static str, Value, Vec<(&'static str, Value)>);

/// One update case, read the same way as a [`CreateCase`].
type UpdateCase = CreateCase;

/// An operation and how it names a resource in its request body.
type NamedRequest = (&'static str, fn(&str) -> Value);

#[tokio::test]
async fn the_members_the_service_fills_in_are_reported_by_every_namespace_reader() {
    // The three defaults under test are the service's own, not the request's:
    //   adminUsername              -> "admin", the administrator of the first database
    //   kmsKeyId                   -> "AWS_OWNED_KMS_KEY", the Amazon-owned key
    //   lakehouseRegistrationStatus -> "NOT_REGISTERED", until a lakehouse registration
    // The rows differ along which of them the request overrides, and in the size of the
    // `logExports` list (`LogExportList`, @length 0..16), whose values are the three types
    // `CreateNamespace$logExports` documents: userlog, connectionlog, useractivitylog.
    let defaults = vec![
        ("adminUsername", json!("admin")),
        ("kmsKeyId", json!("AWS_OWNED_KMS_KEY")),
        ("lakehouseRegistrationStatus", json!("NOT_REGISTERED")),
        ("logExports", json!([])),
    ];
    let cases: Vec<CreateCase> = vec![
        ("no optional member set", json!({}), defaults.clone()),
        (
            "an admin user named",
            json!({ "adminUsername": "dbadmin", "adminUserPassword": "Passw0rd!" }),
            vec![
                ("adminUsername", json!("dbadmin")),
                ("kmsKeyId", json!("AWS_OWNED_KMS_KEY")),
                ("lakehouseRegistrationStatus", json!("NOT_REGISTERED")),
            ],
        ),
        (
            "a customer managed key named",
            json!({ "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc" }),
            vec![
                ("adminUsername", json!("admin")),
                (
                    "kmsKeyId",
                    json!("arn:aws:kms:us-east-1:000000000000:key/abc"),
                ),
                ("lakehouseRegistrationStatus", json!("NOT_REGISTERED")),
            ],
        ),
        (
            "one log export",
            json!({ "logExports": ["userlog"] }),
            vec![
                ("logExports", json!(["userlog"])),
                ("adminUsername", json!("admin")),
                ("kmsKeyId", json!("AWS_OWNED_KMS_KEY")),
            ],
        ),
        (
            "every documented log export",
            json!({ "logExports": ["userlog", "connectionlog", "useractivitylog"] }),
            vec![(
                "logExports",
                json!(["userlog", "connectionlog", "useractivitylog"]),
            )],
        ),
        (
            "an empty log export list",
            json!({ "logExports": [] }),
            vec![("logExports", json!([]))],
        ),
        (
            "the admin password managed by the service",
            json!({ "manageAdminPassword": true }),
            vec![
                ("adminUsername", json!("admin")),
                ("kmsKeyId", json!("AWS_OWNED_KMS_KEY")),
            ],
        ),
    ];

    let router = emulator();
    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("fidelity-ns-{index:02}");
        let mut body = request.as_object().cloned().unwrap_or_default();
        body.insert("namespaceName".to_string(), json!(name));
        let (status, created) = call(&router, "CreateNamespace", Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNamespace expected 200, got {status} with {created}"
        );

        // The same record must come back out of every operation that returns a namespace:
        // the create's own answer, a get, a list and the delete that removes it.
        let (status, read) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNamespace expected 200, got {status} with {read}"
        );
        let (status, listed) = call(&router, "ListNamespaces", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNamespaces expected 200, got {status} with {listed}"
        );
        let from_list = listed["namespaces"]
            .as_array()
            .expect("ListNamespaces answers a list")
            .iter()
            .find(|entry| entry["namespaceName"] == json!(name.clone()))
            .cloned()
            .unwrap_or(Value::Null);

        for (member, want) in expected {
            for (reader, answer) in [
                ("CreateNamespace", &created["namespace"]),
                ("GetNamespace", &read["namespace"]),
                ("ListNamespaces", &from_list),
            ] {
                assert_eq!(
                    &answer[member], want,
                    "case {case:?}: {reader} {member}: expected {want}, got {:?}",
                    answer[member]
                );
            }
        }

        // And a read must report exactly what the create reported, member for member.
        assert_eq!(
            read["namespace"], created["namespace"],
            "case {case:?}: GetNamespace must report the created record, got {:?}",
            read["namespace"]
        );
    }
}

#[tokio::test]
async fn an_updated_namespace_reports_what_the_update_stored() {
    // `UpdateNamespace` writes the members it names and nothing else, and the write is
    // observable through `GetNamespace`. The rows differ along which setting is updated;
    // each is one of the pairings `UpdateNamespace` documents.
    let cases: [UpdateCase; 5] = [
        (
            "one log export",
            json!({ "logExports": ["userlog"] }),
            vec![("logExports", json!(["userlog"]))],
        ),
        (
            "three log exports",
            json!({ "logExports": ["userlog", "connectionlog", "useractivitylog"] }),
            vec![(
                "logExports",
                json!(["userlog", "connectionlog", "useractivitylog"]),
            )],
        ),
        (
            "no log export at all",
            json!({ "logExports": [] }),
            vec![("logExports", json!([]))],
        ),
        (
            "a customer managed key",
            json!({ "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/rotated" }),
            vec![(
                "kmsKeyId",
                json!("arn:aws:kms:us-east-1:000000000000:key/rotated"),
            )],
        ),
        (
            "the admin user and its password",
            json!({ "adminUsername": "newadmin", "adminUserPassword": "Passw0rd!" }),
            vec![("adminUsername", json!("newadmin"))],
        ),
    ];

    let router = emulator();
    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("update-ns-{index:02}");
        let (status, created) = call(
            &router,
            "CreateNamespace",
            json!({ "namespaceName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNamespace expected 200, got {status} with {created}"
        );

        let mut body = request.as_object().cloned().unwrap_or_default();
        body.insert("namespaceName".to_string(), json!(name.clone()));
        let (status, updated) = call(&router, "UpdateNamespace", Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateNamespace expected 200, got {status} with {updated}"
        );

        let (status, read) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": name.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNamespace expected 200, got {status} with {read}"
        );
        for (member, want) in expected {
            assert_eq!(
                &updated["namespace"][member], want,
                "case {case:?}: UpdateNamespace {member}: expected {want}, got {:?}",
                updated["namespace"][member]
            );
            assert_eq!(
                &read["namespace"][member], want,
                "case {case:?}: GetNamespace after the update {member}: expected {want}, \
                 got {:?}",
                read["namespace"][member]
            );
        }
        // The identity the create minted survives an update, and the password never
        // reaches the wire.
        assert_eq!(
            read["namespace"]["namespaceId"], created["namespace"]["namespaceId"],
            "case {case:?}: an update must not re-mint the namespace id, got {:?}",
            read["namespace"]["namespaceId"]
        );
        assert!(
            !updated.to_string().contains("Passw0rd!"),
            "case {case:?}: the admin password must never be echoed, got {updated}"
        );
    }
}

#[tokio::test]
async fn an_update_that_breaks_a_documented_pairing_is_refused_and_changes_nothing() {
    // Every row is a combination `UpdateNamespace`'s own documentation rules out, so each
    // is a 400 that leaves the stored namespace exactly as the create left it. The last
    // row is the Amazon S3 Tables log-publishing update, which this emulator does not
    // serve: it answers 501, never a publish status for tables nothing published.
    let cases: [(&str, Value, u16, &str); 6] = [
        (
            "an admin username with no password",
            json!({ "adminUsername": "lonely" }),
            400,
            "ValidationException",
        ),
        (
            "an admin password with no username",
            json!({ "adminUserPassword": "Passw0rd!" }),
            400,
            "ValidationException",
        ),
        (
            "iam roles with no default role",
            json!({ "iamRoles": ["arn:aws:iam::000000000000:role/one"] }),
            400,
            "ValidationException",
        ),
        (
            "a kms key together with log exports",
            json!({ "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc",
                    "logExports": ["userlog"] }),
            400,
            "ValidationException",
        ),
        (
            "an s3 table parameter without the s3table destination",
            json!({ "s3TableGranularity": "namespace" }),
            400,
            "ValidationException",
        ),
        (
            "the s3 tables log destination",
            json!({ "logDestinationType": "s3table", "s3TableGranularity": "namespace" }),
            501,
            "NotImplemented",
        ),
    ];

    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "pairing-ns", "logExports": ["connectionlog"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNamespace expected 200, got {status} with {created}"
    );

    for (case, request, expected_status, expected_code) in cases {
        let mut body = request.as_object().cloned().unwrap_or_default();
        body.insert("namespaceName".to_string(), json!("pairing-ns"));
        let (status, refused) = call(&router, "UpdateNamespace", Value::Object(body)).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (expected_status, expected_code),
            "case {case:?}: got {status} with {refused}"
        );
        let (status, read) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": "pairing-ns" }),
        )
        .await;
        assert_eq!(
            (status, &read["namespace"]),
            (200, &created["namespace"]),
            "case {case:?}: a refused update must change nothing, got {status} with {read}"
        );
    }
}

#[tokio::test]
async fn an_updated_namespace_is_modifying_and_reaches_available_on_its_own() {
    // `NamespaceStatus` models `MODIFYING` beside `AVAILABLE`, and the recorded
    // `UpdateNamespace` answers in it. A caller acting straight after the update still
    // observes the transient status; the clock alone settles it, with nobody calling.
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "settle-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNamespace expected 200, got {status} with {created}"
    );
    assert_eq!(
        created["namespace"]["status"],
        json!("AVAILABLE"),
        "a created namespace carries no compute, so it is available at once, got {:?}",
        created["namespace"]["status"]
    );

    let (status, updated) = call(
        &router,
        "UpdateNamespace",
        json!({ "namespaceName": "settle-ns", "logExports": ["userlog"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateNamespace expected 200, got {status} with {updated}"
    );
    assert_eq!(
        updated["namespace"]["status"],
        json!("MODIFYING"),
        "UpdateNamespace answers in MODIFYING, got {:?}",
        updated["namespace"]["status"]
    );
    let (_, read) = call(
        &router,
        "GetNamespace",
        json!({ "namespaceName": "settle-ns" }),
    )
    .await;
    assert_eq!(
        read["namespace"]["status"],
        json!("MODIFYING"),
        "GetNamespace straight after the update: got {:?}",
        read["namespace"]["status"]
    );

    // Nobody calls anything for the span; the clock alone settles it.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (_, read) = call(
        &router,
        "GetNamespace",
        json!({ "namespaceName": "settle-ns" }),
    )
    .await;
    assert_eq!(
        read["namespace"]["status"],
        json!("AVAILABLE"),
        "GetNamespace once the transition fell due: got {:?}",
        read["namespace"]["status"]
    );
    assert_eq!(
        read["namespace"]["logExports"],
        json!(["userlog"]),
        "the update's value survives the transition, got {:?}",
        read["namespace"]["logExports"]
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a `date-time` timestamp is a string on the wire.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_date_time_timestamp_member_is_serialized_as_an_rfc3339_string() {
    // The members the model marks `@timestampFormat("date-time")` and this emulator sets:
    // `Namespace$creationDate` and `Workgroup$creationDate`
    // (`smithy_query(query="shape", shape="Namespace,Workgroup")`). Each is checked on
    // EVERY operation that returns the resource carrying it, because the format belongs to
    // the member, not to one response.
    let router = emulator();

    let (status, created_ns) = call(
        &router,
        "CreateNamespace",
        json!({ "namespaceName": "clock-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNamespace expected 200, got {status} with {created_ns}"
    );
    let (status, updated_ns) = call(
        &router,
        "UpdateNamespace",
        json!({ "namespaceName": "clock-ns", "logExports": ["userlog"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateNamespace expected 200, got {status} with {updated_ns}"
    );
    let (status, read_ns) = call(
        &router,
        "GetNamespace",
        json!({ "namespaceName": "clock-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetNamespace expected 200, got {status} with {read_ns}"
    );
    let (status, listed_ns) = call(&router, "ListNamespaces", json!({})).await;
    assert_eq!(
        status, 200,
        "ListNamespaces expected 200, got {status} with {listed_ns}"
    );

    let (status, created_wg) = call(
        &router,
        "CreateWorkgroup",
        json!({ "workgroupName": "clock-wg", "namespaceName": "clock-ns" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkgroup expected 200, got {status} with {created_wg}"
    );
    let (status, read_wg) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "clock-wg" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetWorkgroup expected 200, got {status} with {read_wg}"
    );
    let (status, listed_wg) = call(&router, "ListWorkgroups", json!({})).await;
    assert_eq!(
        status, 200,
        "ListWorkgroups expected 200, got {status} with {listed_wg}"
    );
    let (status, updated_wg) = call(
        &router,
        "UpdateWorkgroup",
        json!({ "workgroupName": "clock-wg", "baseCapacity": 64 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateWorkgroup expected 200, got {status} with {updated_wg}"
    );

    let first_namespace = listed_ns["namespaces"][0].clone();
    let first_workgroup = listed_wg["workgroups"][0].clone();
    let cases: [(&str, &Value); 7] = [
        (
            "CreateNamespace namespace.creationDate",
            &created_ns["namespace"]["creationDate"],
        ),
        (
            "GetNamespace namespace.creationDate",
            &read_ns["namespace"]["creationDate"],
        ),
        (
            "UpdateNamespace namespace.creationDate",
            &updated_ns["namespace"]["creationDate"],
        ),
        (
            "ListNamespaces namespaces[0].creationDate",
            &first_namespace["creationDate"],
        ),
        (
            "CreateWorkgroup workgroup.creationDate",
            &created_wg["workgroup"]["creationDate"],
        ),
        (
            "GetWorkgroup workgroup.creationDate",
            &read_wg["workgroup"]["creationDate"],
        ),
        (
            "ListWorkgroups workgroups[0].creationDate",
            &first_workgroup["creationDate"],
        ),
    ];
    for (case, value) in cases {
        assert!(
            !value.is_number(),
            "case {case:?}: a date-time member must not be an epoch number, got {value}"
        );
        assert_recent_date_time(case, value);
    }

    // The instant is the resource's own: it does not move when the resource is read
    // again, and an update does not re-mint it.
    assert_eq!(
        read_ns["namespace"]["creationDate"], created_ns["namespace"]["creationDate"],
        "a namespace's creationDate is stable across reads, got {:?} then {:?}",
        created_ns["namespace"]["creationDate"], read_ns["namespace"]["creationDate"]
    );
    assert_eq!(
        updated_ns["namespace"]["creationDate"], created_ns["namespace"]["creationDate"],
        "an update must not re-mint creationDate, got {:?}",
        updated_ns["namespace"]["creationDate"]
    );
    assert_eq!(
        read_wg["workgroup"]["creationDate"], created_wg["workgroup"]["creationDate"],
        "a workgroup's creationDate is stable across reads, got {:?} then {:?}",
        created_wg["workgroup"]["creationDate"], read_wg["workgroup"]["creationDate"]
    );
}

// ---------------------------------------------------------------------------
// Rule 3: an unusable namespace name is a 400 ValidationException.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_namespace_name_that_resolves_to_nothing_is_a_bad_request_on_every_operation() {
    // Three names, so the sentence is built from the request rather than recited: each
    // refusal names the namespace the caller asked for.
    let names = ["absent-one", "absent-two-9", "a-much-longer-absent-name"];
    let readers: [NamedRequest; 5] = [
        ("GetNamespace", |name| json!({ "namespaceName": name })),
        ("DeleteNamespace", |name| json!({ "namespaceName": name })),
        ("UpdateNamespace", |name| {
            json!({ "namespaceName": name, "logExports": ["userlog"] })
        }),
        ("CreateWorkgroup", |name| {
            json!({ "workgroupName": "any-workgroup", "namespaceName": name })
        }),
        ("CreateSnapshot", |name| {
            json!({ "namespaceName": name, "snapshotName": "any-snapshot" })
        }),
    ];

    let router = emulator();
    for name in names {
        for (operation, request) in readers {
            let (status, body) = call(&router, operation, request(name)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation:?} on {name:?}: got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("Serverless namespace {name} not found."),
                "case {operation:?} on {name:?}: the refusal names the namespace asked \
                 for, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_namespace_name_already_taken_is_a_bad_request() {
    // Three names again: the refusal is worded from the request, and the stored namespace
    // is untouched by the attempt.
    let names = ["taken-one", "taken-two-9", "a-much-longer-taken-name"];
    let router = emulator();
    for name in names {
        let (status, created) = call(
            &router,
            "CreateNamespace",
            json!({ "namespaceName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateNamespace expected 200, got {status} with {created}"
        );

        // A second create of the same name, with DIFFERENT members, is still refused.
        let (status, refused) = call(
            &router,
            "CreateNamespace",
            json!({ "namespaceName": name, "dbName": "other", "adminUsername": "other" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ConflictException"),
            "case {name:?}: a duplicate create got {status} with {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("The namespace {name} already exists."),
            "case {name:?}: the refusal names the namespace asked for, got {refused}"
        );

        let (status, read) = call(&router, "GetNamespace", json!({ "namespaceName": name })).await;
        assert_eq!(
            (status, &read["namespace"]),
            (200, &created["namespace"]),
            "case {name:?}: the refused create must change nothing, got {status} with {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a modelled `@pattern` is stated un-anchored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pattern_violation_states_the_constraint_without_its_anchors() {
    // `NamespaceName` and `WorkgroupName` are both @length(3..64) @pattern("^[a-z0-9-]+$")
    // (`smithy_query(query="shape", shape="NamespaceName,WorkgroupName")`), so the
    // constraint a caller is told about is `[a-z0-9-]+`. The rows span the ways a value can
    // fall outside that character set.
    let bad_names = [
        "SIM2REAL-Probe",
        "UPPER",
        "under_score",
        "has space",
        "dots.are.out",
        "é-accented",
    ];
    let operations: [NamedRequest; 2] = [
        ("CreateNamespace", |name| json!({ "namespaceName": name })),
        ("CreateWorkgroup", |name| {
            json!({ "workgroupName": name, "namespaceName": "pattern-ns" })
        }),
    ];

    let router = emulator();
    for (operation, request) in operations {
        for name in bad_names {
            let (status, body) = call(&router, operation, request(name)).await;
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {operation:?} on {name:?}: got {status} with {body}"
            );
            assert!(
                message.contains("Member must satisfy regular expression pattern: [a-z0-9-]+"),
                "case {operation:?} on {name:?}: expected the un-anchored pattern, got \
                 {message:?}"
            );
            assert!(
                !message.contains("^[a-z0-9-]+$"),
                "case {operation:?} on {name:?}: the anchors must not reach the caller, \
                 got {message:?}"
            );
            assert!(
                message.starts_with("1 validation error detected: Value '"),
                "case {operation:?} on {name:?}: the envelope is unchanged, got {message:?}"
            );
            assert!(
                message.contains(&format!("Value '{name}' at '")),
                "case {operation:?} on {name:?}: the refused value is echoed, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_violation_of_another_class_keeps_the_frameworks_wording() {
    // Only the interpolated pattern is this service's wording. A length violation and an
    // absent required member must still read exactly as the framework states them, so
    // rewording one class cannot drift the others.
    let cases: [(&str, Value, &str); 3] = [
        (
            "shorter than the 3 character minimum",
            json!({ "namespaceName": "ab" }),
            "1 validation error detected: Value 'ab' at 'namespaceName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3",
        ),
        (
            "longer than the 64 character maximum",
            json!({ "namespaceName": "a".repeat(65) }),
            "failed to satisfy constraint: Member must have length less than or equal to 64",
        ),
        (
            "the required member absent",
            json!({}),
            "Member must not be null",
        ),
    ];

    let router = emulator();
    for (case, request, expected) in cases {
        let (status, body) = call(&router, "CreateNamespace", request).await;
        let message = error_message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: got {status} with {body}"
        );
        assert!(
            message.contains(expected),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}
