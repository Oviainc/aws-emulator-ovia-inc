// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RetryBuild`, driven over the wire the router serves:
//! `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: `RetryBuild` restarts a stored build by minting a NEW build of
//! that build's project — it takes the project's next build number, a fresh id and
//! ARN, starts `IN_PROGRESS` under the calling identity, carries the settings the
//! retried build had resolved (the request carries none), and links the pair through
//! `autoRetryConfig` (`previousAutoRetry` back, `nextAutoRetry` forward, the retry
//! count incremented). The retried build keeps the outcome it had reported.
//!
//! Every assertion is about state: what the retry persisted is what a later
//! `BatchGetBuilds` / `ListBuildsForProject` reports.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so an ARN assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with. `Build$initiator` reports the
/// calling identity, which is this.
const ACCESS_KEY_ID: &str = "AKIDRETRYBUILDTEST";

/// Long enough for a build's clock-owed transition out of `IN_PROGRESS` to have fallen
/// due (the service settles it 750 ms after the call that started it).
const PAST_THE_SETTLE_SPAN: Duration = Duration::from_millis(1_100);

/// The members a retry carries over unchanged from the build it restarts: every
/// `Build` member that describes HOW the build runs rather than how it is going.
///
/// Source: the `Build` shape's members —
/// `smithy_query(query="shape",shape="Build",model_path="services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json")`
/// — minus the identity members (`id`, `arn`, `buildNumber`), the progress members
/// (`startTime`, `endTime`, `currentPhase`, `buildStatus`, `buildComplete`, `phases`),
/// the members a running build produces (`resolvedSourceVersion`,
/// `exportedEnvironmentVariables`, `reportArns`, `networkInterface`), the caller
/// (`initiator`), the batch membership (`buildBatchArn`) and the retry link
/// (`autoRetryConfig`), each of which is asserted on its own below.
const CARRIED_MEMBERS: [&str; 17] = [
    "projectName",
    "source",
    "secondarySources",
    "secondarySourceVersions",
    "sourceVersion",
    "artifacts",
    "secondaryArtifacts",
    "cache",
    "environment",
    "serviceRole",
    "logs",
    "timeoutInMinutes",
    "queuedTimeoutInMinutes",
    "vpcConfig",
    "encryptionKey",
    "fileSystemLocations",
    "debugSession",
];

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(std::sync::Arc::new(
        engine::InMemoryEngine::new(),
    )));
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

/// Create `name` with `extra`'s members merged into a minimal valid request.
async fn create_project(router: &MultiServiceRouter, name: &str, extra: Value) {
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
    let (status, response) = call(router, "CreateProject", body).await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {response}"
    );
}

/// Start a build, asserting it succeeded, and hand back the build.
async fn start_build(router: &MultiServiceRouter, project: &str, extra: Value) -> Value {
    let mut body = json!({"projectName": project});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    let (status, response) = call(router, "StartBuild", body.clone()).await;
    assert_eq!(
        status, 200,
        "StartBuild {body}: expected 200, got {status} with {response}"
    );
    response["build"].clone()
}

/// Retry a build, asserting it succeeded, and hand back the new build.
async fn retry_build(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "RetryBuild", body.clone()).await;
    assert_eq!(
        status, 200,
        "RetryBuild {body}: expected 200, got {status} with {response}"
    );
    response["build"].clone()
}

