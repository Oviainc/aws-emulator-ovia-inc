// SPDX-License-Identifier: CC0-1.0

//! The model-side contract of `StartAsyncInvoke`, end to end over the router.
//!
//! Three rules, each asserted over a SET of inputs rather than one request:
//!
//! 1. **Which models it serves.** Asynchronous invocation is published per
//!    model, so the operation serves exactly the models the inventory marks
//!    [`state::ModelEntry::supports_async_invoke`] — the video generation models
//!    — and refuses every other RECOGNISED model with `ValidationException`
//!    (400). The sweep below walks the WHOLE inventory, so neither half of the
//!    rule can be satisfied by special-casing one id.
//! 2. **What it accepts as `modelInput`.** The provider validates `modelInput`
//!    against the invoked model's own published request schema and refuses a
//!    body outside it with `ValidationException` (400) naming the offending
//!    member, before any job is minted.
//! 3. **How a boundary constraint refusal is worded.** A `@pattern` requirement
//!    is quoted WITHOUT the anchors a whole-value pattern wraps it in, for every
//!    member of this service that carries one.
//!
//! Every assertion names its case and the actual value.

use serde_json::json;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState as ServiceState;

// The harness every suite of this target shares: the router over one shared
// engine, the HTTP helpers, and the `InvocationArn` pattern check.
use crate::bedrock_runtime_inference::{
    emulator, encode_path, get, is_invocation_arn, post_json, Call,
};

/// A valid output destination, so a row never fails on the destination instead
/// of on the model or the model input under test.
fn destination() -> serde_json::Value {
    json!({ "s3OutputDataConfig": { "s3Uri": "s3://emulator-bucket/async-out" } })
}

/// A `modelInput` the named model's published request schema accepts, or `None`
/// for a model this crate publishes no schema for.
///
/// Members and values from the request tables in the Amazon Bedrock User Guide:
/// "Amazon Nova Reel" (`taskType`, `textToVideoParams.text`,
/// `videoGenerationConfig`) and "Luma AI Ray v2" (`prompt`, `aspect_ratio`,
/// `loop`, `duration`, `resolution`).
fn valid_model_input(model_id: &str) -> Option<serde_json::Value> {
    match model_id {
        "amazon.nova-reel-v1:0" | "amazon.nova-reel-v1:1" => Some(json!({
            "taskType": "TEXT_VIDEO",
            "textToVideoParams": { "text": "a kite over a lake at dawn" },
            "videoGenerationConfig": {
                "durationSeconds": 6,
                "fps": 24,
                "dimension": "1280x720",
                "seed": 7,
            },
        })),
        "luma.ray-v2:0" => Some(json!({
            "prompt": "a kite over a lake at dawn",
            "aspect_ratio": "16:9",
            "loop": false,
            "duration": "9s",
            "resolution": "720p",
        })),
        _ => None,
    }
}

/// A `StartAsyncInvoke` request body for `model_id` carrying `model_input`.
fn start_request(model_id: &str, model_input: serde_json::Value) -> serde_json::Value {
    json!({
        "modelId": model_id,
        "modelInput": model_input,
        "outputDataConfig": destination(),
    })
}

