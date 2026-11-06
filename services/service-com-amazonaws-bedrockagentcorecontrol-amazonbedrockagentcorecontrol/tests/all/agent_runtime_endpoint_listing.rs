// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAgentRuntimeEndpoints`.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised, against the one shared
//! engine — so what `CreateAgentRuntimeEndpoint` writes is what the listing reads.
//!
//! The rule under test: a listing of the endpoints of ONE agent runtime reports every
//! endpoint persisted on that runtime, each summarised from that endpoint's own record
//! — so the listing cannot disagree with `GetAgentRuntimeEndpoint` — in one
//! deterministic order, cut into pages of at most `maxResults` entries that resume
//! exactly where the previous page ended; and because the operation models no
//! not-found error, an `agentRuntimeId` inside the modelled grammar that names no
//! runtime of this account is answered the modelled success with an empty page, the
//! modelled `ValidationException` being reserved for input that fails a modelled
//! constraint.
//!
//! Sources: `smithy_query(query="operation",operation="ListAgentRuntimeEndpoints,\
//! CreateAgentRuntimeEndpoint,GetAgentRuntimeEndpoint")` for the routes, the success
//! statuses, the `maxResults` range and the modelled errors, and
//! `smithy_query(query="shape",shape="AgentRuntimeEndpoint,EndpointName,\
//! AgentRuntimeEndpointStatus")` for the members, the name grammar and the status
//! values asserted below.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRole";

/// The `AgentRuntimeVersion` a created runtime carries: the grammar `^([1-9][0-9]{0,4})$`
/// starts at 1, and only an update publishes another.
const FIRST_VERSION: &str = "1";

/// The modelled bounds of `maxResults` (`MaxResults`, range 1..100, from
/// `smithy_query operation=ListAgentRuntimeEndpoints`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The settle spans `src/state.rs` gives the runtime `CREATING` -> `READY` transition
/// (`RUNTIME_SETTLE_DELAY`, 500 ms) and then the endpoint's own
/// (`AGENT_RUNTIME_ENDPOINT_SETTLE_DELAY`, 3 s), plus a second of slack: an endpoint
/// fronts a runtime version, so it is ready one span after the version it serves is.
const PAST_ENDPOINT_SETTLE: Duration = Duration::from_millis(4_500);

/// The members an `AgentRuntimeEndpoint` list entry and a `GetAgentRuntimeEndpoint`
/// answer both carry (`smithy_query shape=AgentRuntimeEndpoint` against that operation's
/// output): what the listing must report exactly as the single read does.
const SHARED_MEMBERS: [&str; 10] = [
    "name",
    "id",
    "agentRuntimeEndpointArn",
    "agentRuntimeArn",
    "description",
    "status",
    "targetVersion",
    "liveVersion",
    "createdAt",
    "lastUpdatedAt",
];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what
/// the handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status and the parsed body.
struct Answer {
    status: u16,
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
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The member at `pointer`, or `Value::Null` when the answer carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

fn text(body: &Value, pointer: &str) -> String {
    at(body, pointer).as_str().unwrap_or_default().to_string()
}

/// The error code `restJson1` puts in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A container artifact whose URI satisfies `RuntimeContainerUri`'s ECR pattern.
fn container_artifact(tag: &str) -> Value {
    json!({"containerConfiguration": {
        "containerUri": format!("public.ecr.aws/agentcore/demo:{tag}")
    }})
}

/// Create one agent runtime and return its `agentRuntimeId`.
async fn create_runtime(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": container_artifact("v1"),
        "roleArn": ROLE_ARN,
    });
    let answer = send(router, "PUT", "/runtimes/", Some(&body)).await;
    assert_eq!(
        answer.status, 202,
        "creating the runtime {name:?} must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
    text(&answer.body, "/agentRuntimeId")
}

