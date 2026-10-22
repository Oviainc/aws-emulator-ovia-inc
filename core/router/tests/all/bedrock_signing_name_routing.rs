// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `bedrock`, shared by SIX emulated
//! services.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every service that signs under one name sees the same key. All six Bedrock-family
//! services sign as `bedrock` while serving distinct endpoints —
//!
//! | service | endpoint prefix | protocol |
//! |---|---|---|
//! | Bedrock (control plane) | `bedrock` | restJson1 |
//! | Bedrock Runtime | `bedrock-runtime` | restJson1 |
//! | Bedrock Agent | `bedrock-agent` | restJson1 |
//! | Bedrock Agent Runtime | `bedrock-agent-runtime` | restJson1 |
//! | Bedrock Data Automation | `bedrock-data-automation` | restJson1 |
//! | Bedrock Data Automation Runtime | `bedrock-data-automation-runtime` | awsJson1_1 |
//!
//! — so a service that claims on the signing name ALONE claims the other five's traffic
//! too, the router finds six claimants and answers `500 AmbiguousService` for ALL of it.
//! That is a code no AWS model declares, and a 5xx where every modelled failure of these
//! operations but `InternalServerException` is a 4xx, so an SDK retries a request that
//! can never succeed.
//!
//! This suite pins the fix: with all six mounted, a request addressed to ONE of them
//! reaches it and answers with that service's OWN response — never the router's
//! `AmbiguousService` (500, several claimants) or `UnknownService` (404, none).
//!
//! Each row addresses a path (or, for the awsJson1_1 member, an `X-Amz-Target`) that
//! exactly one of the six models binds. Where two of them bind the SAME first path
//! segment — the control plane and Data Automation both publish `POST /tagResource`,
//! `/untagResource` and `/listTagsForResource`; Bedrock Agent and Bedrock Agent Runtime
//! both publish `/agents`, `/flows`, `/knowledgebases` and `/tags` — the modelled path
//! space separates nothing, and AWS tells those apart by endpoint host, which an
//! emulator behind one local endpoint does not see. Those paths are therefore NOT rows
//! here: they are a distinct defect from the one this suite covers, and inventing an
//! owner for them would be a guess.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The two refusals the ROUTER itself produces, which no row may provoke: 500 when
/// several services claim a request and 404 when none does.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// A SigV4 `Authorization` header whose credential scope names `bedrock` — the signing
/// name all six services share. The signature is never checked: the router here runs
/// with request authentication off.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        .to_string()
}

/// Every service that shares the `bedrock` signing name, mounted on one engine, exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One request to dispatch: its method, URI, extra headers and body. Every one is signed
/// for `bedrock`, which is the whole point.
struct Call<'a> {
    method: &'a str,
    uri: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: &'a str,
}

/// Dispatch `call` and report the status, the wire error code (`None` on a success) and
/// the raw body.
async fn dispatch(router: &MultiServiceRouter, call: &Call<'_>) -> (u16, Option<String>, String) {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(call.uri)
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", authorization());
    for (name, value) in call.headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(call.body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    let code = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .and_then(Value::as_str)
                .map(|raw| raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string())
        });
    (status, code, body)
}

