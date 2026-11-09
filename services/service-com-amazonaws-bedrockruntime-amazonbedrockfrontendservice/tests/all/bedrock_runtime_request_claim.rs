// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS.
//!
//! Bedrock Runtime signs as `bedrock` — a SigV4 signing name it shares with five other
//! emulated Bedrock-family services (the Bedrock control plane, Bedrock Agent, Bedrock
//! Agent Runtime, Bedrock Data Automation and Bedrock Data Automation Runtime all carry
//! `aws.auth#sigv4` name `bedrock`; `smithy_query(query="service_info")` on each model
//! reports it, against the distinct endpoint prefixes `bedrock-runtime`, `bedrock`,
//! `bedrock-agent`, `bedrock-agent-runtime`, `bedrock-data-automation` and
//! `bedrock-data-automation-runtime`). The router resolves a request's routing key from
//! the SigV4 credential scope first, so a service that claims on the signing name ALONE
//! claims all five peers' traffic too — and the router, finding several claimants,
//! refuses every such request rather than picking one.
//!
//! The rule this suite pins is therefore narrower than the signing name: a request is
//! claimed only when it also addresses THIS model's own wire surface, which for a
//! `restJson1` service is the set of first URI path segments its operations bind. The
//! table below spans that surface — one row per segment this model publishes, one row per
//! peer segment it must NOT claim (including the peer segments that merely share a
//! PREFIX with this model's, which is why the match is exact segment equality), the RPC
//! root path, and a request signed for another service entirely.

use std::sync::Arc;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::Service;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked here: `can_handle` reads only the scope's service field.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Whether this service claims a request for `method path` signed under `signing_name`.
fn claims(
    service: &EmulatedService<engine::InMemoryEngine>,
    signing_name: &str,
    method: &str,
    path: &str,
) -> bool {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::new())
        .expect("build the request");
    let (parts, _body) = request.into_parts();
    service.can_handle(&parts)
}

/// One row: a request under a signing name, and whether this service claims it.
struct Row {
    case: &'static str,
    signing_name: &'static str,
    method: &'static str,
    path: &'static str,
    claimed: bool,
}

