// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the AgentSpace lifecycle and the backlog-task
//! workflow inside one: `CreateAgentSpace`, `GetAgentSpace`, `ListAgentSpaces`,
//! `CreateBacklogTask`, `GetBacklogTask` and `DeleteAgentSpace`.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    MAX_AGENT_SPACES, MAX_TASKS_PER_AGENT_SPACE,
};

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The `TaskType` value set, from the model's enum
/// (`smithy_query(query="shape", shape="TaskType")`).
const TASK_TYPES: &[&str] = &[
    "INVESTIGATION",
    "EVALUATION",
    "RELEASE_READINESS_REVIEW",
    "RELEASE_TESTING",
];

/// The `Priority` value set, from the model's enum
/// (`smithy_query(query="shape", shape="Priority")`).
const PRIORITIES: &[&str] = &["CRITICAL", "HIGH", "MEDIUM", "LOW", "MINIMAL"];

/// The recognised `AgentSpacePreferenceKey` value set, from the model's enum
/// (`smithy_query(query="shape", shape="AgentSpacePreferenceKey")`).
const PREFERENCE_KEYS: &[&str] = &["elevatedActionsEnabled"];

/// The published bounds of `ListAgentSpacesInput$maxResults` (`@range(min: 1, max:
/// 100)`), from `smithy_query(query="operation", operation="ListAgentSpaces")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

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

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
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
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(status, 201, "case {name:?}: expected 201, got {status}");
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// Every stored agent space id, collected by walking `ListAgentSpaces` pages.
async fn list_all_ids(router: &MultiServiceRouter, max_results: Option<i64>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut uri = "/v1/agentspaces/list".to_string();
        let mut params: Vec<String> = Vec::new();
        if let Some(limit) = max_results {
            params.push(format!("maxResults={limit}"));
        }
        if let Some(token) = next.as_deref() {
            params.push(format!("nextToken={token}"));
        }
        if !params.is_empty() {
            uri.push('?');
            uri.push_str(&params.join("&"));
        }
        let (status, body) = call(router, "POST", &uri, None).await;
        assert_eq!(status, 200, "case list({max_results:?}): got {status}: {body}");
        let page = body["agentSpaces"]
            .as_array()
            .unwrap_or_else(|| panic!("case list({max_results:?}): no agentSpaces in {body}"));
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case list({max_results:?}): page of {} exceeds maxResults {limit}",
                page.len()
            );
        }
        for space in page {
            ids.push(
                space["agentSpaceId"]
                    .as_str()
                    .expect("agentSpaceId is a string")
                    .to_string(),
            );
        }
        match body["nextToken"].as_str() {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
        assert!(
            ids.len() < 10_000,
            "case list({max_results:?}): pagination did not terminate"
        );
    }
    ids
}