/// Publish one more version of `runtime_id` and return the version it published.
async fn update_runtime(router: &MultiServiceRouter, runtime_id: &str, tag: &str) -> String {
    let body = json!({
        "agentRuntimeArtifact": container_artifact(tag),
        "roleArn": ROLE_ARN,
    });
    let answer = send(router, "PUT", &format!("/runtimes/{runtime_id}/"), Some(&body)).await;
    assert_eq!(
        answer.status, 202,
        "publishing another version must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
    text(&answer.body, "/agentRuntimeVersion")
}

/// `PUT /runtimes/{agentRuntimeId}/runtime-endpoints/` — the route the model binds
/// `CreateAgentRuntimeEndpoint` at.
async fn create_endpoint(router: &MultiServiceRouter, runtime_id: &str, body: &Value) -> Answer {
    send(
        router,
        "PUT",
        &format!("/runtimes/{runtime_id}/runtime-endpoints/"),
        Some(body),
    )
    .await
}

/// Create one endpoint and assert the create itself succeeded, so a later assertion is
/// about the LISTING rather than about the create.
async fn endpoint(router: &MultiServiceRouter, runtime_id: &str, body: &Value) {
    let answer = create_endpoint(router, runtime_id, body).await;
    assert_eq!(
        answer.status, 202,
        "creating the endpoint {body} must succeed for this suite to say anything, got {} with {}",
        answer.status, answer.body
    );
}

/// `GET /runtimes/{agentRuntimeId}/runtime-endpoints/{endpointName}/` — the route the
/// model binds `GetAgentRuntimeEndpoint` at.
async fn get_endpoint(router: &MultiServiceRouter, runtime_id: &str, name: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/runtimes/{runtime_id}/runtime-endpoints/{name}/"),
        None,
    )
    .await
}

/// `POST /runtimes/{agentRuntimeId}/runtime-endpoints/` — the route the model binds
/// `ListAgentRuntimeEndpoints` at, with its two query members.
async fn list_endpoints(
    router: &MultiServiceRouter,
    runtime_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        format!("/runtimes/{runtime_id}/runtime-endpoints/")
    } else {
        format!(
            "/runtimes/{runtime_id}/runtime-endpoints/?{}",
            query.join("&")
        )
    };
    send(router, "POST", &uri, None).await
}

