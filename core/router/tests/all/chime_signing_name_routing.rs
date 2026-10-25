// SPDX-License-Identifier: CC0-1.0

//! The media-insights pipeline configuration calls of `media-pipelines-chime`, dispatched
//! through a router carrying every service that signs as `chime`.
//!
//! `aws.auth#sigv4` names `chime` on all six Chime-family models while each serves its own
//! endpoint (`chime`, `identity-chime`, `media-pipelines-chime`, `meetings-chime`,
//! `messaging-chime`, `voice-chime`), and the router resolves a request's routing key from
//! the SigV4 credential scope first — so a service that claims on the signing name alone
//! claims its five siblings' traffic too, every request has six claimants, and the router
//! (which refuses rather than guesses) answers `500 AmbiguousService`, a code no AWS model
//! declares, for all of it. That is what this service's every operation used to answer.
//!
//! Which member claims each request is the rule
//! `all/chime_family_signing_name_routing.rs` pins, over the whole family's path spaces.
//! What THIS suite pins is the consequence for one member: a request the claim rule
//! resolves to `media-pipelines-chime` is SERVED by it — its handler and its store run —
//! so an identifier that resolves to no stored configuration answers the operation's
//! modelled 404 `NotFoundException` in restJson1's own error framing, and a stored one
//! round-trips. Both routers are needed: the claim table alone cannot show that the
//! resolved claimant then answers from state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name all six
/// services share — the case the ambiguity came from. The signature is never checked: the
/// router here runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The router's own refusals. Either one means the request reached NOBODY
/// (`404 UnknownService`) or every claimant (`500 AmbiguousService`) instead of the one
/// service that owns the path.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// The `ErrorCode` value the modelled `NotFoundException` carries in its body `Code`
/// member. Source: `smithy_query(query="shape", shape="ErrorCode")` on the media pipelines
/// model — `BadRequest | Forbidden | NotFound | ResourceLimitExceeded | …`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// Every service that signs as `chime`, mounted on ONE shared engine exactly as
/// `router/src/lib.rs` mounts them, so a request two of them claim really is refused as
/// `AmbiguousService` rather than quietly served by the first.
fn router_with_the_chime_family() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_chime_ucbuzzconsoleservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::EmulatedService::new(engine),
    );
    router
}

/// An ARN as a client puts it in a non-greedy `@httpLabel`: its `:` and `/`
/// percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// One answer from the router, reduced to what the rows assert on.
struct Answer {
    status: u16,
    /// The wire error-shape name: `x-amzn-errortype` (which restJson1 resolves the shape
    /// from), falling back to the body's `__type` so a router refusal — which uses the
    /// generic awsJson envelope — is still recognised.
    error_code: String,
    content_type: String,
    body: Value,
}

impl Answer {
    fn str_at(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Dispatch one `chime`-signed request through the whole family.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    body: Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("a JSON value serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let error_type = header("x-amzn-errortype");
    let raw = response.into_body();
    let body: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    let error_code = if error_type.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        error_type
    };
    Answer {
        status,
        error_code,
        content_type,
        body,
    }
}

/// Assert one answer is the media pipelines service's modelled not-found refusal, reached
/// through the family router: the whole rule, so every row checks all of it.
fn assert_modelled_not_found(case: &str, answer: &Answer) {
    assert!(
        !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
        "case {case:?}: the request must reach the media pipelines handler, got the router's \
         own {:?}: HTTP {} with {:?}",
        answer.error_code,
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} ({:?}) with {:?}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        answer.error_code, "NotFoundException",
        "case {case:?}: expected the modelled NotFoundException, got {:?} with {:?}",
        answer.error_code, answer.body
    );
    assert_eq!(
        answer.content_type, "application/json",
        "case {case:?}: a restJson1 error is framed as application/json, got {:?}",
        answer.content_type
    );
    assert_eq!(
        answer.str_at("/Code"),
        ERROR_CODE_NOT_FOUND,
        "case {case:?}: expected body Code {ERROR_CODE_NOT_FOUND:?}, got {:?}",
        answer.body
    );
    for member in ["/Message", "/RequestId"] {
        assert!(
            !answer.str_at(member).is_empty(),
            "case {case:?}: the modelled NotFoundException carries {member}, got {:?}",
            answer.body
        );
    }
}

