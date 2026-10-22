// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's project -> sandbox slice, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! The rule under test: `StartSandbox` mints ONE sandbox of the named project,
//! queued, identified by a derived id and the matching `sandbox/` ARN, carrying the
//! settings that project stores, and persists it so `BatchGetSandboxes` reports the
//! same record back; a project the account does not hold is refused instead.
//!
//! Each suite in `tests/all/` drives the wire itself, so the harness below (the
//! router, `call`, the project body) is this suite's own — the sibling suite's copy is
//! private to it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::Sandbox;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredSandbox,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so an ARN assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDSANDBOX";

/// Long enough for a sandbox's clock-owed transition out of `QUEUED` to have fallen
/// due (the service settles it [`state::SANDBOX_SETTLE_MILLIS`] after `StartSandbox`).
const PAST_THE_SETTLE_SPAN: Duration =
    Duration::from_millis(state::SANDBOX_SETTLE_MILLIS as u64 + 400);

/// The members a sandbox reports from its project, as (sandbox member, project
/// member): the model names all but the log configuration alike (`Sandbox$logConfig`
/// against `Project$logsConfig`).
const CARRIED_FROM_THE_PROJECT: &[(&str, &str)] = &[
    ("source", "source"),
    ("sourceVersion", "sourceVersion"),
    ("secondarySources", "secondarySources"),
    ("secondarySourceVersions", "secondarySourceVersions"),
    ("environment", "environment"),
    ("fileSystemLocations", "fileSystemLocations"),
    ("timeoutInMinutes", "timeoutInMinutes"),
    ("queuedTimeoutInMinutes", "queuedTimeoutInMinutes"),
    ("vpcConfig", "vpcConfig"),
    ("logConfig", "logsConfig"),
    ("encryptionKey", "encryptionKey"),
    ("serviceRole", "serviceRole"),
];

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// As [`emulator`], over an engine the caller already holds — so a test can seed or
/// inspect the service's state through the very store the handlers write.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// A minimal valid `CreateProject` request for `name`, with `extra`'s members merged in.
fn create_project_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
    });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    body
}

/// Create `name`, asserting it succeeded, and hand back the stored project.
async fn create_project(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let (status, body) = call(router, "CreateProject", create_project_body(name, extra)).await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {body}"
    );
    body["project"].clone()
}

/// Start a sandbox, asserting it succeeded, and hand back the sandbox.
async fn start_sandbox(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "StartSandbox", body.clone()).await;
    assert_eq!(
        status, 200,
        "StartSandbox {body}: expected 200, got {status} with {response}"
    );
    response["sandbox"].clone()
}

