// SPDX-License-Identifier: CC0-1.0

//! Cover for `CreateWebhook`, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test is that a webhook is a record kept against a project whose
//! source provider sends repository events: it is created once per project, computed
//! from the project the request names, and reported by a later read exactly as it was
//! persisted. Every table below spans a catalogue the model declares, not one recorded
//! request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so a URL assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDWEBHOOK";

/// Every `SourceType` the model declares, from
/// `smithy_query(query="shape", shape="SourceType")`, paired with whether a
/// repository-event webhook can be created for a project with that source.
const SOURCE_TYPES: &[(&str, bool)] = &[
    ("BITBUCKET", true),
    ("CODECOMMIT", false),
    ("CODEPIPELINE", false),
    ("GITHUB", true),
    ("GITHUB_ENTERPRISE", true),
    ("GITLAB", true),
    ("GITLAB_SELF_MANAGED", true),
    ("NO_SOURCE", false),
    ("S3", false),
];

/// Every `WebhookBuildType` the model declares, from
/// `smithy_query(query="shape", shape="WebhookBuildType")`, with the source type each
/// one is available for: `RUNNER_BUILDKITE_BUILD` is documented as "only available for
/// NO_SOURCE source type projects".
const BUILD_TYPES: &[(&str, &str)] = &[
    ("BUILD", "GITHUB"),
    ("BUILD_BATCH", "GITHUB"),
    ("RUNNER_BUILDKITE_BUILD", "NO_SOURCE"),
];

/// One router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// As [`emulator`], over an engine the caller already holds — so a test can inspect the
/// service's state through the very store the handlers write.
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

/// Create a project named `name` whose source is `source_type`, asserting it succeeded.
async fn create_project(router: &MultiServiceRouter, name: &str, source_type: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        json!({
            "name": name,
            "source": {"type": source_type, "buildspec": "version: 0.2"},
            "artifacts": {"type": "NO_ARTIFACTS"},
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
            },
            "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?} ({source_type}): expected 200, got {status} with {body}"
    );
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The project a later read reports, through the implemented reader `BatchGetProjects`.
async fn read_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// A created webhook's shape, checked against what the model and the docs say the
/// members mean, for a project whose source is `source_type`.
fn assert_webhook_shape(case: &str, webhook: &Value, source_type: &str) {
    let payload_url = webhook["payloadUrl"].as_str().unwrap_or_default();
    assert!(
        payload_url.starts_with(&format!("https://codebuild.{REGION}.amazonaws.com/webhooks?t="))
            && payload_url.ends_with("&v=1"),
        "{case}: expected a regional CodeBuild payload URL, got {payload_url:?}"
    );
    assert!(
        webhook["url"].is_null(),
        "{case}: no hook is registered with the repository provider, so url must be \
         absent, got {:?}",
        webhook["url"]
    );
    assert_eq!(
        webhook["status"].as_str(),
        Some("ACTIVE"),
        "{case}: expected the persisted webhook to report ACTIVE, got {:?}",
        webhook["status"]
    );
    // AWS documents it on `Webhook$secret`: "A Bitbucket webhook does not support
    // secret." Every other provider's webhook carries one, and `lastModifiedSecret`
    // reports when that secret last changed, so the two members travel together.
    let expects_secret = source_type != "BITBUCKET";
    assert_eq!(
        webhook["secret"].is_string(),
        expects_secret,
        "{case}: expected secret present={expects_secret}, got {:?}",
        webhook["secret"]
    );
    assert_eq!(
        !webhook["lastModifiedSecret"].is_null(),
        expects_secret,
        "{case}: expected lastModifiedSecret present={expects_secret}, got {:?}",
        webhook["lastModifiedSecret"]
    );
}

/// The whole catalogue of source types: a webhook can be created exactly for the
/// providers that send repository events, and the created record is what a later read
/// reports.
#[tokio::test]
async fn create_webhook_spans_every_source_type() {
    let router = emulator();

    for (source_type, supported) in SOURCE_TYPES {
        let case = format!("source type {source_type}");
        let project = format!("webhook-{}", source_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;

        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "branchFilter": "^refs/heads/main$"}),
        )
        .await;

        if !supported {
            assert_eq!(
                status, 400,
                "{case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_type(&body).contains("InvalidInputException"),
                "{case}: expected InvalidInputException, got {body}"
            );
            assert!(
                read_project(&router, &project).await["webhook"].is_null(),
                "{case}: a refused CreateWebhook must persist nothing"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        let webhook = body["webhook"].clone();
        assert_webhook_shape(&case, &webhook, source_type);
        assert_eq!(
            webhook["branchFilter"].as_str(),
            Some("^refs/heads/main$"),
            "{case}: expected the request's branchFilter, got {webhook}"
        );

        // Round trip: the project reports exactly the webhook that was created, and
        // reads do not recompute it.
        let stored = read_project(&router, &project).await["webhook"].clone();
        assert_eq!(
            stored, webhook,
            "{case}: BatchGetProjects must report the created webhook, got {stored}"
        );
        let again = read_project(&router, &project).await["webhook"].clone();
        assert_eq!(
            again, webhook,
            "{case}: a second read must report the same webhook, got {again}"
        );
    }

    // Each project's payload URL and secret are its own.
    let first = read_project(&router, "webhook-github").await["webhook"].clone();
    let second = read_project(&router, "webhook-gitlab").await["webhook"].clone();
    assert_ne!(
        first["payloadUrl"], second["payloadUrl"],
        "two projects must not share a payload URL: {first} vs {second}"
    );
    assert_ne!(
        first["secret"], second["secret"],
        "two projects must not share a webhook secret"
    );
}

/// A project takes at most one webhook: the second request is refused and changes
/// nothing.
#[tokio::test]
async fn a_second_webhook_for_one_project_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "webhook-twice", "GITHUB").await;

    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "webhook-twice"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "first CreateWebhook: expected 200, got {status} with {body}"
    );
    let created = body["webhook"].clone();

    // A repeat differing in its parameters is still a second webhook for the same
    // project, which is what makes it a conflict.
    for (case, request) in [
        ("an identical repeat", json!({"projectName": "webhook-twice"})),
        (
            "a repeat with other parameters",
            json!({
                "projectName": "webhook-twice",
                "branchFilter": "^refs/heads/release$",
                "buildType": "BUILD_BATCH",
            }),
        ),
    ] {
        let (status, body) = call(&router, "CreateWebhook", request).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceAlreadyExistsException"),
            "{case}: expected ResourceAlreadyExistsException, got {body}"
        );
    }

    let stored = read_project(&router, "webhook-twice").await["webhook"].clone();
    assert_eq!(
        stored, created,
        "a refused repeat must leave the first webhook untouched, got {stored}"
    );
    // And the store holds the webhook on the project itself, not a second copy.
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    let project = guard
        .projects
        .get("webhook-twice")
        .expect("the project is persisted");
    assert_eq!(
        project.project.webhook.as_ref().and_then(|w| w.secret.clone()),
        created["secret"].as_str().map(str::to_string),
        "the persisted webhook must carry the secret the caller was given"
    );
}

