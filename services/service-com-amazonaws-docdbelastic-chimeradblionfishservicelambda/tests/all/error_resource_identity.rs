// SPDX-License-Identifier: CC0-1.0

//! How this service reports the identity of a resource an error is about.
//!
//! The rule these suites assert: on an error shape that models `resourceId` and
//! `resourceType` (`ResourceNotFoundException`, `ConflictException`), `resourceType`
//! carries the AWS RESOURCE TYPE NAME of the kind of resource the request addressed —
//! the capitalised name this service's API gives that resource class, `Cluster` or
//! `Snapshot` — and `resourceId` carries the identifier the request named. The
//! lowercase, hyphenated token (`cluster`, `cluster-snapshot`) is the ARN's resource
//! segment and this service's internal key for the kind, and the operation's own name
//! (`ClusterSnapshot`) is neither; none of them is what `resourceType` reports.
//!
//! The rule generalises over the kind of resource and over the identifier's form, so
//! the tables below span every not-found and conflict path of the implemented
//! lifecycle and, for each, identifiers that differ along form, region, which kind of
//! ARN segment they spell, and whether they are an ARN at all: the type name follows
//! the resource the OPERATION addresses, never the text of the identifier.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The AWS resource type NAMES this service reports in `resourceType`.
///
/// Source: the resource class each operation reads and writes one of, as its own
/// members name it — `GetCluster` answers `cluster` and is addressed by `clusterArn`,
/// `GetClusterSnapshot` answers `snapshot` and is addressed by `snapshotArn`
/// (`smithy_query(query="operation", operation="GetCluster,GetClusterSnapshot")`). The
/// `resourceType` members of `ResourceNotFoundException` ("The type of the resource
/// that could not be found") and `ConflictException` ("The type of the resource where
/// there was an access conflict") are both `@required` Strings.
const CLUSTER_TYPE: &str = "Cluster";
const SNAPSHOT_TYPE: &str = "Snapshot";

/// The `@required` members of both error shapes, which every error body must carry.
///
/// Source: `smithy_query(query="shape",
/// shape="ResourceNotFoundException,ConflictException")`.
const REQUIRED_ERROR_MEMBERS: &[&str] = &["message", "resourceId", "resourceType"];

/// A SigV4 `Authorization` header for this service in `region`.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does: a label is a single
/// path segment and an ARN carries both `:` and `/`, so neither may survive raw.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when the
/// body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization(REGION))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// Assert the three `@required` members of an error shape are present, then return the
/// reported `resourceType` / `resourceId` pair.
fn resource_identity(case: &str, body: &Value) -> (String, String) {
    for member in REQUIRED_ERROR_MEMBERS {
        assert!(
            body.get(member)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: required error member `{member}` missing or empty in {body}"
        );
    }
    (
        body["resourceType"].as_str().unwrap_or_default().to_string(),
        body["resourceId"].as_str().unwrap_or_default().to_string(),
    )
}

/// One identifier an unknown-resource lookup can name, labelled by what makes it
/// differ from the others.
struct Identifier {
    case: &'static str,
    value: String,
}

/// Identifiers that differ along form, region, which kind of ARN segment they spell,
/// and whether they are an ARN at all.
///
/// `segment` is the ARN resource segment of the kind the row is built for, so each
/// table also carries a row spelling the OTHER kind's segment: the reported type name
/// must follow the resource the operation addresses, not the identifier's text.
fn identifiers(segment: &str, other_segment: &str) -> Vec<Identifier> {
    vec![
        Identifier {
            case: "an ARN of this kind in the signing region",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{segment}/11111111-2222-3333-4444-555555555555"
            ),
        },
        Identifier {
            case: "an ARN of this kind in another region",
            value: format!(
                "arn:aws:docdb-elastic:us-west-2:{ACCOUNT}:{segment}/00000000-0000-0000-0000-000000000000"
            ),
        },
        Identifier {
            case: "an ARN of this kind in another account",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:210987654321:{segment}/ABCDEF01-2345-6789-abcd-ef0123456789"
            ),
        },
        Identifier {
            case: "an ARN spelling the other kind's resource segment",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{other_segment}/99999999-8888-7777-6666-555555555555"
            ),
        },
        Identifier {
            case: "an identifier that is not an ARN at all",
            value: "not-an-arn-at-all".to_string(),
        },
    ]
}