/// Read sandboxes back by id or ARN.
async fn batch_get_sandboxes(router: &MultiServiceRouter, ids: &[String]) -> Value {
    let (status, body) = call(router, "BatchGetSandboxes", json!({"ids": ids})).await;
    assert_eq!(
        status, 200,
        "BatchGetSandboxes {ids:?}: expected 200, got {status} with {body}"
    );
    body
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

fn expected_arn(resource: &str) -> String {
    format!(
        "arn:aws:codebuild:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A string member, for an assertion message that shows what was actually there.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// RULE: `StartSandbox` mints a queued sandbox of the named project, identified by an
/// id derived under the project's name and the matching `sandbox/` ARN, carrying the
/// settings the project stores — and a later read reports that same record.
///
/// The table spans the project shapes a sandbox reads from: no source, a source with a
/// version, the optional timeouts, a VPC and log configuration, secondary sources and
/// file-system locations, plus a project whose own name collides with the ARN's
/// resource segment.
#[tokio::test]
async fn start_sandbox_mints_a_queued_sandbox_of_each_project_and_reads_it_back() {
    let router = emulator();

    // (case, project name, the extra members the project is created with)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("a minimal NO_SOURCE project", "sandbox-minimal", json!({})),
        (
            "a GitHub source with a sourceVersion and the minimum timeout",
            "sandbox-github",
            json!({
                "source": {"type": "GITHUB", "location": "https://github.com/acme/app"},
                "sourceVersion": "refs/heads/main",
                "timeoutInMinutes": 5,
            }),
        ),
        (
            "an S3 source with a queued timeout, a VPC, logs and a CMK",
            "sandbox-configured",
            json!({
                "source": {"type": "S3", "location": "bucket/source.zip"},
                "queuedTimeoutInMinutes": 480,
                "encryptionKey": "arn:aws:kms:us-west-2:000000000000:alias/aws/s3",
                "vpcConfig": {
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnets": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                },
                "logsConfig": {
                    "cloudWatchLogs": {
                        "status": "ENABLED",
                        "groupName": "sandbox-logs",
                        "streamName": "stream",
                    },
                    "s3Logs": {"status": "DISABLED"},
                },
            }),
        ),
        (
            "two secondary sources and a file-system location",
            "sandbox-secondary",
            json!({
                "secondarySources": [
                    {"type": "S3", "location": "bucket/one.zip", "sourceIdentifier": "one"},
                    {"type": "S3", "location": "bucket/two.zip", "sourceIdentifier": "two"},
                ],
                "secondarySourceVersions": [
                    {"sourceIdentifier": "one", "sourceVersion": "v1"},
                ],
                "fileSystemLocations": [{
                    "type": "EFS",
                    "location": "fs-123.efs.us-west-2.amazonaws.com:/",
                    "mountPoint": "/mnt/efs",
                    "identifier": "efs1",
                }],
            }),
        ),
        // The id is `<projectName>:<uuid>` and the ARN `sandbox/<id>`, so a project
        // called `sandbox` is the case where a sloppy parse of either goes wrong.
        ("a project named sandbox", "sandbox", json!({})),
    ];

    for (case, name, extra) in cases {
        let project = create_project(&router, name, extra).await;
        let sandbox = start_sandbox(&router, json!({"projectName": name})).await;

        let id = text(&sandbox, "id");
        assert!(
            id.starts_with(&format!("{name}:")) && id.len() > name.len() + 1,
            "{case}: the id must be the project name and a derived uuid, got {id:?}"
        );
        assert_eq!(
            sandbox["arn"],
            expected_arn(&format!("sandbox/{id}")).as_str(),
            "{case}: unexpected arn in {sandbox}"
        );
        assert_eq!(
            sandbox["projectName"], name,
            "{case}: the sandbox belongs to the project asked for, got {sandbox}"
        );
        assert_eq!(
            sandbox["status"], "QUEUED",
            "{case}: a just-started sandbox is queued, got {sandbox}"
        );
        assert!(
            sandbox["requestTime"].is_number(),
            "{case}: requestTime must report when it was requested, got {sandbox}"
        );
        assert!(
            sandbox["startTime"].is_null() && sandbox["endTime"].is_null(),
            "{case}: a queued sandbox has neither started nor ended, got {sandbox}"
        );

        let session = sandbox["currentSession"].clone();
        assert!(
            !text(&session, "id").is_empty() && text(&session, "id") != id,
            "{case}: the session carries its own id, got {session}"
        );
        assert_eq!(
            session["status"], "QUEUED",
            "{case}: the session of a queued sandbox is queued, got {session}"
        );
        assert_eq!(
            session["currentPhase"], "QUEUED",
            "{case}: the session sits in the QUEUED phase, got {session}"
        );
        assert_eq!(
            session["phases"][0]["phaseType"], "SUBMITTED",
            "{case}: the request was submitted first, got {session}"
        );
        assert_eq!(
            session["phases"][0]["phaseStatus"], "SUCCEEDED",
            "{case}: the submitted phase succeeded, got {session}"
        );
        assert_eq!(
            session["phases"][1]["phaseType"], "QUEUED",
            "{case}: the open phase is the queue, got {session}"
        );
        assert!(
            session["phases"][1]["phaseStatus"].is_null(),
            "{case}: the phase it currently sits in has no outcome yet, got {session}"
        );

        for (sandbox_member, project_member) in CARRIED_FROM_THE_PROJECT {
            assert_eq!(
                sandbox[sandbox_member], project[project_member],
                "{case}: {sandbox_member} must be the project's {project_member}, \
                 got {} against {}",
                sandbox[sandbox_member], project[project_member]
            );
        }

        // The round trip: the read path reports the record the write persisted.
        let fetched = batch_get_sandboxes(&router, std::slice::from_ref(&id)).await;
        assert_eq!(
            fetched["sandboxes"][0], sandbox,
            "{case}: BatchGetSandboxes must report the stored sandbox, got {fetched}"
        );
        assert_eq!(
            fetched["sandboxesNotFound"],
            json!([]),
            "{case}: a sandbox that exists is not missing, got {fetched}"
        );
    }
}

/// One row of the read table: the case, the ids asked for, the ids expected back in
/// order, and the ids expected under `sandboxesNotFound`.
type ReadCase = (&'static str, Vec<String>, Vec<String>, Vec<String>);

/// RULE: `BatchGetSandboxes` resolves each entry of `ids` — the model documents them
/// as "a comma separated list of sandboxIds or sandboxArns" — against the store, in
/// the order asked, and reports the entries it holds nothing for under
/// `sandboxesNotFound`, as the caller wrote them.
#[tokio::test]
async fn batch_get_sandboxes_resolves_ids_and_arns_and_reports_the_rest_as_not_found() {
    let router = emulator();
    create_project(&router, "read-proj", json!({})).await;
    let first = start_sandbox(&router, json!({"projectName": "read-proj"})).await;
    let second = start_sandbox(&router, json!({"projectName": "read-proj"})).await;
    let first_id = text(&first, "id");
    let second_id = text(&second, "id");
    let first_arn = text(&first, "arn");
    let unknown = "read-proj:00000000-0000-8000-8000-000000000000".to_string();

    // (case, the ids asked for, the ids expected back in order, the ids expected missing)
    let cases: Vec<ReadCase> = vec![
        (
            "one id",
            vec![first_id.clone()],
            vec![first_id.clone()],
            vec![],
        ),
        (
            "one ARN",
            vec![first_arn.clone()],
            vec![first_id.clone()],
            vec![],
        ),
        (
            "an id and an ARN together",
            vec![second_id.clone(), first_arn.clone()],
            vec![second_id.clone(), first_id.clone()],
            vec![],
        ),
        (
            "start order",
            vec![first_id.clone(), second_id.clone()],
            vec![first_id.clone(), second_id.clone()],
            vec![],
        ),
        (
            "reversed",
            vec![second_id.clone(), first_id.clone()],
            vec![second_id.clone(), first_id.clone()],
            vec![],
        ),
        (
            "the same id twice",
            vec![first_id.clone(), first_id.clone()],
            vec![first_id.clone(), first_id.clone()],
            vec![],
        ),
        (
            "a known and an unknown id",
            vec![first_id.clone(), unknown.clone()],
            vec![first_id.clone()],
            vec![unknown.clone()],
        ),
        (
            "only unknown ids",
            vec![unknown.clone()],
            vec![],
            vec![unknown.clone()],
        ),
        ("no ids at all", vec![], vec![], vec![]),
    ];

    for (case, asked, expected_found, expected_missing) in cases {
        let body = batch_get_sandboxes(&router, &asked).await;
        let reported: Vec<String> = body["sandboxes"]
            .as_array()
            .expect("a sandboxes list")
            .iter()
            .map(|sandbox| text(sandbox, "id"))
            .collect();
        assert_eq!(
            reported, expected_found,
            "{case}: expected {expected_found:?}, got {reported:?}"
        );
        assert_eq!(
            body["sandboxesNotFound"],
            json!(expected_missing),
            "{case}: expected {expected_missing:?} missing, got {body}"
        );
    }

    // `SandboxIds` carries no `@length`, so the entry count is bounded by this service
    // instead: at the ceiling is served, one past it is refused.
    let at_the_ceiling = vec![first_id.clone(); state::MAX_BATCH_GET_SANDBOXES_IDS];
    let body = batch_get_sandboxes(&router, &at_the_ceiling).await;
    assert_eq!(
        body["sandboxes"].as_array().map_or(0, Vec::len),
        state::MAX_BATCH_GET_SANDBOXES_IDS,
        "at the ceiling: expected {} records, got {body}",
        state::MAX_BATCH_GET_SANDBOXES_IDS
    );

    let past_the_ceiling = vec![first_id; state::MAX_BATCH_GET_SANDBOXES_IDS + 1];
    let (status, body) = call(
        &router,
        "BatchGetSandboxes",
        json!({"ids": past_the_ceiling}),
    )
    .await;
    assert_eq!(
        status, 400,
        "one id past the ceiling: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "one id past the ceiling: expected InvalidInputException, got {body}"
    );
    assert!(
        text(&body, "message").contains("ids"),
        "one id past the ceiling: the refusal must name the member, got {body}"
    );
}

/// RULE: each `StartSandbox` of a project mints a distinct sandbox, and the store
/// keeps them in start order (which is what the sandbox list operations page over).
#[tokio::test]
async fn each_start_mints_a_distinct_sandbox_and_keeps_start_order() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // (project, how many sandboxes to start)
    for (project, count) in [("one-sandbox", 1), ("three-sandboxes", 3), ("five", 5)] {
        create_project(&router, project, json!({})).await;
        let mut ids_in_start_order = Vec::new();
        for index in 1..=count {
            let sandbox = start_sandbox(&router, json!({"projectName": project})).await;
            let id = text(&sandbox, "id");
            assert!(
                !ids_in_start_order.contains(&id),
                "{project}: sandbox {index} reused the id {id:?} \
                 (already had {ids_in_start_order:?})"
            );
            ids_in_start_order.push(id);
        }

        let handle = engine.state::<CodeBuildState>();
        let stored = handle
            .lock()
            .sandboxes_by_project
            .get(project)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            stored, ids_in_start_order,
            "{project}: the store must keep the {count} sandboxes in start order, \
             got {stored:?}"
        );
    }
}

/// RULE: a `StartSandbox` idempotency token in force resolves to the sandbox the first
/// request with it started, rather than starting a second one; a different token, a
/// different project, or no token at all starts a new sandbox.
#[tokio::test]
async fn an_idempotency_token_resolves_to_the_sandbox_it_started() {
    let router = emulator();
    create_project(&router, "idem-sandbox", json!({})).await;
    create_project(&router, "other-sandbox", json!({})).await;

    let request = json!({"projectName": "idem-sandbox", "idempotencyToken": "token-1"});
    let first = start_sandbox(&router, request.clone()).await;
    let replay = start_sandbox(&router, request).await;
    assert_eq!(
        replay["id"], first["id"],
        "a replayed token must resolve to the same sandbox, got {replay}"
    );
    assert_eq!(
        replay["requestTime"], first["requestTime"],
        "a replayed token must not re-request the sandbox, got {replay}"
    );

    // (case, the request, whether it must be a sandbox of its own)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a different token",
            json!({"projectName": "idem-sandbox", "idempotencyToken": "token-2"}),
        ),
        (
            "the same token on another project",
            json!({"projectName": "other-sandbox", "idempotencyToken": "token-1"}),
        ),
        ("no token at all", json!({"projectName": "idem-sandbox"})),
        (
            "no token at all, again",
            json!({"projectName": "idem-sandbox"}),
        ),
    ];
    let mut seen = vec![text(&first, "id")];
    for (case, request) in cases {
        let sandbox = start_sandbox(&router, request).await;
        let id = text(&sandbox, "id");
        assert!(
            !seen.contains(&id),
            "{case}: must start a sandbox of its own, got {id:?} already in {seen:?}"
        );
        seen.push(id);
    }
}