/// No project, no webhook.
#[tokio::test]
async fn create_webhook_for_an_unknown_project_is_refused() {
    let router = emulator();
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "no-such-project"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "CreateWebhook on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "CreateWebhook on an unknown project: expected ResourceNotFoundException, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no-such-project"),
        "the refusal must name the project that was not found, got {body}"
    );
}

/// Every `WebhookBuildType`, each against the source type it is available for and one
/// it is not: a runner webhook needs a `NO_SOURCE` project, the repository-event build
/// types need a repository.
#[tokio::test]
async fn build_type_must_match_the_project_source() {
    let router = emulator();

    for (build_type, source_type) in BUILD_TYPES {
        let case = format!("buildType {build_type} on {source_type}");
        let project = format!("bt-ok-{}", build_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "buildType": build_type}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["webhook"]["buildType"].as_str(),
            Some(*build_type),
            "{case}: expected the request's buildType, got {body}"
        );
        assert_webhook_shape(&case, &body["webhook"], source_type);

        // The same build type against a source it is not available for.
        let other_source = if *source_type == "NO_SOURCE" {
            "GITHUB"
        } else {
            "NO_SOURCE"
        };
        let case = format!("buildType {build_type} on {other_source}");
        let project = format!("bt-bad-{}", build_type.to_ascii_lowercase());
        create_project(&router, &project, other_source).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "buildType": build_type}),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
    }
}

