// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Amazon Transcribe signs as `transcribe`, and so does Amazon Transcribe Streaming
//! (`services/service-com-amazonaws-transcribestreaming-transcribe`): a different
//! service, with its own protocol (`restJson1`), endpoint prefix
//! (`transcribestreaming`) and five operations. The router resolves a request's routing
//! key from the SigV4 credential scope first, then from `X-Amz-Target`, so both services
//! see the key `transcribe`; claiming on that key alone makes each service claim the
//! other's traffic, the router finds two claimants and answers `500 AmbiguousService`
//! for every Transcribe request instead of serving either service.
//!
//! The claim is therefore the routing key AND the request's first URI path segment,
//! matched by exact equality against this service's model-derived
//! `ROUTE_PATH_SEGMENTS` — which, for this `awsJson1_1` service, carries the root path
//! its `POST /` traffic arrives on plus the literal first segments of its (wire-inert)
//! `@http` bindings. This suite pins that rule over the class of requests it generalises
//! over: both routing tiers, the root path, each family of its own `@http` first
//! segments, the peer's URIs, near-miss spellings of a segment, and requests that name
//! no service at all.

use std::sync::Arc;

use service_com_amazonaws_transcribe_transcribe::EmulatedService;
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
/// from, the first path segment (this service's root path, each family of its own
/// modelled segments, the peer's segments, a near miss), the presence of a query string,
/// and a foreign or absent signing name.
const CASES: &[Case] = &[
    Case {
        // Every operation of this awsJson1_1 service arrives as `POST /` with the
        // operation in `X-Amz-Target`; the root path is this service's own.
        name: "awsJson1_1 POST / signed for transcribe",
        path_and_query: "/",
        signing_name: Some("transcribe"),
        target: Some("Transcribe.ListVocabularies"),
        claimed: true,
    },
    Case {
        // No credential scope: the key comes from the `X-Amz-Target` prefix instead,
        // and the claim must be the same.
        name: "awsJson1_1 POST / routed by X-Amz-Target alone",
        path_and_query: "/",
        signing_name: None,
        target: Some("Transcribe.GetVocabulary"),
        claimed: true,
    },
    Case {
        name: "root path with a query string",
        path_and_query: "/?Version=2017-10-26",
        signing_name: Some("transcribe"),
        target: Some("Transcribe.ListTranscriptionJobs"),
        claimed: true,
    },
    Case {
        // The signing name is normalised (trimmed, lowercased) before it is compared.
        name: "root path signed for TRANSCRIBE in upper case",
        path_and_query: "/",
        signing_name: Some("TRANSCRIBE"),
        target: None,
        claimed: true,
    },
    // One row per family of this service's own modelled `@http` first segments
    // (`OPERATIONS.md`: `/transcriptionjobs`, `/vocabularies`, `/vocabularyFilters`,
    // `/callanalyticscategories`, `/callanalyticsjobs`, `/languagemodels`,
    // `/medicalvocabularies`, `/medicalscribejobs`, `/medicaltranscriptionjobs`,
    // `/tags`). They are inert under awsJson1_1, but they are this service's path space
    // and no other service's.
    Case {
        name: "collection path /transcriptionjobs",
        path_and_query: "/transcriptionjobs",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /transcriptionjobs/{name}",
        path_and_query: "/transcriptionjobs/demo-job",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /vocabularies/{name}",
        path_and_query: "/vocabularies/demo-vocabulary",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        // A camelCase segment, matched case-sensitively as HTTP path semantics require.
        name: "camelCase path /vocabularyFilters",
        path_and_query: "/vocabularyFilters",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /callanalyticscategories/{name}",
        path_and_query: "/callanalyticscategories/demo-category",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "tagging path /tags/{arn}",
        path_and_query: "/tags/arn%3Aaws%3Atranscribe%3Aus-east-1%3A000000000000%3Avocabulary",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    Case {
        name: "instance path /medicalvocabularies/{name}",
        path_and_query: "/medicalvocabularies/demo-medical-vocabulary",
        signing_name: Some("transcribe"),
        target: None,
        claimed: true,
    },
    // The peer's four restJson1 URIs: this service must claim NONE of them, or the
    // router is ambiguous again.
    Case {
        name: "the peer's /stream-transcription",
        path_and_query: "/stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /medical-stream-transcription",
        path_and_query: "/medical-stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /medical-scribe-stream",
        path_and_query: "/medical-scribe-stream",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /medical-scribe-stream/{SessionId}",
        path_and_query: "/medical-scribe-stream/11111111-2222-4333-8444-555555555555",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "the peer's /call-analytics-stream-transcription",
        path_and_query: "/call-analytics-stream-transcription",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        // Matching is exact segment equality, never a prefix test or a fold: a path
        // that merely resembles a modelled segment is not this service's.
        name: "near miss /vocabularyfilters in the wrong case",
        path_and_query: "/vocabularyfilters",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "near miss /transcriptionjobsextra",
        path_and_query: "/transcriptionjobsextra",
        signing_name: Some("transcribe"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's path",
        path_and_query: "/transcriptionjobs",
        signing_name: Some("s3"),
        target: None,
        claimed: false,
    },
    Case {
        name: "another service's target on the root path",
        path_and_query: "/",
        signing_name: None,
        target: Some("DynamoDB_20120810.GetItem"),
        claimed: false,
    },
    Case {
        // No credential scope, no target, and a loopback host names no service: nothing
        // identifies a target service, so nothing is claimed.
        name: "no routing signal at all",
        path_and_query: "/",
        signing_name: None,
        target: None,
        claimed: false,
    },
];

#[test]
fn only_this_services_own_path_space_is_claimed() {
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