/// The `invocationArn`s `ListAsyncInvokes` reports, which is what says whether a
/// refused request minted a job.
async fn listed(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let call = get(router, "/async-invoke").await;
    assert_eq!(
        call.status,
        200,
        "case \"list every invocation\": expected 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["asyncInvokeSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `message` member of a modelled refusal.
fn message_of(call: &Call) -> String {
    call.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: `StartAsyncInvoke` serves exactly the models asynchronous invocation is
/// published for. Every other model the inventory RECOGNISES is refused with
/// `ValidationException` (400) — never admitted to the async pipeline — a
/// retired model is still refused on its lifecycle (404), and a refused request
/// mints no job.
///
/// The table is the whole seeded inventory, so the rule is asserted over every
/// model id this service knows rather than over the ones an issue named.
#[tokio::test]
async fn start_async_invoke_serves_exactly_the_models_asynchronous_invocation_is_published_for() {
    let router = emulator();
    let inventory = ServiceState::default().models;
    let mut accepted: Vec<String> = Vec::new();
    let mut async_capable = 0_usize;

    for (model_id, entry) in &inventory {
        let input = valid_model_input(model_id).unwrap_or_else(|| json!({ "prompt": "a kite" }));
        let call = post_json(&router, "/async-invoke", &start_request(model_id, input)).await;

        if entry.lifecycle == state::ModelLifecycle::EndOfLife {
            // Resolution refuses a retired model before any capability gate:
            // the identifier names a resource that is gone.
            assert_eq!(
                call.status,
                404,
                "case {model_id:?} (retired): expected 404, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ResourceNotFoundException",
                "case {model_id:?} (retired): expected ResourceNotFoundException, got {:?}",
                call.error_code()
            );
            continue;
        }

        if entry.supports_async_invoke {
            async_capable += 1;
            assert_eq!(
                call.status,
                200,
                "case {model_id:?} (async-capable): expected 200, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let arn = call.json()["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_invocation_arn(&arn),
                "case {model_id:?} (async-capable): expected a modelled invocationArn, got {arn:?}"
            );

            // Round trip: the minted record is what the reader reports back.
            let fetched = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
            assert_eq!(
                fetched.status,
                200,
                "case {model_id:?}: expected GetAsyncInvoke to answer 200, got {} / {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            let record = fetched.json();
            assert_eq!(
                record["modelArn"].as_str(),
                Some(
                    format!("arn:aws:bedrock:us-east-1:000000000000:foundation-model/{model_id}")
                        .as_str()
                ),
                "case {model_id:?}: expected the modelArn of the submitted model, got {:?}",
                record["modelArn"]
            );
            assert_eq!(
                record["status"].as_str(),
                Some("InProgress"),
                "case {model_id:?}: expected a freshly submitted job to be InProgress, got {:?}",
                record["status"]
            );
            accepted.push(arn);
        } else {
            assert_eq!(
                call.status,
                400,
                "case {model_id:?} (not async-capable): expected 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {model_id:?} (not async-capable): expected ValidationException, got {:?}",
                call.error_code()
            );
            assert_eq!(
                message_of(&call),
                state::ASYNC_INVOKE_UNSUPPORTED_MESSAGE,
                "case {model_id:?} (not async-capable): expected the capability refusal, got {:?}",
                message_of(&call)
            );
        }
    }

    assert_eq!(
        async_capable,
        state::ASYNC_INVOKE_MODEL_IDS.len(),
        "expected the inventory to mark exactly the published async models, got {async_capable} \
         of {:?}",
        state::ASYNC_INVOKE_MODEL_IDS
    );
    let mut stored = listed(&router).await;
    stored.sort();
    accepted.sort();
    assert_eq!(
        stored, accepted,
        "expected the store to hold exactly the accepted jobs, got {stored:?}"
    );
}

/// RULE: every id `StartAsyncInvoke` serves carries the request schema its
/// `modelInput` is checked against, and the three lists that express that — the
/// inventory flag, [`state::ASYNC_INVOKE_MODEL_IDS`] and the schemas — agree.
#[tokio::test]
async fn every_async_capable_model_carries_the_schema_its_input_is_checked_against() {
    let inventory = ServiceState::default().models;
    for model_id in state::ASYNC_INVOKE_MODEL_IDS {
        let entry = inventory
            .get(model_id)
            .unwrap_or_else(|| panic!("case {model_id:?}: expected the inventory to carry it"));
        assert!(
            entry.supports_async_invoke,
            "case {model_id:?}: expected the inventory to mark it async-capable, got {entry:?}"
        );
        let valid = valid_model_input(model_id)
            .unwrap_or_else(|| panic!("case {model_id:?}: this suite publishes no valid input"));
        assert_eq!(
            state::async_model_input_violation(model_id, &valid),
            None,
            "case {model_id:?}: expected the published input to satisfy the schema, got {:?}",
            state::async_model_input_violation(model_id, &valid)
        );
        // An empty body satisfies no model's schema: each one requires at least
        // the member that carries the prompt or the task type.
        assert!(
            state::async_model_input_violation(model_id, &json!({})).is_some(),
            "case {model_id:?}: expected an empty modelInput to be refused"
        );
    }
    let marked: Vec<&str> = inventory
        .values()
        .filter(|entry| entry.supports_async_invoke)
        .map(|entry| entry.model_id.as_str())
        .collect();
    assert_eq!(
        marked,
        state::ASYNC_INVOKE_MODEL_IDS.to_vec(),
        "expected the inventory's async-capable ids to be exactly the published list, got \
         {marked:?}"
    );
}

/// RULE: an async-capable model is served under every spelling the modelled
/// `modelId` pattern allows — the bare id, a cross-region inference profile id,
/// and the `foundation-model`/`inference-profile` ARNs — and each resolves to
/// the SAME model, which the reported `modelArn` shows.
#[tokio::test]
async fn an_async_capable_model_is_served_under_every_spelling_of_its_identifier() {
    let router = emulator();
    let model_id = "luma.ray-v2:0";
    let rows = [
        ("the bare id", model_id.to_string()),
        ("a cross-region profile id", format!("us.{model_id}")),
        (
            "a foundation-model ARN",
            format!("arn:aws:bedrock:us-east-1::foundation-model/{model_id}"),
        ),
        (
            "an inference-profile ARN",
            format!("arn:aws:bedrock:us-east-1:000000000000:inference-profile/us.{model_id}"),
        ),
    ];
    for (case, spelling) in rows {
        let input = valid_model_input(model_id).expect("a published input for the model");
        let call = post_json(&router, "/async-invoke", &start_request(&spelling, input)).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?} ({spelling:?}): expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let arn = call.json()["invocationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let record = get(&router, &format!("/async-invoke/{}", encode_path(&arn)))
            .await
            .json();
        assert_eq!(
            record["modelArn"].as_str(),
            Some(
                format!("arn:aws:bedrock:us-east-1:000000000000:foundation-model/{model_id}")
                    .as_str()
            ),
            "case {case:?}: expected every spelling to resolve to the same model, got {:?}",
            record["modelArn"]
        );
    }

    // An identifier that names no model the service offers is still a different
    // fault from one that names a model it does not serve asynchronously.
    let unknown = post_json(
        &router,
        "/async-invoke",
        &start_request("nobody.such-model-v1", json!({ "prompt": "a kite" })),
    )
    .await;
    assert_eq!(
        unknown.status, 400,
        "case \"an identifier naming no model\": expected 400, got {} / {}",
        unknown.status,
        String::from_utf8_lossy(&unknown.body)
    );
    assert_eq!(
        message_of(&unknown),
        state::INVALID_MODEL_IDENTIFIER_MESSAGE,
        "case \"an identifier naming no model\": expected the unreadable-identifier refusal, got \
         {:?}",
        message_of(&unknown)
    );
}

/// One `modelInput` and the schema complaint it draws.
struct SchemaRow {
    case: &'static str,
    model_id: &'static str,
    model_input: serde_json::Value,
    /// The detail of the refusal: the failing member's JSON pointer and the
    /// constraint it broke, which is what the message is built from.
    detail: &'static str,
}

/// The rows differ along WHAT the published schema states — a required member,
/// a value set, a numeric bound, a step, a type — and across both providers,
/// which is what the validation rule generalises over. The `durationSeconds`
/// row the issue cited is ONE of them.
fn schema_rows() -> Vec<SchemaRow> {
    vec![
        SchemaRow {
            case: "a duration outside the published value set",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 7, "fps": 24 },
            }),
            detail: "#/videoGenerationConfig/durationSeconds: 7 is not a valid enum value",
        },
        SchemaRow {
            case: "a frame rate outside the published value set",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "fps": 30 },
            }),
            detail: "#/videoGenerationConfig/fps: 30 is not a valid enum value",
        },
        SchemaRow {
            case: "a dimension outside the published value set",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "dimension": "640x480" },
            }),
            detail: "#/videoGenerationConfig/dimension: 640x480 is not a valid enum value",
        },
        SchemaRow {
            case: "a task type outside the published value set",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "textToVideoParams": { "text": "a kite" },
            }),
            detail: "#/taskType: MULTI_SHOT_AUTOMATED is not a valid enum value",
        },
        SchemaRow {
            case: "a required top-level member omitted",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({ "textToVideoParams": { "text": "a kite" } }),
            detail: "#: required key [taskType] not found",
        },
        SchemaRow {
            case: "a required nested member omitted",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({ "taskType": "TEXT_VIDEO", "textToVideoParams": {} }),
            detail: "#/textToVideoParams: required key [text] not found",
        },
        SchemaRow {
            case: "a seed one below the published floor",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "seed": -1 },
            }),
            detail: "#/videoGenerationConfig/seed: -1 is not greater or equal to 0",
        },
        SchemaRow {
            case: "a seed one above the published ceiling",
            model_id: "amazon.nova-reel-v1:0",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "seed": 2_147_483_647_i64 },
            }),
            detail: "#/videoGenerationConfig/seed: 2147483647 is not less or equal to 2147483646",
        },
        SchemaRow {
            case: "a multi-shot duration that is not a whole number of shots",
            model_id: "amazon.nova-reel-v1:1",
            model_input: json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "multiShotAutomatedParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 13 },
            }),
            detail: "#/videoGenerationConfig/durationSeconds: 13 is not a multiple of 6",
        },
        SchemaRow {
            case: "a multi-shot duration past the published ceiling",
            model_id: "amazon.nova-reel-v1:1",
            model_input: json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "multiShotAutomatedParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 126 },
            }),
            detail: "#/videoGenerationConfig/durationSeconds: 126 is not less or equal to 120",
        },
        SchemaRow {
            case: "a multi-shot duration below the floor that task type publishes",
            model_id: "amazon.nova-reel-v1:1",
            model_input: json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "multiShotAutomatedParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 6 },
            }),
            detail: "#/videoGenerationConfig/durationSeconds: 6 is not greater or equal to 12",
        },
        SchemaRow {
            case: "a single-shot duration outside the value set that task type publishes",
            model_id: "amazon.nova-reel-v1:1",
            model_input: json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 12 },
            }),
            detail: "#/videoGenerationConfig/durationSeconds: 12 is not a valid enum value",
        },
        SchemaRow {
            case: "the parameter block the task type selects omitted",
            model_id: "amazon.nova-reel-v1:1",
            model_input: json!({ "taskType": "MULTI_SHOT_AUTOMATED" }),
            detail: "#: required key [multiShotAutomatedParams] not found",
        },
        SchemaRow {
            case: "the other provider's required member omitted",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "aspect_ratio": "16:9" }),
            detail: "#: required key [prompt] not found",
        },
        SchemaRow {
            case: "the other provider's duration value set",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "prompt": "a kite", "duration": "7s" }),
            detail: "#/duration: 7s is not a valid enum value",
        },
        SchemaRow {
            case: "the other provider's aspect-ratio value set",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "prompt": "a kite", "aspect_ratio": "5:4" }),
            detail: "#/aspect_ratio: 5:4 is not a valid enum value",
        },
        SchemaRow {
            case: "the other provider's resolution value set",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "prompt": "a kite", "resolution": "1080p" }),
            detail: "#/resolution: 1080p is not a valid enum value",
        },
        SchemaRow {
            case: "a member of the wrong type",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "prompt": 7 }),
            detail: "#/prompt: 7 is not of the expected type",
        },
        SchemaRow {
            case: "a member below its published minimum length",
            model_id: "luma.ray-v2:0",
            model_input: json!({ "prompt": "" }),
            detail: "#/prompt: expected minLength: 1, actual: 0",
        },
    ]
}

