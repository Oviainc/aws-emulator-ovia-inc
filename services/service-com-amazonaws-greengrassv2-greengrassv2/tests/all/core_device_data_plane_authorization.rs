// SPDX-License-Identifier: CC0-1.0

//! Who `ResolveComponentCandidates` serves, over the SET of candidates a caller can name
//! and the SET of ways a caller can present IAM credentials.
//!
//! The rule under test: Greengrass serves component resolution on its data-plane endpoint
//! to a core device that authenticates with its own IoT device certificate (the
//! operation's own Smithy documentation: "To use this operation, you must use the data
//! plane API endpoint and authenticate with an IoT device certificate"), so a request
//! that presents IAM control-plane credentials instead is refused `403`
//! `AccessDeniedException` with the authorization layer's fixed sentence — and refused
//! BEFORE any candidate is resolved, so the answer does not depend on what the request
//! named.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body.
//!
//! The rows differ along what the rule generalises over — the class of candidate named
//! (a published and settled version, a version of a real component that was never
//! published, a component that never existed, several at once, none at all), the platform
//! the request reports, and the CARRIER the IAM credential arrives in (the `Authorization`
//! header, or a presigned URL's query) — not along the component names, which the rule
//! says nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The route this operation is bound to (`POST /greengrass/v2/resolveComponentCandidates`).
const RESOLVE_PATH: &str = "/greengrass/v2/resolveComponentCandidates";

/// The sentence the authorization layer refuses an unauthorizable caller with. Entirely
/// fixed text: no part of it comes from the request or from state.
const NOT_AUTHORIZED: &str = "Unable to determine service/operation name to be authorized";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The component published before the table runs, so one row names a version that really
/// exists and is settled.
const PUBLISHED: &str = "com.example.Published";
const PUBLISHED_VERSION: &str = "1.0.0";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// How a request presents (or does not present) an IAM credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
    /// The `Authorization` header an AWS SDK or the CLI signs a request with.
    SigV4Header,
    /// The same credential carried in a presigned URL's query instead.
    PresignedQuery,
    /// No IAM credential in either carrier — a core device, whose identity is its IoT
    /// device certificate.
    None,
}

/// Dispatch one request to `path`, presenting `credential`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    credential: Credential,
    body: Vec<u8>,
) -> (u16, Value) {
    let query = match credential {
        Credential::PresignedQuery => format!(
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIDEXAMPLE%2F20260101%2F{REGION}%2Fgreengrass%2Faws4_request\
             &X-Amz-SignedHeaders=host&X-Amz-Signature=00"
        ),
        Credential::SigV4Header | Credential::None => String::new(),
    };
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://greengrass.{REGION}.amazonaws.com{path}{query}"
        ))
        .header("content-type", "application/json");
    if credential == Credential::SigV4Header {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body))
                .expect("a well-formed test request"),
        )
        .await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

/// `ResolveComponentCandidates` with `body`, presenting `credential`.
async fn resolve(
    router: &MultiServiceRouter,
    credential: Credential,
    body: &Value,
) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    call(router, "POST", RESOLVE_PATH, credential, bytes).await
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
}

/// The minimal recipe the parser accepts for `name`@`version`: the three keys the IoT
/// Greengrass component recipe reference requires.
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// Publish `name`@`version` and wait for it to settle, so a row that names it names a
/// component version a core device really could install.
async fn publish(router: &MultiServiceRouter, name: &str, version: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/greengrass/v2/createComponentVersion",
        Credential::SigV4Header,
        serde_json::to_vec(&json!({ "inlineRecipe": base64::encode(recipe(name, version).as_bytes()) }))
            .expect("a serializable request body"),
    )
    .await;
    assert_eq!(
        status, 201,
        "publishing {name}:{version} must answer 201, got {status} {body}"
    );
    // The publish validated the recipe, so the version is settled by the time this
    // helper returns and no wait is owed.
}

/// One candidate class a request can name.
#[derive(Debug)]
struct CandidateCase {
    label: &'static str,
    /// The `componentCandidates` member, which is what the row varies.
    candidates: Value,
    /// The `platform` member, or `Value::Null` when the row reports none.
    platform: Value,
}