/// RULE: a project the account does not hold is refused with
/// `ResourceNotFoundException` and nothing is persisted; a request without a
/// `projectName` — which the model leaves optional — is refused as invalid input.
#[tokio::test]
async fn start_sandbox_refuses_a_project_the_account_does_not_hold() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "held-proj", json!({})).await;
    create_project(&router, "doomed-proj", json!({})).await;
    let (status, body) = call(&router, "DeleteProject", json!({"name": "doomed-proj"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject doomed-proj: expected 200, got {status} with {body}"
    );

    // (case, the projectName asked for)
    let cases: Vec<(&str, &str)> = vec![
        ("a name never created", "never-created"),
        ("a project just deleted", "doomed-proj"),
        ("a held name in the wrong case", "HELD-PROJ"),
        ("a name that is a prefix of a held one", "held"),
    ];
    for (case, project_name) in cases {
        let (status, body) = call(&router, "StartSandbox", json!({"projectName": project_name})).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "{case}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(project_name),
            "{case}: the refusal must name the project asked for, got {body}"
        );
    }

    let (status, body) = call(&router, "StartSandbox", json!({})).await;
    assert_eq!(
        status, 400,
        "an absent projectName: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "an absent projectName: expected InvalidInputException, got {body}"
    );
    assert!(
        text(&body, "message").contains("projectName"),
        "an absent projectName: the refusal must name the member, got {body}"
    );

    let handle = engine.state::<CodeBuildState>();
    let guard = handle.lock();
    assert!(
        guard.sandboxes.is_empty() && guard.sandboxes_by_project.is_empty(),
        "a refused StartSandbox must persist nothing, got {:?}",
        guard.sandboxes.keys().collect::<Vec<_>>()
    );
}

