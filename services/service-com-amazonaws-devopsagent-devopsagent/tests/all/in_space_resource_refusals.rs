// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two refusal arms that EVERY operation of this service
//! addressed to a resource INSIDE an agent space shares — the asset, asset-file,
//! backlog-task, recommendation and Trigger planes at once.
//!
//! The rules under test:
//!
//! 1. An operation addressed to a resource inside an agent space authorises the caller
//!    against the containing space before it can know anything that space holds, so an
//!    `agentSpaceId` that resolves to no space fails CLOSED with HTTP 403
//!    `AccessDeniedException` and the wholly fixed message `Access denied` — whatever
//!    the request names inside the space, and whether or not a resource with that
//!    identifier exists in some other space.
//! 2. Inside a space the caller reached, an identifier that names no such resource is
//!    HTTP 404 `ResourceNotFoundException` whose message is the fixed text for the KIND
//!    of resource that was resolved plus the identifier the request carried, taken
//!    verbatim from it and nothing else — never the containing space's identifier.
//!
//! The per-plane suites pin these arms one plane at a time; this one spans the
//! operations, so an operation resolving its space and its inner resource in ONE
//! chained lookup — answering a request for a space that does not exist as a missing
//! resource, and naming the space in a refusal that resolved something inside it —
//! fails here whichever plane it belongs to.
//!
//! The tables span the classes the rules generalise over: the agent-space rows differ
//! in length (both published bounds of `AgentSpaceIdentifier`, 1..2048), character
//! class and form, and include the minted identifier of a space that was DELETED; the
//! inner-identifier rows differ in length, character class and form across `ResourceId`
//! (`^[a-zA-Z0-9_.-]+$`, 1..128), `AssetFilePath` (`^[a-zA-Z0-9_./ ()-]+$`, 1..512) and
//! the narrower forms this service's own identifiers take; and the rows are crossed
//! with every in-space operation, with the SERVED cases exercised too, so a refusal
//! that ignored the store would fail here.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which is
/// how the router resolves which service claims a request. Signature verification is
/// switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The published bounds of `AgentSpaceIdentifier`, the shape every in-space plane's
/// `agentSpaceId` label targets (`@length(min: 1, max: 2048)`, no pattern), from
/// `smithy_query(query="operation", operation="GetBacklogTask,GetAsset,GetTrigger")`.
const AGENT_SPACE_IDENTIFIER_MIN_LEN: usize = 1;
const AGENT_SPACE_IDENTIFIER_MAX_LEN: usize = 2048;

/// The published upper bound of `ResourceId`, the shape the `assetId`, `taskId`,
/// `recommendationId` and `triggerId` labels target (`@length(min: 1, max: 128)`,
/// `@pattern ^[a-zA-Z0-9_.-]+$`).
const RESOURCE_ID_MAX_LEN: usize = 128;

/// The published upper bound of `AssetFilePath`, the shape the greedy `{path+}` label
/// targets (`@length(min: 1, max: 512)`, `@pattern ^[a-zA-Z0-9_./ ()-]+$`).
const ASSET_FILE_PATH_MAX_LEN: usize = 512;

/// The fail-closed wording, written out here rather than read from the service so the
/// test pins the text itself.
const ACCESS_DENIED: &str = "Access denied";

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine),
    );
    router
}

/// One answer as it came off the wire.
struct Answer {
    status: u16,
    /// The error shape `x-amzn-errortype` names (restJson1's error code), falling back
    /// to the body's `__type`; empty on a success.
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `message` of an error answer.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// Send one request and return what came back.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    let error_type = if header.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header
    };
    // Either source may carry a `namespace#Shape` or a trailing `:` — the shape name is
    // what names the error.
    let error_type = error_type
        .split(['#', ':'])
        .find(|part| !part.is_empty() && part.contains("Exception"))
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_type,
        body: parsed,
    }
}

/// Create an agent space and return its minted id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        answer.status, 201,
        "case {name:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer.body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {}", answer.body))
        .to_string()
}

/// The identifiers the resources inside one agent space are reached by: either the ones
/// a space really holds, or ones no space holds.
struct Inner {
    asset: String,
    /// The asset-file path in its wire (URI) form.
    path: String,
    task: String,
    recommendation: String,
    trigger: String,
}

/// The create request the asset plane serves, holding one file.
fn asset_request(path: &str) -> Value {
    json!({
        "assetType": "runbook",
        "content": {"file": {"path": path, "body": {"text": "step 1"}}},
    })
}

