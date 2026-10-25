// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Amazon Chime SDK Messaging signs as `chime`, and so does every other Amazon Chime
//! SDK service in this workspace (Chime, Chime SDK Identity, Chime SDK Meetings, Chime
//! SDK Voice, Chime SDK Media Pipelines): the models all declare `aws.auth#sigv4` name
//! `chime` while each carries its OWN `aws.api#service` `endpointPrefix` (`chime`,
//! `identity-chime`, `messaging-chime`, `meetings-chime`, `voice-chime`,
//! `media-pipelines-chime`). The router resolves a request's routing key from the SigV4
//! credential scope first, so all six see the key `chime`; claiming on that key alone
//! makes every one of them claim every chime-signed request, the router finds six
//! claimants and refuses the request as `500 AmbiguousService`.
//!
//! The rule pinned here is the one AWS itself applies, and it is the one
//! `service_common::chime_family::claims_chime_family_request` states for the whole
//! family: a
//! service claims a request addressed to the ENDPOINT it serves, and — when the host
//! names no endpoint of the family, which is every request from a client pointed at the
//! emulator — only a request inside its OWN URI path space, which for a space the family
//! shares means the part of it this model owns.
//!
//! The table spans the class that rule generalises over rather than re-running one
//! recorded call: each family of this model's own first path segments, a collection
//! path, an instance path and a deep path, query-literal-pinned routes, both routing
//! tiers (credential scope and host), every sibling's path space, near misses, and
//! requests that name another service or none. The path and query values come from
//! `smithy_query(query="digest")` on the six models.

use std::sync::Arc;

use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::Service;

// ONE percent-encoder per test binary: the `@httpLabel` encoding rule lives in
// `channel_messaging.rs`, which this suite shares rather than copying.
use crate::channel_messaging::encode;

/// A channel ARN, as it travels inside ONE `@httpLabel` path segment.
const CHANNEL: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/channel/c1";
/// An `AppInstanceArn`, the label of the one `/app-instances/…` route this model binds.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";

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

/// The emulator endpoint a recorded client is pointed at: a loopback host, which names
/// no AWS service, so every case on it is decided by the modelled route.
const LOOPBACK: &str = "127.0.0.1:9297";