/// RULE: a create persists the resource, and a read returns exactly what was
/// persisted — for every shape of input the model allows, not just a minimal one.
#[tokio::test]
async fn created_agent_space_is_returned_unchanged_by_get_and_list() {
    // The rows differ along what the rule generalises over: which optional members
    // are present, the character classes `AgentSpaceName`/`Description` allow, and
    // the size of the tag and preference maps.
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({"name": "demo-space"})),
        (
            "described",
            json!({"name": "demo-space", "description": "bootstrap demo"}),
        ),
        (
            "localized",
            json!({"name": "ops-space", "locale": "en-US", "description": "ops"}),
        ),
        (
            "three-letter locale with subtags",
            json!({"name": "ops-space-2", "locale": "fil-PH"}),
        ),
        (
            "kms encrypted",
            json!({
                "name": "secure space",
                "kmsKeyArn": "arn:aws:kms:us-west-2:123456789012:key/12345678-1234-1234-1234-123456789012",
            }),
        ),
        (
            "tagged",
            json!({"name": "tagged-space", "tags": {"team": "devops", "env": "test"}}),
        ),
        (
            "preferences set",
            json!({"name": "pref-space", "preferences": {"elevatedActionsEnabled": true}}),
        ),
        (
            "unicode and punctuation name",
            json!({"name": "éq(uipe) #1 — ops", "description": "ünïcode ✓"}),
        ),
        (
            "name at the 255-character maximum",
            json!({"name": "n".repeat(255)}),
        ),
        ("single-character name", json!({"name": "n"})),
    ];

    let router = new_router();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for (case, input) in &cases {
        let (status, created) = call(&router, "POST", "/v1/agentspaces", Some(input.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAgentSpace, got {status}: {created}"
        );
        let space = &created["agentSpace"];
        let id = space["agentSpaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no agentSpaceId in {created}"))
            .to_string();

        // A minted id satisfies the modelled `AgentSpaceId` pattern
        // `^[a-zA-Z0-9-]{1,64}$` and is never handed out twice.
        assert!(
            (1..=64).contains(&id.chars().count())
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case:?}: minted id {id:?} violates ^[a-zA-Z0-9-]{{1,64}}$"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {case:?}: minted id {id:?} was handed out twice"
        );

        // Every member the request carried is persisted, and the required
        // timestamps are present.
        for member in ["name", "description", "locale", "kmsKeyArn", "preferences"] {
            if let Some(expected) = input.get(member) {
                assert_eq!(
                    &space[member], expected,
                    "case {case:?}: {member} came back as {:?}",
                    space[member]
                );
            }
        }
        // Both required timestamps are present. (They render as epoch seconds:
        // the framework's restJson1 default. The model marks them `date-time`, but
        // `@timestampFormat` is not plumbed through the schema codegen in
        // `vendor/`, which is outside this crate.)
        assert!(
            !space["createdAt"].is_null() && !space["updatedAt"].is_null(),
            "case {case:?}: createdAt/updatedAt missing from {created}"
        );
        if let Some(tags) = input.get("tags") {
            assert_eq!(
                &created["tags"], tags,
                "case {case:?}: tags came back as {:?}",
                created["tags"]
            );
        }

        // READ BACK: GetAgentSpace answers exactly what was persisted.
        let (status, fetched) = call(&router, "GET", &format!("/v1/agentspaces/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAgentSpace, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["agentSpace"], created["agentSpace"],
            "case {case:?}: GetAgentSpace returned {:?}, not what CreateAgentSpace persisted ({:?})",
            fetched["agentSpace"], created["agentSpace"]
        );
        assert_eq!(
            fetched["tags"], created["tags"],
            "case {case:?}: tags read back as {:?}",
            fetched["tags"]
        );

        // A second read is identical: reading changes nothing.
        let (_, again) = call(&router, "GET", &format!("/v1/agentspaces/{id}"), None).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two reads of one agent space disagree"
        );
    }

    // Every created space is enumerated by ListAgentSpaces, exactly once.
    let listed = list_all_ids(&router, None).await;
    let listed_set: BTreeSet<String> = listed.iter().cloned().collect();
    assert_eq!(
        listed.len(),
        listed_set.len(),
        "ListAgentSpaces returned a duplicate: {listed:?}"
    );
    assert_eq!(
        listed_set, seen_ids,
        "ListAgentSpaces returned {listed_set:?}, expected the created set {seen_ids:?}"
    );

    // An id that names nothing is a 404, not an empty 200.
    for case in ["does-not-exist", "as-0000000000000000"] {
        let (status, body) = call(&router, "GET", &format!("/v1/agentspaces/{case}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetAgentSpace, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: `includedData` selects how much data the read returns; `METADATA_ONLY`
/// answers the resource without its associated tag set, and the default is
/// `ALL_DATA`.
#[tokio::test]
async fn get_agent_space_honours_included_data() {
    let router = new_router();
    let (status, created) = call(
        &router,
        "POST",
        "/v1/agentspaces",
        Some(json!({"name": "tagged", "tags": {"team": "devops"}})),
    )
    .await;
    assert_eq!(status, 201, "setup: got {status}: {created}");
    let id = created["agentSpace"]["agentSpaceId"].as_str().unwrap();

    // Rows: the enum's value set (smithy_query shape="IncludedData") plus the
    // absent member, which takes the documented ALL_DATA default.
    let cases: [(&str, Option<&str>, bool); 3] = [
        ("absent (default)", None, true),
        ("ALL_DATA", Some("ALL_DATA"), true),
        ("METADATA_ONLY", Some("METADATA_ONLY"), false),
    ];
    for (case, included, expect_tags) in cases {
        let uri = match included {
            Some(value) => format!("/v1/agentspaces/{id}?includedData={value}"),
            None => format!("/v1/agentspaces/{id}"),
        };
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
        assert_eq!(
            body["agentSpace"]["agentSpaceId"].as_str(),
            Some(id),
            "case {case:?}: wrong agent space in {body}"
        );
        assert_eq!(
            body["tags"].is_null(),
            !expect_tags,
            "case {case:?}: tags came back as {:?}",
            body["tags"]
        );
    }
}

/// RULE: an `@idempotent` create keyed by `@idempotencyToken` returns the resource
/// the first call created; a different token creates a different resource.
#[tokio::test]
async fn create_agent_space_replays_the_same_client_token() {
    let router = new_router();

    // Rows differ along the token's own value space (`^[!-~]+$`, max 64) and the
    // body sent with the repeat.
    let cases: Vec<(&str, String)> = vec![
        ("short token", "tok-1".to_string()),
        ("punctuation token", "~!@#$%^&*()_+-={}|".to_string()),
        ("digits token", "0123456789".to_string()),
        ("token at the 64-character maximum", "t".repeat(64)),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, token) in &cases {
        let body = json!({"name": "idem-space", "clientToken": token});
        let (status, first) = call(&router, "POST", "/v1/agentspaces", Some(body.clone())).await;
        assert_eq!(status, 201, "case {case:?}: got {status}: {first}");
        let (status, second) = call(&router, "POST", "/v1/agentspaces", Some(body)).await;
        assert_eq!(status, 201, "case {case:?}: repeat got {status}: {second}");
        assert_eq!(
            second, first,
            "case {case:?}: the repeat returned {second}, not the first result {first}"
        );
        ids.insert(
            first["agentSpace"]["agentSpaceId"]
                .as_str()
                .expect("agentSpaceId")
                .to_string(),
        );
    }

    // One resource per distinct token, and no extra space minted by the repeats.
    assert_eq!(
        ids.len(),
        cases.len(),
        "expected one agent space per distinct token, got {ids:?}"
    );
    let listed: BTreeSet<String> = list_all_ids(&router, None).await.into_iter().collect();
    assert_eq!(
        listed, ids,
        "the repeats created extra agent spaces: listed {listed:?}, created {ids:?}"
    );
}

/// RULE: `ListAgentSpaces` bounds a page by `maxResults` and resumes exactly where
/// the previous page stopped, so walking the pages enumerates every stored
/// resource once.
#[tokio::test]
async fn list_agent_spaces_pages_by_max_results() {
    let router = new_router();
    let mut created: BTreeSet<String> = BTreeSet::new();
    for n in 0..7 {
        created.insert(create_space(&router, &format!("space-{n}")).await);
    }

    // Rows: page sizes below, equal to and above the stored count, plus each
    // published bound of the member's @range(1..100) and the default (absent).
    for max_results in [
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(7),
        Some(8),
        Some(MAX_RESULTS_MAX),
    ] {
        let ids = list_all_ids(&router, max_results).await;
        let unique: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "case maxResults={max_results:?}: a resource was returned twice: {ids:?}"
        );
        assert_eq!(
            unique, created,
            "case maxResults={max_results:?}: enumerated {unique:?}, expected {created:?}"
        );
    }

    // One past each published bound is refused by the modelled range.
    for case in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/v1/agentspaces/list?maxResults={case}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={case}: expected 400 for a value outside @range(1..100), got {status}: {body}"
        );
    }

    // A token this service never issued names no position in the listing — whether
    // it is not a token at all, carries no version marker, or is well-formed hex
    // that decodes to something other than a minted agentSpaceId.
    for (case, token) in [
        ("not a token", "not-a-token".to_string()),
        ("hex without the version marker", hex::encode("as-0000000000000001")),
        ("marked hex that names nothing", format!("v1:{}", hex::encode("zzz"))),
        (
            "marked hex naming a non-id",
            format!("v1:{}", hex::encode("as-not-hex-at-all!")),
        ),
        ("marker only", "v1:".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/v1/agentspaces/list?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a forged nextToken produced {body}"
        );
    }

    // An empty store lists nothing and offers no token.
    let empty = new_router();
    let (status, body) = call(&empty, "POST", "/v1/agentspaces/list", None).await;
    assert_eq!(status, 200, "empty list got {status}: {body}");
    assert_eq!(
        body["agentSpaces"],
        json!([]),
        "an empty store listed {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "an empty store offered a nextToken: {body}"
    );
}