/// The `runtimeEndpoints` of a listing answer.
fn reported(body: &Value) -> Vec<Value> {
    body["runtimeEndpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `name` of every entry of a listing answer, in the order it reported them.
fn reported_names(body: &Value) -> Vec<String> {
    reported(body)
        .iter()
        .map(|entry| text(entry, "/name"))
        .collect()
}

/// One row of the listing table: the endpoints created on a runtime of this row's own,
/// and the bodies they were created with.
struct Listed {
    case: &'static str,
    runtime: &'static str,
    endpoints: Vec<Value>,
}

/// A listing reports every endpoint persisted on the runtime, in ascending name order,
/// and each entry carries exactly what `GetAgentRuntimeEndpoint` reports for it.
///
/// The rows differ along how many endpoints the runtime holds (none, one, several), the
/// order they were created in against the order they are reported in, the shape of the
/// names (the shortest the pattern admits, digits and underscores, the 48-character
/// bound) and whether an endpoint carries a description.
#[tokio::test]
async fn a_listing_reports_every_persisted_endpoint_in_name_order() {
    let router = router();

    // `EndpointName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`: a leading letter plus up to 47
    // more characters. `AgentEndpointDescription` is len 1..256.
    let rows: Vec<Listed> = vec![
        Listed {
            case: "a runtime that holds no endpoint at all",
            runtime: "listing_runtime_empty",
            endpoints: vec![],
        },
        Listed {
            case: "one endpoint, named with the shortest name the pattern admits",
            runtime: "listing_runtime_one",
            endpoints: vec![json!({"name": "e"})],
        },
        Listed {
            case: "three endpoints created in descending name order",
            runtime: "listing_runtime_three",
            endpoints: vec![
                json!({"name": "prod", "description": "the production endpoint"}),
                json!({"name": "canary"}),
                json!({"name": "beta", "description": "d".repeat(256)}),
            ],
        },
        Listed {
            case: "names spanning digits, underscores and the 48-character bound",
            runtime: "listing_runtime_wide",
            endpoints: vec![
                json!({"name": format!("z{}", "a".repeat(47))}),
                json!({"name": "live_v2_endpoint_42", "description": "x"}),
                json!({"name": "A", "agentRuntimeVersion": FIRST_VERSION}),
                json!({"name": "e_1"}),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let runtime_id = create_runtime(&router, row.runtime).await;
        for body in &row.endpoints {
            endpoint(&router, &runtime_id, body).await;
        }

        let answer = list_endpoints(&router, &runtime_id, None, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: listing the endpoints of a stored runtime is HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        let listing = answer.body;

        let mut expected_names: Vec<String> = row
            .endpoints
            .iter()
            .map(|body| text(body, "/name"))
            .collect();
        expected_names.sort();
        assert_eq!(
            reported_names(&listing),
            expected_names,
            "case {case:?}: the listing reports every stored endpoint once, in ascending name \
             order, got {listing}"
        );
        assert_eq!(
            at(&listing, "/nextToken"),
            Value::Null,
            "case {case:?}: one page holds every endpoint here, so there is nothing to resume, \
             got {listing}"
        );

        for entry in reported(&listing) {
            let name = text(&entry, "/name");
            let read = get_endpoint(&router, &runtime_id, &name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the listed endpoint {name:?} is readable on its own, got {} with \
                 {}",
                read.status, read.body
            );
            for member in SHARED_MEMBERS {
                let pointer = format!("/{member}");
                assert_eq!(
                    at(&entry, &pointer),
                    at(&read.body, &pointer),
                    "case {case:?}: the listing reports {member:?} for {name:?} exactly as the \
                     single read does, got {entry} against {}",
                    read.body
                );
            }
        }
    }
}

/// Paging walks every endpoint exactly once, whatever page size the caller asks for.
///
/// The rows differ along the page size: one entry at a time, sizes that do and do not
/// divide the endpoint count, the count itself, one above it, and the modelled maximum.
#[tokio::test]
async fn pages_cover_every_endpoint_exactly_once() {
    let router = router();
    let runtime_id = create_runtime(&router, "listing_runtime_paged").await;

    let names = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
    ];
    for name in names {
        endpoint(&router, &runtime_id, &json!({"name": name})).await;
    }
    let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    expected.sort();

    for page_size in [
        MAX_RESULTS_MIN,
        2,
        3,
        names.len() as i64,
        names.len() as i64 + 1,
        MAX_RESULTS_MAX,
    ] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One page per entry at worst, plus one call to prove the walk ends.
        let mut calls = 0;
        loop {
            let answer = list_endpoints(&router, &runtime_id, Some(page_size), token.as_deref())
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: every page of a stored runtime's endpoints is HTTP 200, got {} \
                 with {}",
                answer.status, answer.body
            );
            let page = reported_names(&answer.body);
            assert!(
                page.len() as i64 <= page_size,
                "case {case:?}: a page holds at most maxResults entries, got {} in {}",
                page.len(),
                answer.body
            );
            walked.extend(page);
            token = at(&answer.body, "/nextToken")
                .as_str()
                .map(ToString::to_string);
            calls += 1;
            assert!(
                calls <= names.len() + 1,
                "case {case:?}: the walk must end within one page per endpoint, got {calls} calls \
                 having walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages together report every endpoint exactly once, in ascending \
             name order, got {walked:?}"
        );
    }
}

/// `maxResults` is modelled `range 1..100`: each bound is accepted and the value just
/// past it is refused.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let runtime_id = create_runtime(&router, "listing_runtime_bounds").await;
    endpoint(&router, &runtime_id, &json!({"name": "only"})).await;

    // The published bounds of `MaxResults` (range 1..100) and one past each.
    let rows: [(i64, u16); 4] = [
        (MAX_RESULTS_MIN, 200),
        (MAX_RESULTS_MAX, 200),
        (MAX_RESULTS_MIN - 1, 400),
        (MAX_RESULTS_MAX + 1, 400),
    ];

    for (max_results, expected_status) in rows {
        let case = format!("maxResults={max_results}");
        let answer = list_endpoints(&router, &runtime_id, Some(max_results), None).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for a maxResults {} the modelled \
             range 1..100, got {} with {}",
            if expected_status == 200 {
                "inside"
            } else {
                "outside"
            },
            answer.status,
            answer.body
        );
        if expected_status == 200 {
            assert_eq!(
                reported_names(&answer.body),
                vec!["only".to_string()],
                "case {case:?}: a page size inside the range reports the stored endpoint, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                error_code(&answer.body),
                "ValidationException",
                "case {case:?}: a maxResults outside the modelled range is the modelled \
                 ValidationException, got {}",
                answer.body
            );
        }
    }
}

/// Resolving the runtime is not an input question: `ListAgentRuntimeEndpoints` models
/// no not-found error (its errors are `AccessDeniedException`,
/// `InternalServerException`, `ThrottlingException` and `ValidationException`) and its
/// `runtimeEndpoints` is required, so an `agentRuntimeId` inside the modelled grammar
/// that names no runtime of this account is answered the modelled success: an empty
/// `runtimeEndpoints` and no `nextToken`.
///
/// The rows differ along how the id fails to resolve (never created at all, created and
/// then deleted) and along its shape — the shortest the pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` admits, underscores and digits, and
/// the 100-character prefix bound — and each is asked for with no page size and with
/// each bound of the modelled `maxResults` range.
#[tokio::test]
async fn a_runtime_this_account_does_not_hold_is_an_empty_page() {
    let router = router();

    // A runtime that IS held, with an endpoint on it: what the empty pages below must
    // not be an artefact of.
    let held = create_runtime(&router, "listing_runtime_held").await;
    endpoint(&router, &held, &json!({"name": "held_endpoint"})).await;

    // One row's id is a runtime this suite creates and then deletes — the cleanup probe
    // a reconciler makes after a runtime is gone.
    let deleted = create_runtime(&router, "listing_runtime_deleted").await;
    let removed = send(&router, "DELETE", &format!("/runtimes/{deleted}/"), None).await;
    assert_eq!(
        removed.status, 202,
        "deleting the runtime must succeed for this suite to say anything, got {} with {}",
        removed.status, removed.body
    );

    // Ids inside `AgentRuntimeId`'s pattern that name no runtime of this account: the
    // shortest the pattern admits, a prefix of digits and underscores, the 100-character
    // prefix bound, and the id of the runtime just deleted.
    let absent: Vec<(String, String)> = vec![
        (
            "the shortest id the pattern admits".to_string(),
            "a-0123456789".to_string(),
        ),
        (
            "a prefix of letters, digits and underscores".to_string(),
            "another_runtime_entirely7-zz99zz99zz".to_string(),
        ),
        (
            "the 100-character prefix bound".to_string(),
            format!("r{}-AB12cd34EF", "x".repeat(99)),
        ),
        (
            "a runtime created and then deleted".to_string(),
            deleted.clone(),
        ),
    ];

    for (shape, runtime_id) in &absent {
        for max_results in [None, Some(MAX_RESULTS_MIN), Some(MAX_RESULTS_MAX)] {
            let case = format!("{shape} ({runtime_id:?}), maxResults={max_results:?}");
            let answer = list_endpoints(&router, runtime_id, max_results, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a runtime that is not there holds no endpoint, which is the \
                 modelled success, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                at(&answer.body, "/runtimeEndpoints"),
                json!([]),
                "case {case:?}: the required runtimeEndpoints is reported as an empty list, got {}",
                answer.body
            );
            assert_eq!(
                at(&answer.body, "/nextToken"),
                Value::Null,
                "case {case:?}: an empty page leaves nothing to resume, got {}",
                answer.body
            );
        }
    }

    // The empty pages above are about those ids only: the runtime that IS held still
    // reports its endpoint.
    let still_held = list_endpoints(&router, &held, None, None).await;
    assert_eq!(
        (still_held.status, reported_names(&still_held.body)),
        (200, vec!["held_endpoint".to_string()]),
        "case {:?}: the runtime this account does hold reports its endpoint, got {} with {}",
        "held runtime with one endpoint",
        still_held.status,
        still_held.body
    );
}

/// One row of the refusal table: a request whose input fails a modelled constraint, and
/// the member the refusal must name.
struct Refused {
    case: &'static str,
    runtime_id: &'static str,
    max_results: Option<i64>,
    next_token: Option<&'static str>,
    member: &'static str,
}

/// `ValidationException` stays for input that fails a modelled constraint, whether or
/// not the `agentRuntimeId` resolves: an absent runtime does not turn a bad page token
/// or an out-of-range page size into a success.
#[tokio::test]
async fn input_constraints_are_still_refused_for_an_absent_runtime() {
    let router = router();

    // An id inside `AgentRuntimeId`'s pattern that no row creates.
    let absent = "noSuchRuntime-abcdefghij";

    // A token this service never issued (inside `NextToken`'s own len 1..2048 and
    // `^\S*$`, so the boundary walk passes it through), each value just past a published
    // bound of `maxResults` (range 1..100), and an id outside `AgentRuntimeId`'s own
    // pattern `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` (it starts with a digit).
    let rows: Vec<Refused> = vec![
        Refused {
            case: "a nextToken this service did not issue",
            runtime_id: absent,
            max_results: None,
            next_token: Some("not-a-token"),
            member: "nextToken",
        },
        Refused {
            case: "maxResults below the modelled range",
            runtime_id: absent,
            max_results: Some(MAX_RESULTS_MIN - 1),
            next_token: None,
            member: "maxResults",
        },
        Refused {
            case: "maxResults above the modelled range",
            runtime_id: absent,
            max_results: Some(MAX_RESULTS_MAX + 1),
            next_token: None,
            member: "maxResults",
        },
        Refused {
            case: "an agentRuntimeId outside the modelled pattern",
            runtime_id: "0notAnId-abcdefghij",
            max_results: None,
            next_token: None,
            member: "agentRuntimeId",
        },
    ];

    for row in rows {
        let Refused {
            case,
            runtime_id,
            max_results,
            next_token,
            member,
        } = row;
        let answer = list_endpoints(&router, runtime_id, max_results, next_token).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: input that fails a modelled constraint is refused even when the \
             runtime is not there, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            "ValidationException",
            "case {case:?}: the refusal is the modelled ValidationException, got {}",
            answer.body
        );
        assert!(
            answer.body.to_string().contains(member),
            "case {case:?}: the refusal names the member that failed ({member}), got {}",
            answer.body
        );
    }

    // A token this listing DID issue for one runtime is well-formed input, so a listing
    // of a runtime that is not there resumes onto an empty page rather than a refusal.
    let held = create_runtime(&router, "listing_runtime_token_source").await;
    for name in ["first", "second"] {
        endpoint(&router, &held, &json!({"name": name})).await;
    }
    let page = list_endpoints(&router, &held, Some(1), None).await;
    let token = text(&page.body, "/nextToken");
    assert!(
        !token.is_empty(),
        "case {:?}: a page that leaves an endpoint over hands back a token, got {}",
        "the token this listing issued",
        page.body
    );
    let resumed = list_endpoints(&router, absent, None, Some(&token)).await;
    assert_eq!(
        (resumed.status, at(&resumed.body, "/runtimeEndpoints")),
        (200, json!([])),
        "case {:?}: a well-formed token against a runtime that is not there is an empty page, got \
         {} with {}",
        "a token issued for another runtime",
        resumed.status,
        resumed.body
    );
}