/// RULE: `StartAsyncInvoke` refuses a `modelInput` that breaks the invoked
/// model's published request schema with `ValidationException` (400), in a
/// message built from the failing member and the constraint it broke — and
/// mints no job, so the refused request leaves nothing for `ListAsyncInvokes`
/// to report.
#[tokio::test]
async fn a_model_input_outside_the_models_published_schema_is_refused_and_mints_no_job() {
    let router = emulator();
    for row in schema_rows() {
        let call = post_json(
            &router,
            "/async-invoke",
            &start_request(row.model_id, row.model_input.clone()),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {:?} ({}): expected 400, got {} / {}",
            row.case,
            row.model_id,
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {:?} ({}): expected ValidationException, got {:?}",
            row.case,
            row.model_id,
            call.error_code()
        );
        assert_eq!(
            message_of(&call),
            state::malformed_model_input_message(row.detail),
            "case {:?} ({}): expected the schema complaint for the failing member, got {:?}",
            row.case,
            row.model_id,
            message_of(&call)
        );
    }

    let stored = listed(&router).await;
    assert!(
        stored.is_empty(),
        "expected a refused request to mint no job, got {stored:?}"
    );
}

/// RULE: a `modelInput` INSIDE the published schema is accepted, including at
/// each published bound of a numeric member and at every value of a published
/// value set — the schema refuses what the provider publishes as invalid, not
/// everything it does not recognise.
#[tokio::test]
async fn a_model_input_inside_the_published_schema_is_accepted_at_its_bounds() {
    let router = emulator();
    let mut rows: Vec<(String, &str, serde_json::Value)> = vec![
        // The seed bounds Nova Reel publishes, at each end.
        (
            "seed at the published floor".to_string(),
            "amazon.nova-reel-v1:0",
            json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "seed": 0 },
            }),
        ),
        (
            "seed at the published ceiling".to_string(),
            "amazon.nova-reel-v1:0",
            json!({
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": { "text": "a kite" },
                "videoGenerationConfig": { "seed": 2_147_483_646_i64 },
            }),
        ),
        // The multi-shot duration bounds, at each end and at a step inside.
        (
            "multi-shot duration at the published floor".to_string(),
            "amazon.nova-reel-v1:1",
            json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "multiShotAutomatedParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 12 },
            }),
        ),
        (
            "multi-shot duration at the published ceiling".to_string(),
            "amazon.nova-reel-v1:1",
            json!({
                "taskType": "MULTI_SHOT_AUTOMATED",
                "multiShotAutomatedParams": { "text": "a kite" },
                "videoGenerationConfig": { "durationSeconds": 120 },
            }),
        ),
        // A member the transcribed schema does not carry is not refused: the
        // schemas state what the provider publishes.
        (
            "an unlisted member".to_string(),
            "luma.ray-v2:0",
            json!({ "prompt": "a kite", "unlisted_member": true }),
        ),
    ];
    // Every value of each value set Luma Ray v2 publishes, including the entries
    // the issue did not cite.
    for aspect_ratio in ["1:1", "3:4", "4:3", "9:16", "16:9", "9:21", "21:9"] {
        rows.push((
            format!("aspect_ratio {aspect_ratio}"),
            "luma.ray-v2:0",
            json!({ "prompt": "a kite", "aspect_ratio": aspect_ratio }),
        ));
    }
    for duration in ["5s", "9s"] {
        rows.push((
            format!("duration {duration}"),
            "luma.ray-v2:0",
            json!({ "prompt": "a kite", "duration": duration }),
        ));
    }
    for resolution in ["540p", "720p"] {
        rows.push((
            format!("resolution {resolution}"),
            "luma.ray-v2:0",
            json!({ "prompt": "a kite", "resolution": resolution }),
        ));
    }

    let mut minted = 0_usize;
    for (case, model_id, model_input) in rows {
        let call = post_json(&router, "/async-invoke", &start_request(model_id, model_input)).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?} ({model_id}): expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            is_invocation_arn(call.json()["invocationArn"].as_str().unwrap_or_default()),
            "case {case:?} ({model_id}): expected a modelled invocationArn, got {:?}",
            call.json()["invocationArn"]
        );
        minted += 1;
    }
    assert_eq!(
        listed(&router).await.len(),
        minted,
        "expected one job per accepted request, got {} for {minted} requests",
        listed(&router).await.len()
    );
}