/// The persisted record of `id`, read back through `BatchGetBuilds`.
async fn stored_build(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, fetched) = call(router, "BatchGetBuilds", json!({"ids": [id]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds {id:?}: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["buildsNotFound"],
        json!([]),
        "BatchGetBuilds {id:?}: the build must be held, got {fetched}"
    );
    fetched["builds"][0].clone()
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

/// How the build being retried stood when the retry arrived.
#[derive(Clone, Copy, Debug)]
enum Before {
    /// Still running: the retry arrives inside the settle span.
    InProgress,
    /// Stopped by the caller.
    Stopped,
    /// Left alone until the clock owed it its terminal state.
    Settled,
}

/// One row of the retry table.
#[derive(Debug)]
struct Case {
    /// Row name, printed by every assertion.
    name: &'static str,
    /// The project this row builds under — one per row, so the build numbers a row
    /// asserts are that project's own.
    project: &'static str,
    /// Members merged into this row's `CreateProject` request.
    project_extra: fn() -> Value,
    /// Members merged into this row's `StartBuild` request.
    start_extra: fn() -> Value,
    /// What happened to the build before it was retried.
    before: Before,
    /// `autoRetryConfig.autoRetryLimit` the retried build carries, which the retry
    /// carries forward.
    auto_retry_limit: Value,
}

/// RULE: a retry is a NEW build of the retried build's project — next build number,
/// fresh id and ARN, `IN_PROGRESS` under the calling identity — carrying the settings
/// the retried build resolved, linked to it through `autoRetryConfig`; and the retried
/// build keeps the outcome it reported, gaining only the forward link.
///
/// The rows differ along what the rule generalises over: the settings the retried
/// build resolved (project defaults, start-time overrides, extra structures), where its
/// auto-retry limit came from (nowhere, the project, a start-time override), and the
/// state it was in when the retry arrived (running, stopped, clock-settled).
#[tokio::test]
async fn a_retry_starts_a_new_linked_build_from_the_retried_builds_settings() {
    let cases = [
        Case {
            name: "project defaults, retried while running",
            project: "retry-defaults",
            project_extra: || json!({}),
            start_extra: || json!({}),
            before: Before::InProgress,
            auto_retry_limit: json!(null),
        },
        Case {
            name: "start-time overrides, retried while running",
            project: "retry-overrides",
            project_extra: || json!({"description": "overridden at start"}),
            start_extra: || {
                json!({
                    "buildspecOverride": "version: 0.2\nphases: {}",
                    "timeoutInMinutesOverride": 25,
                    "queuedTimeoutInMinutesOverride": 15,
                    "computeTypeOverride": "BUILD_GENERAL1_MEDIUM",
                    "environmentVariablesOverride": [
                        {"name": "STAGE", "value": "beta", "type": "PLAINTEXT"},
                    ],
                    "sourceVersion": "refs/heads/main",
                    "encryptionKeyOverride": "alias/aws/s3",
                    "serviceRoleOverride": "arn:aws:iam::000000000000:role/other",
                    "debugSessionEnabled": true,
                })
            },
            before: Before::InProgress,
            auto_retry_limit: json!(null),
        },
        Case {
            name: "project auto-retry limit, retried after being stopped",
            project: "retry-stopped",
            project_extra: || json!({"autoRetryLimit": 2}),
            start_extra: || json!({}),
            before: Before::Stopped,
            auto_retry_limit: json!(2),
        },
        Case {
            name: "auto-retry limit overridden at start, retried after it settled",
            project: "retry-settled",
            project_extra: || json!({"autoRetryLimit": 2}),
            start_extra: || json!({"autoRetryLimitOverride": 1}),
            before: Before::Settled,
            auto_retry_limit: json!(1),
        },
        Case {
            name: "cache, logs and vpc structures carried over",
            project: "retry-structures",
            project_extra: || {
                json!({
                    "cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]},
                    "logsConfig": {
                        "cloudWatchLogs": {
                            "status": "ENABLED",
                            "groupName": "retry-group",
                            "streamName": "retry-stream",
                        },
                        "s3Logs": {"status": "DISABLED"},
                    },
                    "vpcConfig": {
                        "vpcId": "vpc-0123456789abcdef0",
                        "subnets": ["subnet-0123456789abcdef0"],
                        "securityGroupIds": ["sg-0123456789abcdef0"],
                    },
                    "fileSystemLocations": [{
                        "type": "EFS",
                        "location": "fs-0123456789abcdef0.efs.us-west-2.amazonaws.com:/",
                        "mountPoint": "/mnt/efs",
                        "identifier": "efs1",
                    }],
                })
            },
            start_extra: || json!({"cacheOverride": {"type": "NO_CACHE"}}),
            before: Before::InProgress,
            auto_retry_limit: json!(null),
        },
    ];

    for case in &cases {
        let router = emulator();
        create_project(&router, case.project, (case.project_extra)()).await;
        let original = start_build(&router, case.project, (case.start_extra)()).await;
        let original_id = original["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the build carries an id", case.name))
            .to_string();

        // The retried build's own outcome before the retry, which the retry must not
        // disturb.
        let expected_status = match case.before {
            Before::InProgress => {
                assert_eq!(
                    original["autoRetryConfig"]["autoRetryLimit"],
                    case.auto_retry_limit,
                    "case {:?}: expected autoRetryLimit {}, got {original}",
                    case.name,
                    case.auto_retry_limit
                );
                "IN_PROGRESS"
            }
            Before::Stopped => {
                let (status, stopped) =
                    call(&router, "StopBuild", json!({"id": original_id.clone()})).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: StopBuild expected 200, got {status} with {stopped}",
                    case.name
                );
                "STOPPED"
            }
            Before::Settled => {
                tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
                // The project's buildspec declares no command, so the build the clock
                // finished ran nothing that could fail.
                "SUCCEEDED"
            }
        };

        let retry = retry_build(&router, json!({"id": original_id.clone()})).await;
        let retry_id = retry["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the retry carries an id", case.name))
            .to_string();

        // A new build, with its own identity.
        assert_ne!(
            retry_id, original_id,
            "case {:?}: a retry must be a new build, got {retry}",
            case.name
        );
        let minted = retry_id
            .strip_prefix(&format!("{}:", case.project))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the retry id must be <projectName>:<uuid>, got {retry_id:?}",
                    case.name
                )
            });
        assert_eq!(
            (
                minted.len(),
                minted.match_indices('-').map(|(at, _)| at).collect::<Vec<_>>(),
                minted.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            ),
            (36, vec![8, 13, 18, 23], true),
            "case {:?}: the retry id must carry an 8-4-4-4-12 hex UUID, got {minted:?}",
            case.name
        );
        assert_eq!(
            retry["arn"],
            expected_arn(&format!("build/{retry_id}")).as_str(),
            "case {:?}: unexpected retry arn in {retry}",
            case.name
        );
        assert_eq!(
            (retry["buildNumber"].as_i64(), original["buildNumber"].as_i64()),
            (Some(2), Some(1)),
            "case {:?}: a retry takes the project's next build number, got {retry}",
            case.name
        );

        // A fresh build, in progress, started by this caller.
        assert_eq!(
            (
                retry["buildStatus"].as_str(),
                retry["currentPhase"].as_str(),
                retry["buildComplete"].as_bool(),
                retry["endTime"].clone(),
                retry["initiator"].as_str(),
            ),
            (
                Some("IN_PROGRESS"),
                Some("QUEUED"),
                Some(false),
                json!(null),
                Some(ACCESS_KEY_ID),
            ),
            "case {:?}: a retry starts as a fresh in-progress build, got {retry}",
            case.name
        );
        let (retry_start, original_start) =
            (retry["startTime"].as_f64(), original["startTime"].as_f64());
        // Read as `f64`, which is what a restJson1 timestamp carries on the wire: an
        // instant whose millisecond remainder is zero is written as a whole number and one
        // that is not carries a fraction, so comparing the two as integers would compare
        // nothing at all whenever either side has sub-second precision.
        assert!(
            retry_start
                .is_some_and(|retry_at| original_start.is_some_and(|was_at| retry_at >= was_at)),
            "case {:?}: the retry starts no earlier than the build it retries, got {retry}",
            case.name
        );
        assert_eq!(
            retry["phases"]
                .as_array()
                .map(|phases| phases
                    .iter()
                    .map(|phase| phase["phaseType"].as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()),
            Some(vec!["SUBMITTED".to_string(), "QUEUED".to_string()]),
            "case {:?}: a retry reports the phases of a queued build, got {retry}",
            case.name
        );
        assert_eq!(
            retry["resolvedSourceVersion"],
            json!(null),
            "case {:?}: nothing has been cloned for the retry yet, got {retry}",
            case.name
        );

        // The settings come from the build that was retried, not from the request —
        // which carries no settings at all.
        for name in CARRIED_MEMBERS {
            assert_eq!(
                retry[name], original[name],
                "case {:?}: {name} must be carried over from the retried build, got {retry}",
                case.name
            );
        }

        // The pair is linked, in both directions.
        assert_eq!(
            (
                retry["autoRetryConfig"]["autoRetryNumber"].clone(),
                retry["autoRetryConfig"]["previousAutoRetry"].clone(),
                retry["autoRetryConfig"]["nextAutoRetry"].clone(),
                retry["autoRetryConfig"]["autoRetryLimit"].clone(),
            ),
            (
                json!(1),
                json!(expected_arn(&format!("build/{original_id}"))),
                json!(null),
                case.auto_retry_limit.clone(),
            ),
            "case {:?}: the retry must link back to the build it retries, got {retry}",
            case.name
        );

        // ROUND TRIP: the retry is persisted, and reads report it verbatim.
        let read_back = stored_build(&router, &retry_id).await;
        assert_eq!(
            read_back, retry,
            "case {:?}: BatchGetBuilds must report the persisted retry, got {read_back}",
            case.name
        );

        // The retried build keeps its own outcome and gains only the forward link.
        let retried = stored_build(&router, &original_id).await;
        assert_eq!(
            retried["buildStatus"].as_str(),
            Some(expected_status),
            "case {:?}: the retried build must keep its outcome, got {retried}",
            case.name
        );
        assert_eq!(
            (
                retried["id"].clone(),
                retried["buildNumber"].clone(),
                retried["startTime"].clone(),
                retried["autoRetryConfig"]["nextAutoRetry"].clone(),
            ),
            (
                original["id"].clone(),
                original["buildNumber"].clone(),
                original["startTime"].clone(),
                json!(expected_arn(&format!("build/{retry_id}"))),
            ),
            "case {:?}: the retried build must gain the forward link and nothing else, \
             got {retried}",
            case.name
        );

        // Both builds are the project's, newest first.
        let (_, listed) = call(
            &router,
            "ListBuildsForProject",
            json!({"projectName": case.project}),
        )
        .await;
        assert_eq!(
            listed["ids"],
            json!([retry_id, original_id]),
            "case {:?}: the retry belongs to the project's build list, got {listed}",
            case.name
        );
    }
}

