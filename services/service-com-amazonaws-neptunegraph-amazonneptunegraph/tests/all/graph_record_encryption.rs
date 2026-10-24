// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two members a graph record reports about the GRAPH rather
//! than about the request that made it: the key its data is encrypted with and the
//! build of the software serving it.
//!
//! The rules under test, asserted over a SET of inputs rather than one request:
//!
//! * every graph of this service is encrypted, so a graph record always names a key —
//!   the customer managed key the create request supplied, or, when it supplied none,
//!   the sentinel that names the service-owned key (which has no ARN a caller could
//!   name);
//! * every graph record names the build of the graph software serving the graph, which
//!   is one value for every graph this emulator holds and does not change between reads.
//!
//! The rows differ along what those rules generalise over — whether a key was supplied
//! and the form of the key ARN when one was — and each row is asserted on EVERY
//! projection of the record: `CreateGraph`, `GetGraph`, `UpdateGraph`, `StopGraph`,
//! `StartGraph`, `DeleteGraph` and the `GraphSummary` of `ListGraphs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    GRAPH_SOFTWARE_BUILD_NUMBER, PROVISIONING_SPAN, SERVICE_OWNED_KMS_KEY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// Key ARNs matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`
/// (`smithy_query(query="shape",shape="KmsKeyArn")`). The three differ along the
/// clauses the pattern leaves open: the partition suffix, the Region, and the shape of
/// the 36-character key id.
const KEY_DEFAULT_PARTITION: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";
const KEY_OTHER_PARTITION: &str =
    "arn:aws-us-gov:kms:us-gov-west-1:000000000000:key/87654321-4321-4321-4321-210987654321";
const KEY_ID_WITHOUT_HYPHENS: &str =
    "arn:aws:kms:eu-west-1:000000000000:key/abcdefABCDEF0123456789abcdefABCDEF01";