/// One member carrying a `@pattern`, the value that breaks it, and the
/// expression the refusal must quote.
struct PatternRow {
    case: &'static str,
    /// The request, or `None` for the row that drives a GET.
    body: Option<serde_json::Value>,
    path: &'static str,
    /// The member path the refusal names.
    member: &'static str,
    /// The value the request sent.
    value: &'static str,
    /// The model's `@pattern`, as the refusal must quote it: without the
    /// anchors the whole-value pattern wraps it in.
    pattern: &'static str,
}

/// The rows are the members of this service's async-invoke surface that carry a
/// `@pattern`, from `smithy_query shape=AsyncInvokeS3OutputDataConfig,
/// AsyncInvokeIdempotencyToken,AsyncInvokeIdentifier,InvocationArn`: a body
/// member, a nested union member, two more nested members and a path label.
fn pattern_rows() -> Vec<PatternRow> {
    vec![
        PatternRow {
            case: "the output destination",
            body: Some(json!({
                "modelId": "luma.ray-v2:0",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "not-a-valid-s3-uri" } },
            })),
            path: "/async-invoke",
            member: "outputDataConfig.s3OutputDataConfig.s3Uri",
            value: "not-a-valid-s3-uri",
            pattern: r"s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?",
        },
        PatternRow {
            case: "the destination's bucket owner",
            body: Some(json!({
                "modelId": "luma.ray-v2:0",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": {
                    "s3Uri": "s3://emulator-bucket/async-out",
                    "bucketOwner": "12345",
                } },
            })),
            path: "/async-invoke",
            member: "outputDataConfig.s3OutputDataConfig.bucketOwner",
            value: "12345",
            pattern: r"[0-9]{12}",
        },
        PatternRow {
            case: "the destination's KMS key",
            body: Some(json!({
                "modelId": "luma.ray-v2:0",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": {
                    "s3Uri": "s3://emulator-bucket/async-out",
                    "kmsKeyId": "not-a-kms-key",
                } },
            })),
            path: "/async-invoke",
            member: "outputDataConfig.s3OutputDataConfig.kmsKeyId",
            value: "not-a-kms-key",
            pattern: r"arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:((key/[a-zA-Z0-9-]{36})|(alias/[a-zA-Z0-9-_/]+))",
        },
        PatternRow {
            case: "the idempotency token",
            body: Some(json!({
                "modelId": "luma.ray-v2:0",
                "clientRequestToken": "has a space",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": {
                    "s3Uri": "s3://emulator-bucket/async-out",
                } },
            })),
            path: "/async-invoke",
            member: "clientRequestToken",
            value: "has a space",
            pattern: r"[!-~]*",
        },
        PatternRow {
            case: "the model identifier",
            body: Some(json!({
                "modelId": "not a model id!",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": {
                    "s3Uri": "s3://emulator-bucket/async-out",
                } },
            })),
            path: "/async-invoke",
            member: "modelId",
            value: "not a model id!",
            pattern: r"[a-zA-Z_\.\-/0-9:]+",
        },
        PatternRow {
            case: "the invocation ARN read back",
            body: None,
            path: "/async-invoke/not-an-arn",
            member: "invocationArn",
            value: "not-an-arn",
            pattern: r"arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}",
        },
    ]
}

