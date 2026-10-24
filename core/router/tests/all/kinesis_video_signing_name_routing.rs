// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `kinesisvideo`, which FIVE emulated
//! services share.
//!
//! The sibling suite `kinesisvideo_signing_name_routing.rs` pins one route per service and
//! the envelope and state round trip behind them. This suite pins the CLAIM RULE itself at
//! the front door, over the dimensions that one does not vary: every first path segment the
//! five models bind (including the PREFIX trap below), the segment's case, the signal the
//! routing key is resolved from (credential scope, then endpoint host), a Kinesis Video
//! route signed for a different service, and the root path. The two overlap deliberately on
//! `CreateStream`, the call this issue reports as unreachable.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every Kinesis Video service sees the same key `kinesisvideo`
//! (`smithy_query(query="service_info")` gives all five `endpointPrefix` and `sigv4Name`
//! `kinesisvideo`): the control plane `Kinesis Video`, `Kinesis Video Media`, `Kinesis
//! Video Archived Media`, `Kinesis Video WebRTC Storage` and `Kinesis Video Signaling`. A
//! service that claims on the signing name ALONE claims its four peers' traffic too, the
//! router finds five claimants and answers `500 AmbiguousService` for all of it — so the
//! whole family, control plane included, is unreachable and no `CreateStream` can even be
//! set up.
//!
//! AWS separates these five by HTTP method and request URI, as their models declare, and
//! their modelled first URI path segments are DISJOINT. So each crate's `can_handle` adds
//! the framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements) over its own
//! generated `ROUTE_PATH_SEGMENTS`, and this suite pins the result in the assembled
//! router: every route one of the five models binds reaches exactly that service, a first
//! segment no Kinesis Video model binds is claimed by none of them, and
//! `AmbiguousService` is never emitted for this prefix.
//!
//! The rows deliberately include `/getMediaForFragmentList`, whose first segment
//! (`getMediaForFragmentList`) has the Media data plane's only segment (`getMedia`) as a
//! PREFIX: matching is exact segment equality, so only Archived Media may claim it.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// The signing region every request in this suite is scoped to.
const REGION: &str = "us-east-1";

/// The stream the control plane creates up front, then every other row reads against.
const STREAM_NAME: &str = "routing-check-stream";

/// A signaling-channel ARN satisfying the modelled `ChannelArn` `@pattern`
/// `^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`.
const CHANNEL_ARN: &str =
    "arn:aws:kinesisvideo:us-east-1:000000000000:channel/routing-check-channel/1700000000000";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The five services that share the `kinesisvideo` signing name, mounted on ONE engine,
/// exactly as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideowebrtcstorage_awsacuityroutingservicelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One request to dispatch: the signing name its credential scope names, the endpoint host
/// it is addressed at, and the route and body it carries.
struct Call {
    signing_name: String,
    host: String,
    path: String,
    body: String,
}

impl Call {
    /// A `kinesisvideo`-signed POST to `path`, addressed at the emulator's own endpoint.
    fn post(path: &str, body: &str) -> Self {
        Self {
            signing_name: "kinesisvideo".to_string(),
            host: "emulator.local".to_string(),
            path: path.to_string(),
            body: body.to_string(),
        }
    }

    /// The same call, addressed at `host` — the endpoint a client was configured with,
    /// which must not change which service serves it.
    fn at_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    /// The same call, signed for `signing_name` instead.
    fn signed_for(mut self, signing_name: &str) -> Self {
        self.signing_name = signing_name.to_string();
        self
    }
}

