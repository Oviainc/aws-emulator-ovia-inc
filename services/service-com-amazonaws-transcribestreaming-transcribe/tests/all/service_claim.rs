// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Amazon Transcribe Streaming signs as `transcribe`, and so does the Amazon Transcribe
//! control plane (`services/service-com-amazonaws-transcribe-transcribe`): a different
//! service, with its own protocol (`awsJson1_1`), endpoint prefix (`transcribe`) and 44
//! operations. The router resolves a request's routing key from the SigV4 credential
//! scope first, then from `X-Amz-Target`, so both services see the key `transcribe`;
//! claiming on that key alone makes each service claim the other's traffic, the router
//! finds two claimants and answers `500 AmbiguousService` for every Transcribe request
//! instead of serving either service.
//!
//! The claim is therefore the routing key AND the request's first URI path segment,
//! matched by exact equality against this service's model-derived
//! `ROUTE_PATH_SEGMENTS` — the four URIs its `restJson1` operations are bound to — or
//! against the data-plane paths of their WebSocket transport, which the model does not
//! describe (each modeled URI plus `-websocket`). This suite pins that rule over the
//! class of requests it generalises over: both routing tiers, each of its own bound
//! URIs, each WebSocket path, the peer's root path and modelled segments, near-miss
//! spellings, and requests that name no service at all.

use std::sync::Arc;

use service_com_amazonaws_transcribestreaming_transcribe::EmulatedService;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming `signing_name`; the signature is never inspected by
/// the routing key.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The request parts `can_handle` is handed: the URI, and the headers the routing key
/// is read from.
fn parts(
    path_and_query: &str,
    signing_name: Option<&str>,
    target: Option<&str>,
) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:9297{path_and_query}"))
        .header("host", "127.0.0.1:9297");
    if let Some(signing_name) = signing_name {
        builder = builder.header("authorization", authorization(signing_name));
    }
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(())
        .expect("the request is well formed")
        .into_parts()
        .0
}

/// One claim case: the request, and whether THIS service must claim it.
struct Case {
    name: &'static str,
    path_and_query: &'static str,
    signing_name: Option<&'static str>,
    target: Option<&'static str>,
    claimed: bool,
}

/// The rows differ along what the rule generalises over: the routing tier the key comes
/// from, the first path segment (each of this service's own bound URIs, the peer's root
/// path and modelled segments, a near miss), the presence of a query string or a
/// trailing path, and a foreign or absent signing name.
const CASES: &[Case] = &[
    // One row per URI this service's five operations are bound to
    // (`smithy_query(query="digest")`: `/stream-transcription`,
    // `/medical-stream-transcription`, `/medical-scribe-stream`,
    // `/medical-scribe-stream/{SessionId}`, `/call-analytics-stream-transcription`).
    Case {
        name: "StartStreamTranscription's URI",
        path_and_query: "/stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "StartMedicalStreamTranscription's URI",
        path_and_query: "/medical-stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "StartMedicalScribeStream's URI",
        path_and_query: "/medical-scribe-stream",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "GetMedicalScribeStream's URI, with its session label",
        path_and_query: "/medical-scribe-stream/11111111-2222-4333-8444-555555555555",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        // Modelled but not served by this emulator; it is still this service's URI, so
        // the claim must reach it rather than leaving the request ambiguous.
        name: "StartCallAnalyticsStreamTranscription's URI",
        path_and_query: "/call-analytics-stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "a bound URI with a query string",
        path_and_query: "/stream-transcription?language-code=en-US",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        // The signing name is normalised (trimmed, lowercased) before it is compared.
        name: "a bound URI signed for TRANSCRIBE in upper case",
        path_and_query: "/stream-transcription",
        signing_name: Some("TRANSCRIBE"),
        target: None,
        claimed: true,
    },
    // The WebSocket transport of the two operations AWS documents one for: the same
    // operation at the same URI plus `-websocket`, which the model does not describe,
    // so it is claimed from this service's own data-plane path set.
    Case {
        name: "StartStreamTranscription's WebSocket URI, query-signed",
        path_and_query: "/stream-transcription-websocket?language-code=en-US&media-encoding=pcm",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "StartMedicalStreamTranscription's WebSocket URI",
        path_and_query: "/medical-stream-transcription-websocket?sample-rate=16000",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        // Exact segment equality again: a WebSocket path AWS does not publish is not
        // this service's to claim.
        name: "near miss /stream-transcriptions-websocket",
        path_and_query: "/stream-transcriptions-websocket",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "a WebSocket path for an operation AWS publishes no WebSocket transport for",
        path_and_query: "/medical-scribe-stream-websocket",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's WebSocket URI",
        path_and_query: "/stream-transcription-websocket",
        signing_name: Some("polly"),
        target: None,
        claimed: false,
    },
    // The peer's path space: this service must claim NONE of it, or the router is
    // ambiguous again.
    Case {
        name: "the peer's root path, which carries all its awsJson1_1 traffic",
        path_and_query: "/",
        signing_name: Some("transcribe"),
        target: Some("Transcribe.ListVocabularies"),
        claimed: false,
    },
    Case {
        name: "the peer's root path routed by X-Amz-Target alone",
        path_and_query: "/",
        signing_name: None,
        target: Some("Transcribe.ListTranscriptionJobs"),
        claimed: false,
    },
    Case {
        name: "the peer's /transcriptionjobs",
        path_and_query: "/transcriptionjobs",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /vocabularyFilters/{name}",
        path_and_query: "/vocabularyFilters/demo-filter",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /medicalscribejobs/{name}",
        path_and_query: "/medicalscribejobs/demo-scribe-job",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        // Matching is exact segment equality, never a prefix test: a path that merely
        // resembles one of this service's URIs is not its own.
        name: "near miss /stream-transcriptions",
        path_and_query: "/stream-transcriptions",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "near miss /medical-scribe-streams",
        path_and_query: "/medical-scribe-streams",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's URI",
        path_and_query: "/stream-transcription",
        signing_name: Some("polly"),
        target: None,
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service: nothing
        // identifies a target service, so nothing is claimed.
        name: "no routing signal at all",
        path_and_query: "/stream-transcription",
        signing_name: None,
        target: None,
        claimed: false,
    },
];

#[test]
fn only_this_services_own_bound_uris_are_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for case in CASES {
        let parts = parts(case.path_and_query, case.signing_name, case.target);
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, case.claimed,
            "case {:?}: expected can_handle({}) to be {}, got {claimed}",
            case.name, case.path_and_query, case.claimed
        );
    }
}