/// The create request the backlog plane serves, with the required members only.
fn task_request() -> Value {
    json!({
        "taskType": "INVESTIGATION",
        "title": "investigate the oncall backlog",
        "priority": "MEDIUM",
    })
}

/// The create request the Trigger plane serves, firing on a fixed schedule.
fn trigger_request() -> Value {
    json!({
        "type": "SCHEDULE",
        "condition": {"schedule": {"expression": "cron(0 9 * * ? *)"}},
        "action": {"agent": "custom", "prompt": "review the oncall backlog"},
    })
}

/// The file path the seeded asset holds, in both its wire and its stored form.
const HELD_PATH: &str = "docs/present.md";

/// Create one resource of every plane that HAS a create in this service's modelled
/// surface and return the identifiers they are reached by.
///
/// Nothing in the modelled surface creates a recommendation (the model declares only
/// Get, List and Update), so the recommendation member always names nothing — which is
/// why no recommendation row is a served row here; `recommendation_reads` covers that
/// plane's served path by seeding the store the handler reads through.
async fn seed(router: &MultiServiceRouter, space: &str) -> Inner {
    let created_asset = call(
        router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(asset_request(HELD_PATH)),
    )
    .await;
    assert_eq!(
        created_asset.status, 201,
        "case seed asset: expected 201, got {} with {}",
        created_asset.status, created_asset.body
    );
    let created_task = call(
        router,
        "POST",
        &format!("/backlog/agent-space/{space}/tasks"),
        Some(task_request()),
    )
    .await;
    assert_eq!(
        created_task.status, 201,
        "case seed task: expected 201, got {} with {}",
        created_task.status, created_task.body
    );
    let created_trigger = call(
        router,
        "POST",
        &format!("/trigger/agent-space/{space}/triggers"),
        Some(trigger_request()),
    )
    .await;
    assert_eq!(
        created_trigger.status, 201,
        "case seed trigger: expected 201, got {} with {}",
        created_trigger.status, created_trigger.body
    );

    Inner {
        asset: minted(&created_asset, "asset", "assetId"),
        path: HELD_PATH.to_string(),
        task: minted(&created_task, "task", "taskId"),
        recommendation: absent_recommendation_ids()[0].clone(),
        trigger: minted(&created_trigger, "trigger", "triggerId"),
    }
}

/// The identifier a create reported for the resource it minted.
fn minted(answer: &Answer, member: &str, id_member: &str) -> String {
    answer.body[member][id_member]
        .as_str()
        .unwrap_or_else(|| panic!("case seed {member}: no {id_member} in {}", answer.body))
        .to_string()
}

/// One request addressed to a resource inside an agent space.
struct InSpaceCall {
    op: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// Whether this request is SERVED when its identifiers name resources the space
    /// holds — true for the reads of the planes this service can create a resource in.
    served_when_present: bool,
}