/// What the router must do with a request every one of the five services could claim on
/// the shared signing name alone.
enum Expect {
    /// The owning service's OWN answer: this exact status and wire error code (`None` on a
    /// success). Only the one service whose model binds the route can produce it.
    Answer(u16, Option<&'static str>),
    /// The route is bound by exactly one of the five, but its operation is still a
    /// scaffold — and a `501 NotImplemented` is also what a wrongly claiming peer renders
    /// for a route its own routing table cannot name, so the two are indistinguishable.
    /// What is asserted is the selection itself: exactly ONE service claimed the request,
    /// so neither `AmbiguousService` (two or more claimed) nor `UnknownService` (none
    /// did).
    ExactlyOneClaimant,
    /// No Kinesis Video model binds this request's first path segment, so none of the five
    /// may claim it: `404 UnknownService` is correct and `AmbiguousService` is not.
    NoClaimant,
}

/// The wire error code of a response: `x-amzn-errortype` (restJson1 and the router's own
/// refusals) reduced to its shape name, else the `__type` of a JSON error document, and
/// `None` when the response carries no error code at all.
fn error_code(response: &http::Response<Bytes>) -> Option<String> {
    let from_header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|raw| {
            // `x-amzn-errortype` may carry `Shape:<url>` or a namespaced `ns#Shape`.
            let before_url = raw.split(':').next().unwrap_or(raw);
            before_url
                .split('#')
                .next_back()
                .unwrap_or(before_url)
                .to_string()
        })
        .filter(|code| !code.is_empty());
    from_header.or_else(|| {
        serde_json::from_slice::<serde_json::Value>(response.body())
            .ok()
            .and_then(|json| {
                json.get("__type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
    })
}

async fn dispatch(router: &MultiServiceRouter, call: &Call) -> http::Response<Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{}{}", call.host, call.path))
        .header("host", call.host.clone())
        .header("authorization", authorization(&call.signing_name))
        .header("content-type", "application/json")
        .body(Bytes::from(call.body.clone()))
        .expect("the request is well formed");
    router.dispatch(request).await
}

#[tokio::test]
async fn every_kinesis_video_route_is_claimed_by_exactly_one_of_the_five_services() {
    let router = build_router();

    // The control plane's own route, which the issue reports as 500 AmbiguousService. Its
    // 200 carrying a `StreamARN` can only come from the Kinesis Video control plane: no
    // other Kinesis Video model binds `/createStream` at all.
    let created = dispatch(
        &router,
        &Call::post(
            "/createStream",
            &format!(r#"{{"StreamName":"{STREAM_NAME}","DataRetentionInHours":1}}"#),
        ),
    )
    .await;
    let created_status = created.status().as_u16();
    let created_body = String::from_utf8_lossy(created.body()).to_string();
    assert_eq!(
        created_status, 200,
        "case \"CreateStream\": expected the control plane to serve it, got {created_status} \
         {created_body}"
    );
    let stream_arn = serde_json::from_str::<serde_json::Value>(&created_body)
        .ok()
        .and_then(|json| {
            json.get("StreamARN")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            panic!("case \"CreateStream\": expected a StreamARN, got {created_body}")
        });

    // (case, call, expectation). The rows span all five services' modelled path spaces,
    // differing along the one thing that separates them — the first URI path segment —
    // plus the endpoint host (which must not matter), the signing name (which must), and
    // segments no model binds.
    let rows: Vec<(&str, Call, Expect)> = vec![
        (
            // The stream just created reads back through the control plane's own reader.
            "control plane DescribeStream by name",
            Call::post(
                "/describeStream",
                &format!(r#"{{"StreamName":"{STREAM_NAME}"}}"#),
            ),
            Expect::Answer(200, None),
        ),
        (
            "control plane DescribeStream by ARN",
            Call::post(
                "/describeStream",
                &format!(r#"{{"StreamARN":"{stream_arn}"}}"#),
            ),
            Expect::Answer(200, None),
        ),
        (
            // The call a client makes to find the data plane at all.
            "control plane GetDataEndpoint for GET_MEDIA",
            Call::post(
                "/getDataEndpoint",
                &format!(r#"{{"StreamName":"{STREAM_NAME}","APIName":"GET_MEDIA"}}"#),
            ),
            Expect::Answer(200, None),
        ),
        (
            "control plane ListStreams",
            Call::post("/listStreams", "{}"),
            Expect::Answer(200, None),
        ),
        (
            // Addressed at the data-plane host a GetDataEndpoint answer points to: the
            // credential scope, not the host, resolves the routing key, so the control
            // plane still serves its own route.
            "control plane DescribeStream on the data endpoint host",
            Call::post(
                "/describeStream",
                &format!(r#"{{"StreamName":"{STREAM_NAME}"}}"#),
            )
            .at_host("kinesisvideo.us-east-1.amazonaws.com"),
            Expect::Answer(200, None),
        ),
        (
            // `/getMedia` is the Media data plane's ONLY route, and a stream name no call
            // in this suite created resolves to nothing there — so its own modelled 404 is
            // the answer AWS gives too, and only it can give it here: the control plane
            // does not bind `/getMedia`, and a routing failure is
            // `AmbiguousService`/`UnknownService`, not `ResourceNotFoundException`.
            "media GetMedia on a stream nothing created",
            Call::post(
                "/getMedia",
                r#"{"StreamName":"no-such-stream","StartSelector":{"StartSelectorType":"EARLIEST"}}"#,
            ),
            Expect::Answer(404, Some("ResourceNotFoundException")),
        ),
        (
            // The same route for the stream the control plane just created. What is
            // asserted is the selection only: the data plane's fragment log is written by
            // its own producer entry point, so what it answers for a control-plane stream
            // is that separate store question's to settle, not this routing rule's.
            "media GetMedia by name",
            Call::post(
                "/getMedia",
                &format!(
                    r#"{{"StreamName":"{STREAM_NAME}","StartSelector":{{"StartSelectorType":"EARLIEST"}}}}"#
                ),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "media GetMedia by ARN",
            Call::post(
                "/getMedia",
                &format!(
                    r#"{{"StreamARN":"{stream_arn}","StartSelector":{{"StartSelectorType":"NOW"}}}}"#
                ),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "media GetMedia with a trailing path segment",
            Call::post(
                "/getMedia/",
                &format!(
                    r#"{{"StreamName":"{STREAM_NAME}","StartSelector":{{"StartSelectorType":"EARLIEST"}}}}"#
                ),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // Archived Media's `/listFragments` is implemented, so its own modelled 404
            // for an unknown stream proves which service answered.
            "archived media ListFragments on an unknown stream",
            Call::post(
                "/listFragments",
                r#"{"StreamName":"no-such-stream","FragmentSelector":{"FragmentSelectorType":"PRODUCER_TIMESTAMP","TimestampRange":{"StartTimestamp":1700000000,"EndTimestamp":1700000100}}}"#,
            ),
            Expect::Answer(404, Some("ResourceNotFoundException")),
        ),
        (
            // THE PREFIX TRAP: `getMedia` is a prefix of `getMediaForFragmentList`, so a
            // `starts_with` test would have BOTH the Media and the Archived Media service
            // claim this and reinstate the ambiguity. Exact segment equality gives it to
            // Archived Media alone.
            "archived media GetMediaForFragmentList",
            Call::post(
                "/getMediaForFragmentList",
                &format!(r#"{{"StreamName":"{STREAM_NAME}","Fragments":["1"]}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "archived media GetClip",
            Call::post(
                "/getClip",
                &format!(
                    r#"{{"StreamName":"{STREAM_NAME}","ClipFragmentSelector":{{"FragmentSelectorType":"PRODUCER_TIMESTAMP","TimestampRange":{{"StartTimestamp":1700000000,"EndTimestamp":1700000100}}}}}}"#
                ),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "archived media GetHLSStreamingSessionURL",
            Call::post(
                "/getHLSStreamingSessionURL",
                &format!(r#"{{"StreamName":"{STREAM_NAME}"}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // Still a scaffold (its own stub issue), so only the selection is asserted.
            "archived media GetDASHStreamingSessionURL",
            Call::post(
                "/getDASHStreamingSessionURL",
                &format!(r#"{{"StreamName":"{STREAM_NAME}"}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "archived media GetImages",
            Call::post(
                "/getImages",
                &format!(r#"{{"StreamName":"{STREAM_NAME}"}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // WebRTC Storage's `/joinStorageSession` is implemented and answers an empty
            // 200 for a well-formed channel ARN — an answer no peer's route produces.
            "webrtc storage JoinStorageSession",
            Call::post(
                "/joinStorageSession",
                &format!(r#"{{"channelArn":"{CHANNEL_ARN}"}}"#),
            ),
            Expect::Answer(200, None),
        ),
        (
            "webrtc storage JoinStorageSessionAsViewer",
            Call::post(
                "/joinStorageSessionAsViewer",
                &format!(r#"{{"channelArn":"{CHANNEL_ARN}","clientId":"viewer-1"}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // Signaling binds both its operations under `/v1/…`, a first segment no other
            // Kinesis Video model uses. Both are served from that crate's own channel
            // registry now; this suite asserts only the selection, and the signaling
            // crate's own suite asserts what each answers.
            "signaling GetIceServerConfig",
            Call::post(
                "/v1/get-ice-server-config",
                &format!(r#"{{"ChannelARN":"{CHANNEL_ARN}"}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            "signaling SendAlexaOfferToMaster",
            Call::post(
                "/v1/send-alexa-offer-to-master",
                &format!(
                    r#"{{"ChannelARN":"{CHANNEL_ARN}","SenderClientId":"alexa","MessagePayload":"b2ZmZXI="}}"#
                ),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // A control-plane segment the model spells with a capital, so the claim is
            // case-sensitive exactly as HTTP paths are. The operation is a scaffold, so
            // only the selection is asserted.
            "control plane TagResource",
            Call::post(
                "/TagResource",
                &format!(r#"{{"ResourceARN":"{stream_arn}","Tags":[{{"Key":"k","Value":"v"}}]}}"#),
            ),
            Expect::ExactlyOneClaimant,
        ),
        (
            // No Kinesis Video model binds this first segment.
            "a first segment no Kinesis Video model binds",
            Call::post("/notAKinesisVideoRoute", "{}"),
            Expect::NoClaimant,
        ),
        (
            // The same route in the wrong case is a DIFFERENT segment: no model binds it.
            "getMedia in the wrong case",
            Call::post("/getmedia", "{}"),
            Expect::NoClaimant,
        ),
        (
            // The root path: every one of the five publishes a non-empty segment list, so
            // none of them claims `POST /`.
            "the root path",
            Call::post("/", "{}"),
            Expect::NoClaimant,
        ),
        (
            // The path conjunct never replaces the signing name: a Kinesis Video route
            // signed for another service is claimed by none of the five.
            "a Kinesis Video route signed for another service",
            Call::post("/getMedia", "{}").signed_for("kinesis"),
            Expect::NoClaimant,
        ),
    ];

    for (case, call, expect) in &rows {
        let response = dispatch(&router, call).await;
        let status = response.status().as_u16();
        let code = error_code(&response);
        let body = String::from_utf8_lossy(response.body()).to_string();

        assert_ne!(
            code.as_deref(),
            Some("AmbiguousService"),
            "case {case:?}: more than one service claimed {}, got {status} {body}",
            call.path
        );
        match expect {
            Expect::Answer(expected_status, expected_code) => {
                assert_eq!(
                    (status, code.as_deref()),
                    (*expected_status, *expected_code),
                    "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the \
                     one service that owns {}, got {status} {code:?} {body}",
                    call.path
                );
            }
            Expect::ExactlyOneClaimant => {
                assert!(
                    !matches!(code.as_deref(), Some("UnknownService")),
                    "case {case:?}: no service claimed {}, got {status} {body}",
                    call.path
                );
            }
            Expect::NoClaimant => {
                assert_eq!(
                    (status, code.as_deref()),
                    (404, Some("UnknownService")),
                    "case {case:?}: no Kinesis Video model binds {}, so the router must \
                     answer 404 UnknownService, got {status} {code:?} {body}",
                    call.path
                );
            }
        }
    }
}