/// One not-found path: the request to make for a given identifier, and the resource
/// type name the error must report.
struct Lookup {
    operation: &'static str,
    method: &'static str,
    /// The path the identifier is addressed through, as a `@httpLabel` route.
    path: fn(&str) -> String,
    /// The request body, built from the identifier when the identifier travels in it.
    body: fn(&str) -> Option<Value>,
    expected_type: &'static str,
    /// The ARN resource segment of the kind looked up, and of the other kind.
    segments: (&'static str, &'static str),
}

/// Every lookup of the implemented lifecycle that can answer "no such resource",
/// across both resource kinds.
fn lookups() -> Vec<Lookup> {
    vec![
        Lookup {
            operation: "GetCluster",
            method: "GET",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| None,
            expected_type: CLUSTER_TYPE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "UpdateCluster",
            method: "PUT",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| Some(json!({"shardCount": 2})),
            expected_type: CLUSTER_TYPE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "DeleteCluster",
            method: "DELETE",
            path: |arn| format!("/cluster/{}", label(arn)),
            body: |_| None,
            expected_type: CLUSTER_TYPE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            // The missing resource travels in the BODY here, not in a label, and it is
            // the cluster being snapshotted — so the error is about a `Cluster` even
            // though the route is the snapshot collection.
            operation: "CreateClusterSnapshot",
            method: "POST",
            path: |_| "/cluster-snapshot".to_string(),
            body: |arn| Some(json!({"clusterArn": arn, "snapshotName": "probe-snapshot"})),
            expected_type: CLUSTER_TYPE,
            segments: ("cluster", "cluster-snapshot"),
        },
        Lookup {
            operation: "GetClusterSnapshot",
            method: "GET",
            path: |arn| format!("/cluster-snapshot/{}", label(arn)),
            body: |_| None,
            expected_type: SNAPSHOT_TYPE,
            segments: ("cluster-snapshot", "cluster"),
        },
        Lookup {
            operation: "DeleteClusterSnapshot",
            method: "DELETE",
            path: |arn| format!("/cluster-snapshot/{}", label(arn)),
            body: |_| None,
            expected_type: SNAPSHOT_TYPE,
            segments: ("cluster-snapshot", "cluster"),
        },
    ]
}