#[tokio::test]
async fn every_bedrock_signed_request_reaches_exactly_one_of_the_six_services() {
    let router = build_router();

    // (case, call) — one row per member of the family, each addressing a wire surface
    // only that member's model binds: a URI path segment for the five restJson1
    // services, and the `X-Amz-Target` RPC form for the awsJson1_1 one. The rows differ
    // along what separates the services (the path space, and REST vs RPC addressing),
    // which is exactly what the claim rule reads.
    let rows: [(&str, Call<'_>); 7] = [
        (
            // The request this issue reports as 500 `AmbiguousService`: Bedrock Data
            // Automation's own `PUT /data-automation-projects/`.
            "bedrock-data-automation CreateDataAutomationProject",
            Call {
                method: "PUT",
                uri: "https://emulator.local/data-automation-projects/",
                headers: &[],
                body: r#"{"projectName":"routing-check","standardOutputConfiguration":{}}"#,
            },
        ),
        (
            // …and its blueprint path space, so the claim is not keyed to one segment.
            "bedrock-data-automation ListBlueprints",
            Call {
                method: "POST",
                uri: "https://emulator.local/blueprints/",
                headers: &[],
                body: "{}",
            },
        ),
        (
            // The control plane owns `/guardrails`.
            "bedrock ListGuardrails",
            Call {
                method: "GET",
                uri: "https://emulator.local/guardrails",
                headers: &[],
                body: "",
            },
        ),
        (
            // Bedrock Runtime owns `/model/{modelId}/…`. EXACT segment equality is what
            // keeps this out of the control plane's `model-copy-jobs`,
            // `model-customization`, `model-import-jobs` and `model-invocation-job(s)`.
            "bedrock-runtime InvokeModel",
            Call {
                method: "POST",
                uri: "https://emulator.local/model/anthropic.claude-v2/invoke",
                headers: &[],
                body: r#"{"prompt":"hi"}"#,
            },
        ),
        (
            // Bedrock Agent owns `/prompts/`, which Bedrock Agent Runtime does not bind.
            "bedrock-agent ListPrompts",
            Call {
                method: "GET",
                uri: "https://emulator.local/prompts/",
                headers: &[],
                body: "",
            },
        ),
        (
            // Bedrock Agent Runtime owns `/sessions/`, which Bedrock Agent does not bind.
            "bedrock-agent-runtime CreateSession",
            Call {
                method: "PUT",
                uri: "https://emulator.local/sessions/",
                headers: &[],
                body: "{}",
            },
        ),
        (
            // Bedrock Data Automation Runtime is awsJson1_1: every operation posts to
            // the ROOT path and names the service in `X-Amz-Target`, so no path can
            // claim it and no path claim of its five REST peers may swallow it.
            "bedrock-data-automation-runtime GetDataAutomationStatus",
            Call {
                method: "POST",
                uri: "https://emulator.local/",
                headers: &[(
                    "x-amz-target",
                    "AmazonBedrockKeystoneRuntimeService.GetDataAutomationStatus",
                )],
                body: r#"{"invocationArn":"arn:aws:bedrock:us-east-1:000000000000:data-automation-invocation/absent"}"#,
            },
        ),
    ];

    for (case, call) in rows {
        let (status, code, body) = dispatch(&router, &call).await;
        assert!(
            code.as_deref()
                .is_none_or(|code| !ROUTER_REFUSALS.contains(&code)),
            "case {case:?}: the request must reach exactly one of the six services \
             sharing the `bedrock` signing name, got {status} {code:?} with {body}"
        );
        assert_ne!(
            status, 500,
            "case {case:?}: no well-formed request on this path may answer 5xx, \
             got {status} {code:?} with {body}"
        );
    }
}

#[tokio::test]
async fn the_service_that_owns_the_path_is_the_one_that_answers() {
    let router = build_router();

    // Two rows where the answering service is unmistakable, so the suite pins WHICH
    // service served the request and not only that one did.
    //
    // Bedrock Data Automation's `CreateDataAutomationProject` is implemented and answers
    // its modelled 201 with a `data-automation-project` ARN; no other member of the
    // family has that operation at all.
    let (status, code, body) = dispatch(
        &router,
        &Call {
            method: "PUT",
            uri: "https://emulator.local/data-automation-projects/",
            headers: &[],
            body: r#"{"projectName":"routing-owner-check","standardOutputConfiguration":{}}"#,
        },
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateDataAutomationProject must answer its modeled 201, got {status} {code:?} \
         with {body}"
    );
    assert!(
        body.contains(":data-automation-project/"),
        "the 201 must carry a data-automation-project ARN, got {body}"
    );

    // The control plane's `ListGuardrails` is implemented and answers its own listing on
    // a fresh engine; Bedrock Runtime's `/guardrail/…` path space is a different
    // segment, so only the control plane can produce this.
    let (status, code, body) = dispatch(
        &router,
        &Call {
            method: "GET",
            uri: "https://emulator.local/guardrails",
            headers: &[],
            body: "",
        },
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "ListGuardrails must answer the control plane's own listing, got {status} \
         {code:?} with {body}"
    );
}

#[tokio::test]
async fn a_bedrock_signed_request_on_a_path_no_member_binds_reaches_nobody() {
    let router = build_router();

    // The complement of the rule: narrowing the claim to each model's own wire surface
    // must not leave some member claiming EVERY `bedrock`-signed path. A path none of
    // the six binds is the router's 404 `UnknownService` — one claimant too few, never
    // the 500 that says several claimed it.
    let rows: [(&str, Call<'_>); 2] = [
        (
            "a path no bedrock-family model binds",
            Call {
                method: "POST",
                uri: "https://emulator.local/not-a-bedrock-path",
                headers: &[],
                body: "{}",
            },
        ),
        (
            "an X-Amz-Target naming no bedrock-family service, on the root path",
            Call {
                method: "POST",
                uri: "https://emulator.local/",
                headers: &[("x-amz-target", "SomeOtherService.SomeOperation")],
                body: "{}",
            },
        ),
    ];

    for (case, call) in rows {
        let (status, code, body) = dispatch(&router, &call).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("UnknownService")),
            "case {case:?}: expected the router's 404 UnknownService, got {status} \
             {code:?} with {body}"
        );
    }
}