/// RULE: the link is a chain — retrying a retry increments the retry count and points
/// each build at its neighbours, however deep the chain goes.
#[tokio::test]
async fn retrying_a_retry_extends_the_chain() {
    let router = emulator();
    create_project(&router, "retry-chain", json!({"autoRetryLimit": 1})).await;
    let first = start_build(&router, "retry-chain", json!({})).await;

    // Deliberately past `autoRetryLimit`: the limit bounds the retries CodeBuild
    // makes on its own, and a caller asking explicitly is not counted against it.
    let depth = 4;
    let mut chain = vec![first["id"].as_str().expect("an id").to_string()];
    for step in 1..=depth {
        let previous_id = chain.last().expect("the chain is never empty").clone();
        let retry = retry_build(&router, json!({"id": previous_id.clone()})).await;
        let retry_id = retry["id"].as_str().expect("an id").to_string();
        assert_eq!(
            (
                retry["autoRetryConfig"]["autoRetryNumber"].as_i64(),
                retry["buildNumber"].as_i64(),
            ),
            (Some(step), Some(step + 1)),
            "step {step}: the retry count and the build number both advance by one, got {retry}"
        );
        assert_eq!(
            retry["autoRetryConfig"]["previousAutoRetry"],
            json!(expected_arn(&format!("build/{previous_id}"))),
            "step {step}: the retry must point back at the build it retries, got {retry}"
        );
        assert_eq!(
            retry["autoRetryConfig"]["autoRetryLimit"].as_i64(),
            Some(1),
            "step {step}: the limit is carried along the chain, got {retry}"
        );
        chain.push(retry_id);
    }

    // Every link holds in the persisted records: each build points forward at its
    // successor and back at its predecessor, and the last one points nowhere.
    for (position, id) in chain.iter().enumerate() {
        let record = stored_build(&router, id).await;
        let expected_next = chain
            .get(position + 1)
            .map(|next| json!(expected_arn(&format!("build/{next}"))))
            .unwrap_or(json!(null));
        let expected_previous = position
            .checked_sub(1)
            .and_then(|before| chain.get(before))
            .map(|previous| json!(expected_arn(&format!("build/{previous}"))))
            .unwrap_or(json!(null));
        assert_eq!(
            (
                record["autoRetryConfig"]["nextAutoRetry"].clone(),
                record["autoRetryConfig"]["previousAutoRetry"].clone(),
            ),
            (expected_next, expected_previous),
            "link {position}: unexpected neighbours in the persisted chain, got {record}"
        );
    }

    let (_, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "retry-chain"}),
    )
    .await;
    let mut newest_first = chain.clone();
    newest_first.reverse();
    assert_eq!(
        listed["ids"],
        json!(newest_first),
        "every build of the chain belongs to the project's list, newest first, got {listed}"
    );
}