/// RULE: this service claims a request only when the resolved routing key is its SigV4
/// signing name AND the request's first URI path segment is one its own `restJson1` model
/// binds; every other request — a peer's path under the same signing name, the RPC root
/// path, a path no Bedrock model binds, or any path under a different signing name — is
/// left to whoever owns it.
#[test]
fn bedrock_runtime_claims_exactly_the_path_space_its_model_binds() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // The `claimed: true` rows are every first path segment this model publishes, read
    // from `smithy_query(query="digest")` on
    // `smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json`:
    // `/model/{modelId}/…` (Converse, ConverseStream, InvokeModel,
    // InvokeModelWithResponseStream, InvokeModelWithBidirectionalStream, CountTokens),
    // `/guardrail/{id}/version/{v}/apply` (ApplyGuardrail), `/guardrail-checks/invoke`
    // (InvokeGuardrailChecks) and `/async-invoke…` (StartAsyncInvoke, GetAsyncInvoke,
    // ListAsyncInvokes).
    //
    // The `claimed: false` rows are the peer segments under the same signing name, taken
    // from the same query on each peer's model: `/guardrails` and `/foundation-models`
    // (control plane), `/model-invocation-job/{id}` and `/model-customization-jobs`
    // (control plane — segments this model's `model` is a strict PREFIX of, which a
    // prefix test would wrongly hand to both services), `/prompts/` (Bedrock Agent),
    // `/retrieveAndGenerate` (Bedrock Agent Runtime) and `/blueprints/` (Bedrock Data
    // Automation).
    let rows = [
        Row {
            case: "Converse",
            signing_name: "bedrock",
            method: "POST",
            path: "/model/anthropic.claude-3-haiku-20240307-v1%3A0/converse",
            claimed: true,
        },
        Row {
            case: "ConverseStream",
            signing_name: "bedrock",
            method: "POST",
            path: "/model/anthropic.claude-3-haiku-20240307-v1%3A0/converse-stream",
            claimed: true,
        },
        Row {
            case: "InvokeModel",
            signing_name: "bedrock",
            method: "POST",
            path: "/model/amazon.titan-text-express-v1/invoke",
            claimed: true,
        },
        Row {
            case: "InvokeModelWithResponseStream",
            signing_name: "bedrock",
            method: "POST",
            path: "/model/amazon.nova-pro-v1%3A0/invoke-with-response-stream",
            claimed: true,
        },
        Row {
            case: "CountTokens",
            signing_name: "bedrock",
            method: "POST",
            path: "/model/meta.llama3-8b-instruct-v1%3A0/count-tokens",
            claimed: true,
        },
        Row {
            case: "ApplyGuardrail",
            signing_name: "bedrock",
            method: "POST",
            path: "/guardrail/abcdefghij12/version/DRAFT/apply",
            claimed: true,
        },
        Row {
            case: "InvokeGuardrailChecks",
            signing_name: "bedrock",
            method: "POST",
            path: "/guardrail-checks/invoke",
            claimed: true,
        },
        Row {
            case: "StartAsyncInvoke",
            signing_name: "bedrock",
            method: "POST",
            path: "/async-invoke",
            claimed: true,
        },
        Row {
            case: "ListAsyncInvokes",
            signing_name: "bedrock",
            method: "GET",
            path: "/async-invoke?maxResults=10",
            claimed: true,
        },
        Row {
            case: "GetAsyncInvoke",
            signing_name: "bedrock",
            method: "GET",
            path: "/async-invoke/arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Aasync-invoke%2Fabc123def456",
            claimed: true,
        },
        Row {
            case: "the control plane's CreateGuardrail",
            signing_name: "bedrock",
            method: "POST",
            path: "/guardrails",
            claimed: false,
        },
        Row {
            case: "the control plane's ListFoundationModels",
            signing_name: "bedrock",
            method: "GET",
            path: "/foundation-models",
            claimed: false,
        },
        Row {
            case: "the control plane's GetModelInvocationJob, whose segment this model's \
                   `model` is a prefix of",
            signing_name: "bedrock",
            method: "GET",
            path: "/model-invocation-job/abcdefghijkl",
            claimed: false,
        },
        Row {
            case: "the control plane's ListModelCustomizationJobs, another segment \
                   `model` is a prefix of",
            signing_name: "bedrock",
            method: "GET",
            path: "/model-customization-jobs",
            claimed: false,
        },
        Row {
            case: "Bedrock Agent's ListPrompts",
            signing_name: "bedrock",
            method: "GET",
            path: "/prompts/",
            claimed: false,
        },
        Row {
            case: "Bedrock Agent Runtime's RetrieveAndGenerate",
            signing_name: "bedrock",
            method: "POST",
            path: "/retrieveAndGenerate",
            claimed: false,
        },
        Row {
            case: "Bedrock Data Automation's CreateBlueprint",
            signing_name: "bedrock",
            method: "PUT",
            path: "/blueprints/",
            claimed: false,
        },
        Row {
            case: "the RPC root path Bedrock Data Automation Runtime posts to",
            signing_name: "bedrock",
            method: "POST",
            path: "/",
            claimed: false,
        },
        Row {
            case: "a path no Bedrock model binds",
            signing_name: "bedrock",
            method: "POST",
            path: "/not-a-bedrock-path",
            claimed: false,
        },
        Row {
            case: "this model's own path, signed for another service",
            signing_name: "dynamodb",
            method: "POST",
            path: "/model/anthropic.claude-3-haiku-20240307-v1%3A0/converse",
            claimed: false,
        },
    ];

    for row in rows {
        let actual = claims(&service, row.signing_name, row.method, row.path);
        assert_eq!(
            actual, row.claimed,
            "case {:?}: expected claimed={} for {} {} signed for {:?}, got {actual}",
            row.case, row.claimed, row.method, row.path, row.signing_name
        );
    }
}

