// SPDX-License-Identifier: CC0-1.0

//! `ListHarnessVersions` over HTTP: the versions one stored harness holds.
//!
//! One rule is under test, read off the Smithy model rather than off any one recorded
//! request: a listing of versions reports EXACTLY the versions the addressed harness
//! persists — one `HarnessVersionSummary` per version, every member read off that stored
//! harness, in ascending version order and paged by the caller's `maxResults` — while a
//! `harnessId` no stored harness holds is the operation's modelled
//! `ResourceNotFoundException` rather than an empty page, and a `nextToken` this service
//! never issued is its modelled `ValidationException`.
//!
//! The suites below take that rule apart: what a page carries (rows spanning the harness's
//! name, its configuration and the `maxResults` the caller asks for, including both
//! modelled bounds), the status the clock owes a listed version, which harness a listing
//! reads, and what it refuses.
//!
//! Sources: `smithy_query(query="operation",operation="ListHarnessVersions,CreateHarness,\
//! GetHarness,DeleteHarness")` for the route, the status, the members, the `maxResults`
//! range and the errors, and `smithy_query(query="shape",shape="HarnessVersionSummary,\
//! HarnessVersion,HarnessStatus,HarnessName")` for the summary's members, the version
//! grammar and the status values asserted below.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/HarnessRole";

/// The settle span `src/state.rs` gives the harness `CREATING` -> `READY` transition, plus
/// slack, so a row can wait the clock out and list a settled version too.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A `harnessId` inside `HarnessId`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$` that no row ever creates.
const ABSENT_HARNESS_ID: &str = "noSuchHarness-abcdefghij";

/// The `HarnessVersion` a created harness carries: the grammar `^([1-9][0-9]{0,4})$` starts
/// at 1, and only an update publishes another one.
const FIRST_VERSION: &str = "1";

/// Every `HarnessStatus` value (`smithy_query shape=HarnessStatus`).
const HARNESS_STATUS_VALUES: [&str; 7] = [
    "CREATING",
    "CREATE_FAILED",
    "UPDATING",
    "UPDATE_FAILED",
    "READY",
    "DELETING",
    "DELETE_FAILED",
];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what the
/// handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the `x-amzn-errortype` header and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Answer {
        status: response.status().as_u16(),
        error_type,
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The member at `pointer`, or `Value::Null` when the body carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A `CreateHarness` body for `name`: the two `@required` members, the `model` this service
/// has no default for, and whatever else the row configures.
fn harness_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "harnessName": name,
        "executionRoleArn": ROLE_ARN,
        "model": {"bedrockModelConfig": {"modelId": "amazon.nova-pro-v1:0"}},
    });
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            body[key] = value;
        }
    }
    body
}

/// Create one harness and return (harnessId, the created `harness` member).
async fn create(router: &MultiServiceRouter, name: &str, extra: Value) -> (String, Value) {
    let answer = send(router, "POST", "/harnesses", Some(&harness_body(name, extra))).await;
    assert_eq!(
        answer.status, 201,
        "creating {name:?} must succeed for this row to say anything, got {} with {}",
        answer.status, answer.body
    );
    let harness = at(&answer.body, "/harness");
    let id = at(&harness, "/harnessId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateHarness reports the minted harnessId, got {harness}"))
        .to_string();
    (id, harness)
}

/// `GET /harnesses/{harnessId}/versions{query}` — the route the model binds
/// `ListHarnessVersions` at.
async fn list_versions(router: &MultiServiceRouter, harness_id: &str, query: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/harnesses/{harness_id}/versions{query}"),
        None,
    )
    .await
}