/// Every operation this service serves that is addressed to a resource INSIDE an agent
/// space, as one request each. The routes are the ones the crate's `OPERATIONS.md`
/// states for them.
fn in_space_calls(space: &str, inner: &Inner) -> Vec<InSpaceCall> {
    let assets = format!("/asset/agent-space/{space}/assets");
    let asset = format!("{assets}/{}", inner.asset);
    let file = format!("{asset}/files/{}", inner.path);
    let file_body = json!({"content": {"text": "a line"}});
    vec![
        InSpaceCall {
            op: "CreateAsset",
            method: "POST",
            uri: assets.clone(),
            body: Some(asset_request(HELD_PATH)),
            served_when_present: false,
        },
        InSpaceCall {
            op: "GetAsset",
            method: "GET",
            uri: asset.clone(),
            body: None,
            served_when_present: true,
        },
        InSpaceCall {
            op: "ListAssetVersions",
            method: "GET",
            uri: format!("{asset}/versions"),
            body: None,
            served_when_present: true,
        },
        InSpaceCall {
            op: "DeleteAsset",
            method: "DELETE",
            uri: asset.clone(),
            body: None,
            served_when_present: false,
        },
        InSpaceCall {
            op: "CreateAssetFile",
            method: "POST",
            uri: file.clone(),
            body: Some(file_body.clone()),
            served_when_present: false,
        },
        InSpaceCall {
            op: "GetAssetFile",
            method: "GET",
            uri: file.clone(),
            body: None,
            served_when_present: true,
        },
        InSpaceCall {
            op: "UpdateAssetFile",
            method: "PATCH",
            uri: file.clone(),
            body: Some(file_body),
            served_when_present: false,
        },
        InSpaceCall {
            op: "DeleteAssetFile",
            method: "DELETE",
            uri: file,
            body: None,
            served_when_present: false,
        },
        InSpaceCall {
            op: "CreateBacklogTask",
            method: "POST",
            uri: format!("/backlog/agent-space/{space}/tasks"),
            body: Some(task_request()),
            served_when_present: false,
        },
        InSpaceCall {
            op: "GetBacklogTask",
            method: "GET",
            uri: format!("/backlog/agent-space/{space}/tasks/{}", inner.task),
            body: None,
            served_when_present: true,
        },
        InSpaceCall {
            op: "GetRecommendation",
            method: "GET",
            uri: format!(
                "/backlog/agent-space/{space}/recommendations/{}",
                inner.recommendation
            ),
            body: None,
            served_when_present: false,
        },
        InSpaceCall {
            op: "CreateTrigger",
            method: "POST",
            uri: format!("/trigger/agent-space/{space}/triggers"),
            body: Some(trigger_request()),
            served_when_present: false,
        },
        InSpaceCall {
            op: "GetTrigger",
            method: "GET",
            uri: format!("/trigger/agent-space/{space}/triggers/{}", inner.trigger),
            body: None,
            served_when_present: true,
        },
    ]
}

/// Values that satisfy `AgentSpaceIdentifier`'s length bound and name no stored agent
/// space: the shortest admissible one, a digit-only one, one in the form this service
/// mints, a mixed-case hyphenated one, an all-hyphen one, one holding characters an
/// `AgentSpaceId` pattern would not admit (this label carries no pattern), and one at
/// the bound's maximum.
fn absent_agent_space_ids() -> Vec<String> {
    vec![
        "a".repeat(AGENT_SPACE_IDENTIFIER_MIN_LEN),
        "7".to_string(),
        "as-0000000000000000".to_string(),
        "Mixed-Case-Agent-Space-42".to_string(),
        "---".to_string(),
        "as_0000000000000000.v2".to_string(),
        "z".repeat(AGENT_SPACE_IDENTIFIER_MAX_LEN),
    ]
}

/// Values that satisfy `ResourceId` (`^[a-zA-Z0-9_.-]+$`, 1..128) and name no stored
/// resource: the shortest admissible one, a digit-only one, one in the form this service
/// mints, each of the pattern's punctuation characters alone, a mixed-case dotted one,
/// and one at the pattern's maximum length.
fn absent_resource_ids() -> Vec<String> {
    vec![
        "a".to_string(),
        "7".to_string(),
        "asset-0000000000000000".to_string(),
        "_".to_string(),
        ".".to_string(),
        "-".to_string(),
        "Mixed.Case_Resource-42".to_string(),
        "z".repeat(RESOURCE_ID_MAX_LEN),
    ]
}

/// Recommendation identifiers in the form this service's recommendation plane requires
/// (`rec-` and a UUID) that name no stored recommendation — so the request passes that
/// plane's own identifier guard and reaches the lookup. The rows differ in digit case
/// and value, which the UUID form admits either of.
fn absent_recommendation_ids() -> Vec<String> {
    vec![
        "rec-00000000-0000-0000-0000-000000000000".to_string(),
        "rec-ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "rec-ABCDEF01-2345-6789-abcd-EF0123456789".to_string(),
    ]
}

/// Goal identifiers in the form the Trigger plane requires (`goal-` and a UUID, since a
/// Trigger is the schedule of a goal) that name no stored Trigger.
fn absent_goal_ids() -> Vec<String> {
    vec![
        "goal-00000000-0000-0000-0000-000000000000".to_string(),
        "goal-ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "goal-ABCDEF01-2345-6789-abcd-EF0123456789".to_string(),
    ]
}

