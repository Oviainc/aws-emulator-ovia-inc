// SPDX-License-Identifier: CC0-1.0

//! Amazon Transcribe and Amazon Transcribe Streaming share one SigV4 signing name —
//! each must still be claimed by exactly one service in the assembled router.
//!
//! Both services declare `aws.auth#sigv4` name `transcribe` (the control plane's
//! endpoint prefix is `transcribe`, the streaming service's is `transcribestreaming`),
//! and the router resolves a request's routing key from the SigV4 credential scope
//! FIRST, so both see the same key. A service that claims on the signing name alone
//! therefore claims its peer's traffic too, the router finds two claimants and answers
//! `500 AmbiguousService` — which made BOTH services unreachable, every Transcribe call
//! answering a retryable server fault instead of its modelled 200 or 4xx.
//!
//! Their modelled URI path spaces are disjoint, so each crate's `can_handle` adds the
//! framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements, the same one
//! `service_common::claims_request` applies for a REST service) over its own generated
//! `ROUTE_PATH_SEGMENTS`: the `awsJson1_1` control plane claims the ROOT path its
//! `POST /` traffic arrives on plus the literal segments of its (wire-inert) `@http`
//! bindings, and the `restJson1` streaming service claims the four URIs its operations
//! are bound to. This suite pins that split in the production router, across both
//! protocol envelopes, both routing tiers (credential scope and `X-Amz-Target`) and
//! either endpoint host.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `transcribe` signing name, which is what both
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/transcribe/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request both services could claim on their signing name.
enum Expect {
    /// Served by the service the case names, with this status, and a body containing
    /// this marker. The marker is a string ONLY that service can produce — an output
    /// member or an error code the other service's model does not carry — so it
    /// identifies which crate answered.
    Serves {
        who: &'static str,
        status: u16,
        marker: &'static str,
    },
    /// Claimed by exactly one of the two: neither `AmbiguousService` (both claimed) nor
    /// `UnknownService` (neither did). All that a request to a still-scaffolded
    /// operation guarantees — such an operation answers the scaffold's
    /// `501 NotImplemented`, which is its own issue, not a routing failure.
    ClaimedByExactlyOne,
}

/// One routing case: the request, and what the router must do with it.
struct Case {
    name: &'static str,
    host: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    /// `X-Amz-Target`, for the awsJson1_1 control-plane envelope.
    target: Option<&'static str>,
    /// Whether to sign for `transcribe`. A case with `false` carries no credential
    /// scope, so the router falls through to the `X-Amz-Target` routing tier.
    signed: bool,
    body: &'static str,
    expect: Expect,
}

/// A `StartTranscriptionJob` input: `TranscriptionJobName` matches
/// `^[0-9a-zA-Z._-]+$` and `Media.MediaFileUri` matches `^(s3://|http(s*)://).+$`.
const START_JOB_BODY: &str = r#"{"TranscriptionJobName":"routing-check-job",
    "LanguageCode":"en-US","Media":{"MediaFileUri":"s3://routing-check/audio.mp3"}}"#;