/// The `harnessVersions` member of a page, as a slice.
fn versions(body: &Value) -> Vec<Value> {
    at(body, "/harnessVersions")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A page carries one summary per version the stored harness holds, every member read off
/// that harness.
///
/// The rows differ along the harness's name (the shortest the pattern admits, underscores
/// and digits, the 40-character bound), the configuration the create carried, and the
/// `maxResults` the listing asks for — absent, the modelled minimum 1, the modelled maximum
/// 100 and a value in between — because none of those changes how many versions a created
/// harness holds.
#[tokio::test]
async fn a_page_reports_one_summary_per_version_the_harness_holds() {
    let router = router();

    // (case, harnessName, the members the create adds, the query the listing carries)
    let rows: Vec<(&str, String, Value, &str)> = vec![
        (
            "the required members only, no maxResults",
            "versionsMinimal".to_string(),
            json!({}),
            "",
        ),
        (
            "a tool and a system prompt, maxResults at the modelled minimum",
            "versionsConfigured".to_string(),
            json!({
                "systemPrompt": [{"text": "You are a careful assistant."}],
                "tools": [{"type": "agentcore_code_interpreter", "name": "python"}],
            }),
            "?maxResults=1",
        ),
        (
            "a single-letter name, maxResults between the bounds",
            "h".to_string(),
            json!({}),
            "?maxResults=7",
        ),
        (
            "letters, digits and underscores, maxResults at the modelled maximum",
            "versions_Probe_42".to_string(),
            json!({"maxIterations": 4, "maxTokens": 2048, "timeoutSeconds": 60}),
            "?maxResults=100",
        ),
        (
            "a 40-character name, the stem's own bound",
            format!("h{}", "a".repeat(39)),
            json!({"memory": {"disabled": {}}}),
            "",
        ),
    ];

    for (case, name, extra, query) in rows {
        let (id, created) = create(&router, &name, extra).await;
        let answer = list_versions(&router, &id, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: listing the versions of a stored harness is HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        let listed = versions(&answer.body);
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the harness holds the one version its create published, so the page \
             carries exactly one summary, got {}",
            answer.body
        );
        let summary = &listed[0];
        // Every identity member is the stored harness's own, as `CreateHarness` reported it.
        for member in ["harnessId", "harnessName", "arn", "createdAt", "updatedAt"] {
            assert_eq!(
                at(summary, &format!("/{member}")),
                at(&created, &format!("/{member}")),
                "case {case:?}: the summary's {member} is the stored harness's, got {summary}"
            );
        }
        assert_eq!(
            at(summary, "/harnessVersion"),
            json!(FIRST_VERSION),
            "case {case:?}: a created harness holds version {FIRST_VERSION}, got {summary}"
        );
        let status = at(summary, "/status").as_str().unwrap_or_default().to_string();
        assert!(
            HARNESS_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: the summary's status is a HarnessStatus value, got {status:?}"
        );
        assert!(
            at(summary, "/failureReason").is_null(),
            "case {case:?}: a version that did not fail reports no failureReason, got {summary}"
        );
        assert!(
            at(&answer.body, "/nextToken").is_null(),
            "case {case:?}: a page that holds every version carries no nextToken, got {}",
            answer.body
        );
    }
}

/// A listed version reports the status the clock owes its harness: the transient one until
/// the settle span passes, the settled one afterwards — and the same status `GetHarness`
/// reports for that harness, because both read one record.
#[tokio::test]
async fn a_listed_version_reports_the_status_the_clock_owes() {
    let router = router();
    let (id, _) = create(&router, "versionsClock", json!({})).await;

    let fresh = list_versions(&router, &id, "").await;
    assert_eq!(
        at(&fresh.body, "/harnessVersions/0/status"),
        json!("CREATING"),
        "case \"straight after the create\": a caller acting inside the settle span sees the \
         transient status, got {}",
        fresh.body
    );

    // Stored, never slept on by the handler: the transition is due at an instant, so waiting
    // the span out is what makes it observable.
    tokio::time::sleep(PAST_SETTLE).await;

    let settled = list_versions(&router, &id, "").await;
    assert_eq!(
        at(&settled.body, "/harnessVersions/0/status"),
        json!("READY"),
        "case \"past the settle span\": the CREATING -> READY transition is due, got {}",
        settled.body
    );
    let read = send(&router, "GET", &format!("/harnesses/{id}"), None).await;
    assert_eq!(
        at(&settled.body, "/harnessVersions/0/status"),
        at(&read.body, "/harness/status"),
        "case \"the listing and the read\": both report the status of one stored harness, got {} \
         and {}",
        settled.body,
        read.body
    );
    // The listing is a read: it moved nothing, so the status is still the one the clock owes.
    let again = list_versions(&router, &id, "").await;
    assert_eq!(
        at(&again.body, "/harnessVersions/0/status"),
        json!("READY"),
        "case \"a second listing\": a read changes nothing a later caller sees, got {}",
        again.body
    );
}

/// One harness's listing reports that harness's versions and no other's.
#[tokio::test]
async fn a_listing_reports_only_the_named_harnesss_versions() {
    let router = router();
    let (first, _) = create(&router, "versionsFirst", json!({})).await;
    let (second, _) = create(&router, "versionsSecond", json!({})).await;
    let (third, _) = create(&router, "versionsThird", json!({})).await;

    // (case, the harness addressed, the ids no other harness may appear under)
    let rows: Vec<(&str, &String, [&String; 2])> = vec![
        ("the first harness", &first, [&second, &third]),
        ("the second harness", &second, [&first, &third]),
        ("the third harness", &third, [&first, &second]),
    ];

    for (case, addressed, others) in rows {
        let answer = list_versions(&router, addressed, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        let ids: Vec<String> = versions(&answer.body)
            .iter()
            .map(|summary| {
                at(summary, "/harnessId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            vec![addressed.to_string()],
            "case {case:?}: a listing reports the versions of the harness the path names, got {}",
            answer.body
        );
        for other in others {
            assert!(
                !ids.contains(other),
                "case {case:?}: {other} is another harness, so its versions are not on this \
                 page, got {}",
                answer.body
            );
        }
    }
}

/// What the listing refuses, and what it serves instead of refusing.
///
/// The rows span the shape of the offending input: a `harnessId` inside its `@pattern` that
/// names no stored harness, a harness that was deleted, a `maxResults` one past each
/// modelled bound, a `nextToken` outside the codec this service issues, and a well-formed
/// token that resumes past every stored version — which is an empty final page, not an
/// error.
#[tokio::test]
async fn a_version_listing_refuses_what_it_cannot_serve() {
    let router = router();
    let (stored, _) = create(&router, "versionsRefusals", json!({})).await;
    let (deleted, _) = create(&router, "versionsDeleted", json!({})).await;
    let removed = send(&router, "DELETE", &format!("/harnesses/{deleted}"), None).await;
    assert_eq!(
        removed.status, 200,
        "the delete must succeed for the deleted-harness row to say anything, got {} with {}",
        removed.status, removed.body
    );

    // (case, the harnessId addressed, the query, the status owed, the errortype owed)
    let rows: Vec<(&str, &str, &str, u16, &str)> = vec![
        (
            "a harnessId inside its pattern that names no stored harness",
            ABSENT_HARNESS_ID,
            "",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a harness that was deleted",
            deleted.as_str(),
            "",
            404,
            "ResourceNotFoundException",
        ),
        (
            "maxResults one below the modelled range 1..100",
            stored.as_str(),
            "?maxResults=0",
            400,
            "ValidationException",
        ),
        (
            "maxResults one past the modelled range 1..100",
            stored.as_str(),
            "?maxResults=101",
            400,
            "ValidationException",
        ),
        (
            "a nextToken this service never issued",
            stored.as_str(),
            "?nextToken=not-a-token-this-service-issued",
            400,
            "ValidationException",
        ),
        (
            // Hex, the codec `src/state.rs` issues tokens in, for a key that sorts past
            // every version key: a well-formed token resuming after the last version.
            "a nextToken that resumes past every stored version",
            stored.as_str(),
            "?nextToken=7a7a7a7a7a",
            200,
            "",
        ),
    ];

    for (case, harness_id, query, status, error_type) in rows {
        let answer = list_versions(&router, harness_id, query).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (status, error_type),
            "case {case:?}: expected HTTP {status} {error_type:?}, got {} {:?} with {}",
            answer.status,
            answer.error_type,
            answer.body
        );
        if error_type == "ResourceNotFoundException" {
            assert_eq!(
                at(&answer.body, "/message"),
                json!(format!("Agent with name {harness_id} not found.")),
                "case {case:?}: the refusal names the identifier the request carried, got {}",
                answer.body
            );
        }
        if error_type.is_empty() {
            assert!(
                versions(&answer.body).is_empty()
                    && at(&answer.body, "/nextToken").is_null(),
                "case {case:?}: a page resuming past every version is empty and ends the walk, \
                 got {}",
                answer.body
            );
        }
    }

    // The harness the rows above page over still lists its version, so each refusal is about
    // the input the row changed rather than about the harness.
    let served = list_versions(&router, &stored, "?maxResults=1").await;
    assert_eq!(
        (served.status, versions(&served.body).len()),
        (200, 1),
        "case \"the bounds themselves\": maxResults=1 is inside the modelled range, got {} with {}",
        served.status,
        served.body
    );
}