/// The rows span the candidate classes the refusal must not depend on: a published and
/// settled version, an unpublished version of a published component, a component that was
/// never created, a mixture, the empty list the operation otherwise serves as an empty
/// success, and a requirement the operation otherwise refuses as bad input.
fn cases() -> Vec<CandidateCase> {
    vec![
        CandidateCase {
            label: "a published, settled component version",
            candidates: json!([{
                "componentName": PUBLISHED,
                "componentVersion": PUBLISHED_VERSION,
                "versionRequirements": {},
            }]),
            platform: json!({
                "name": "linux",
                "attributes": { "os": "linux", "architecture": "amd64" },
            }),
        },
        CandidateCase {
            label: "a version of a published component that was never published",
            candidates: json!([{
                "componentName": PUBLISHED,
                "componentVersion": "9.9.9",
                "versionRequirements": {},
            }]),
            platform: json!({ "name": "linux", "attributes": { "os": "linux" } }),
        },
        CandidateCase {
            label: "a component that was never created",
            candidates: json!([{ "componentName": "com.example.NeverCreated" }]),
            platform: Value::Null,
        },
        CandidateCase {
            label: "a published version beside one that does not exist",
            candidates: json!([
                { "componentName": PUBLISHED, "componentVersion": PUBLISHED_VERSION },
                {
                    "componentName": "com.example.NeverCreated",
                    "versionRequirements": { "com.example.Nucleus": ">=1.0.0" },
                },
            ]),
            platform: json!({ "attributes": { "architecture": "aarch64" } }),
        },
        CandidateCase {
            label: "no candidates at all",
            candidates: json!([]),
            platform: Value::Null,
        },
        CandidateCase {
            label: "a version requirement that is not a semantic-version requirement",
            candidates: json!([{
                "componentName": PUBLISHED,
                "versionRequirements": { "com.example.Nucleus": "not-a-requirement" },
            }]),
            platform: Value::Null,
        },
    ]
}

/// The request body for one row.
fn request_body(case: &CandidateCase) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("componentCandidates".to_string(), case.candidates.clone());
    if !case.platform.is_null() {
        body.insert("platform".to_string(), case.platform.clone());
    }
    Value::Object(body)
}

#[tokio::test]
async fn an_iam_caller_is_refused_whatever_it_asks_to_resolve() {
    let router = emulator();
    publish(&router, PUBLISHED, PUBLISHED_VERSION).await;

    for carrier in [Credential::SigV4Header, Credential::PresignedQuery] {
        for case in &cases() {
            let (status, body) = resolve(&router, carrier, &request_body(case)).await;
            assert_eq!(
                status, 403,
                "case {:?} with {carrier:?}: expected 403, got {status} {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {:?} with {carrier:?}: expected AccessDeniedException, got {body}",
                case.label
            );
            assert_eq!(
                text(&body, "message"),
                NOT_AUTHORIZED,
                "case {:?} with {carrier:?}: expected the authorization sentence, got {body}",
                case.label
            );
            // The refusal precedes resolution, so it carries nothing the request named
            // and no recipe of any published version.
            assert!(
                body.get("resolvedComponentVersions").is_none(),
                "case {:?} with {carrier:?}: the refusal must resolve nothing, got {body}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_is_about_the_caller_and_not_about_the_catalog() {
    let router = emulator();
    publish(&router, PUBLISHED, PUBLISHED_VERSION).await;

    let candidate = json!({
        "componentCandidates": [{
            "componentName": PUBLISHED,
            "componentVersion": PUBLISHED_VERSION,
            "versionRequirements": {},
        }]
    });

    // The SAME request, with and without an IAM credential: the credential is the only
    // thing that decides the answer.
    let (refused_status, refused) = resolve(&router, Credential::SigV4Header, &candidate).await;
    assert_eq!(
        refused_status, 403,
        "case an IAM caller: expected 403, got {refused_status} {refused}"
    );

    let (served_status, served) = resolve(&router, Credential::None, &candidate).await;
    assert_eq!(
        served_status, 200,
        "case a core device: expected 200, got {served_status} {served}"
    );
    let resolved = served
        .get("resolvedComponentVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        resolved.len(),
        1,
        "case a core device: expected the published version to resolve, got {served}"
    );
    assert_eq!(
        text(&resolved[0], "componentVersion"),
        PUBLISHED_VERSION,
        "case a core device: expected version {PUBLISHED_VERSION}, got {served}"
    );

    // The refusal changed nothing: the component the IAM caller was refused for is still
    // readable through the control-plane reader that caller IS authorized for.
    let (status, metadata) = call(
        &router,
        "GET",
        &format!(
            "/greengrass/v2/components/{}/metadata",
            format!(
                "arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{PUBLISHED}:versions:{PUBLISHED_VERSION}"
            )
            .replace(':', "%3A")
        ),
        Credential::SigV4Header,
        Vec::new(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the control-plane reader after the refusal: expected 200, got {status} {metadata}"
    );
    assert_eq!(
        text(&metadata, "componentVersion"),
        PUBLISHED_VERSION,
        "case the control-plane reader after the refusal: expected the published version, got {metadata}"
    );
}