fn cases() -> Vec<Case> {
    let channel = encode(CHANNEL);
    let app_instance = encode(APP_INSTANCE);
    vec![
        // This model's own routes, one row per family of its first path segments, in the
        // collection, instance and deep shapes the model binds.
        Case {
            name: "CreateChannel: POST /channels",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "ListChannels: GET /channels",
            method: "GET",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "DescribeChannel: GET /channels/{ChannelArn}",
            method: "GET",
            path_and_query: format!("/channels/{channel}"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "SendChannelMessage: POST /channels/{ChannelArn}/messages",
            method: "POST",
            path_and_query: format!("/channels/{channel}/messages"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateChannelBan: POST /channels/{ChannelArn}/bans",
            method: "POST",
            path_and_query: format!("/channels/{channel}/bans"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "DeleteChannelMessage: DELETE /channels/{ChannelArn}/messages/{MessageId}",
            method: "DELETE",
            path_and_query: format!("/channels/{channel}/messages/m-1"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            // A route the model pins with a query literal.
            name: "RedactChannelMessage: POST …/messages/{MessageId}?operation=redact",
            method: "POST",
            path_and_query: format!("/channels/{channel}/messages/m-1?operation=redact"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "SearchChannels: POST /channels?operation=search",
            method: "POST",
            path_and_query: "/channels?operation=search".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "ListChannelMembershipsForAppInstanceUser: GET /channels?scope=…",
            method: "GET",
            path_and_query: "/channels?scope=app-instance-user-memberships".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "CreateChannelFlow: POST /channel-flows",
            method: "POST",
            path_and_query: "/channel-flows".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "GetMessagingSessionEndpoint: GET /endpoints/messaging-session",
            method: "GET",
            path_and_query: "/endpoints/messaging-session".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            // `app-instances` is a first segment Chime SDK Identity binds too, so only
            // the whole route can place this: it is the one `/app-instances/…` route
            // THIS model declares.
            name: "PutMessagingStreamingConfigurations: PUT /app-instances/{arn}/streaming-configurations",
            method: "PUT",
            path_and_query: format!("/app-instances/{app_instance}/streaming-configurations"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            // The routing key is normalised (trimmed, lower-cased) before comparison.
            name: "signed for CHIME in upper case",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("CHIME"),
            claimed: true,
        },
        Case {
            // `/channels` is a segment no sibling model publishes, so a request inside
            // this service's own space reaches it even when it names no modelled route
            // and is answered in this service's own envelope.
            name: "an unmodelled method on this service's own path space",
            method: "PATCH",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "an unmodelled sub-resource under /channels",
            method: "GET",
            path_and_query: format!("/channels/{channel}/not-a-sub-resource"),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: true,
        },
        // The host tier: when the host names an endpoint of the family, that endpoint's
        // service serves the request and no sibling claims it.
        Case {
            name: "this service's own endpoint host",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: "messaging-chime.us-east-1.amazonaws.com",
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "this service's endpoint host, with no credential scope at all",
            method: "GET",
            path_and_query: "/channels".to_string(),
            host: "messaging-chime.us-east-1.amazonaws.com",
            signing_name: None,
            claimed: true,
        },
        Case {
            name: "a sibling's endpoint host, even on this service's path",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: "identity-chime.us-east-1.amazonaws.com",
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Amazon Chime's own endpoint host",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: "chime.us-east-1.amazonaws.com",
            signing_name: Some("chime"),
            claimed: false,
        },
        // Every sibling's path space: this service must claim NONE of it, or the router
        // is ambiguous again.
        Case {
            name: "Chime SDK Identity's CreateAppInstance: POST /app-instances",
            method: "POST",
            path_and_query: "/app-instances".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "Chime SDK Identity's DescribeAppInstance: GET /app-instances/{arn}",
            method: "GET",
            path_and_query: format!("/app-instances/{app_instance}"),
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
            name: "Chime SDK Media Pipelines' ListMediaPipelines: GET /sdk-media-pipelines",
            method: "GET",
            path_and_query: "/sdk-media-pipelines".to_string(),
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
            name: "near miss /channelsextra",
            method: "GET",
            path_and_query: "/channelsextra".to_string(),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "near miss /Channels in the wrong case",
            method: "GET",
            path_and_query: "/Channels".to_string(),
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
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: Some("s3"),
            claimed: false,
        },
        Case {
            name: "no routing signal at all",
            method: "POST",
            path_and_query: "/channels".to_string(),
            host: LOOPBACK,
            signing_name: None,
            claimed: false,
        },
        // The ONE residual shared route: `GET /tags` is declared IDENTICALLY by five of
        // the six models, and nothing on the wire distinguishes them once the host names
        // no endpoint — each of the five owns a DIFFERENT resource space, named only by
        // the ARN the request carries. So the family rule resolves that route by ENDPOINT
        // PREFIX only: on the emulator's own host no member claims it, and pointed at a
        // member's endpoint host exactly one does. Pinned so the behaviour is not
        // mistaken for an oversight.
        Case {
            name: "the tagging route five family models share: GET /tags",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode(CHANNEL)),
            host: LOOPBACK,
            signing_name: Some("chime"),
            claimed: false,
        },
        Case {
            name: "that same tagging route on this service's endpoint host",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode(CHANNEL)),
            host: "messaging-chime.us-east-1.amazonaws.com",
            signing_name: Some("chime"),
            claimed: true,
        },
        Case {
            name: "that same tagging route on a sibling's endpoint host",
            method: "GET",
            path_and_query: format!("/tags?arn={}", encode(CHANNEL)),
            host: "meetings-chime.us-east-1.amazonaws.com",
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