/// Paths that satisfy `AssetFilePath` (`^[a-zA-Z0-9_./ ()-]+$`, 1..512) and name no file
/// of the seeded asset, each as the wire form the greedy label carries and the form the
/// refusal names it by: a flat one, a nested one, a deeply nested one, a
/// percent-encoded space and parens (the pattern admits both, the URI does not carry
/// them raw), and one at the length bound's maximum.
fn absent_paths() -> Vec<(String, String)> {
    vec![
        ("absent.md".to_string(), "absent.md".to_string()),
        ("docs/absent.md".to_string(), "docs/absent.md".to_string()),
        (
            "a/b/c/d/absent.bin".to_string(),
            "a/b/c/d/absent.bin".to_string(),
        ),
        (
            "notes/absent%20(1).bin".to_string(),
            "notes/absent (1).bin".to_string(),
        ),
        (
            "z".repeat(ASSET_FILE_PATH_MAX_LEN),
            "z".repeat(ASSET_FILE_PATH_MAX_LEN),
        ),
    ]
}

/// RULE 1: an operation addressed to a resource inside an agent space authorises the
/// caller against the containing space FIRST, so a space that resolves to nothing is
/// HTTP 403 `AccessDeniedException` with the wholly fixed message `Access denied`,
/// disclosing nothing about what any space holds.
#[tokio::test]
async fn every_in_space_operation_fails_closed_on_a_space_that_resolves_to_nothing() {
    let router = new_router();
    let present = create_space(&router, "in-space-fail-closed").await;
    // The identifiers of resources that really exist — in a space the request below
    // does NOT name — so the refusal is decided by the space the request named and not
    // by the store being empty.
    let held = seed(&router, &present).await;
    let nowhere = Inner {
        asset: "asset-0000000000000000".to_string(),
        path: "docs/absent.md".to_string(),
        task: "task-0000000000000000".to_string(),
        recommendation: absent_recommendation_ids()[1].clone(),
        trigger: absent_goal_ids()[1].clone(),
    };

    // A space this service minted and that was then deleted is as unreachable as one
    // that never existed, and its identifier is one no literal here supplied.
    let deleted = create_space(&router, "in-space-deleted").await;
    let removed = call(&router, "DELETE", &format!("/v1/agentspaces/{deleted}"), None).await;
    assert_eq!(
        removed.status, 204,
        "case delete {deleted:?}: expected 204, got {} with {}",
        removed.status, removed.body
    );

    let mut spaces = absent_agent_space_ids();
    spaces.push(deleted);
    for space_id in spaces {
        assert_ne!(
            space_id, present,
            "the table must not name the stored AgentSpace"
        );
        for (which, inner) in [
            ("identifiers another space holds", &held),
            ("identifiers no space holds", &nowhere),
        ] {
            for request in in_space_calls(&space_id, inner) {
                let case = format!("{} in {space_id:?} with {which}", request.op);
                let answer =
                    call(&router, request.method, &request.uri, request.body.clone()).await;
                assert_eq!(
                    answer.status, 403,
                    "case {case}: expected 403, got {} with {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_type, "AccessDeniedException",
                    "case {case}: expected AccessDeniedException, got {:?} ({})",
                    answer.error_type, answer.body
                );
                assert_eq!(
                    answer.message(),
                    ACCESS_DENIED,
                    "case {case}: expected the fixed access-denied wording, got {:?}",
                    answer.message()
                );
            }
        }
    }

    // The space that DOES exist still answers its reads from the store, so the arm
    // above is a lookup and not a blanket refusal — and nothing addressed to a space
    // that resolves to nothing touched what this one holds.
    for request in in_space_calls(&present, &held) {
        if !request.served_when_present {
            continue;
        }
        let answer = call(&router, request.method, &request.uri, request.body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {} in the stored space: expected 200, got {} with {}",
            request.op, answer.status, answer.body
        );
    }
}

/// One request whose identifiers reach a space the caller holds but name no resource in
/// it, with the refusal wording the kind of resource that was resolved carries.
struct MissingRow {
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expected: String,
}