/// RULE: a request this service claims is routed by the URI its MODEL binds —
/// `override_operation` takes over exactly the two operations whose output is a
/// `@streaming` union (which the generated object-safe routing table cannot serve) and
/// serves every other MODELLED route through the shared request-id report, which
/// delegates to the very operation that table names for the request. So
/// `override_operation` answers for a request exactly when the model binds a route this
/// service serves, and declines every other one — leaving the router's routing-error path
/// untouched.
///
/// The rows are every `/model/{modelId}/…` route the model publishes
/// (`smithy_query(query="digest")`: `converse`, `converse-stream`, `invoke`,
/// `invoke-with-response-stream`, `invoke-with-bidirectional-stream`, `count-tokens`),
/// a route outside that path space, and tails/methods the model binds nothing for. Each
/// row is checked BOTH ways: against `override_operation` and against the framework's own
/// route matcher over this service's routing table
/// (`service_common::addresses_modeled_route`), so the two literal tails routed by hand
/// cannot drift away from the model — a tail this service overrides that resolves to no
/// route, or a routable operation it fails to serve, fails here.
#[test]
fn override_operation_routes_exactly_the_modelled_event_stream_uris() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // (case, method, path, overridden here, resolved by the generated routing table)
    let rows: [(&str, &str, &str, bool, bool); 10] = [
        (
            "ConverseStream, a @streaming output",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/converse-stream",
            true,
            true,
        ),
        (
            "InvokeModelWithResponseStream, a @streaming output",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/invoke-with-response-stream",
            true,
            true,
        ),
        (
            "Converse, served by the generated table",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/converse",
            true,
            true,
        ),
        (
            "InvokeModel, served by the generated table",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/invoke",
            true,
            true,
        ),
        (
            "CountTokens, served by the generated table",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/count-tokens",
            true,
            true,
        ),
        // The model binds this route, but the generated table wires no operation for it
        // (its INPUT is a `@streaming` union too, which the scaffold does not generate),
        // so the framework's route matcher resolves nothing and the request keeps
        // answering the framework's "modeled but not served" response. This service must
        // not override it either: a half-served bidirectional stream would be worse than
        // that answer.
        (
            "InvokeModelWithBidirectionalStream: modelled, wired nowhere",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/invoke-with-bidirectional-stream",
            false,
            false,
        ),
        (
            "StartAsyncInvoke, outside the /model path space",
            "POST",
            "/async-invoke",
            true,
            true,
        ),
        (
            "a tail the model binds nothing for",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/converse-streamx",
            false,
            false,
        ),
        (
            "a streaming tail on the wrong method",
            "GET",
            "/model/amazon.nova-pro-v1%3A0/converse-stream",
            false,
            false,
        ),
        (
            "a deeper path under a streaming tail",
            "POST",
            "/model/amazon.nova-pro-v1%3A0/converse-stream/extra",
            false,
            false,
        ),
    ];

    for (case, method, path, overridden, routable) in rows {
        let mut request = aws_smithy_runtime_api::http::Request::new(
            aws_smithy_types::body::SdkBody::empty(),
        );
        request
            .set_uri(format!("https://emulator.local{path}").as_str())
            .expect("set the uri");
        request.set_method(method).expect("set the method");
        let actual = service.override_operation(&request).is_some();
        assert_eq!(
            actual, overridden,
            "case {case:?}: expected override_operation().is_some() == {overridden} for \
             {method} {path}, got {actual}"
        );

        let parts = http::Request::builder()
            .method(method)
            .uri(format!("https://emulator.local{path}"))
            .header("host", "emulator.local")
            .header("authorization", authorization("bedrock"))
            .body(bytes::Bytes::new())
            .expect("build the request")
            .into_parts()
            .0;
        let bound = service_common::addresses_modeled_route(&parts, service.definition());
        assert_eq!(
            bound, routable,
            "case {case:?}: expected the routing table to resolve {method} {path} == \
             {routable}, got {bound}"
        );
    }
}