/// RULE: a retry needs a build to restart and a project to restart it under; without
/// either, the request is refused and no build is persisted.
#[tokio::test]
async fn a_retry_with_nothing_to_restart_is_refused() {
    let router = emulator();
    create_project(&router, "retry-refused", json!({})).await;
    create_project(&router, "retry-orphan", json!({})).await;
    let live = start_build(&router, "retry-refused", json!({})).await;
    let orphan = start_build(&router, "retry-orphan", json!({})).await;
    let orphan_id = orphan["id"].as_str().expect("an id").to_string();
    // The project goes, the build stays: AWS keeps a deleted project's build history.
    let (status, deleted) = call(&router, "DeleteProject", json!({"name": "retry-orphan"})).await;
    assert_eq!(
        status, 200,
        "DeleteProject: expected 200, got {status} with {deleted}"
    );

    let cases: [(&str, Value, &str); 5] = [
        (
            "no id at all",
            json!({}),
            "InvalidInputException",
        ),
        (
            "an id naming no stored build",
            json!({"id": "retry-refused:00000000-0000-0000-0000-000000000000"}),
            "ResourceNotFoundException",
        ),
        (
            "a project name used as a build id",
            json!({"id": "retry-refused"}),
            "ResourceNotFoundException",
        ),
        (
            "the id of another account's ARN form",
            json!({"id": expected_arn(&format!("build/{}", live["id"].as_str().unwrap_or_default()))}),
            "ResourceNotFoundException",
        ),
        (
            "a build whose project was deleted",
            json!({"id": orphan_id}),
            "ResourceNotFoundException",
        ),
    ];

    for (name, body, expected) in &cases {
        let (status, response) = call(&router, "RetryBuild", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} with {response}"
        );
        assert!(
            error_type(&response).contains(expected),
            "case {name:?}: expected {expected}, got {response}"
        );
    }

    // Nothing was started: the project still holds only the build it began with.
    let (_, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "retry-refused"}),
    )
    .await;
    assert_eq!(
        listed["ids"],
        json!([live["id"].clone()]),
        "a refused retry must persist no build, got {listed}"
    );
}