/// `filterGroups`: a group has to carry an `EVENT` filter to be satisfiable, and the
/// groups a caller gives are persisted as given.
#[tokio::test]
async fn filter_groups_need_an_event_filter() {
    let router = emulator();

    // Accepted: one group, several groups, and a group combining EVENT with another
    // filter type — the filter types come from the model's `WebhookFilterType`.
    let accepted: &[(&str, Value)] = &[
        (
            "one EVENT group",
            json!([[{"type": "EVENT", "pattern": "PUSH"}]]),
        ),
        (
            "two EVENT groups",
            json!([
                [{"type": "EVENT", "pattern": "PUSH"}],
                [{"type": "EVENT", "pattern": "PULL_REQUEST_CREATED,PULL_REQUEST_UPDATED"}],
            ]),
        ),
        (
            "EVENT with a HEAD_REF filter",
            json!([[
                {"type": "EVENT", "pattern": "PUSH"},
                {"type": "HEAD_REF", "pattern": "^refs/heads/main$", "excludeMatchedPattern": true},
            ]]),
        ),
    ];
    for (index, (case, filter_groups)) in accepted.iter().enumerate() {
        let project = format!("fg-ok-{index}");
        create_project(&router, &project, "GITHUB").await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": filter_groups}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body["webhook"]["filterGroups"], filter_groups,
            "{case}: expected the request's filterGroups, got {body}"
        );
        assert_eq!(
            read_project(&router, &project).await["webhook"]["filterGroups"],
            *filter_groups,
            "{case}: a later read must report the stored filterGroups"
        );
    }

    // Refused: any group without an EVENT filter, wherever it sits in the array.
    let refused: &[(&str, Value)] = &[
        (
            "a lone HEAD_REF group",
            json!([[{"type": "HEAD_REF", "pattern": "^refs/heads/main$"}]]),
        ),
        (
            "an empty group",
            json!([[]]),
        ),
        (
            "a second group without EVENT",
            json!([
                [{"type": "EVENT", "pattern": "PUSH"}],
                [{"type": "FILE_PATH", "pattern": "^src/"}],
            ]),
        ),
    ];
    for (index, (case, filter_groups)) in refused.iter().enumerate() {
        let project = format!("fg-bad-{index}");
        create_project(&router, &project, "GITHUB").await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": filter_groups}),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("filterGroups"),
            "{case}: the refusal must name the member it is about, got {body}"
        );
        assert!(
            read_project(&router, &project).await["webhook"].is_null(),
            "{case}: a refused CreateWebhook must persist nothing"
        );
    }
}

/// `manualCreation` is "only available for GitHub webhooks": accepted there, refused
/// for every other provider that takes a webhook at all.
#[tokio::test]
async fn manual_creation_is_github_only() {
    let router = emulator();

    for (source_type, supported) in SOURCE_TYPES.iter().filter(|(_, supported)| *supported) {
        let case = format!("manualCreation on {source_type}");
        let project = format!("mc-{}", source_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "manualCreation": true}),
        )
        .await;
        let _ = supported;
        if *source_type == "GITHUB" {
            assert_eq!(
                status, 200,
                "{case}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["webhook"]["manualCreation"].as_bool(),
                Some(true),
                "{case}: expected the request's manualCreation, got {body}"
            );
            // A manually created webhook is exactly the pair of values the caller
            // registers by hand with the provider.
            assert!(
                body["webhook"]["payloadUrl"].is_string() && body["webhook"]["secret"].is_string(),
                "{case}: expected payloadUrl and secret, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "{case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_type(&body).contains("InvalidInputException"),
                "{case}: expected InvalidInputException, got {body}"
            );
        }
    }
}

/// The webhook belongs to the project, so connecting one is a change to the project's
/// settings — which is what `Project$lastModified` reports.
#[tokio::test]
async fn creating_a_webhook_touches_the_projects_last_modified() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "webhook-stamped", "GITHUB").await;

    let created_at = engine
        .state::<CodeBuildState>()
        .lock()
        .projects
        .get("webhook-stamped")
        .map(|stored| (stored.created_millis, stored.last_modified_millis))
        .expect("the project is persisted");
    assert_eq!(
        created_at.0, created_at.1,
        "a freshly created project was last modified when it was created"
    );

    // Rewind the stored instants, so the assertion below is about the write and not
    // about how fast the test runs.
    {
        let state = engine.state::<CodeBuildState>();
        let mut guard = state.lock();
        let stored = guard
            .projects
            .get_mut("webhook-stamped")
            .expect("the project is persisted");
        stored.created_millis -= 60_000;
        stored.last_modified_millis -= 60_000;
        stored.project.created = Some(state::datetime(stored.created_millis));
        stored.project.last_modified = Some(state::datetime(stored.last_modified_millis));
    }

    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "webhook-stamped"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWebhook: expected 200, got {status} with {body}"
    );

    let guard = engine.state::<CodeBuildState>();
    let guard = guard.lock();
    let stored = guard
        .projects
        .get("webhook-stamped")
        .expect("the project is persisted");
    assert!(
        stored.last_modified_millis > stored.created_millis,
        "expected lastModified to move past created, got {} vs {}",
        stored.last_modified_millis,
        stored.created_millis
    );
    assert_eq!(
        stored.project.last_modified.map(|at| at.to_millis().unwrap_or_default()),
        Some(stored.last_modified_millis),
        "the reported lastModified must be the instant the store holds"
    );
}