/// A `nextToken` this service did not issue is refused with the modelled
/// `ValidationException`; one it did issue resumes the walk.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    let runtime_id = create_runtime(&router, "listing_runtime_tokens").await;
    for name in ["first", "second"] {
        endpoint(&router, &runtime_id, &json!({"name": name})).await;
    }

    // Every row satisfies `NextToken`'s own constraints (len 1..2048, `^\S*$`), so the
    // boundary walk passes them through and the handler is what refuses them: a word
    // that is not hex at all, an odd number of hex digits, and hex that decodes to
    // bytes that are not text.
    let rows = ["not-a-token", "abc", "ff"];
    for token in rows {
        let case = format!("nextToken={token:?}");
        let answer = list_endpoints(&router, &runtime_id, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this service did not issue is refused, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer.body),
            "ValidationException",
            "case {case:?}: the refusal is the modelled ValidationException, got {}",
            answer.body
        );
        assert_eq!(
            text(&answer.body, "/fieldList/0/name"),
            "nextToken",
            "case {case:?}: the member that failed is reported in the fieldList, got {}",
            answer.body
        );
    }

    let first = list_endpoints(&router, &runtime_id, Some(1), None).await;
    let token = text(&first.body, "/nextToken");
    assert!(
        !token.is_empty(),
        "case {:?}: a page that leaves an endpoint over hands back a token, got {}",
        "the token this listing issued",
        first.body
    );
    let resumed = list_endpoints(&router, &runtime_id, Some(1), Some(&token)).await;
    assert_eq!(
        (resumed.status, reported_names(&resumed.body)),
        (200, vec!["second".to_string()]),
        "case {:?}: the token this listing issued resumes it after the entry the page ended on, \
         got {} with {}",
        "the token this listing issued",
        resumed.status,
        resumed.body
    );
}

