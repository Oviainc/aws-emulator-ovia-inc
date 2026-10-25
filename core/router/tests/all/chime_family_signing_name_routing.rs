// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `chime` SigV4 signing name.
//!
//! SIX emulated services share that one signing name — Amazon Chime (the legacy console
//! service) and the five Chime SDK services (Identity, Media Pipelines, Meetings,
//! Messaging, Voice) — while each serves its own endpoint (`chime`, `identity-chime`,
//! `media-pipelines-chime`, `meetings-chime`, `messaging-chime`, `voice-chime`). The
//! router resolves a request's routing key from the SigV4 credential scope first, so all
//! six see the key `chime`: a service that claims on the signing name alone claims the
//! other five's traffic too, and the router answers `500 AmbiguousService` — an error code
//! no AWS model declares — for every one of them.
//!
//! The rule these suites pin: a request signed for a signing name several emulated
//! services share is claimed by exactly ONE of them — the one whose own ENDPOINT the
//! request addresses, and, when it addresses none, the one whose model binds the request's
//! URI path space. Where two of the six bind the same path space, the segment their models
//! differ at decides; where five bind the same space identically and the request carries
//! no signal that names one of them (the `/tags` space, whose resource ARN travels in the
//! request body on tag and untag), the family's DECLARED tagging owner claims it, which it
//! can answer for any member's resource because the tags live in the ARN-keyed directory
//! the family shares — never two claimants, which would be a retryable 500.
//!
//! Rows therefore span the dimensions that rule generalises over: which endpoint owns the
//! operation, a `Host` that names the owner, a sibling, or no service at all, both sides
//! of each shared path space, a request signed for a different service entirely, and an
//! unsigned request addressed by endpoint host.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name all six
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header for a request signed for a service outside the family.
const SQS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTSQS/20260101/us-east-1/sqs/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// Each service's `endpointPrefix`, which is how a row names the one that must claim it.
/// `NOBODY` is the router's `404 UnknownService`; two or more claimants render as a list,
/// which is the `500 AmbiguousService` these suites exist to exclude.
const CHIME: &str = "chime";
const IDENTITY: &str = "identity-chime";
const MEDIA: &str = "media-pipelines-chime";
const MEETINGS: &str = "meetings-chime";
const MESSAGING: &str = "messaging-chime";
const VOICE: &str = "voice-chime";
const NOBODY: &str = "<nobody>";

/// An app instance ARN, the scope every Chime SDK Identity and Messaging resource hangs
/// off.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:000000000000:app-instance/ai1";

type Chime =
    service_com_amazonaws_chime_ucbuzzconsoleservice::EmulatedService<engine::InMemoryEngine>;
type Identity =
    service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService<
        engine::InMemoryEngine,
    >;
type Media =
    service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService<
        engine::InMemoryEngine,
    >;
type Meetings =
    service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService<
        engine::InMemoryEngine,
    >;
type Messaging =
    service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService<
        engine::InMemoryEngine,
    >;
type Voice =
    service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::EmulatedService<
        engine::InMemoryEngine,
    >;
type Lambda =
    service_com_amazonaws_lambda_awsgirapiservice::EmulatedService<engine::InMemoryEngine>;

/// The six services that share the `chime` signing name, each paired with its own
/// `endpointPrefix`, over ONE shared engine exactly as `router/src/lib.rs` mounts them.
struct Family {
    chime: Chime,
    identity: Identity,
    media: Media,
    meetings: Meetings,
    messaging: Messaging,
    voice: Voice,
}