/// RULE: a `RetryBuild` idempotency token in force resolves to the build the first
/// request with it started (AWS: "valid for five minutes"); the same token with a
/// changed parameter is a parameter mismatch, not a second build; and a token is
/// scoped to the operation that issued it, so `StartBuild` and `RetryBuild` never
/// resolve each other's builds.
#[tokio::test]
async fn a_retry_idempotency_token_resolves_to_the_build_it_started() {
    let router = emulator();
    create_project(&router, "retry-idem", json!({})).await;
    let first = start_build(&router, "retry-idem", json!({"idempotencyToken": "shared"})).await;
    let first_id = first["id"].as_str().expect("an id").to_string();
    let second = start_build(&router, "retry-idem", json!({})).await;
    let second_id = second["id"].as_str().expect("an id").to_string();

    // A token StartBuild issued does not resolve a retry: the retry is a new build.
    let retried = retry_build(
        &router,
        json!({"id": first_id.clone(), "idempotencyToken": "shared"}),
    )
    .await;
    let retried_id = retried["id"].as_str().expect("an id").to_string();
    assert_eq!(
        (retried_id != first_id, retried["buildNumber"].as_i64()),
        (true, Some(3)),
        "a StartBuild token must not resolve a RetryBuild, got {retried}"
    );

    // The same RetryBuild request with that token replays to the build it started.
    let replay = retry_build(
        &router,
        json!({"id": first_id.clone(), "idempotencyToken": "shared"}),
    )
    .await;
    assert_eq!(
        (replay["id"].as_str(), replay["buildNumber"].as_i64()),
        (Some(retried_id.as_str()), Some(3)),
        "a replayed token must resolve to the same build and consume no build number, \
         got {replay}"
    );

    // The same token with a changed parameter is refused.
    let (status, mismatched) = call(
        &router,
        "RetryBuild",
        json!({"id": second_id.clone(), "idempotencyToken": "shared"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a retry token reused with changed parameters: expected 400, got {status} with \
         {mismatched}"
    );
    assert!(
        error_type(&mismatched).contains("InvalidInputException"),
        "a retry token reused with changed parameters: expected InvalidInputException, \
         got {mismatched}"
    );

    // A different token is a different build.
    let other = retry_build(
        &router,
        json!({"id": first_id.clone(), "idempotencyToken": "another"}),
    )
    .await;
    assert_eq!(
        other["buildNumber"].as_i64(),
        Some(4),
        "a new token starts the next build, got {other}"
    );

    // Four builds exist: two started, two retried — the replay and the refusal added
    // nothing.
    let (_, listed) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "retry-idem"}),
    )
    .await;
    assert_eq!(
        listed["ids"].as_array().map(Vec::len),
        Some(4),
        "the replay and the refusal must not have started a build, got {listed}"
    );
}