/// The listing reports the version each endpoint PINS, and the status and served version
/// the clock owes it: `CREATING` with no `liveVersion` until its settle instant, then
/// `READY` serving the version it pins.
#[tokio::test]
async fn the_listing_reports_the_pinned_version_and_the_clock_owed_status() {
    let router = router();
    let runtime_id = create_runtime(&router, "listing_runtime_versions").await;
    endpoint(&router, &runtime_id, &json!({"name": "pinned_to_one"})).await;
    let second_version = update_runtime(&router, &runtime_id, "v2").await;
    endpoint(
        &router,
        &runtime_id,
        &json!({"name": "pinned_to_two", "agentRuntimeVersion": second_version}),
    )
    .await;

    let expected_pins = [
        ("pinned_to_one", FIRST_VERSION.to_string()),
        ("pinned_to_two", second_version.clone()),
    ];

    let answer = list_endpoints(&router, &runtime_id, None, None).await;
    assert_eq!(
        answer.status, 200,
        "listing the endpoints of a stored runtime is HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    for entry in reported(&answer.body) {
        let name = text(&entry, "/name");
        let (_, pinned) = expected_pins
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("the listing reported an endpoint no row created: {name:?}"));
        assert_eq!(
            text(&entry, "/targetVersion"),
            *pinned,
            "case {name:?}: the entry reports the version the create pinned, got {entry}"
        );
        assert_eq!(
            text(&entry, "/status"),
            "CREATING",
            "case {name:?}: an endpoint is created, not ready — the transition to READY is the \
             clock's, got {entry}"
        );
        assert_eq!(
            at(&entry, "/liveVersion"),
            Value::Null,
            "case {name:?}: nothing is served behind the name until the endpoint is READY, got \
             {entry}"
        );
    }

    tokio::time::sleep(PAST_ENDPOINT_SETTLE).await;

    let settled = list_endpoints(&router, &runtime_id, None, None).await;
    assert_eq!(
        settled.status, 200,
        "the listing stays readable once the clock-owed transition is due, got {} with {}",
        settled.status, settled.body
    );
    for entry in reported(&settled.body) {
        let name = text(&entry, "/name");
        assert_eq!(
            text(&entry, "/status"),
            "READY",
            "case {name:?}: the clock owes the endpoint READY once its settle instant passes, got \
             {entry}"
        );
        assert_eq!(
            text(&entry, "/liveVersion"),
            text(&entry, "/targetVersion"),
            "case {name:?}: a READY endpoint serves the version it pins, got {entry}"
        );
        let read = get_endpoint(&router, &runtime_id, &name).await;
        for member in SHARED_MEMBERS {
            let pointer = format!("/{member}");
            assert_eq!(
                at(&entry, &pointer),
                at(&read.body, &pointer),
                "case {name:?}: the settled listing reports {member:?} exactly as the single read \
                 does, got {entry} against {}",
                read.body
            );
        }
    }
}
