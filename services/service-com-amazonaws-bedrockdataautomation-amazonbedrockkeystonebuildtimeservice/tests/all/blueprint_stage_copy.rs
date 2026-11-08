// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CopyBlueprintStage`, and of the per-stage content a
//! blueprint holds that it copies between.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation,
//! the handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The suites are TABLE-driven: the copy table spans EVERY ordered pair of the
//! modelled stages (`smithy_query(shape="BlueprintStage")` -> `["DEVELOPMENT",
//! "LIVE"]`), crossed with schema size and with the presence of an
//! `@idempotencyToken`, rather than asserting one recorded request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`). `can_handle` resolves the route key from it, and the handlers
/// read the region out of its credential scope when minting an ARN.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef"
        .to_string()
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Dispatch one request and return its status and decoded JSON body (`Value::Null`
/// for an empty body).
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateBlueprint` at one stage, per its modelled `@http`
/// `{method: PUT, uri: /blueprints/}` -> the created `blueprint` object.
async fn create_blueprint(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    stage: &str,
    schema: &str,
) -> Value {
    let body = json!({
        "blueprintName": name,
        "type": "DOCUMENT",
        "schema": schema,
        "blueprintStage": stage,
    });
    let (status, payload) = call(router, "PUT", "/blueprints/", &body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: CreateBlueprint returned no blueprint: {payload:?}"))
}

/// `CopyBlueprintStage`, per its modelled `@http`
/// `{method: PUT, uri: /blueprints/{blueprintArn}/copy-stage}`.
async fn copy_stage(
    router: &MultiServiceRouter,
    arn: &str,
    source: &str,
    target: &str,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "sourceStage": source, "targetStage": target });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    let uri = format!("/blueprints/{}/copy-stage", encode_label(arn));
    call(router, "PUT", &uri, &body).await
}

/// `GetBlueprint` scoped to one stage, or to none when `stage` is `None`.
async fn get_blueprint(
    router: &MultiServiceRouter,
    arn: &str,
    stage: Option<&str>,
) -> (u16, Value) {
    let body = match stage {
        Some(stage) => json!({ "blueprintStage": stage }),
        None => json!({}),
    };
    let uri = format!("/blueprints/{}/", encode_label(arn));
    call(router, "POST", &uri, &body).await
}

/// `UpdateBlueprint`, which writes a schema AT A STAGE.
async fn update_blueprint(
    router: &MultiServiceRouter,
    arn: &str,
    schema: &str,
    stage: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "schema": schema });
    if let Some(stage) = stage {
        body["blueprintStage"] = json!(stage);
    }
    let uri = format!("/blueprints/{}/", encode_label(arn));
    call(router, "PUT", &uri, &body).await
}

/// The ARNs one `ListBlueprints` page reports under a `blueprintStageFilter`.
async fn listed_arns(router: &MultiServiceRouter, case: &str, filter: &str) -> Vec<String> {
    let (status, payload) = call(
        router,
        "POST",
        "/blueprints/",
        &json!({ "blueprintStageFilter": filter }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListBlueprints 200 for filter {filter:?}, \
         got {status} with {payload:?}"
    );
    payload["blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListBlueprints served no page: {payload:?}"))
        .iter()
        .map(|item| string_at(item, "blueprintArn", case))
        .collect()
}

/// The blueprint read back at `stage`, or a panic naming the case.
async fn blueprint_at(router: &MultiServiceRouter, case: &str, arn: &str, stage: &str) -> Value {
    let (status, payload) = get_blueprint(router, arn, Some(stage)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBlueprint 200 at stage {stage:?}, \
         got {status} with {payload:?}"
    );
    payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetBlueprint returned no blueprint: {payload:?}"))
}

/// The two modelled stages, from `smithy_query(shape="BlueprintStage")` ->
/// `["DEVELOPMENT", "LIVE"]`.
const STAGES: [&str; 2] = ["DEVELOPMENT", "LIVE"];

/// The other of the two modelled stages.
fn other_stage(stage: &str) -> &'static str {
    if stage == STAGES[0] {
        STAGES[1]
    } else {
        STAGES[0]
    }
}

/// A schema of roughly `fields` members, so a case can copy a big schema as well as
/// a small one. Well inside the modelled `BlueprintSchema` bound (len 1..100000).
fn schema_of(fields: usize) -> String {
    let members: Vec<String> = (0..fields)
        .map(|i| format!("\"field{i}\":{{\"type\":\"string\"}}"))
        .collect();
    format!("{{\"class\":\"invoice\",{}}}", members.join(","))
}

/// A copy places the content stored at `sourceStage` at `targetStage` too: both
/// stages then hold that schema, the source keeps its own `lastModifiedTime`, the
/// target is the stage an unscoped read reports, and either stage's filter lists the
/// blueprint. Copying a stage onto itself writes nothing.
///
/// The table spans every ordered pair of the two modelled stages, crossed with
/// schema size and with the presence of a `clientToken`.
#[tokio::test]
async fn copy_blueprint_stage_places_the_source_stages_content_at_the_target_stage() {
    struct Case {
        name: &'static str,
        source: &'static str,
        target: &'static str,
        fields: usize,
        token: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "promote-development-to-live",
            source: "DEVELOPMENT",
            target: "LIVE",
            fields: 1,
            token: None,
        },
        Case {
            name: "demote-live-to-development-big-schema",
            source: "LIVE",
            target: "DEVELOPMENT",
            fields: 120,
            token: None,
        },
        Case {
            name: "promote-with-client-token",
            source: "DEVELOPMENT",
            target: "LIVE",
            fields: 3,
            // 33 characters, the modelled `ClientToken` minimum.
            token: Some("copyblueprintstagetoken0123456789"),
        },
        Case {
            name: "development-onto-itself",
            source: "DEVELOPMENT",
            target: "DEVELOPMENT",
            fields: 2,
            token: None,
        },
        Case {
            name: "live-onto-itself",
            source: "LIVE",
            target: "LIVE",
            fields: 2,
            token: Some("liveontoitselfclienttoken01234567"),
        },
    ];

    for case in cases {
        let name = case.name;
        let router = emulator();
        let schema = schema_of(case.fields);
        // The blueprint starts out at its SOURCE stage, which is what the copy reads.
        let created = create_blueprint(&router, name, name, case.source, &schema).await;
        let arn = string_at(&created, "blueprintArn", name);
        let created_at = timestamp_at(&created, "creationTime", name);

        // Real elapsed time before the copy, so a bumped lastModifiedTime is a
        // genuinely later instant rather than a fabricated one.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let (status, payload) = copy_stage(&router, &arn, case.source, case.target, case.token).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CopyBlueprintStage 200, got {status} with {payload:?}"
        );
        // `CopyBlueprintStageResponse` models no members, so the answer carries none.
        assert!(
            matches!(&payload, Value::Null) || payload.as_object().is_some_and(serde_json::Map::is_empty),
            "case {name:?}: CopyBlueprintStage must answer an empty body, got {payload:?}"
        );

        // The target stage now holds the source stage's schema...
        let at_target = blueprint_at(&router, name, &arn, case.target).await;
        assert_eq!(
            string_at(&at_target, "schema", name),
            schema,
            "case {name:?}: stage {:?} must hold the schema copied from {:?}, got {at_target:?}",
            case.target,
            case.source
        );
        assert_eq!(
            string_at(&at_target, "blueprintStage", name),
            case.target,
            "case {name:?}: a read scoped to {:?} must report that stage, got {at_target:?}",
            case.target
        );

        // ...and the source stage keeps holding its own.
        let at_source = blueprint_at(&router, name, &arn, case.source).await;
        assert_eq!(
            string_at(&at_source, "schema", name),
            schema,
            "case {name:?}: a copy must leave stage {:?} holding its own schema, got {at_source:?}",
            case.source
        );
        assert_eq!(
            string_at(&at_source, "blueprintStage", name),
            case.source,
            "case {name:?}: a read scoped to {:?} must report that stage, got {at_source:?}",
            case.source
        );
        assert_eq!(
            timestamp_at(&at_source, "creationTime", name),
            created_at,
            "case {name:?}: a copy must not move creationTime: {at_source:?}"
        );

        let copied_onto_itself = case.source == case.target;
        if copied_onto_itself {
            // Nothing was written, so the stage keeps the instant of its create and
            // the blueprint still exists at that one stage only.
            assert_eq!(
                timestamp_at(&at_source, "lastModifiedTime", name),
                created_at,
                "case {name:?}: copying a stage onto itself must write nothing: {at_source:?}"
            );
            let absent = other_stage(case.source);
            let (status, payload) = get_blueprint(&router, &arn, Some(absent)).await;
            assert_eq!(
                status, 404,
                "case {name:?}: the blueprint must exist at no stage {absent:?}, \
                 got {status} with {payload:?}"
            );
        } else {
            assert_eq!(
                timestamp_at(&at_source, "lastModifiedTime", name),
                created_at,
                "case {name:?}: a copy must not move stage {:?}'s lastModifiedTime: {at_source:?}",
                case.source
            );
            assert!(
                timestamp_at(&at_target, "lastModifiedTime", name) > created_at,
                "case {name:?}: stage {:?} was written by the copy, so its \
                 lastModifiedTime must be after {created_at:?}, got {:?}",
                case.target,
                timestamp_at(&at_target, "lastModifiedTime", name)
            );
        }

        // An unscoped read reports the stage the blueprint was last written at, which
        // the copy made the target stage.
        let (status, payload) = get_blueprint(&router, &arn, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected an unscoped GetBlueprint 200, got {status} with {payload:?}"
        );
        let unscoped = payload.get("blueprint").expect("a blueprint");
        assert_eq!(
            string_at(unscoped, "blueprintStage", name),
            case.target,
            "case {name:?}: an unscoped read must report the stage last written: {unscoped:?}"
        );

        // A stage filter lists the blueprint at every stage it exists at, and `ALL`
        // lists it exactly once however many stages that is.
        for stage in STAGES {
            let listed = listed_arns(&router, name, stage).await;
            let expected = stage == case.source || stage == case.target;
            assert_eq!(
                listed.contains(&arn),
                expected,
                "case {name:?}: filter {stage:?} must {} the blueprint, got {listed:?}",
                if expected { "list" } else { "not list" }
            );
        }
        let all = listed_arns(&router, name, "ALL").await;
        assert_eq!(
            all.iter().filter(|listed| **listed == arn).count(),
            1,
            "case {name:?}: the ALL filter must list the blueprint once, got {all:?}"
        );
    }
}

/// A copy whose SOURCE stage holds nothing names a resource that does not exist: the
/// modelled `ResourceNotFoundException` (HTTP 404) with this service's fixed
/// sentence, and nothing written.
#[tokio::test]
async fn copy_blueprint_stage_refuses_a_source_stage_that_holds_nothing() {
    // A well-formed `BlueprintArn` (len ..128, pattern from
    // `smithy_query(operation="CopyBlueprintStage")`) that this store never minted.
    const ABSENT_ARN: &str = "arn:aws:bedrock:us-east-1:123456789012:blueprint/0123456789ab";

    // (case, the stage the blueprint is created at — `None` for a blueprint that does
    // not exist at all, source, target)
    let cases: &[(&str, Option<&str>, &str, &str)] = &[
        ("live-source-of-a-development-blueprint", Some("DEVELOPMENT"), "LIVE", "DEVELOPMENT"),
        ("development-source-of-a-live-blueprint", Some("LIVE"), "DEVELOPMENT", "LIVE"),
        ("no-such-blueprint-promote", None, "DEVELOPMENT", "LIVE"),
        ("no-such-blueprint-demote", None, "LIVE", "DEVELOPMENT"),
    ];

    for (case, created_stage, source, target) in cases {
        let router = emulator();
        let schema = schema_of(1);
        let arn = match created_stage {
            Some(stage) => {
                let created = create_blueprint(&router, case, case, stage, &schema).await;
                string_at(&created, "blueprintArn", case)
            }
            None => ABSENT_ARN.to_string(),
        };

        let (status, payload) = copy_stage(&router, &arn, source, target, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a copy from a stage holding nothing must be 404, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some("Blueprint Arn not found."),
            "case {case:?}: wrong refusal wording in {payload:?}"
        );

        // The refusal wrote nothing: the blueprint still exists at its own stage
        // only, and at none at all when there was none to begin with.
        if let Some(stage) = created_stage {
            let at_stage = blueprint_at(&router, case, &arn, stage).await;
            assert_eq!(
                string_at(&at_stage, "schema", case),
                schema,
                "case {case:?}: a refused copy must leave stage {stage:?} alone: {at_stage:?}"
            );
            let (status, payload) = get_blueprint(&router, &arn, Some(other_stage(stage))).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused copy must place nothing at {:?}, \
                 got {status} with {payload:?}",
                other_stage(stage)
            );
        }
    }
}

/// A write names ONE stage, so each stage's schema and `lastModifiedTime` move only
/// when that stage is written — and an `@idempotencyToken` replay of a copy is a
/// no-op rather than a second copy of whatever the source holds by then.
#[tokio::test]
async fn a_stages_content_moves_only_when_that_stage_is_written() {
    // 33 characters, the modelled `ClientToken` minimum.
    const TOKEN: &str = "stagecopyreplaytoken012345678901x";

    for first_stage in STAGES {
        let case = format!("first-stage={first_stage}");
        let second_stage = other_stage(first_stage);
        let router = emulator();

        let first_schema = schema_of(1);
        let created = create_blueprint(
            &router,
            &case,
            &format!("stage-writes-{first_stage}"),
            first_stage,
            &first_schema,
        )
        .await;
        let arn = string_at(&created, "blueprintArn", &case);

        // Copy it to the other stage, under a client token.
        let (status, payload) = copy_stage(&router, &arn, first_stage, second_stage, Some(TOKEN)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CopyBlueprintStage 200, got {status} with {payload:?}"
        );

        // Rewrite the FIRST stage only: the second stage keeps the copy it holds.
        let second_schema = schema_of(7);
        let (status, payload) =
            update_blueprint(&router, &arn, &second_schema, Some(first_stage)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBlueprint 200, got {status} with {payload:?}"
        );
        let at_first = blueprint_at(&router, &case, &arn, first_stage).await;
        assert_eq!(
            string_at(&at_first, "schema", &case),
            second_schema,
            "case {case:?}: stage {first_stage:?} must hold the written schema: {at_first:?}"
        );
        let at_second = blueprint_at(&router, &case, &arn, second_stage).await;
        assert_eq!(
            string_at(&at_second, "schema", &case),
            first_schema,
            "case {case:?}: a write at {first_stage:?} must leave {second_stage:?} \
             holding its own schema: {at_second:?}"
        );

        // The SAME token replays that copy: a no-op, so the second stage still holds
        // the schema the first copy placed there.
        let (status, payload) = copy_stage(&router, &arn, first_stage, second_stage, Some(TOKEN)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed copy must be 200, got {status} with {payload:?}"
        );
        let at_second = blueprint_at(&router, &case, &arn, second_stage).await;
        assert_eq!(
            string_at(&at_second, "schema", &case),
            first_schema,
            "case {case:?}: a token replay must copy nothing again: {at_second:?}"
        );

        // A copy under a FRESH token is a new request, so it copies what the source
        // stage holds NOW.
        let (status, payload) = copy_stage(
            &router,
            &arn,
            first_stage,
            second_stage,
            Some("stagecopysecondtoken012345678901x"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fresh-token copy must be 200, got {status} with {payload:?}"
        );
        let at_second = blueprint_at(&router, &case, &arn, second_stage).await;
        assert_eq!(
            string_at(&at_second, "schema", &case),
            second_schema,
            "case {case:?}: stage {second_stage:?} must hold the schema copied now: {at_second:?}"
        );
        let at_first = blueprint_at(&router, &case, &arn, first_stage).await;
        assert_eq!(
            string_at(&at_first, "schema", &case),
            second_schema,
            "case {case:?}: the source stage keeps its own schema: {at_first:?}"
        );
    }
}