/// RULE: a boundary refusal of a `@pattern` is `400 ValidationException` whose
/// message quotes the model's expression WITHOUT the anchors a whole-value
/// pattern wraps it in — so no `^`/`$` reaches the client-facing text — for
/// every member of this surface that carries one.
#[tokio::test]
async fn a_pattern_constraint_is_quoted_without_the_anchors_that_wrap_it() {
    let router = emulator();
    for row in pattern_rows() {
        let call = match &row.body {
            Some(body) => post_json(&router, row.path, body).await,
            None => get(&router, row.path).await,
        };
        assert_eq!(
            call.status,
            400,
            "case {:?}: expected 400, got {} / {}",
            row.case,
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            call.error_code()
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            row.value, row.member, row.pattern
        );
        assert_eq!(
            message_of(&call),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            row.case,
            message_of(&call)
        );
    }

    // And the refusal of a MALFORMED ARN stays distinct from the refusal of a
    // well-formed one that names no job: same code, different reason.
    let absent = get(
        &router,
        &format!(
            "/async-invoke/{}",
            encode_path("arn:aws:bedrock:us-east-1:000000000000:async-invoke/abcdef123456")
        ),
    )
    .await;
    assert_eq!(
        absent.status, 400,
        "case \"a well-formed ARN naming no job\": expected 400, got {} / {}",
        absent.status,
        String::from_utf8_lossy(&absent.body)
    );
    assert!(
        !message_of(&absent).contains("failed to satisfy constraint"),
        "case \"a well-formed ARN naming no job\": expected the absent-job reason, not a \
         constraint refusal, got {:?}",
        message_of(&absent)
    );
}
