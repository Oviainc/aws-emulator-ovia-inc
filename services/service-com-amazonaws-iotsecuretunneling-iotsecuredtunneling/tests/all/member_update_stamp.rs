// SPDX-License-Identifier: CC0-1.0

//! The one stamp rule every member-level update of a tunnel obeys, driven
//! through each operation that performs such an update.
//!
//! RULE: a successful call that changes a member the tunnel reports leaves the
//! record's `lastUpdatedAt` ("The last time the tunnel was updated", the
//! modelled `Tunnel$lastUpdatedAt` documentation) at the instant of that
//! change, and leaves every other stamp the record carries — when it was
//! created, when its lifetime runs out, and either end's connection state —
//! exactly as it was. Whichever member the call wrote, the stamp is written in
//! one place in this crate, so the rule cannot differ between the operations
//! that reach it.
//!
//! The table spans what the rule generalises over: which operation performs the
//! update (`RotateTunnelAccessToken`, `TagResource`, `UntagResource`), which
//! member of the record it writes (an access token, both access tokens, the
//! destination configuration, the tag set), and how many values the request
//! names. The `clientMode` rows are the modelled `ClientMode` values, so the
//! rotation rows cover each selection of the ends a rotation writes.
//!
//! Each row rewinds the tunnel's whole timeline before the update, so a stamp
//! left at the instant of the change is plainly distinguishable from the one
//! `OpenTunnel` wrote whatever the clock's resolution.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::StoredTunnel;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-east-2";

/// How far the timeline of a tunnel is moved back before the update under test.
const REWIND: Duration = Duration::from_secs(300);

/// The tags every tunnel in the table is opened with, so a removal has
/// something to remove and an addition has something to sit beside.
const INITIAL_TAGS: &[(&str, &str)] = &[("env", "test"), ("owner", "team")];

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The stored record of `tunnel_id`, or a panic naming `case`.
fn stored(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    case: &str,
) -> StoredTunnel {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"))
}

/// The `lastUpdatedAt` `DescribeTunnel` reports for `tunnel_id`.
async fn described_last_updated_at(
    router: &MultiServiceRouter,
    tunnel_id: &str,
    case: &str,
) -> Option<f64> {
    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTunnel answered {status} {described:?}"
    );
    described["tunnel"]["lastUpdatedAt"].as_f64()
}

/// The `lastUpdatedAt` `ListTunnels` reports for `tunnel_id`, so the other
/// reader of the same record is held to the same stamp.
async fn listed_last_updated_at(router: &MultiServiceRouter, tunnel_id: &str) -> Option<f64> {
    let (_, listed) = call(router, "ListTunnels", &json!({})).await;
    listed["tunnelSummaries"]
        .as_array()?
        .iter()
        .find(|summary| summary["tunnelId"].as_str() == Some(tunnel_id))
        .and_then(|summary| summary["lastUpdatedAt"].as_f64())
}

/// Move the whole timeline of `tunnel_id` back by [`REWIND`], so every stamp
/// the record carries is in the past and a stamp written by the call under test
/// is unmistakable.
fn rewind(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, case: &str) {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
    tunnel.created_at -= REWIND;
    tunnel.last_updated_at -= REWIND;
    tunnel.source_connection_state.last_updated_at -= REWIND;
    tunnel.destination_connection_state.last_updated_at -= REWIND;
}

/// The member-level update under test.
#[derive(Debug, Clone, Copy)]
enum Update {
    /// `RotateTunnelAccessToken` with this `clientMode`, optionally carrying a
    /// `destinationConfig` whose `services` list is the one given.
    Rotate {
        client_mode: &'static str,
        destination_services: Option<&'static [&'static str]>,
    },
    /// `TagResource` with these key/value pairs.
    Tag(&'static [(&'static str, &'static str)]),
    /// `UntagResource` with these keys.
    Untag(&'static [&'static str]),
}

impl Update {
    /// The operation name and request body for the tunnel `tunnel_id`/`arn`.
    fn request(self, tunnel_id: &str, arn: &str) -> (&'static str, Value) {
        match self {
            Self::Rotate {
                client_mode,
                destination_services,
            } => {
                let mut input = json!({"tunnelId": tunnel_id, "clientMode": client_mode});
                if let Some(services) = destination_services {
                    input["destinationConfig"] =
                        json!({"thingName": "thing-stamped", "services": services});
                }
                ("RotateTunnelAccessToken", input)
            }
            Self::Tag(tags) => (
                "TagResource",
                json!({
                    "resourceArn": arn,
                    "tags": tags
                        .iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect::<Vec<Value>>(),
                }),
            ),
            Self::Untag(keys) => (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": keys}),
            ),
        }
    }
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    update: Update,
}