/// RULE: a backlog task is created under its parent agent space with the caller's
/// own `taskType`, `title` and `priority`, a minted id, the initial status and
/// version 1 — and a read returns exactly that.
#[tokio::test]
async fn created_backlog_task_is_returned_unchanged_by_get() {
    let router = new_router();
    let space_id = create_space(&router, "backlog-space").await;

    // The rows span the full modelled catalogs: every TaskType against every
    // Priority, with and without a description and an external reference.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for (index, task_type) in TASK_TYPES.iter().enumerate() {
        for priority in PRIORITIES {
            let mut input = json!({
                "taskType": task_type,
                "title": format!("{task_type} / {priority}"),
                "priority": priority,
            });
            if index % 2 == 0 {
                input["description"] = json!("why this task exists");
            }
            if index % 3 == 0 {
                input["reference"] = json!({
                    "system": "JIRA",
                    "title": "ticket",
                    "referenceId": format!("OPS-{index}"),
                    "referenceUrl": "https://example.invalid/OPS",
                    "associationId": "assoc-1",
                });
            }
            cases.push((format!("{task_type}/{priority}"), input));
        }
    }

    let mut task_ids: BTreeSet<String> = BTreeSet::new();
    for (case, input) in &cases {
        let (status, created) = call(
            &router,
            "POST",
            &format!("/backlog/agent-space/{space_id}/tasks"),
            Some(input.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateBacklogTask, got {status}: {created}"
        );
        let task = &created["task"];
        let task_id = task["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {created}"))
            .to_string();

        // The minted id satisfies the modelled `ResourceId` pattern
        // `^[a-zA-Z0-9_.-]+$` (1..128) and is unique.
        assert!(
            (1..=128).contains(&task_id.chars().count())
                && task_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
            "case {case:?}: minted taskId {task_id:?} violates ^[a-zA-Z0-9_.-]+$"
        );
        assert!(
            task_ids.insert(task_id.clone()),
            "case {case:?}: minted taskId {task_id:?} was handed out twice"
        );

        assert_eq!(
            task["agentSpaceId"].as_str(),
            Some(space_id.as_str()),
            "case {case:?}: task stored under {:?}",
            task["agentSpaceId"]
        );
        for member in ["taskType", "title", "priority", "description", "reference"] {
            if let Some(expected) = input.get(member) {
                assert_eq!(
                    &task[member], expected,
                    "case {case:?}: {member} came back as {:?}",
                    task[member]
                );
            }
        }
        assert_eq!(
            task["status"].as_str(),
            Some("PENDING_TRIAGE"),
            "case {case:?}: initial status was {:?}",
            task["status"]
        );
        assert_eq!(
            task["version"].as_i64(),
            Some(1),
            "case {case:?}: initial version was {:?}",
            task["version"]
        );
        assert!(
            !task["createdAt"].is_null() && !task["updatedAt"].is_null(),
            "case {case:?}: createdAt/updatedAt missing from {created}"
        );

        // READ BACK: GetBacklogTask answers exactly what was persisted.
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/backlog/agent-space/{space_id}/tasks/{task_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetBacklogTask, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["task"], created["task"],
            "case {case:?}: GetBacklogTask returned {:?}, not what was created ({:?})",
            fetched["task"], created["task"]
        );
    }

    // Inside a space the caller reaches, a task id that names nothing — including
    // one that belongs to another space — is 404: a task is reachable only through
    // its owner. An `agentSpaceId` that resolves to no space is refused BEFORE
    // anything inside it is looked up, so it fails closed with 403 instead.
    let other_space = create_space(&router, "other-space").await;
    let borrowed = task_ids.iter().next().expect("at least one task").clone();
    for (case, space, task, expected_status, expected_code) in [
        (
            "unknown task",
            space_id.as_str(),
            "does-not-exist",
            404,
            "ResourceNotFoundException",
        ),
        (
            "task of another space",
            other_space.as_str(),
            borrowed.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "absent space",
            "as-0000000000000000",
            borrowed.as_str(),
            403,
            "AccessDeniedException",
        ),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/backlog/agent-space/{space}/tasks/{task}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} from GetBacklogTask, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Creating a task under an agent space that does not exist fails closed the
    // same way.
    let (status, body) = call(
        &router,
        "POST",
        "/backlog/agent-space/as-0000000000000000/tasks",
        Some(json!({"taskType": "INVESTIGATION", "title": "orphan", "priority": "LOW"})),
    )
    .await;
    assert_eq!(
        status, 403,
        "CreateBacklogTask against an unknown agent space got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "CreateBacklogTask against an unknown agent space produced {body}"
    );

    // And the create is idempotent per client token, like CreateAgentSpace.
    let body = json!({
        "taskType": "INVESTIGATION",
        "title": "investigate latency",
        "priority": "HIGH",
        "clientToken": "tok-2",
    });
    let (status, first) = call(
        &router,
        "POST",
        &format!("/backlog/agent-space/{space_id}/tasks"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 201, "idempotent create got {status}: {first}");
    let (status, second) = call(
        &router,
        "POST",
        &format!("/backlog/agent-space/{space_id}/tasks"),
        Some(body),
    )
    .await;
    assert_eq!(status, 201, "idempotent repeat got {status}: {second}");
    assert_eq!(
        second, first,
        "the repeat returned {second}, not the first result {first}"
    );
}