/// RULE: the move out of `QUEUED` is owed by the CLOCK, so it is materialised from the
/// stored instants when the sandbox is read — once, and identically on every later
/// read. A sandbox that leaves the queue is a LIVE session: it reports `RUNNING`, sits
/// in the `RUN_SANDBOX` phase, and has not ended.
#[tokio::test]
async fn a_sandbox_settles_once_from_stored_state_and_never_changes_again() {
    let router = emulator();
    create_project(&router, "settle-sandbox", json!({})).await;
    let started = start_sandbox(&router, json!({"projectName": "settle-sandbox"})).await;
    let id = text(&started, "id");

    // Before the span elapses the caller still sees the transient state.
    let early = batch_get_sandboxes(&router, std::slice::from_ref(&id)).await;
    assert_eq!(
        early["sandboxes"][0]["status"], "QUEUED",
        "a sandbox read straight after StartSandbox is still QUEUED, got {early}"
    );

    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let settled = batch_get_sandboxes(&router, std::slice::from_ref(&id)).await;
    let record = settled["sandboxes"][0].clone();
    assert_eq!(
        record["status"], "RUNNING",
        "a provisioned sandbox is a live session, got {record}"
    );
    assert!(
        record["startTime"].is_number() && record["endTime"].is_null(),
        "a live sandbox reports when it started and has not ended, got {record}"
    );
    let session = record["currentSession"].clone();
    assert_eq!(
        session["status"], "RUNNING",
        "the session runs with the sandbox, got {session}"
    );
    assert_eq!(
        session["currentPhase"], state::SANDBOX_PHASE_RUN_SANDBOX,
        "a live session sits in the RUN_SANDBOX phase, got {session}"
    );
    let phases = session["phases"].as_array().expect("the phases").clone();
    let reported: Vec<(String, String)> = phases
        .iter()
        .map(|phase| (text(phase, "phaseType"), text(phase, "phaseStatus")))
        .collect();
    assert_eq!(
        reported,
        vec![
            ("SUBMITTED".to_string(), "SUCCEEDED".to_string()),
            ("QUEUED".to_string(), "SUCCEEDED".to_string()),
            ("PROVISIONING".to_string(), "SUCCEEDED".to_string()),
            ("RUN_SANDBOX".to_string(), String::new()),
        ],
        "the session must have been submitted, queued, provisioned and now be running, \
         got {session}"
    );

    // Read again: identical, because it was computed once from the stored instants.
    let reread = batch_get_sandboxes(&router, &[id]).await;
    assert_eq!(
        reread["sandboxes"][0], record,
        "a settled sandbox must never be re-fabricated per call, got {reread}"
    );
}

