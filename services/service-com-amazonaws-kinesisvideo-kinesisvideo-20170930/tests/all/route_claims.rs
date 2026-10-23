// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS at the router's front door.
//!
//! Five emulated services sign as `kinesisvideo` and are addressed by that one
//! endpoint prefix: this control plane, Kinesis Video Media, Archived Media,
//! Signaling Channels and WebRTC Storage. The router resolves a request's routing
//! key from its SigV4 credential scope first, so a service that claims on the
//! signing name alone claims its four peers' traffic too; every one of them is then
//! a claimant for every request, and the router — which refuses rather than guesses —
//! answers `500 AmbiguousService` for ALL of them, a code no AWS model carries.
//!
//! The rule under test is the one `service.rs` applies: a request is claimed when its
//! routing key names THIS endpoint AND its first URI path segment is one this
//! service's OWN model declares. So this suite drives [`Service::can_handle`] directly
//! — the seam the fix changed — over a table that spans this service's own routes,
//! each peer's routes, a path no Kinesis Video model binds, and a request scoped to
//! another service entirely. The peers' side of the split is the same rule over their
//! own generated segment lists; `core/router/tests/` is where a suite that assembles
//! every service lives, while this one needs no sibling service crate and so belongs
//! with the crate whose claim rule it pins.

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming the `kinesisvideo` signing name — what all five
/// services' requests carry.
const KINESISVIDEO_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The same, scoped to a different service: Kinesis Data Streams.
const KINESIS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesis/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// How a request is addressed: by its SigV4 credential scope, or — when it carries
/// none — by the `Host` label, which resolves to the endpoint prefix.
enum AddressedBy {
    Scope(&'static str),
    HostOnly,
}

/// One claim case.
struct Case {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    host: &'static str,
    addressed_by: AddressedBy,
    claimed: bool,
}

/// Every route below is a literal URI from a Kinesis Video model, read with
/// `smithy_query(query="digest")` on each of the five smithy-model/ files: this
/// service's own operations, then one route from each peer — Media `/getMedia`,
/// Archived Media `/getClip`, `/getHLSStreamingSessionURL` and `/listFragments`,
/// Signaling Channels `/v1/get-ice-server-config` and `/v1/send-alexa-offer-to-master`,
/// WebRTC Storage `/joinStorageSession` and `/joinStorageSessionAsViewer`. Every one
/// of those first segments is disjoint from this service's.
const CASES: &[Case] = &[
    Case {
        name: "its own CreateStream",
        method: "POST",
        path: "/createStream",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own ListTagsForStream",
        method: "POST",
        path: "/listTagsForStream",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own TagStream",
        method: "POST",
        path: "/tagStream",
        host: "127.0.0.1:9297",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own UntagStream",
        method: "POST",
        path: "/untagStream",
        host: "127.0.0.1:9297",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own TagResource, whose URI is capitalised",
        method: "POST",
        path: "/TagResource",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own DescribeStream on a loopback endpoint",
        method: "POST",
        path: "/describeStream",
        host: "127.0.0.1:9297",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: true,
    },
    Case {
        name: "its own ListTagsForStream addressed by Host alone, with no credential scope",
        method: "POST",
        path: "/listTagsForStream",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::HostOnly,
        claimed: true,
    },
    Case {
        name: "Kinesis Video Media's GetMedia",
        method: "POST",
        path: "/getMedia",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "Archived Media's GetClip",
        method: "POST",
        path: "/getClip",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "Archived Media's GetHLSStreamingSessionURL",
        method: "POST",
        path: "/getHLSStreamingSessionURL",
        host: "127.0.0.1:9297",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "Archived Media's ListFragments, whose name resembles this service's listings",
        method: "POST",
        path: "/listFragments",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "Signaling Channels' GetIceServerConfig",
        method: "POST",
        path: "/v1/get-ice-server-config",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "Signaling Channels' SendAlexaOfferToMaster",
        method: "POST",
        path: "/v1/send-alexa-offer-to-master",
        host: "127.0.0.1:9297",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "WebRTC Storage's JoinStorageSession",
        method: "POST",
        path: "/joinStorageSession",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "WebRTC Storage's JoinStorageSessionAsViewer",
        method: "POST",
        path: "/joinStorageSessionAsViewer",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "a path no Kinesis Video model binds",
        method: "POST",
        path: "/notAnOperation",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        claimed: false,
    },
    Case {
        name: "one of its own URIs under a method its model does not bind",
        method: "GET",
        path: "/listTagsForStream",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESISVIDEO_AUTHORIZATION),
        // The claim rule is the routing key and the first path SEGMENT; it reads no
        // method, so this request is claimed by this service and then answered by its
        // own protocol, which binds no `GET /listTagsForStream` route. That is the
        // right split: the request addresses THIS endpoint's path space, so no peer
        // may answer it.
        claimed: true,
    },
    Case {
        name: "one of its own URIs scoped to another service entirely",
        method: "POST",
        path: "/listTagsForStream",
        host: "kinesis.us-east-1.amazonaws.com",
        addressed_by: AddressedBy::Scope(KINESIS_AUTHORIZATION),
        claimed: false,
    },
];

/// This service claims exactly the requests inside its own modelled path space, under
/// a routing key that names its endpoint — not its four same-signing-name peers'
/// routes, and not a path nobody binds.
#[test]
fn the_service_claims_only_the_routes_its_own_model_binds() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = kvs::EmulatedService::new(engine);

    for case in CASES {
        let mut builder = http::Request::builder()
            .method(case.method)
            .uri(format!("https://{}{}", case.host, case.path))
            .header("host", case.host);
        if let AddressedBy::Scope(authorization) = case.addressed_by {
            builder = builder.header("authorization", authorization);
        }
        let (parts, ()) = builder
            .body(())
            .expect("the request is well formed")
            .into_parts();

        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, case.claimed,
            "case {:?}: expected can_handle({} {}) == {}, got {claimed}",
            case.name, case.method, case.path, case.claimed
        );
    }
}