#[tokio::test]
async fn a_not_found_error_reports_the_aws_resource_type_name() {
    let router = emulator();
    // A cluster and a snapshot DO exist, so the suite proves the type name is reported
    // for a resource that is genuinely absent from a non-empty store.
    let (status, created) = call(&router, "POST", "/cluster", Some(minimal_create("present"))).await;
    assert_eq!(
        status, 200,
        "the fixture cluster must be created, got {status} with {created}"
    );

    for lookup in lookups() {
        let (segment, other_segment) = lookup.segments;
        for identifier in identifiers(segment, other_segment) {
            let case = format!("{} / {}", lookup.operation, identifier.case);
            let path = (lookup.path)(&identifier.value);
            let body = (lookup.body)(&identifier.value);
            let (status, response) = call(&router, lookup.method, &path, body).await;

            assert_eq!(
                status, 404,
                "case {case:?}: an absent resource is HTTP 404, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {response}"
            );

            let (resource_type, resource_id) = resource_identity(&case, &response);
            assert_eq!(
                resource_type, lookup.expected_type,
                "case {case:?}: resourceType must be the AWS resource type name \
                 {expected:?} of the resource this operation addresses, got {resource_type:?}",
                expected = lookup.expected_type
            );
            assert_eq!(
                resource_id, identifier.value,
                "case {case:?}: resourceId must be the identifier the request named, \
                 got {resource_id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_conflict_error_reports_the_aws_resource_type_name() {
    let router = emulator();

    // A second cluster of an existing name collides with that CLUSTER.
    let (status, created) = call(&router, "POST", "/cluster", Some(minimal_create("twinned"))).await;
    assert_eq!(
        status, 200,
        "the first cluster must be created, got {status} with {created}"
    );
    let cluster_arn = created["cluster"]["clusterArn"]
        .as_str()
        .expect("the created cluster carries its ARN")
        .to_string();

    let snapshot_body = json!({"clusterArn": cluster_arn, "snapshotName": "twinned-snapshot"});
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(snapshot_body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first snapshot must be created, got {status} with {snapshot}"
    );
    let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
        .as_str()
        .expect("the created snapshot carries its ARN")
        .to_string();

    // The rows differ in the KIND of resource the request collides with, and so in the
    // type name the conflict must report.
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a duplicate cluster name",
            "/cluster",
            minimal_create("twinned"),
            CLUSTER_TYPE,
            cluster_arn.as_str(),
        ),
        (
            "a duplicate cluster name in another case",
            "/cluster",
            minimal_create("TWINNED"),
            CLUSTER_TYPE,
            cluster_arn.as_str(),
        ),
        (
            "a duplicate snapshot name",
            "/cluster-snapshot",
            snapshot_body.clone(),
            SNAPSHOT_TYPE,
            snapshot_arn.as_str(),
        ),
    ];

    for (case, path, body, expected_type, expected_id) in cases {
        let (status, response) = call(&router, "POST", path, Some(body)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a colliding create is HTTP 409, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {response}"
        );

        let (resource_type, resource_id) = resource_identity(case, &response);
        assert_eq!(
            resource_type, expected_type,
            "case {case:?}: resourceType must be the AWS resource type name \
             {expected_type:?}, got {resource_type:?}"
        );
        assert_eq!(
            resource_id, expected_id,
            "case {case:?}: resourceId must name the resource already holding the name, \
             got {resource_id:?}"
        );
    }
}

#[tokio::test]
async fn a_reported_resource_type_is_never_the_arn_segment() {
    // The regression this pins: the ARN resource segment and this service's internal
    // key for a resource kind are lowercase and hyphenated, and the operation's own
    // name is `ClusterSnapshot`; none of them is a resource type name. Asserted for
    // both kinds over every spelling an ARN segment and an operation name have.
    let router = emulator();
    let rows = [
        (
            "Cluster",
            CLUSTER_TYPE,
            format!("/cluster/{}", label("arn:aws:docdb-elastic:eu-west-2:000000000000:cluster/absent")),
            ["cluster", "clusters", "Clusters", "GetCluster"],
        ),
        (
            "ClusterSnapshot",
            SNAPSHOT_TYPE,
            format!(
                "/cluster-snapshot/{}",
                label("arn:aws:docdb-elastic:eu-west-2:000000000000:cluster-snapshot/absent")
            ),
            [
                "cluster-snapshot",
                "clusterSnapshot",
                "cluster_snapshot",
                "ClusterSnapshot",
            ],
        ),
    ];

    for (case, expected_type, path, rejected) in rows {
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is HTTP 404, got {status} with {response}"
        );
        let (resource_type, _) = resource_identity(case, &response);
        assert_eq!(
            resource_type, expected_type,
            "case {case:?}: expected the resource type name {expected_type:?}, got \
             {resource_type:?}"
        );
        for token in rejected {
            assert_ne!(
                resource_type, token,
                "case {case:?}: resourceType must not be the ARN segment or an internal \
                 key spelling {token:?}, got {resource_type:?}"
            );
        }
    }
}