/// RULE: the sandbox store is bounded at its growth site — `StartSandbox` mints a
/// record per call, so one past the ceiling evicts the oldest sandbox that has already
/// reached a terminal status, pruning the per-project index with it.
#[tokio::test]
async fn the_sandbox_store_is_bounded_at_its_growth_site() {
    let ceiling = i64::try_from(state::MAX_SANDBOXES).expect("the ceiling fits in i64");
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let handle = engine.state::<CodeBuildState>();
    {
        let mut guard = handle.lock();
        for index in 0..=ceiling {
            let id = format!("bounded:{index:08}");
            guard.insert_sandbox(
                id.clone(),
                StoredSandbox {
                    sandbox: Sandbox {
                        id: Some(id),
                        project_name: Some("bounded".to_string()),
                        // Terminal: `endTime` is set, so it is evictable.
                        end_time: Some(state::datetime(index)),
                        ..Sandbox::default()
                    },
                    request_millis: index,
                    settle_due_millis: index,
                },
            );
        }
    }

    let guard = handle.lock();
    assert_eq!(
        guard.sandboxes.len(),
        state::MAX_SANDBOXES,
        "one sandbox past the ceiling must evict one, got {}",
        guard.sandboxes.len()
    );
    assert!(
        !guard.sandboxes.contains_key("bounded:00000000"),
        "the oldest terminal sandbox is the one evicted, got {:?}",
        guard.sandboxes.keys().next()
    );
    let indexed = guard
        .sandboxes_by_project
        .get("bounded")
        .map_or(0, Vec::len);
    assert_eq!(
        indexed,
        state::MAX_SANDBOXES,
        "eviction must prune the per-project index too, got {indexed}"
    );
}