impl Family {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        Self {
            chime: Chime::new(Arc::clone(&engine)),
            identity: Identity::new(Arc::clone(&engine)),
            media: Media::new(Arc::clone(&engine)),
            meetings: Meetings::new(Arc::clone(&engine)),
            messaging: Messaging::new(Arc::clone(&engine)),
            voice: Voice::new(engine),
        }
    }

    /// Who claims a request: the one endpoint prefix that does, [`NOBODY`], or \u{2014} the
    /// failure these suites exclude \u{2014} every claimant, comma-separated.
    fn claimant(&self, parts: &http::request::Parts) -> String {
        let claimants: Vec<&str> = [
            (CHIME, self.chime.can_handle(parts)),
            (IDENTITY, self.identity.can_handle(parts)),
            (MEDIA, self.media.can_handle(parts)),
            (MEETINGS, self.meetings.can_handle(parts)),
            (MESSAGING, self.messaging.can_handle(parts)),
            (VOICE, self.voice.can_handle(parts)),
        ]
        .into_iter()
        .filter_map(|(name, claims)| claims.then_some(name))
        .collect();
        match claimants.as_slice() {
            [] => NOBODY.to_string(),
            [one] => (*one).to_string(),
            many => many.join(", "),
        }
    }
}

/// A router with all six of them mounted, so a request two services claim really is
/// refused as `AmbiguousService`. `with_lambda` additionally mounts the emulator's Lambda
/// service, which `CreateChannelFlow` resolves its processors against.
fn router_with_the_family(with_lambda: bool) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(Chime::new(Arc::clone(&engine)));
    router.register_service(Identity::new(Arc::clone(&engine)));
    router.register_service(Media::new(Arc::clone(&engine)));
    router.register_service(Meetings::new(Arc::clone(&engine)));
    router.register_service(Messaging::new(Arc::clone(&engine)));
    router.register_service(Voice::new(Arc::clone(&engine)));
    if with_lambda {
        router.register_service(Lambda::new(engine));
    }
    router
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str, authorization: Option<&str>) -> http::request::Parts {
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    request
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// An ARN as a client puts it in a non-greedy `@httpLabel`: its `:` and `/`
/// percent-encoded.
fn label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// One answer from the router: the HTTP status, the restJson1 `__type` error code (the
/// empty string on a success), the `Content-Type` and the body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    body: Value,
) -> (u16, String, String, Value) {
    let payload = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .header(
            "x-amz-chime-bearer",
            format!("{APP_INSTANCE}/user/alice"),
        )
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, content_type, parsed)
}

