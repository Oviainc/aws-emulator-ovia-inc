// SPDX-License-Identifier: CC0-1.0

//! Which `dataAutomationProfileArn` `InvokeBlueprintOptimizationAsync` accepts, driven
//! over the real `restJson1` wire path (`MultiServiceRouter::dispatch` with a
//! signed-looking request), so what is asserted is what a client sees.
//!
//! RULE. A submission runs under the Data Automation profile its `dataAutomationProfileArn`
//! names, and AWS publishes those profiles rather than an account creating them: an ARN
//! that satisfies the modelled `DataAutomationProfileArn` `@pattern` but whose resource id
//! names no published profile resolves to nothing, so the submission is refused 400
//! `ValidationException` with the fixed sentence
//! `state::data_automation_profile::INVALID_PROFILE_MESSAGE` and NOTHING is minted — the
//! blueprint it referenced is left exactly as it was, and no optimization job exists.
//!
//! The rows are a TABLE spanning what the rule generalises over: every geography AWS
//! publishes a profile for, both account forms the pattern admits, the partitions it
//! admits, and the ids that name no profile (an opaque id, an unpublished geography,
//! another version, a published id in the wrong case).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{REGION}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

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

/// `CreateBlueprint`, per its modelled `@http` `{method: PUT, uri: /blueprints/}`.
/// Returns the ARN it minted.
async fn create_blueprint(router: &MultiServiceRouter, name: &str) -> String {
    let (status, payload) = call(
        router,
        "PUT",
        "/blueprints/",
        &json!({
            "blueprintName": name,
            "type": "DOCUMENT",
            "schema": SCHEMA,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    let blueprint = payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: the create answered no blueprint"));
    string_at(&blueprint, "blueprintArn", name)
}

/// The schema every fixture blueprint carries, so a read-back can be compared against it.
const SCHEMA: &str = r#"{"class":"invoice"}"#;

/// One submission naming `profile_arn` for the blueprint `blueprint_arn`, with only the
/// members the model marks required beside it.
fn submission(blueprint_arn: &str, profile_arn: &str) -> Value {
    json!({
        "blueprint": { "blueprintArn": blueprint_arn },
        "samples": [{
            "assetS3Object": { "s3Uri": "s3://bda-samples/assets/invoice-0.pdf" },
            "groundTruthS3Object": { "s3Uri": "s3://bda-samples/truth/invoice-0.json" },
        }],
        "outputConfiguration": { "s3Object": { "s3Uri": "s3://bda-output" } },
        "dataAutomationProfileArn": profile_arn,
    })
}

/// RULE: a submission is accepted only where its `dataAutomationProfileArn` names a
/// published Data Automation profile; one that names none is refused 400
/// `ValidationException` with the service's fixed sentence.
#[tokio::test]
async fn only_a_published_profile_arn_is_accepted() {
    let router = emulator();

    // (case, the profile ARN the submission names, whether it resolves)
    let rows: [(&str, String, bool); 10] = [
        (
            "the us profile under this account",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v1"
            ),
            true,
        ),
        (
            "the eu profile",
            "arn:aws:bedrock:eu-west-1:000000000000:data-automation-profile/eu.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the apac profile",
            "arn:aws:bedrock:ap-southeast-2:000000000000:data-automation-profile/\
             apac.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the AWS-owned account field the pattern admits",
            format!("arn:aws:bedrock:{REGION}:aws:data-automation-profile/us.data-automation-v1"),
            true,
        ),
        (
            "another partition",
            "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:data-automation-profile/\
             us.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the china partition",
            "arn:aws-cn:bedrock:cn-north-1:000000000000:data-automation-profile/\
             apac.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "an opaque id naming no profile",
            format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/4f2b9c1d8a7e"),
            false,
        ),
        (
            "a geography AWS publishes no profile for",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 mars.data-automation-v1"
            ),
            false,
        ),
        (
            "another profile version",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v2"
            ),
            false,
        ),
        (
            "a published id in the wrong case",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 US.data-automation-v1"
            ),
            false,
        ),
    ];

    for (index, (case, profile_arn, resolves)) in rows.into_iter().enumerate() {
        let blueprint_arn = create_blueprint(&router, &format!("profile-row-{index}")).await;
        let (status, payload) = call(
            &router,
            "POST",
            "/invokeBlueprintOptimizationAsync",
            &submission(&blueprint_arn, &profile_arn),
        )
        .await;
        if resolves {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the submission to be accepted, got {status} with \
                 {payload:?}"
            );
            assert!(
                !string_at(&payload, "invocationArn", case).is_empty(),
                "case {case:?}: an accepted submission answers the job's ARN, got {payload:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        assert_eq!(
            string_at(&payload, "message", case),
            state::data_automation_profile::INVALID_PROFILE_MESSAGE,
            "case {case:?}: expected the service's fixed sentence, got {payload:?}"
        );
        assert!(
            payload.get("invocationArn").is_none(),
            "case {case:?}: a refused submission mints no invocation, got {payload:?}"
        );
    }
}

/// RULE: a refused submission performs NOTHING — the blueprint it referenced is unchanged
/// and no optimization job was minted for it.
#[tokio::test]
async fn a_refused_submission_leaves_the_blueprint_untouched() {
    let router = emulator();
    let blueprint_arn = create_blueprint(&router, "untouched-by-a-refusal").await;

    // The ids that name no published profile, each one a separate refused submission.
    let unresolvable = [
        format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/4f2b9c1d8a7e"),
        format!(
            "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
             mars.data-automation-v1"
        ),
    ];
    for profile_arn in &unresolvable {
        let (status, payload) = call(
            &router,
            "POST",
            "/invokeBlueprintOptimizationAsync",
            &submission(&blueprint_arn, profile_arn),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {profile_arn:?}: expected 400, got {status} with {payload:?}"
        );
    }

    let (status, read) = call(
        &router,
        "POST",
        &format!("/blueprints/{}/", encode_label(&blueprint_arn)),
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the blueprint survives\": expected 200, got {status} with {read:?}"
    );
    let blueprint = read
        .get("blueprint")
        .cloned()
        .expect("the read answers a blueprint");
    assert_eq!(
        string_at(&blueprint, "blueprintArn", "the blueprint survives"),
        blueprint_arn,
        "case \"the blueprint survives\": the ARN must be the one the create minted"
    );
    assert_eq!(
        string_at(&blueprint, "schema", "the blueprint survives"),
        SCHEMA,
        "case \"the blueprint survives\": the schema must be the one the create carried"
    );
}
