// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Amazon Chime SDK Media Pipelines signs as `chime`, and so does every other Amazon Chime
//! service in this workspace (Chime, Chime SDK Identity, Chime SDK Messaging, Chime SDK
//! Meetings, Chime SDK Voice): the models all declare `aws.auth#sigv4` name `chime` while
//! each carries its OWN `aws.api#service` `endpointPrefix` (`chime`, `identity-chime`,
//! `messaging-chime`, `meetings-chime`, `voice-chime`, `media-pipelines-chime`). The router
//! resolves a request's routing key from the SigV4 credential scope first, so all six see
//! the key `chime`; claiming on that key alone makes every one of them claim every
//! chime-signed request, the router finds six claimants and refuses the request as
//! `500 AmbiguousService` — an error code no AWS model carries — before any handler runs.
//!
//! The rule pinned here is the one AWS itself applies, and the one
//! `service_common::chime_family::claims_chime_family_request` states for the whole family:
//! a service claims a request addressed to the ENDPOINT it serves, and — when the host
//! names no endpoint of the family, which is every request from a client pointed at the
//! emulator — only a request inside its OWN URI path space, which for a space the family
//! shares means the part of it this model owns.
//!
//! The table spans the class that rule generalises over rather than re-running one recorded
//! call: every family of this model's own first path segments, in the collection, instance
//! and deep shapes it binds; both routing tiers (credential scope and host); every
//! sibling's path space; near misses; and requests that name another service or none. The
//! path values come from `smithy_query(query="digest")` on the six models.

use std::sync::Arc;

use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService;
use smithy_server_runtime::Service;

use crate::kinesis_video_stream_pool_lifecycle::encode_label;

/// A media pipeline id, the label of this model's instance routes.
const PIPELINE_ID: &str = "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e";

/// A media insights pipeline configuration ARN, as it travels inside ONE `@httpLabel`.
const CONFIGURATION_ARN: &str =
    "arn:aws:chime:us-east-1:000000000000:media-insights-pipeline-configuration/conf-1";

/// The emulator endpoint a recorded client is pointed at: a loopback host, which names no
/// AWS service, so every case on it is decided by the modelled route.
const LOOPBACK: &str = "127.0.0.1:9297";

/// This service's own endpoint host, and two siblings'.
const OWN_ENDPOINT: &str = "media-pipelines-chime.us-east-1.amazonaws.com";
const MEETINGS_ENDPOINT: &str = "meetings-chime.us-east-1.amazonaws.com";
const LEGACY_ENDPOINT: &str = "chime.us-east-1.amazonaws.com";

/// A SigV4 credential scope naming `signing_name`; the signature itself is never read by
/// the routing key.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The request parts `can_handle` is handed: the method, the URI, and the headers the
/// routing key is read from.
fn parts(
    method: &str,
    path_and_query: &str,
    host: &str,
    signing_name: Option<&str>,
) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path_and_query}"))
        .header("host", host);
    if let Some(signing_name) = signing_name {
        builder = builder.header("authorization", authorization(signing_name));
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
    method: &'static str,
    path_and_query: String,
    host: &'static str,
    signing_name: Option<&'static str>,
    claimed: bool,
}

