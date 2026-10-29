// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RejectLink`: declining an outstanding link request.
//!
//! The rule under test is about state: a link whose request is still outstanding
//! moves to the terminal `REJECTED` status and stays there for every later
//! reader, and a link in any other status cannot be declined at all. The tables
//! span what the rule generalises over — which of the link's two ends addresses
//! the call, whether the request has settled yet, and every member of the
//! modelled `LinkStatus` enum.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::ops::types::LINK_STATUS_VALUES;
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    ComAmazonawsRtbfabricRtbfabricState, LINK_STATUS_PENDING_REQUEST, LINK_STATUS_REJECTED,
    LINK_STATUS_REQUESTED, PROVISIONING_DELAY,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one restJson1 request and return `(status, parsed body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A requester gateway, a responder gateway and a link opened from the
    /// requester to the responder, all over the wire.
    async fn linked_pair(&self, token: &str) -> Pair {
        let gateway = json!({
            "vpcId": "vpc-0123456789abcdef0",
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "clientToken": format!("{token}-requester"),
        });
        let (status, requester) = self.send("POST", "/requester-gateway", Some(gateway)).await;
        assert_eq!(status, 200, "CreateRequesterGateway: {status} {requester:?}");
        let (status, responder) = self
            .send(
                "POST",
                "/responder-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 443,
                    "protocol": "HTTPS",
                    "domainName": "responder.example.com",
                    "clientToken": format!("{token}-responder"),
                })),
            )
            .await;
        assert_eq!(status, 200, "CreateResponderGateway: {status} {responder:?}");
        let requester_id = text(&requester, "gatewayId");
        let responder_id = text(&responder, "gatewayId");
        let (status, link) = self
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(json!({
                    "peerGatewayId": responder_id,
                    "logSettings": log_settings(),
                    "attributes": { "customerProvidedId": "spot-fabric" },
                })),
            )
            .await;
        assert_eq!(status, 200, "CreateLink: {status} {link:?}");
        Pair {
            requester_id,
            responder_id,
            link_id: text(&link, "linkId"),
            created_at: link["createdAt"].clone(),
        }
    }

    /// Put `status` on every stored link.
    ///
    /// The statuses a link reaches beyond its own request are driven by
    /// operations this crate has not implemented yet, so the state is set
    /// directly to cover the whole modelled enum rather than only the part the
    /// wire reaches today.
    fn force_link_status(&self, status: &str) {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let mut store = state.lock();
        for link in store.links.values_mut() {
            link.status = status.to_string();
            link.settles_at = None;
        }
    }
}

/// The gateway pair and the link between them, as the service minted them.
struct Pair {
    requester_id: String,
    responder_id: String,
    link_id: String,
    /// What `CreateLink` reported for `createdAt`; a reject must not change it.
    created_at: Value,
}

/// One row: which end addresses the reject, and whether the clock has settled
/// `PENDING_REQUEST` -> `REQUESTED` first, so both outstanding statuses are
/// covered from both ends.
struct Row {
    name: &'static str,
    /// `true` for the gateway the request was put to (the end the operation's
    /// documentation describes as declining it), `false` for the opener.
    from_peer: bool,
    settle_first: bool,
}

const ROWS: &[Row] = &[
    Row {
        name: "the peer declines an unsettled request",
        from_peer: true,
        settle_first: false,
    },
    Row {
        name: "the peer declines a settled request",
        from_peer: true,
        settle_first: true,
    },
    Row {
        name: "the opener withdraws an unsettled request",
        from_peer: false,
        settle_first: false,
    },
    Row {
        name: "the opener withdraws a settled request",
        from_peer: false,
        settle_first: true,
    },
];

#[tokio::test]
async fn rejecting_an_outstanding_request_moves_the_link_to_rejected_for_every_reader() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();
        let pair = fabric.linked_pair("token").await;
        let (addressed, other) = if row.from_peer {
            (&pair.responder_id, &pair.requester_id)
        } else {
            (&pair.requester_id, &pair.responder_id)
        };

        let expected_before = if row.settle_first {
            tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
            LINK_STATUS_REQUESTED
        } else {
            LINK_STATUS_PENDING_REQUEST
        };
        let (status, before) = fabric
            .send("GET", &format!("/gateway/{addressed}/link/{}", pair.link_id), None)
            .await;
        assert_eq!(
            text(&before, "status"),
            expected_before,
            "case {case:?}: expected {expected_before:?} before the reject, got {status} {:?}",
            before["status"]
        );

        let reject = format!("/gateway/{addressed}/link/{}/reject", pair.link_id);
        let (status, rejected) = fabric.send("POST", &reject, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RejectLink to answer 200, got {status} {rejected:?}"
        );
        assert_eq!(
            text(&rejected, "status"),
            LINK_STATUS_REJECTED,
            "case {case:?}: expected the declined link to be REJECTED, got {:?}",
            rejected["status"]
        );
        assert_eq!(
            text(&rejected, "linkId"),
            pair.link_id,
            "case {case:?}: expected link {:?}, got {:?}",
            pair.link_id,
            rejected["linkId"]
        );
        assert_eq!(
            text(&rejected, "gatewayId").as_str(),
            addressed.as_str(),
            "case {case:?}: expected the addressed gateway {addressed:?} as gatewayId, got {:?}",
            rejected["gatewayId"]
        );
        assert_eq!(
            text(&rejected, "peerGatewayId").as_str(),
            other.as_str(),
            "case {case:?}: expected the other end {other:?} as peerGatewayId, got {:?}",
            rejected["peerGatewayId"]
        );
        // The link's own facts come out of the record, unchanged by the reject:
        // only its status and `updatedAt` move.
        for (member, expected) in [
            ("createdAt", pair.created_at.clone()),
            ("direction", json!("REQUEST")),
            ("connectivityType", json!("DEFAULT")),
            ("logSettings", log_settings()),
            ("attributes", json!({ "customerProvidedId": "spot-fabric" })),
        ] {
            assert_eq!(
                rejected[member], expected,
                "case {case:?}: expected the stored {member} {expected:?}, got {:?}",
                rejected[member]
            );
        }

        // A repeat of this `@idempotent` call asks for the state the link is
        // already in, so it answers the record the first reject left.
        let (status, repeat) = fabric.send("POST", &reject, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated RejectLink to answer 200, got {status} {repeat:?}"
        );
        assert_eq!(
            repeat, rejected,
            "case {case:?}: expected the repeat to answer the first reject's record, got {repeat:?}"
        );

        // REJECTED is terminal and persisted: both ends read it back, and the
        // clock never moves it on, however long nobody looks.
        tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
        for (end, gateway_id) in [("addressed end", addressed), ("other end", other)] {
            let (status, read_back) = fabric
                .send("GET", &format!("/gateway/{gateway_id}/link/{}", pair.link_id), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetLink from the {end} to answer 200, got {status} {read_back:?}"
            );
            assert_eq!(
                text(&read_back, "status"),
                LINK_STATUS_REJECTED,
                "case {case:?}: expected the {end} to read REJECTED back, got {:?}",
                read_back["status"]
            );
            assert_eq!(
                read_back["updatedAt"], rejected["updatedAt"],
                "case {case:?}: expected the {end} to read the reject's updatedAt {:?}, got {:?}",
                rejected["updatedAt"], read_back["updatedAt"]
            );
        }
    }
}

#[tokio::test]
async fn only_an_outstanding_or_an_already_declined_request_can_be_rejected() {
    // The whole modelled catalog, from `LINK_STATUS_VALUES` (this service's
    // `LinkStatus` enum), so a status no operation reaches yet is covered too.
    for status_value in LINK_STATUS_VALUES {
        let case = *status_value;
        let fabric = Fabric::new();
        let pair = fabric.linked_pair("token").await;
        fabric.force_link_status(status_value);
        let link = format!("/gateway/{}/link/{}", pair.responder_id, pair.link_id);

        let (status, body) = fabric.send("POST", &format!("{link}/reject"), None).await;

        if [
            LINK_STATUS_PENDING_REQUEST,
            LINK_STATUS_REQUESTED,
            LINK_STATUS_REJECTED,
        ]
        .contains(status_value)
        {
            assert_eq!(
                status, 200,
                "case {case:?}: expected this status to be rejectable, got {status} {body:?}"
            );
            assert_eq!(
                text(&body, "status"),
                LINK_STATUS_REJECTED,
                "case {case:?}: expected the declined link to be REJECTED, got {:?}",
                body["status"]
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: expected this status to conflict with 409, got {status} {body:?}"
            );
            // The refused call left the link exactly as it was.
            let (_, read_back) = fabric.send("GET", &link, None).await;
            assert_eq!(
                text(&read_back, "status"),
                case,
                "case {case:?}: expected the refused reject to leave the status alone, got {:?}",
                read_back["status"]
            );
        }
    }
}

#[tokio::test]
async fn a_reject_of_a_link_the_gateway_is_not_an_end_of_is_not_found() {
    let fabric = Fabric::new();
    let pair = fabric.linked_pair("token-a").await;
    // A second pair, so there is a gateway that is no end of the first link.
    let other = fabric.linked_pair("token-b").await;

    for (case, gateway_id, link_id) in [
        ("neither exists", "rtb-gw-unknown", "link-unknown"),
        (
            "a stored gateway, a link id nothing minted",
            pair.requester_id.as_str(),
            "link-unknown",
        ),
        (
            "a stored link, addressed from a gateway that is not one of its ends",
            other.responder_id.as_str(),
            pair.link_id.as_str(),
        ),
    ] {
        let (status, body) = fabric
            .send("POST", &format!("/gateway/{gateway_id}/link/{link_id}/reject"), None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected RejectLink to answer 404, got {status} {body:?}"
        );
    }

    // The link that was NOT addressable by those calls is untouched by them.
    let (status, read_back) = fabric
        .send(
            "GET",
            &format!("/gateway/{}/link/{}", pair.requester_id, pair.link_id),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the untouched link to still be readable, got {status} {read_back:?}"
    );
    assert_ne!(
        text(&read_back, "status"),
        LINK_STATUS_REJECTED,
        "expected a refused reject to leave the link's status alone, got {:?}",
        read_back["status"]
    );
}