/// RULE: deleting an agent space removes it and everything reachable through it,
/// so later reads cannot see it and the listing omits it.
#[tokio::test]
async fn deleted_agent_space_and_its_tasks_are_gone() {
    let router = new_router();

    // Rows: a space with no tasks, one task, and several tasks — the rule holds
    // whatever the space owns.
    for task_count in [0_usize, 1, 3] {
        let case = format!("{task_count} task(s)");
        let keep = create_space(&router, "keeper").await;
        let doomed = create_space(&router, "doomed").await;
        let mut doomed_tasks = Vec::new();
        for n in 0..task_count {
            let (status, created) = call(
                &router,
                "POST",
                &format!("/backlog/agent-space/{doomed}/tasks"),
                Some(json!({
                    "taskType": "INVESTIGATION",
                    "title": format!("task-{n}"),
                    "priority": "MEDIUM",
                })),
            )
            .await;
            assert_eq!(status, 201, "case {case:?}: setup got {status}: {created}");
            doomed_tasks.push(created["task"]["taskId"].as_str().unwrap().to_string());
        }

        let (status, body) = call(&router, "DELETE", &format!("/v1/agentspaces/{doomed}"), None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteAgentSpace, got {status}: {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {case:?}: DeleteAgentSpace answered a body: {body}"
        );

        let (status, body) = call(&router, "GET", &format!("/v1/agentspaces/{doomed}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted space is still readable: {status} {body}"
        );

        for task in &doomed_tasks {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/backlog/agent-space/{doomed}/tasks/{task}"),
                None,
            )
            .await;
            // The space is gone, so the request no longer reaches anything inside
            // it: the refusal is the fail-closed one, not a report about the task.
            assert_eq!(
                status, 403,
                "case {case:?}: task {task} outlived its agent space: {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: reading task {task} in a deleted space produced {body}"
            );
        }

        let listed: BTreeSet<String> = list_all_ids(&router, Some(1)).await.into_iter().collect();
        assert!(
            !listed.contains(&doomed),
            "case {case:?}: the deleted space is still listed: {listed:?}"
        );
        assert!(
            listed.contains(&keep),
            "case {case:?}: the surviving space vanished: {listed:?}"
        );

        // A second delete of the same id reports the resource as absent — the
        // modelled 404 — rather than failing with a server error.
        let (status, body) = call(&router, "DELETE", &format!("/v1/agentspaces/{doomed}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: repeat delete got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: repeat delete produced {body}"
        );

        let (status, _) = call(&router, "DELETE", &format!("/v1/agentspaces/{keep}"), None).await;
        assert_eq!(status, 204, "case {case:?}: cleanup delete got {status}");
    }
}

/// RULE: a preference key the service does not recognise is rejected; a recognised
/// one is stored and read back.
#[tokio::test]
async fn agent_space_preferences_accept_only_recognised_keys() {
    let router = new_router();

    // Recognised keys — the model's whole AgentSpacePreferenceKey value set —
    // against both boolean values.
    for key in PREFERENCE_KEYS {
        for value in [true, false] {
            let case = format!("{key}={value}");
            let (status, created) = call(
                &router,
                "POST",
                "/v1/agentspaces",
                Some(json!({"name": "pref-space", "preferences": {(*key): value}})),
            )
            .await;
            assert_eq!(status, 201, "case {case:?}: got {status}: {created}");
            let id = created["agentSpace"]["agentSpaceId"].as_str().unwrap();
            let (status, fetched) = call(&router, "GET", &format!("/v1/agentspaces/{id}"), None).await;
            assert_eq!(status, 200, "case {case:?}: read back {status}: {fetched}");
            assert_eq!(
                fetched["agentSpace"]["preferences"][*key],
                json!(value),
                "case {case:?}: preference read back as {:?}",
                fetched["agentSpace"]["preferences"]
            );
        }
    }

    // Unrecognised keys are rejected, whatever else the map carries.
    let cases: Vec<(&str, Value)> = vec![
        ("unknown key", json!({"elevatedActions": true})),
        (
            "known and unknown together",
            json!({"elevatedActionsEnabled": true, "somethingElse": false}),
        ),
        ("wrong case", json!({"ElevatedActionsEnabled": true})),
        ("empty key", json!({"": true})),
    ];
    for (case, preferences) in &cases {
        let (status, body) = call(
            &router,
            "POST",
            "/v1/agentspaces",
            Some(json!({"name": "bad-prefs", "preferences": preferences})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an unrecognized preference key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // A rejected create stores nothing.
    let listed = list_all_ids(&router, None).await;
    assert_eq!(
        listed.len(),
        PREFERENCE_KEYS.len() * 2,
        "a rejected create persisted an agent space: {listed:?}"
    );
}

/// RULE: every caller-driven store is bounded — a create past the cap is refused
/// with the modelled quota error and the store stops growing.
#[tokio::test]
async fn creates_are_refused_at_the_store_bounds() {
    let router = new_router();

    // Fill the agent-space store to its cap, then ask for one more.
    for n in 0..MAX_AGENT_SPACES {
        create_space(&router, &format!("space-{n}")).await;
    }
    let (status, body) = call(
        &router,
        "POST",
        "/v1/agentspaces",
        Some(json!({"name": "one-too-many"})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case agent-space cap: expected 402 at the cap, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case agent-space cap: expected ServiceQuotaExceededException, got {body}"
    );
    let listed = list_all_ids(&router, Some(MAX_RESULTS_MAX)).await;
    assert_eq!(
        listed.len(),
        MAX_AGENT_SPACES,
        "case agent-space cap: the refused create grew the store to {}",
        listed.len()
    );

    // The task store under one agent space is bounded the same way. Deleting one
    // of the capped spaces first makes room for the space this part needs.
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/v1/agentspaces/{}", listed[0]),
        None,
    )
    .await;
    assert_eq!(status, 204, "case task cap: setup delete got {status}");
    let space = create_space(&router, "task-heavy").await;
    for n in 0..MAX_TASKS_PER_AGENT_SPACE {
        let (status, created) = call(
            &router,
            "POST",
            &format!("/backlog/agent-space/{space}/tasks"),
            Some(json!({
                "taskType": "INVESTIGATION",
                "title": format!("task-{n}"),
                "priority": "LOW",
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case task cap: task {n} got {status}: {created}"
        );
    }
    let (status, body) = call(
        &router,
        "POST",
        &format!("/backlog/agent-space/{space}/tasks"),
        Some(json!({
            "taskType": "INVESTIGATION",
            "title": "one too many",
            "priority": "LOW",
        })),
    )
    .await;
    assert_eq!(
        status, 402,
        "case task cap: expected 402 at the cap, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case task cap: expected ServiceQuotaExceededException, got {body}"
    );
}

/// RULE: an idempotency-token entry is reclaimed with the resource it named, so
/// the same token after a delete creates a new resource instead of replaying a
/// resource that no longer exists.
#[tokio::test]
async fn client_token_is_reclaimed_with_its_resource() {
    let router = new_router();
    let body = json!({"name": "recycled", "clientToken": "tok-recycle"});

    let (status, first) = call(&router, "POST", "/v1/agentspaces", Some(body.clone())).await;
    assert_eq!(status, 201, "case first create: got {status}: {first}");
    let first_id = first["agentSpace"]["agentSpaceId"].as_str().unwrap().to_string();

    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/v1/agentspaces/{first_id}"),
        None,
    )
    .await;
    assert_eq!(status, 204, "case delete: got {status}");

    let (status, second) = call(&router, "POST", "/v1/agentspaces", Some(body)).await;
    assert_eq!(status, 201, "case re-create: got {status}: {second}");
    let second_id = second["agentSpace"]["agentSpaceId"].as_str().unwrap();
    assert_ne!(
        second_id, first_id,
        "case re-create: the token replayed the deleted agent space {first_id}"
    );
    let (status, fetched) = call(&router, "GET", &format!("/v1/agentspaces/{second_id}"), None).await;
    assert_eq!(status, 200, "case re-create: read back {status}: {fetched}");
}