/// RULE: an `Identifier` that resolves to no stored media-insights pipeline configuration
/// is the modelled 404 `NotFoundException`, in this service's own restJson1 error framing,
/// for the delete and the read alike — never one of the router's refusals, and never a 5xx.
///
/// The rows span what the rule generalises over: the identifier's form (a name, this
/// service's own ARN form, a name at each end of the modelled name length bounds) and
/// whether the request addresses this service's own endpoint or a bare emulator host,
/// where only the model's path space names the claimant.
#[tokio::test]
async fn an_unknown_media_insights_configuration_answers_the_modelled_not_found() {
    let router = router_with_the_chime_family();

    // Nothing is created first, so every identifier below is unknown.
    // `MediaInsightsPipelineConfigurationNameString` is len 2..64, pattern
    // `^[0-9a-zA-Z._-]+$` (`smithy_query(query="shape", …)` on the media pipelines model),
    // so the short and long rows sit on its two published bounds.
    let identifiers: [(&str, String); 4] = [
        ("a name nothing was created under", "no-such-configuration".to_string()),
        (
            "this service's own ARN form",
            label(
                "arn:aws:chime:us-east-1:000000000000:media-insights-pipeline-configuration/no-such-configuration",
            ),
        ),
        ("the shortest name the model allows", "ab".to_string()),
        ("the longest name the model allows", "c".repeat(64)),
    ];

    for (identifier_case, identifier) in identifiers {
        for host in ["media-pipelines-chime.us-east-1.amazonaws.com", "localhost:9297"] {
            let uri = format!("/media-insights-pipeline-configurations/{identifier}");
            for method in ["DELETE", "GET"] {
                let case = format!("{method} on {identifier_case} at host {host}");
                let answer = call(&router, method, host, &uri, Value::Null).await;
                assert_modelled_not_found(&case, &answer);
            }
        }
    }
}

/// RULE: what the claim rule resolves is the HANDLER and its store, not merely an error
/// renderer — so a created configuration reads back through the family router by either
/// form of its identifier, deletes with the modelled 204, and is the same modelled 404 as
/// an identifier that never existed afterwards.
#[tokio::test]
async fn a_media_insights_configuration_round_trips_through_the_whole_family() {
    let router = router_with_the_chime_family();
    let name = "family-router-configuration";

    let created = call(
        &router,
        "POST",
        "localhost:9297",
        "/media-insights-pipeline-configurations",
        json!({
            "MediaInsightsPipelineConfigurationName": name,
            "ResourceAccessRoleArn": "arn:aws:iam::000000000000:role/media-insights-access",
            "Elements": [{
                "Type": "AmazonTranscribeProcessor",
                "AmazonTranscribeProcessorConfiguration": {"LanguageCode": "en-US"},
            }],
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"CreateMediaInsightsPipelineConfiguration\": expected HTTP 201, got {} ({:?}) \
         with {:?}",
        created.status, created.error_code, created.body
    );
    let arn =
        created.str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn");
    assert!(
        arn.starts_with("arn:aws:chime:"),
        "case \"CreateMediaInsightsPipelineConfiguration\": the created configuration reports its \
         ARN, got {:?}",
        created.body
    );

    for (case, identifier) in [
        ("by name", name.to_string()),
        ("by ARN", label(&arn)),
    ] {
        let read = call(
            &router,
            "GET",
            "localhost:9297",
            &format!("/media-insights-pipeline-configurations/{identifier}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.str_at(
                    "/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationName"
                )
            ),
            (200, name.to_string()),
            "case {case:?}: the stored configuration must read back, got {} ({:?}) with {:?}",
            read.status,
            read.error_code,
            read.body
        );
    }

    let deleted = call(
        &router,
        "DELETE",
        "localhost:9297",
        &format!("/media-insights-pipeline-configurations/{name}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (deleted.status, deleted.body.clone()),
        (204, Value::Null),
        "case \"the delete of a stored configuration\": expected the modelled 204 with an empty \
         body, got {} ({:?}) with {:?}",
        deleted.status,
        deleted.error_code,
        deleted.body
    );

    for (case, method, identifier) in [
        ("the read after the delete", "GET", name.to_string()),
        ("the second delete, by ARN", "DELETE", label(&arn)),
    ] {
        let answer = call(
            &router,
            method,
            "localhost:9297",
            &format!("/media-insights-pipeline-configurations/{identifier}"),
            Value::Null,
        )
        .await;
        assert_modelled_not_found(case, &answer);
    }
}