/// The rows differ along what the rule generalises over: which service owns the path,
/// the protocol envelope (awsJson1_1 `POST /` with a target vs restJson1 method+URI),
/// the routing tier the key comes from (credential scope vs `X-Amz-Target`), the
/// endpoint host, and the first path segment (owned, near-miss, foreign-case).
const CASES: &[Case] = &[
    Case {
        // The control plane's own `POST /` traffic, on its endpoint host. This is the
        // minimal repro the issue reports as 500 `AmbiguousService`; `ListVocabularies`
        // there is still scaffolded, so the row that pins a SERVED answer uses an
        // implemented operation of the same envelope.
        name: "control plane ListTranscriptionJobs on the control-plane host",
        host: "transcribe.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.ListTranscriptionJobs"),
        signed: true,
        body: "{}",
        expect: Expect::Serves {
            who: "the Transcribe control plane",
            status: 200,
            marker: "TranscriptionJobSummaries",
        },
    },
    Case {
        // The same operation on a loopback endpoint: the host does not decide, the
        // modelled path space does.
        name: "control plane ListTranscriptionJobs on a loopback endpoint",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.ListTranscriptionJobs"),
        signed: true,
        body: "{}",
        expect: Expect::Serves {
            who: "the Transcribe control plane",
            status: 200,
            marker: "TranscriptionJobSummaries",
        },
    },
    Case {
        // …and even on the STREAMING service's endpoint host, for the same reason.
        name: "control plane ListTranscriptionJobs on the streaming host",
        host: "transcribestreaming.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.ListTranscriptionJobs"),
        signed: true,
        body: "{}",
        expect: Expect::Serves {
            who: "the Transcribe control plane",
            status: 200,
            marker: "TranscriptionJobSummaries",
        },
    },
    Case {
        // An UNSIGNED request: with no credential scope the router's next tier is
        // `X-Amz-Target`, whose prefix `Transcribe` resolves to the same shared key, so
        // the claim must still split the same way.
        name: "control plane ListTranscriptionJobs routed by X-Amz-Target alone",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.ListTranscriptionJobs"),
        signed: false,
        body: "{}",
        expect: Expect::Serves {
            who: "the Transcribe control plane",
            status: 200,
            marker: "TranscriptionJobSummaries",
        },
    },
    Case {
        // A control-plane ERROR path, which the issue reports as 500 `AmbiguousService`
        // too. Only the claim is pinned: WHICH modelled 4xx this handler answers for an
        // unknown job name belongs to that operation's own issue (the recording shows
        // the real service answering `BadRequestException` on Transcribe's not-found
        // paths, while the model declares `NotFoundException` for it), so this suite
        // must not cement either code.
        name: "control plane GetTranscriptionJob for a job that does not exist",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.GetTranscriptionJob"),
        signed: true,
        body: r#"{"TranscriptionJobName":"no-such-routing-check-job"}"#,
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // A control-plane MUTATION: dispatch must reach the handler that writes, not
        // only the readers.
        name: "control plane StartTranscriptionJob",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.StartTranscriptionJob"),
        signed: true,
        body: START_JOB_BODY,
        expect: Expect::Serves {
            who: "the Transcribe control plane",
            status: 200,
            marker: "TranscriptionJobStatus",
        },
    },
    Case {
        // A control-plane operation that is still a scaffold: all that is pinned here is
        // that ONE service claimed it, so its answer is its own 501 rather than a
        // routing failure.
        name: "control plane ListVocabularies, still scaffolded",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/",
        target: Some("Transcribe.ListVocabularies"),
        signed: true,
        body: "{}",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // One of the control plane's literal (wire-inert) `@http` first segments. No
        // awsJson operation is named without a target, so the answer is that service's
        // own routing failure — the point is that the request is not ambiguous and does
        // not fall through to the streaming service.
        name: "control plane modelled REST path, no X-Amz-Target",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/transcriptionjobs/routing-check-job",
        target: None,
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // The streaming service's `GetMedicalScribeStream`: its modelled
        // `ResourceNotFoundException` is an error code the control-plane model does not
        // carry, so a 404 with it can only have come from the streaming service.
        name: "streaming GetMedicalScribeStream for an unknown session",
        host: "transcribestreaming.us-east-1.amazonaws.com",
        method: "GET",
        path_and_query: "/medical-scribe-stream/99999999-8888-4777-8666-555555555555",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Transcribe Streaming",
            status: 404,
            marker: "ResourceNotFoundException",
        },
    },
    Case {
        // The same streaming request on the CONTROL PLANE's endpoint host: again the
        // path decides, not the host.
        name: "streaming GetMedicalScribeStream on the control-plane host",
        host: "transcribe.us-east-1.amazonaws.com",
        method: "GET",
        path_and_query: "/medical-scribe-stream/99999999-8888-4777-8666-555555555555",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Transcribe Streaming",
            status: 404,
            marker: "ResourceNotFoundException",
        },
    },
    Case {
        // Each remaining streaming URI: claimed by the streaming service, which refuses
        // a start request carrying none of the required headers with its own 400. The
        // row pins only that exactly one service claimed it.
        name: "streaming StartStreamTranscription",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/stream-transcription",
        target: None,
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "streaming StartMedicalStreamTranscription",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/medical-stream-transcription",
        target: None,
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "streaming StartMedicalScribeStream",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/medical-scribe-stream",
        target: None,
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // The one streaming operation this emulator does not serve still belongs to the
        // streaming service's path space, so it must reach it (and answer its
        // "modelled but not served" response) rather than being ambiguous.
        name: "streaming StartCallAnalyticsStreamTranscription",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/call-analytics-stream-transcription",
        target: None,
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
];

#[test]
fn transcribe_and_transcribe_streaming_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("transcribe-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these cases are about which service claims a request.
                router.set_request_authentication(false);

                for case in CASES {
                    let mut builder = http::Request::builder()
                        .method(case.method)
                        .uri(format!("https://{}{}", case.host, case.path_and_query))
                        .header("host", case.host)
                        .header("content-type", "application/x-amz-json-1.1");
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    if let Some(target) = case.target {
                        builder = builder.header("x-amz-target", target);
                    }
                    let request = builder
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    match &case.expect {
                        Expect::Serves {
                            who,
                            status: expected_status,
                            marker,
                        } => {
                            assert_eq!(
                                status, *expected_status,
                                "case {:?}: expected HTTP {expected_status} from {who}, got \
                                 {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains(*marker),
                                "case {:?}: expected a body from {who} carrying {marker:?}, \
                                 got {status} {body}",
                                case.name
                            );
                        }
                        Expect::ClaimedByExactlyOne => {}
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