fn cases() -> Vec<Case> {
    let configuration = encode_label(CONFIGURATION_ARN);
    vec![
        // This model's own routes, one row per family of its first path segments, in the
        // collection, instance and deep shapes the model binds.
        Case {
            name: "ListMediaPipelines: GET /sdk-media-pipelines",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "GetMediaPipeline: GET /sdk-media-pipelines/{MediaPipelineId}",
            method: "GET",
            path_and_query: format!("/sdk-media-pipelines/{PIPELINE_ID}"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "DeleteMediaPipeline: DELETE /sdk-media-pipelines/{MediaPipelineId}",
            method: "DELETE",
            path_and_query: format!("/sdk-media-pipelines/{PIPELINE_ID}"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaCapturePipeline: POST /sdk-media-capture-pipelines",
            method: "POST",
            path_and_query: "/sdk-media-capture-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaConcatenationPipeline: POST /sdk-media-concatenation-pipelines",
            method: "POST",
            path_and_query: "/sdk-media-concatenation-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaLiveConnectorPipeline: POST /sdk-media-live-connector-pipelines",
            method: "POST",
            path_and_query: "/sdk-media-live-connector-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaStreamPipeline: POST /sdk-media-stream-pipelines",
            method: "POST",
            path_and_query: "/sdk-media-stream-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaInsightsPipeline: POST /media-insights-pipelines",
            method: "POST",
            path_and_query: "/media-insights-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "GetSpeakerSearchTask: GET /media-insights-pipelines/{id}/speaker-search-tasks/{taskId}",
            method: "GET",
            path_and_query: format!(
                "/media-insights-pipelines/{PIPELINE_ID}/speaker-search-tasks/{PIPELINE_ID}"
            ),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            // A route the model pins with a query literal.
            name: "StartSpeakerSearchTask: POST …/speaker-search-tasks?operation=start",
            method: "POST",
            path_and_query: format!(
                "/media-insights-pipelines/{PIPELINE_ID}/speaker-search-tasks?operation=start"
            ),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaInsightsPipelineConfiguration: POST /media-insights-pipeline-configurations",
            method: "POST",
            path_and_query: "/media-insights-pipeline-configurations".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "DeleteMediaInsightsPipelineConfiguration: DELETE …/{Identifier}",
            method: "DELETE",
            path_and_query: format!("/media-insights-pipeline-configurations/{configuration}"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "UpdateMediaInsightsPipelineStatus: PUT /media-insights-pipeline-status/{Identifier}",
            method: "PUT",
            path_and_query: format!("/media-insights-pipeline-status/{PIPELINE_ID}"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateMediaPipelineKinesisVideoStreamPool: POST /media-pipeline-kinesis-video-stream-pools",
            method: "POST",
            path_and_query: "/media-pipeline-kinesis-video-stream-pools".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "GetMediaPipelineKinesisVideoStreamPool: GET …/{Identifier}",
            method: "GET",
            path_and_query: "/media-pipeline-kinesis-video-stream-pools/pool-1".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            // The routing key is normalised (trimmed, lower-cased) before comparison.
            name: "signed for CHIME in upper case",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("CHIME"),
            claimed: true,
        },
        Case {
            // `/sdk-media-pipelines` is a segment no sibling model publishes, so a request
            // inside this service's own space reaches it even when it names no modelled
            // route, and is answered in this service's own envelope.
            name: "an unmodelled method on this service's own path space",
            method: "PATCH",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        // The host tier: when the host names an endpoint of the family, that endpoint's
        // service serves the request and no sibling claims it.
        Case {
            name: "this service's own endpoint host",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: OWN_ENDPOINT,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "this service's endpoint host, with no credential scope at all",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: OWN_ENDPOINT,
            signing_name: None,
            claimed: true,
        },
        Case {
            name: "a sibling's endpoint host, even on this service's path",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: MEETINGS_ENDPOINT,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Amazon Chime's own endpoint host",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LEGACY_ENDPOINT,
            signing_name: Some("chime"),
            claimed: false,
        },
        // Every sibling's path space: this service must claim NONE of it, or the router is
        // ambiguous again.
        Case {
            name: "Chime SDK Identity's CreateAppInstance: POST /app-instances",
            method: "POST",
            path_and_query: "/app-instances".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Chime SDK Identity's CreateAppInstanceUser: POST /app-instance-users",
            method: "POST",
            path_and_query: "/app-instance-users".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Chime SDK Messaging's CreateChannel: POST /channels",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Chime SDK Meetings' CreateMeeting: POST /meetings",
            method: "POST",
            path_and_query: "/meetings".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Chime SDK Voice's CreateVoiceConnector: POST /voice-connectors",
            method: "POST",
            path_and_query: "/voice-connectors".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Amazon Chime's ListAccounts: GET /accounts",
            method: "GET",
            path_and_query: "/accounts".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "the telephony space Chime and Voice share: GET /phone-numbers",
            method: "GET",
            path_and_query: "/phone-numbers".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Amazon Chime's dial-out under the shared /meetings segment",
            method: "POST",
            path_and_query: "/meetings/m-1/dial-outs".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        // Near misses: matching is exact, never a prefix test or a case fold.
        Case {
            name: "near miss /sdk-media-pipelines-extra",
            method: "GET",
            path_and_query: "/sdk-media-pipelines-extra".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "near miss /Sdk-Media-Pipelines in the wrong case",
            method: "GET",
            path_and_query: "/Sdk-Media-Pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            // Every operation of this restJson1 model binds a URI, so it claims no root
            // post.
            name: "the root path",
            method: "POST",
            path_and_query: "/".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "another service's signing name on this service's path",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: Some("s3"),
            claimed: false,
        },
        Case {
            name: "no routing signal at all",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
            host: LOOPBACK,
            signing_name: None,
            claimed: false,
        },
        // The ONE residual shared route: `GET /tags` is declared IDENTICALLY by five of the
        // six models, and nothing on the wire distinguishes them once the host names no
        // endpoint — tag and untag carry the resource ARN in the BODY, which this seam is
        // not handed. So the family rule resolves that route by endpoint prefix, and for a
        // request naming none by the family's DECLARED tagging owner (Chime SDK Identity),
        // which answers out of the tag directory every member shares: this service claims
        // it only at its own endpoint host. Pinned so it is not mistaken for an oversight.
        Case {
            name: "the tagging route five family models share: GET /tags",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode_label(CONFIGURATION_ARN)),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "that same tagging route on this service's endpoint host",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode_label(CONFIGURATION_ARN)),
            host: OWN_ENDPOINT,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "that same tagging route on a sibling's endpoint host",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode_label(CONFIGURATION_ARN)),
            host: MEETINGS_ENDPOINT,
            signing_name: Some("chime"),
            claimed: false,
        },
    ]
}

#[test]
fn only_this_services_own_wire_surface_is_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for case in cases() {
        let parts = parts(
            case.method,
            &case.path_and_query,
            case.host,
            case.signing_name,
        );
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, case.claimed,
            "case {:?}: expected can_handle({} {} on {}) to be {}, got {claimed}",
            case.name, case.method, case.path_and_query, case.host, case.claimed
        );
    }
}