/// RULE 2: inside a space the caller reached, an identifier that names no such resource
/// is HTTP 404 `ResourceNotFoundException` naming the KIND of resource that was resolved
/// and the identifier the request carried — and nothing else, in particular never the
/// containing space.
#[tokio::test]
async fn a_missing_resource_in_a_reachable_space_names_only_what_was_asked_for() {
    let router = new_router();
    let space = create_space(&router, "in-space-missing-resource").await;
    let held = seed(&router, &space).await;
    let file_body = json!({"content": {"text": "a line"}});

    let mut rows: Vec<MissingRow> = Vec::new();
    for asset_id in absent_resource_ids() {
        assert_ne!(asset_id, held.asset, "the table must not name the stored asset");
        let expected = format!("Asset not found: {asset_id}");
        let asset = format!("/asset/agent-space/{space}/assets/{asset_id}");
        let file = format!("{asset}/files/{HELD_PATH}");
        for (op, method, uri, body) in [
            ("GetAsset", "GET", asset.clone(), None),
            (
                "ListAssetVersions",
                "GET",
                format!("{asset}/versions"),
                None,
            ),
            ("DeleteAsset", "DELETE", asset, None),
            (
                "CreateAssetFile",
                "POST",
                file.clone(),
                Some(file_body.clone()),
            ),
            ("GetAssetFile", "GET", file.clone(), None),
            (
                "UpdateAssetFile",
                "PATCH",
                file.clone(),
                Some(file_body.clone()),
            ),
            ("DeleteAssetFile", "DELETE", file, None),
        ] {
            rows.push(MissingRow {
                case: format!("{op} {asset_id:?}"),
                method,
                uri,
                body,
                expected: expected.clone(),
            });
        }
    }
    for (wire_path, named) in absent_paths() {
        let expected = format!("Asset file not found: {named}");
        let file = format!(
            "/asset/agent-space/{space}/assets/{}/files/{wire_path}",
            held.asset
        );
        for (op, method, body) in [
            ("GetAssetFile", "GET", None),
            ("UpdateAssetFile", "PATCH", Some(file_body.clone())),
            ("DeleteAssetFile", "DELETE", None),
        ] {
            rows.push(MissingRow {
                case: format!("{op} {wire_path:?}"),
                method,
                uri: file.clone(),
                body,
                expected: expected.clone(),
            });
        }
    }
    for task_id in absent_resource_ids() {
        assert_ne!(task_id, held.task, "the table must not name the stored task");
        rows.push(MissingRow {
            case: format!("GetBacklogTask {task_id:?}"),
            method: "GET",
            uri: format!("/backlog/agent-space/{space}/tasks/{task_id}"),
            body: None,
            expected: format!("Task not found: {task_id}"),
        });
    }
    for recommendation_id in absent_recommendation_ids() {
        rows.push(MissingRow {
            case: format!("GetRecommendation {recommendation_id:?}"),
            method: "GET",
            uri: format!(
                "/backlog/agent-space/{space}/recommendations/{recommendation_id}"
            ),
            body: None,
            expected: format!("Recommendation not found: {recommendation_id}"),
        });
    }
    for goal_id in absent_goal_ids() {
        assert_ne!(
            goal_id, held.trigger,
            "the table must not name the stored Trigger"
        );
        rows.push(MissingRow {
            // A Trigger is the schedule of a goal, so the goal is what the Trigger
            // plane resolves and the kind its refusal names.
            case: format!("GetTrigger {goal_id:?}"),
            method: "GET",
            uri: format!("/trigger/agent-space/{space}/triggers/{goal_id}"),
            body: None,
            expected: format!("Goal not found: {goal_id}"),
        });
    }

    for row in &rows {
        let answer = call(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            answer.status, 404,
            "case {}: expected 404, got {} with {}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {}: expected ResourceNotFoundException, got {:?} ({})",
            row.case, answer.error_type, answer.body
        );
        assert_eq!(
            answer.message(),
            row.expected,
            "case {}: expected {:?}, got {:?}",
            row.case,
            row.expected,
            answer.message()
        );
        // The caller reached the space, so it is already known to them: a refusal that
        // resolved something inside it names the resource alone.
        assert!(
            !answer.message().contains(space.as_str()),
            "case {}: the refusal must not name the agent space, got {:?}",
            row.case,
            answer.message()
        );
    }

    // Everything the space holds is still served afterwards, so the refusals above were
    // decided from the store and changed nothing in it.
    for (op, uri) in [
        (
            "GetAsset",
            format!("/asset/agent-space/{space}/assets/{}", held.asset),
        ),
        (
            "GetAssetFile",
            format!(
                "/asset/agent-space/{space}/assets/{}/files/{}",
                held.asset, held.path
            ),
        ),
        (
            "GetBacklogTask",
            format!("/backlog/agent-space/{space}/tasks/{}", held.task),
        ),
        (
            "GetTrigger",
            format!("/trigger/agent-space/{space}/triggers/{}", held.trigger),
        ),
    ] {
        let answer = call(&router, "GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {op} after the refusals: expected 200, got {} with {}",
            answer.status, answer.body
        );
    }
}