/// The rows: every operation that writes a member of the tunnel, over every
/// selection of members it can write.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "rotate the source token only",
            update: Update::Rotate {
                client_mode: "SOURCE",
                destination_services: None,
            },
        },
        Case {
            label: "rotate the destination token only",
            update: Update::Rotate {
                client_mode: "DESTINATION",
                destination_services: None,
            },
        },
        Case {
            label: "rotate both tokens",
            update: Update::Rotate {
                client_mode: "ALL",
                destination_services: None,
            },
        },
        Case {
            label: "rotate the destination token and rewrite destinationConfig",
            update: Update::Rotate {
                client_mode: "DESTINATION",
                destination_services: Some(&["SSH", "HTTP"]),
            },
        },
        Case {
            label: "rotate both tokens and rewrite destinationConfig to one service",
            update: Update::Rotate {
                client_mode: "ALL",
                destination_services: Some(&["HTTP"]),
            },
        },
        Case {
            label: "add one tag key",
            update: Update::Tag(&[("stage", "beta")]),
        },
        Case {
            label: "rewrite the value of a stored tag key",
            update: Update::Tag(&[("env", "prod")]),
        },
        Case {
            label: "add two tag keys at once",
            update: Update::Tag(&[("stage", "beta"), ("tier", "gold")]),
        },
        Case {
            label: "remove one tag key",
            update: Update::Untag(&["env"]),
        },
        Case {
            label: "remove both tag keys at once",
            update: Update::Untag(&["env", "owner"]),
        },
    ]
}

/// Open one tunnel carrying [`INITIAL_TAGS`] and return `(tunnelId, arn)`.
async fn open_tunnel(router: &MultiServiceRouter, label: &str) -> (String, String) {
    let (status, opened) = call(
        router,
        "OpenTunnel",
        &json!({
            "description": label,
            "destinationConfig": {"thingName": "thing-stamped", "services": ["SSH"]},
            "tags": INITIAL_TAGS
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: OpenTunnel answered {status} {opened:?}"
    );
    let tunnel_id = opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no tunnelId in {opened:?}"))
        .to_string();
    let arn = opened["tunnelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no tunnelArn in {opened:?}"))
        .to_string();
    (tunnel_id, arn)
}

#[tokio::test]
async fn every_member_update_stamps_the_tunnel_at_the_instant_of_its_change() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for case in cases() {
        let label = case.label;
        let (tunnel_id, arn) = open_tunnel(&router, label).await;
        rewind(&engine, &tunnel_id, label);

        let before = stored(&engine, &tunnel_id, label);
        let before_reported = described_last_updated_at(&router, &tunnel_id, label).await;

        let (operation, input) = case.update.request(&tunnel_id, &arn);
        let sent = SystemTime::now();
        let (status, body) = call(&router, operation, &input).await;
        let answered = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} answered {status} {body:?}"
        );

        let after = stored(&engine, &tunnel_id, label);
        assert!(
            after.last_updated_at >= sent && after.last_updated_at <= answered,
            "case {label:?}: {operation} must stamp lastUpdatedAt with the instant of its \
             change, expected a stamp in {sent:?}..={answered:?}, got {:?}",
            after.last_updated_at
        );
        assert!(
            after.last_updated_at > before.last_updated_at,
            "case {label:?}: expected a stamp later than {:?}, got {:?}",
            before.last_updated_at,
            after.last_updated_at
        );

        // The update writes the member it was asked to write and no other stamp
        // the record carries: a member update is not a creation, not a close,
        // not a connection transition, and does not extend the lifetime.
        assert_eq!(
            (
                after.created_at,
                after.expires_at,
                after.status,
                after.source_connection_state.status,
                after.source_connection_state.last_updated_at,
                after.destination_connection_state.status,
                after.destination_connection_state.last_updated_at,
            ),
            (
                before.created_at,
                before.expires_at,
                before.status,
                before.source_connection_state.status,
                before.source_connection_state.last_updated_at,
                before.destination_connection_state.status,
                before.destination_connection_state.last_updated_at,
            ),
            "case {label:?}: {operation} must leave createdAt, expiresAt, status and both \
             connection states alone, expected {before:?}, got {after:?}"
        );

        // Both readers of the record report the stamp the update wrote, and it
        // is not the one they reported before it.
        let after_reported = described_last_updated_at(&router, &tunnel_id, label).await;
        assert!(
            after_reported > before_reported,
            "case {label:?}: DescribeTunnel must report the new stamp, expected one later than \
             {before_reported:?}, got {after_reported:?}"
        );
        assert_eq!(
            listed_last_updated_at(&router, &tunnel_id).await,
            after_reported,
            "case {label:?}: ListTunnels must report the same lastUpdatedAt as DescribeTunnel, \
             got {after_reported:?}"
        );
    }
}