#[test]
fn exactly_one_service_sharing_the_chime_signing_name_claims_each_request() {
    let family = Family::new();

    // (case, method, host, uri, the endpoint that must claim it)
    //
    // Every row here is SIGNED with the credential scope all six services share, which is
    // the case the ambiguity came from; unsigned requests have their own table below. The
    // operation paths come from each model's `smithy.api#http` bindings
    // (`smithy_query(query="digest", …)` on the six models).
    let rows: Vec<(String, &str, &str, String, &str)> = vec![
        // Each endpoint's own path space, on a bare emulator host that names no endpoint
        // at all: the modelled path is then the only discriminator.
        (
            "messaging DescribeChannelFlow".to_string(),
            "GET",
            "localhost:9297",
            format!("/channel-flows/{}", label(&format!("{APP_INSTANCE}/channel-flow/cf1"))),
            MESSAGING,
        ),
        (
            "messaging CreateChannel".to_string(),
            "POST",
            "localhost:9297",
            "/channels".to_string(),
            MESSAGING,
        ),
        (
            "messaging GetMessagingSessionEndpoint".to_string(),
            "GET",
            "localhost:9297",
            "/endpoints/messaging-session".to_string(),
            MESSAGING,
        ),
        (
            "identity CreateAppInstance".to_string(),
            "POST",
            "localhost:9297",
            "/app-instances".to_string(),
            IDENTITY,
        ),
        (
            "identity CreateAppInstanceUser".to_string(),
            "POST",
            "localhost:9297",
            "/app-instance-users".to_string(),
            IDENTITY,
        ),
        (
            "identity CreateAppInstanceBot".to_string(),
            "POST",
            "localhost:9297",
            "/app-instance-bots".to_string(),
            IDENTITY,
        ),
        (
            "meetings CreateMeeting".to_string(),
            "POST",
            "localhost:9297",
            "/meetings".to_string(),
            MEETINGS,
        ),
        (
            "meetings ListAttendees".to_string(),
            "GET",
            "localhost:9297",
            "/meetings/m1/attendees".to_string(),
            MEETINGS,
        ),
        (
            "legacy chime CreateAccount".to_string(),
            "POST",
            "localhost:9297",
            "/accounts".to_string(),
            CHIME,
        ),
        (
            "legacy chime GetUser".to_string(),
            "GET",
            "localhost:9297",
            "/accounts/a1/users/u1".to_string(),
            CHIME,
        ),
        (
            "media pipelines CreateMediaCapturePipeline".to_string(),
            "POST",
            "localhost:9297",
            "/sdk-media-capture-pipelines".to_string(),
            MEDIA,
        ),
        (
            "media pipelines ListMediaInsightsPipelineConfigurations".to_string(),
            "GET",
            "localhost:9297",
            "/media-insights-pipeline-configurations".to_string(),
            MEDIA,
        ),
        (
            "voice CreateVoiceConnector".to_string(),
            "POST",
            "localhost:9297",
            "/voice-connectors".to_string(),
            VOICE,
        ),
        (
            "voice CreateVoiceProfile".to_string(),
            "POST",
            "localhost:9297",
            "/voice-profiles".to_string(),
            VOICE,
        ),
        (
            "voice ValidateE911Address".to_string(),
            "POST",
            "localhost:9297",
            "/emergency-calling/address".to_string(),
            VOICE,
        ),
        // The ENDPOINT the request addresses decides, so the same path reaches a
        // different service on a different host — including a path the addressed endpoint
        // does not bind, which is its own 4xx to answer rather than a sibling's traffic.
        (
            "messaging DescribeChannelFlow at its own endpoint".to_string(),
            "GET",
            "messaging-chime.us-east-1.amazonaws.com",
            format!("/channel-flows/{}", label(&format!("{APP_INSTANCE}/channel-flow/cf1"))),
            MESSAGING,
        ),
        (
            "messaging DescribeChannelFlow addressed at the legacy chime endpoint".to_string(),
            "GET",
            "chime.us-east-1.amazonaws.com",
            format!("/channel-flows/{}", label(&format!("{APP_INSTANCE}/channel-flow/cf1"))),
            CHIME,
        ),
        (
            "identity CreateAppInstance addressed at the messaging endpoint".to_string(),
            "POST",
            "messaging-chime.us-east-1.amazonaws.com",
            "/app-instances".to_string(),
            MESSAGING,
        ),
        // The `/app-instances` space Identity and Messaging share: Messaging binds only
        // the streaming-configuration sub-space of it, Identity the rest.
        (
            "messaging GetMessagingStreamingConfigurations".to_string(),
            "GET",
            "localhost:9297",
            format!("/app-instances/{}/streaming-configurations", label(APP_INSTANCE)),
            MESSAGING,
        ),
        (
            "messaging PutMessagingStreamingConfigurations".to_string(),
            "PUT",
            "localhost:9297",
            format!("/app-instances/{}/streaming-configurations", label(APP_INSTANCE)),
            MESSAGING,
        ),
        (
            "identity DescribeAppInstance".to_string(),
            "GET",
            "localhost:9297",
            format!("/app-instances/{}", label(APP_INSTANCE)),
            IDENTITY,
        ),
        (
            "identity ListAppInstanceAdmins".to_string(),
            "GET",
            "localhost:9297",
            format!("/app-instances/{}/admins", label(APP_INSTANCE)),
            IDENTITY,
        ),
        (
            "identity GetAppInstanceRetentionSettings".to_string(),
            "GET",
            "localhost:9297",
            format!("/app-instances/{}/retention-settings", label(APP_INSTANCE)),
            IDENTITY,
        ),
        // The `/meetings` space the legacy endpoint and Meetings share: the legacy
        // endpoint binds only the dial-out sub-space of it.
        (
            "legacy chime CreateMeetingDialOut".to_string(),
            "POST",
            "localhost:9297",
            "/meetings/m1/dial-outs".to_string(),
            CHIME,
        ),
        (
            "meetings DeleteMeeting".to_string(),
            "DELETE",
            "localhost:9297",
            "/meetings/m1".to_string(),
            MEETINGS,
        ),
        // The telephony space the legacy endpoint and Voice bind identically: the key a
        // request with no endpoint resolves to IS the legacy endpoint's prefix, so that
        // endpoint answers it, and Voice's half of the space is reached by addressing
        // `voice-chime`.
        (
            "ListPhoneNumbers with no endpoint named".to_string(),
            "GET",
            "localhost:9297",
            "/phone-numbers".to_string(),
            CHIME,
        ),
        (
            "ListPhoneNumbers at the voice endpoint".to_string(),
            "GET",
            "voice-chime.us-east-1.amazonaws.com",
            "/phone-numbers".to_string(),
            VOICE,
        ),
        (
            "GetGlobalSettings with no endpoint named".to_string(),
            "GET",
            "localhost:9297",
            "/settings".to_string(),
            CHIME,
        ),
        (
            "GetGlobalSettings at the voice endpoint".to_string(),
            "GET",
            "voice-chime.us-east-1.amazonaws.com",
            "/settings".to_string(),
            VOICE,
        ),
        (
            "SearchAvailablePhoneNumbers with no endpoint named".to_string(),
            "GET",
            "localhost:9297",
            "/search?type=phone-numbers".to_string(),
            CHIME,
        ),
        (
            "ListSupportedPhoneNumberCountries at the voice endpoint".to_string(),
            "GET",
            "voice-chime.us-east-1.amazonaws.com",
            "/phone-number-countries?product-type=VoiceConnector".to_string(),
            VOICE,
        ),
        (
            "ListPhoneNumberOrders with no endpoint named".to_string(),
            "GET",
            "localhost:9297",
            "/phone-number-orders".to_string(),
            CHIME,
        ),
        // The `/tags` space five of the six bind identically. The resource ARN names the
        // service, and it is in the request BODY on tag and untag, so the endpoint is the
        // only signal the claim seam can read: with one, that endpoint answers; with
        // none, the family's declared tagging owner answers, out of the tag directory
        // every member shares, rather than five claimants in a 500.
        (
            "ListTagsForResource at the messaging endpoint".to_string(),
            "GET",
            "messaging-chime.us-east-1.amazonaws.com",
            format!("/tags?arn={}", label(&format!("{APP_INSTANCE}/channel/c1"))),
            MESSAGING,
        ),
        (
            "ListTagsForResource at the media-pipelines endpoint".to_string(),
            "GET",
            "media-pipelines-chime.us-east-1.amazonaws.com",
            format!(
                "/tags?arn={}",
                label("arn:aws:chime:us-east-1:000000000000:media-pipeline/mp1")
            ),
            MEDIA,
        ),
        (
            "ListTagsForResource at the meetings endpoint".to_string(),
            "GET",
            "meetings-chime.us-east-1.amazonaws.com",
            format!(
                "/tags?arn={}",
                label("arn:aws:chime:us-east-1:000000000000:meeting/m1")
            ),
            MEETINGS,
        ),
        (
            "ListTagsForResource with no endpoint named".to_string(),
            "GET",
            "localhost:9297",
            format!("/tags?arn={}", label(&format!("{APP_INSTANCE}/channel/c1"))),
            IDENTITY,
        ),
        (
            "TagResource with no endpoint named".to_string(),
            "POST",
            "localhost:9297",
            "/tags?operation=tag-resource".to_string(),
            IDENTITY,
        ),
        (
            "UntagResource with no endpoint named".to_string(),
            "POST",
            "localhost:9297",
            "/tags?operation=untag-resource".to_string(),
            IDENTITY,
        ),
        // A path no model in the family binds, and a request signed for a service outside
        // it: neither is any member's, and neither is an ambiguity.
        (
            "a path no family model binds".to_string(),
            "POST",
            "localhost:9297",
            "/NoSuchOperation".to_string(),
            NOBODY,
        ),
    ];

    for (case, method, host, uri, expected) in rows {
        let actual = family.claimant(&parts(method, host, &uri, Some(AUTHORIZATION)));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}, signed for `chime`): expected it to be \
             claimed by {expected}, got {actual}"
        );
    }

    // UNSIGNED requests, which carry no credential scope at all: the framework resolves
    // their routing key from the endpoint host, so the family is reached through the
    // endpoint-prefix half of the rule and the SAME endpoint still claims each one. A
    // bare emulator host names no service, so such a request reaches nobody.
    let unsigned: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "unsigned at the messaging endpoint",
            "GET",
            "messaging-chime.eu-west-1.amazonaws.com",
            "/channel-flows",
            MESSAGING,
        ),
        (
            "unsigned at the identity endpoint",
            "POST",
            "identity-chime.us-east-1.amazonaws.com",
            "/app-instances",
            IDENTITY,
        ),
        (
            "unsigned at the voice endpoint, on the shared telephony space",
            "GET",
            "voice-chime.us-east-1.amazonaws.com",
            "/phone-numbers",
            VOICE,
        ),
        (
            "unsigned at the legacy chime endpoint",
            "GET",
            "chime.us-east-1.amazonaws.com",
            "/accounts",
            CHIME,
        ),
        (
            "unsigned on a bare emulator host, which names no service",
            "GET",
            "localhost:9297",
            "/channel-flows",
            NOBODY,
        ),
    ];
    for (case, method, host, uri, expected) in unsigned {
        let actual = family.claimant(&parts(method, host, uri, None));
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}, unsigned): expected it to be claimed \
             by {expected}, got {actual}"
        );
    }

    // Signed for a service OUTSIDE the family, and addressed at a family endpoint: no
    // member of the family may claim it, however its path reads — otherwise narrowing the
    // claim would have made these services claim another service's traffic.
    for (case, host, uri) in [
        (
            "sqs-signed at the messaging endpoint",
            "messaging-chime.us-east-1.amazonaws.com",
            "/channel-flows",
        ),
        ("sqs-signed on a bare emulator host", "localhost:9297", "/channels"),
        ("sqs-signed at the legacy chime endpoint", "chime.us-east-1.amazonaws.com", "/accounts"),
    ] {
        let actual = family.claimant(&parts("GET", host, uri, Some(SQS_AUTHORIZATION)));
        assert_eq!(
            actual, NOBODY,
            "case {case:?} (GET {uri} to host {host}): a request signed for another service is \
             claimed by no Chime service, got {actual}"
        );
    }
}

