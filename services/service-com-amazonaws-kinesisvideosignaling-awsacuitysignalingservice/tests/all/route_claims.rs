// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS at the router's front door.
//!
//! Five emulated services sign as `kinesisvideo` and are addressed by that one
//! endpoint prefix: this signaling endpoint, the Kinesis Video control plane, Media,
//! Archived Media and WebRTC Storage. The router resolves a request's routing key from
//! its SigV4 credential scope first, so a service that claims on the signing name
//! alone claims its four peers' traffic too; every one of them is then a claimant for
//! every request, and the router, which refuses rather than guesses, answers
//! `500 AmbiguousService` for ALL of them.
//!
//! The rule under test is the one `service.rs` applies: a request is claimed when its
//! routing key names THIS endpoint AND its first URI path segment is one this
//! service's OWN model declares. This suite drives [`Service::can_handle`] directly,
//! the seam the claim rule lives behind.

use std::sync::Arc;

use service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice as kvs;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming the `kinesisvideo` signing name, which all five
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// One claim case: a route, and whether this service's modelled path space holds it.
struct Case {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    claimed: bool,
}

/// Every route is a literal URI from a Kinesis Video model
/// (`smithy_query(query="digest")` on each smithy-model/ file): this service's own two
/// operations, then one route from each peer.
const CASES: &[Case] = &[
    Case {
        name: "its own GetIceServerConfig",
        method: "POST",
        path: "/v1/get-ice-server-config",
        claimed: true,
    },
    Case {
        name: "its own SendAlexaOfferToMaster",
        method: "POST",
        path: "/v1/send-alexa-offer-to-master",
        claimed: true,
    },
    Case {
        name: "the control plane's GetSignalingChannelEndpoint, a signaling operation it owns",
        method: "POST",
        path: "/getSignalingChannelEndpoint",
        claimed: false,
    },
    Case {
        name: "the control plane's TagStream",
        method: "POST",
        path: "/tagStream",
        claimed: false,
    },
    Case {
        name: "Media's GetMedia",
        method: "POST",
        path: "/getMedia",
        claimed: false,
    },
    Case {
        name: "a path under its own first segment that its model does not bind",
        method: "POST",
        path: "/v1/not-an-operation",
        // The claim rule is the routing key and the first path SEGMENT, and `v1` is
        // this service's own segment, so the request is claimed here and then answered
        // by this service's protocol, which binds no such route. No peer may answer a
        // path inside this endpoint's space.
        claimed: true,
    },
];

/// This service claims exactly the requests inside its own modelled path space, not
/// the path spaces of the four services that share its signing name.
#[test]
fn the_service_claims_only_the_routes_its_own_model_binds() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = kvs::EmulatedService::new(engine);

    for case in CASES {
        let host = "kinesisvideo.us-east-1.amazonaws.com";
        let (parts, ()) = http::Request::builder()
            .method(case.method)
            .uri(format!("https://{host}{}", case.path))
            .header("host", host)
            .header("authorization", AUTHORIZATION)
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