/// The provisioned memory every graph here is created with: the bottom of the modelled
/// `ProvisionedMemory` range (16..24576), since this suite is not about sizing.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// One row of the table: the key a `CreateGraph` request supplies (or none) and the key
/// every projection of the resulting record must report.
struct KeyCase {
    case: &'static str,
    supplied: Option<&'static str>,
    expected: &'static str,
}

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GET /graphs/{id}`.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// The `CreateGraph` body for a row, naming the graph after it.
fn create_body(name: &str, supplied: Option<&str>) -> Value {
    let mut body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    if let Some(key) = supplied {
        body["kmsKeyIdentifier"] = json!(key);
    }
    body
}

/// Create a graph and return its id and the `CreateGraph` response.
async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> (String, Value) {
    let (status, created) = call(router, "POST", "/graphs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    (id, created)
}

/// Poll `GetGraph` until the graph reports `expected`, then return its record.
///
/// No handler ever sleeps; only this test waits, because it is asking about a status
/// the clock reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = get(router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = get(router, id).await;
    panic!(
        "case {case:?}: graph never reported {expected}, last status {}",
        body["status"]
    );
}

/// The `GraphSummary` `ListGraphs` reports for `id`.
async fn summary_of(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, listed) = call(router, "GET", "/graphs?maxResults=100", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListGraphs, got {status}: {listed}"
    );
    listed["graphs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListGraphs reports a list, got {listed}"))
        .iter()
        .find(|summary| summary["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {id} in {listed}"))
}

/// Assert one projection of a graph record reports the row's key and this emulator's
/// software build.
fn assert_key_and_build(case: &KeyCase, step: &str, record: &Value) {
    assert_eq!(
        record["kmsKeyIdentifier"],
        json!(case.expected),
        "case {:?} ({step}): expected kmsKeyIdentifier {:?}, got {}",
        case.case,
        case.expected,
        record["kmsKeyIdentifier"]
    );
    assert_eq!(
        record["buildNumber"],
        json!(GRAPH_SOFTWARE_BUILD_NUMBER),
        "case {:?} ({step}): expected buildNumber {GRAPH_SOFTWARE_BUILD_NUMBER:?}, got {}",
        case.case,
        record["buildNumber"]
    );
    assert!(
        record["buildNumber"].as_str().is_some_and(|build| !build.is_empty()),
        "case {:?} ({step}): expected a non-empty buildNumber, got {}",
        case.case,
        record["buildNumber"]
    );
}

#[tokio::test]
async fn every_graph_record_names_its_encryption_key_and_software_build() {
    let router = emulator();

    let cases = [
        KeyCase {
            case: "no key supplied: the service-owned key the sentinel names",
            supplied: None,
            expected: SERVICE_OWNED_KMS_KEY,
        },
        KeyCase {
            case: "a customer managed key in the default partition",
            supplied: Some(KEY_DEFAULT_PARTITION),
            expected: KEY_DEFAULT_PARTITION,
        },
        KeyCase {
            case: "a customer managed key in another partition and Region",
            supplied: Some(KEY_OTHER_PARTITION),
            expected: KEY_OTHER_PARTITION,
        },
        KeyCase {
            case: "a customer managed key whose id carries no hyphens",
            supplied: Some(KEY_ID_WITHOUT_HYPHENS),
            expected: KEY_ID_WITHOUT_HYPHENS,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("key-case-{index}");
        let (id, created) = create(
            &router,
            case.case,
            create_body(&name, case.supplied),
        )
        .await;
        assert_key_and_build(case, "CreateGraph", &created);

        // The read reports the record that was stored, not the response that was
        // rendered: the same two members, before the graph is even in service.
        let (status, fetched) = get(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetGraph, got {status}: {fetched}",
            case.case
        );
        assert_key_and_build(case, "GetGraph", &fetched);

        // The listing projection reports the key too. `GraphSummary` models no
        // `buildNumber` (`smithy_query(query="shape",shape="GraphSummary")`), so the
        // summary must not carry one.
        let summary = summary_of(&router, case.case, &id).await;
        assert_eq!(
            summary["kmsKeyIdentifier"],
            json!(case.expected),
            "case {:?} (ListGraphs): expected kmsKeyIdentifier {:?}, got {}",
            case.case,
            case.expected,
            summary["kmsKeyIdentifier"]
        );
        assert!(
            summary.get("buildNumber").is_none_or(Value::is_null),
            "case {:?} (ListGraphs): GraphSummary models no buildNumber, got {}",
            case.case,
            summary["buildNumber"]
        );

        wait_for_status(&router, case.case, &id, "AVAILABLE").await;

        // A reconfiguration reports the record it left behind, key and build included.
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/graphs/{id}"),
            Some(json!({"provisionedMemory": 32})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateGraph, got {status}: {updated}",
            case.case
        );
        assert_key_and_build(case, "UpdateGraph", &updated);
        wait_for_status(&router, case.case, &id, "AVAILABLE").await;

        // A run-state change does not touch either member: a stopped graph is still an
        // encrypted graph, and the software serving it is the same build.
        let (status, stopped) =
            call(&router, "POST", &format!("/graphs/{id}/stop"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from StopGraph, got {status}: {stopped}",
            case.case
        );
        assert_key_and_build(case, "StopGraph", &stopped);
        wait_for_status(&router, case.case, &id, "STOPPED").await;

        let (status, started) =
            call(&router, "POST", &format!("/graphs/{id}/start"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from StartGraph, got {status}: {started}",
            case.case
        );
        assert_key_and_build(case, "StartGraph", &started);
        wait_for_status(&router, case.case, &id, "AVAILABLE").await;

        // And the record the deletion reports, while it is on its way out.
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/graphs/{id}?skipSnapshot=true"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteGraph, got {status}: {deleted}",
            case.case
        );
        assert_key_and_build(case, "DeleteGraph", &deleted);
    }
}

#[tokio::test]
async fn the_software_build_is_one_value_for_every_graph_and_stable_across_reads() {
    let router = emulator();

    // (case, the CreateGraph body) — the rows differ in the configuration of the graph,
    // which the build of the software serving it does not depend on.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the smallest graph, every option defaulted",
            json!({"graphName": "build-minimal", "provisionedMemory": 16}),
        ),
        (
            "the largest provisionedMemory, no replicas, a public endpoint",
            json!({
                "graphName": "build-large",
                "provisionedMemory": 24576,
                "replicaCount": 0,
                "publicConnectivity": true,
            }),
        ),
        (
            "an encrypted graph with a vector index and the top of the replica range",
            json!({
                "graphName": "build-encrypted",
                "provisionedMemory": 128,
                "replicaCount": 2,
                "kmsKeyIdentifier": KEY_DEFAULT_PARTITION,
                "vectorSearchConfiguration": {"dimension": 1536},
            }),
        ),
    ];

    let mut builds: Vec<(String, String)> = Vec::new();
    for (case, body) in cases {
        let (id, created) = create(&router, case, body).await;
        let build = created["buildNumber"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no buildNumber in {created}"))
            .to_string();
        assert!(
            !build.is_empty(),
            "case {case:?}: expected a non-empty buildNumber, got {build:?}"
        );

        // Reading the graph twice reports the same build: it describes the software,
        // not the read.
        for read in 0..2 {
            let (status, fetched) = get(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?} (read {read}): expected 200 from GetGraph, got {status}: {fetched}"
            );
            assert_eq!(
                fetched["buildNumber"],
                json!(build.clone()),
                "case {case:?} (read {read}): expected buildNumber {build:?}, got {}",
                fetched["buildNumber"]
            );
        }
        builds.push((case.to_string(), build));
    }

    let first = builds
        .first()
        .cloned()
        .expect("the table holds at least one row");
    for (case, build) in &builds {
        assert_eq!(
            build, &first.1,
            "case {case:?}: every graph is served by the same software build, \
             expected {:?}, got {build:?}",
            first.1
        );
    }
}