#[tokio::test]
async fn every_chime_signed_request_is_dispatched_instead_of_refused_as_ambiguous() {
    let router = router_with_the_family(false);

    // (case, method, host, uri, body) — one request per service in the family. What each
    // row asserts is that a SERVICE answered: neither of the router's own refusals may
    // appear, because each one means the request reached nobody (`UnknownService`) or
    // every claimant (`AmbiguousService`). An operation this emulator has not written yet
    // answers the scaffold's `501 NotImplemented`, which is also an answer from the
    // service that owns the path.
    let rows: Vec<(&str, &str, &str, String, Value)> = vec![
        (
            "identity CreateAppInstance",
            "POST",
            "localhost:9297",
            "/app-instances".to_string(),
            json!({"Name": "ai", "ClientRequestToken": "token-1"}),
        ),
        (
            "messaging CreateChannel",
            "POST",
            "localhost:9297",
            "/channels".to_string(),
            json!({"AppInstanceArn": APP_INSTANCE, "Name": "c", "ClientRequestToken": "token-2"}),
        ),
        (
            "messaging DescribeChannelFlow",
            "GET",
            "localhost:9297",
            format!(
                "/channel-flows/{}",
                label(&format!("{APP_INSTANCE}/channel-flow/cf1"))
            ),
            Value::Null,
        ),
        (
            "meetings CreateMeeting",
            "POST",
            "localhost:9297",
            "/meetings".to_string(),
            json!({"ClientRequestToken": "token-3", "MediaRegion": "us-east-1", "ExternalMeetingId": "e1"}),
        ),
        (
            "legacy chime ListAccounts",
            "GET",
            "localhost:9297",
            "/accounts".to_string(),
            Value::Null,
        ),
        (
            "legacy chime ListPhoneNumbers",
            "GET",
            "localhost:9297",
            "/phone-numbers".to_string(),
            Value::Null,
        ),
        (
            "voice ListPhoneNumbers at its own endpoint",
            "GET",
            "voice-chime.us-east-1.amazonaws.com",
            "/phone-numbers".to_string(),
            Value::Null,
        ),
        (
            "voice ListVoiceConnectors",
            "GET",
            "localhost:9297",
            "/voice-connectors".to_string(),
            Value::Null,
        ),
        (
            "media pipelines ListMediaPipelines",
            "GET",
            "localhost:9297",
            "/sdk-media-pipelines".to_string(),
            Value::Null,
        ),
        (
            // The generic tagging space, which names no service in the request: the
            // family's declared tagging owner answers it, so a client pointed at the
            // emulator reaches the operation instead of the router's refusal.
            "the shared tagging space with no endpoint named",
            "POST",
            "localhost:9297",
            "/tags?operation=tag-resource".to_string(),
            json!({"ResourceARN": APP_INSTANCE, "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
    ];

    for (case, method, host, uri, body) in rows {
        let (status, code, _content_type, parsed) =
            call(&router, method, host, &uri, body).await;
        assert!(
            code != "AmbiguousService" && code != "UnknownService",
            "case {case:?}: {method} {uri} must be served by exactly one of the six services \
             sharing the `chime` signing name, got HTTP {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn describe_channel_flow_answers_the_modelled_forbidden_exception() {
    // The Lambda service is mounted beside the family, because `CreateChannelFlow`
    // resolves each processor's Lambda function through the in-process callback.
    let router = router_with_the_family(true);
    let flow_arn = format!("{APP_INSTANCE}/channel-flow/cf-uninvokable");

    // A channel flow whose processor names a Lambda function this emulator does not hold
    // is refused, and nothing is written — which is why the describe below finds no flow.
    let (status, code, content_type, parsed) = call(
        &router,
        "POST",
        "messaging-chime.us-east-1.amazonaws.com",
        "/channel-flows",
        json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": "uninvokable",
            "ClientRequestToken": "token-uninvokable",
            "Processors": [{
                "Name": "p1",
                "ExecutionOrder": 1,
                "FallbackAction": "ABORT",
                "Configuration": {"Lambda": {
                    "ResourceArn": "arn:aws:lambda:us-east-1:000000000000:function:not-created",
                    "InvocationType": "ASYNC"
                }}
            }]
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequestException"),
        "case \"CreateChannelFlow with a processor Lambda the emulator does not hold\": expected \
         the modelled BadRequestException (HTTP 400), got {status} {code:?} ({parsed})"
    );
    assert_eq!(
        content_type, "application/json",
        "case \"CreateChannelFlow refusal\": a restJson1 error is framed as application/json, got \
         {content_type:?}"
    );

    // And describing a channel flow ARN that names no stored flow is the modelled
    // ForbiddenException: a TERMINAL 403 with `Code` `Forbidden` in a restJson1 envelope —
    // never a retryable 5xx, and never an error code no model declares.
    for (case, host) in [
        ("addressed at the messaging endpoint", "messaging-chime.us-east-1.amazonaws.com"),
        ("on a bare emulator host", "localhost:9297"),
    ] {
        let (status, code, content_type, parsed) = call(
            &router,
            "GET",
            host,
            &format!("/channel-flows/{}", label(&flow_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            (
                status,
                parsed.get("Code").and_then(Value::as_str).unwrap_or("<no Code>")
            ),
            (403, "Forbidden"),
            "case {case:?}: expected HTTP 403 with Code \"Forbidden\" from DescribeChannelFlow, \
             got {status} {code:?} ({parsed})"
        );
        assert_eq!(
            content_type, "application/json",
            "case {case:?}: a restJson1 error is framed as application/json, got {content_type:?}"
        );
        assert!(
            parsed.get("Message").and_then(Value::as_str).is_some(),
            "case {case:?}: the modelled ForbiddenException carries a Message, got {parsed}"
        );
    }
}

/// RULE: what the claim rule resolves is the HANDLER and its store — so the request the
/// whole family used to refuse as `AmbiguousService`, Chime SDK Identity's
/// `CreateAppInstance` (restJson1 `POST /app-instances`, modelled code 201), is SERVED by
/// that one service: it answers 201 with the ARN it minted, and that ARN reads back
/// through the same router.
///
/// Appended here rather than in a second suite of its own: this file already mounts the
/// family and owns `router_with_the_family` / `call`, and a claim table cannot show that
/// the resolved claimant then answers from state.
///
/// The rows span how the request names the service — a bare emulator host, where only
/// this model's path space names the claimant, and this service's own endpoint host —
/// and both readers the created instance is visible through: the collection
/// (`GET /app-instances`) and the resource (`GET /app-instances/{AppInstanceArn}`).
#[tokio::test]
async fn create_app_instance_is_served_by_chime_sdk_identity_and_reads_back() {
    let router = router_with_the_family(false);

    for (case, host) in [
        ("on a bare emulator host", "localhost:9297"),
        (
            "addressed at the identity endpoint",
            "identity-chime.us-east-1.amazonaws.com",
        ),
    ] {
        let (status, code, content_type, parsed) = call(
            &router,
            "POST",
            host,
            "/app-instances",
            json!({
                "Name": format!("family-router-{}", host.split('.').next().unwrap_or(host)),
                "ClientRequestToken": format!("token-{}", host.len()),
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (201, ""),
            "case {case:?}: CreateAppInstance must be served by Chime SDK Identity with its \
             modelled 201, got {status} {code:?} ({parsed})"
        );
        assert_eq!(
            content_type, "application/json",
            "case {case:?}: a restJson1 response is framed as application/json, got \
             {content_type:?}"
        );
        let arn = parsed
            .get("AppInstanceArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with("arn:aws:chime:"),
            "case {case:?}: the created app instance reports the ARN it minted, got {parsed}"
        );

        // The collection reader, through the same router: a routing failure carries no
        // `AppInstances`, and no sibling binds this route.
        let (status, code, _content_type, listed) =
            call(&router, "GET", host, "/app-instances", Value::Null).await;
        let arns: Vec<&str> = listed
            .get("AppInstances")
            .and_then(Value::as_array)
            .map(|instances| {
                instances
                    .iter()
                    .filter_map(|instance| {
                        instance.get("AppInstanceArn").and_then(Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: ListAppInstances must be served by Chime SDK Identity, got \
             {status} {code:?} ({listed})"
        );
        assert!(
            arns.contains(&arn.as_str()),
            "case {case:?}: the created app instance must read back through the router, \
             {arn} not in {arns:?}"
        );

        // And the resource reader, whose URI carries the ARN as one percent-encoded label.
        let (status, code, _content_type, described) =
            call(&router, "GET", host, &format!("/app-instances/{}", label(&arn)), Value::Null)
                .await;
        assert_eq!(
            (
                status,
                code.as_str(),
                described
                    .pointer("/AppInstance/AppInstanceArn")
                    .and_then(Value::as_str)
                    .unwrap_or("<no AppInstanceArn>")
            ),
            (200, "", arn.as_str()),
            "case {case:?}: DescribeAppInstance must report the instance just created, got \
             {status} {code:?} ({described})"
        );
    }
}
