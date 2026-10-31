// SPDX-License-Identifier: CC0-1.0

//! The `tunnelId` `OpenTunnel` mints is a well-formed version-4 UUID in its
//! canonical text form, and it is the SAME id the record is filed and read back
//! under.
//!
//! The id is laid out by the `uuid` crate, reached through the workspace's one
//! rendering of 16 opaque bytes as a version-4 UUID
//! (`service_common::uuid_from_random_bytes`), over this service's own minted bytes.
//! So this suite asserts the properties that layout owns — the canonical lowercase
//! hyphenated 8-4-4-4-12 form, the version digit and the variant digit — and the
//! invariants the emulator owns: the id satisfies the modelled `TunnelId` pattern,
//! two mints differ, and `DescribeTunnel`/`ListTunnels`/`RotateTunnelAccessToken`
//! report the very id `OpenTunnel` returned.
//!
//! Every case is a row in a table whose rows differ along the `OpenTunnel` request
//! shape (what the request carries, how many services, tags present or not, the
//! Region signed for), because the minted form is a property of the mint and not of
//! any one request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does, signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/IoTSecuredTunneling/\
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

/// Whether `id` satisfies `TunnelId`'s modelled pattern
/// `^[a-zA-Z0-9_\-+=:]{1,128}$` (from `smithy_query` shape=TunnelId).
fn is_modelled_tunnel_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-+=:".contains(c))
}

/// Why `id` is not a version-4 UUID in canonical text form, or `None` when it is.
///
/// The form is spelled out here — in the TEST, which is where restating a layout is
/// the point — so the suite pins what the emulator renders independently of the
/// library that renders it: five hyphen-separated groups of 8-4-4-4-12 lowercase
/// hexadecimal digits, the version nibble (the first digit of the third group) `4`,
/// and the variant digit (the first of the fourth group) one of `8`, `9`, `a`, `b`.
fn not_a_version_4_uuid(id: &str) -> Option<String> {
    let groups: Vec<&str> = id.split('-').collect();
    if !groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12]) {
        return Some("not five groups of 8-4-4-4-12".to_string());
    }
    if !groups.iter().all(|group| {
        group
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }) {
        return Some("not lowercase hexadecimal throughout".to_string());
    }
    let version = groups[2].chars().next();
    if version != Some('4') {
        return Some(format!("version digit is {version:?}, not '4'"));
    }
    let variant = groups[3].chars().next();
    if !matches!(variant, Some('8' | '9' | 'a' | 'b')) {
        return Some(format!(
            "variant digit is {variant:?}, not one of '8', '9', 'a', 'b'"
        ));
    }
    None
}

/// One `OpenTunnel` request and a label for it.
struct MintCase {
    label: &'static str,
    /// The Region the request is signed for, so the mint is exercised on more
    /// than one endpoint.
    region: &'static str,
    input: Value,
}

/// The table every assertion below runs over: the rows differ along the request
/// shape and the Region, none of which the minted id is a function of.
fn mint_cases() -> Vec<MintCase> {
    vec![
        MintCase {
            label: "fully specified",
            region: "eu-west-1",
            input: json!({
                "description": "demo",
                "destinationConfig": {"thingName": "thing-1", "services": ["SSH"]},
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 60},
                "tags": [{"key": "env", "value": "test"}],
            }),
        },
        MintCase {
            label: "empty request",
            region: "us-east-1",
            input: json!({}),
        },
        MintCase {
            label: "several services, no tags",
            region: "ap-southeast-2",
            input: json!({
                "destinationConfig": {"thingName": "thing-2", "services": ["SSH", "SSH2", "SSH3"]},
            }),
        },
        MintCase {
            label: "description only, lower timeout bound",
            region: "us-west-2",
            input: json!({
                "description": "no destination",
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 1},
            }),
        },
        MintCase {
            label: "upper timeout bound and two tags",
            region: "eu-central-1",
            input: json!({
                "destinationConfig": {"thingName": "thing-3", "services": ["SSH"]},
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 720},
                "tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}],
            }),
        },
    ]
}

#[tokio::test]
async fn open_tunnel_mints_a_canonical_version_4_uuid_tunnel_id() {
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for MintCase {
        label,
        region,
        input,
    } in mint_cases()
    {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        let (status, opened) = call(&router, region, "OpenTunnel", &input).await;
        assert_eq!(status, 200, "case {label:?}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_modelled_tunnel_id(&tunnel_id),
            "case {label:?}: tunnelId must satisfy the modelled pattern, got {tunnel_id:?}"
        );
        assert!(
            not_a_version_4_uuid(&tunnel_id).is_none(),
            "case {label:?}: tunnelId {tunnel_id:?} must be a version-4 UUID in canonical form: {:?}",
            not_a_version_4_uuid(&tunnel_id)
        );

        assert!(
            minted.insert(tunnel_id.clone()),
            "case {label:?}: tunnelId {tunnel_id:?} was already minted for another tunnel"
        );

        // The minted id is the id the resource is filed under: the ARN names it,
        // and every reader reports the same string back.
        let tunnel_arn = opened["tunnelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            tunnel_arn.ends_with(&format!("tunnel/{tunnel_id}")),
            "case {label:?}: the ARN must name the minted id, got {tunnel_arn:?}"
        );
        assert!(
            engine
                .state::<TunnelState>()
                .lock()
                .tunnel(region, &tunnel_id)
                .is_some(),
            "case {label:?}: the tunnel must be persisted under the minted id {tunnel_id:?}"
        );

        let (status, described) = call(
            &router,
            region,
            "DescribeTunnel",
            &json!({"tunnelId": tunnel_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeTunnel answered {described:?}"
        );
        assert_eq!(
            described["tunnel"]["tunnelId"].as_str(),
            Some(tunnel_id.as_str()),
            "case {label:?}: DescribeTunnel reported {:?}",
            described["tunnel"]["tunnelId"]
        );

        let (status, listed) = call(&router, region, "ListTunnels", &json!({})).await;
        assert_eq!(status, 200, "case {label:?}: ListTunnels answered {listed:?}");
        assert!(
            listed["tunnelSummaries"]
                .as_array()
                .is_some_and(|summaries| summaries
                    .iter()
                    .any(|summary| summary["tunnelId"].as_str() == Some(tunnel_id.as_str()))),
            "case {label:?}: the listing must report the minted id {tunnel_id:?}, got {listed:?}"
        );

        let (status, rotated) = call(
            &router,
            region,
            "RotateTunnelAccessToken",
            &json!({"tunnelId": tunnel_id, "clientMode": "ALL"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: RotateTunnelAccessToken answered {rotated:?}"
        );
        assert!(
            rotated["tunnelArn"]
                .as_str()
                .is_some_and(|arn| arn == tunnel_arn),
            "case {label:?}: the rotated tunnel must be the minted one, got {:?}",
            rotated["tunnelArn"]
        );
    }
}

#[tokio::test]
async fn every_mint_in_one_store_is_a_distinct_well_formed_uuid() {
    // The ids of many tunnels opened against ONE endpoint: each is a distinct,
    // well-formed version-4 UUID, so the layout holds for every mint and not only
    // for the first one in a process.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);
    const REGION: &str = "us-east-1";

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for index in 0..16 {
        let label = format!("mint {index}");
        let (status, opened) = call(
            &router,
            REGION,
            "OpenTunnel",
            &json!({
                "destinationConfig": {"thingName": format!("thing-{index}"), "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            not_a_version_4_uuid(&tunnel_id).is_none(),
            "case {label:?}: tunnelId {tunnel_id:?} must be a version-4 UUID in canonical form: {:?}",
            not_a_version_4_uuid(&tunnel_id)
        );
        assert!(
            minted.insert(tunnel_id.clone()),
            "case {label:?}: tunnelId {tunnel_id:?} was already minted for another tunnel"
        );

        // Keep the concurrent-tunnel quota free for the next row.
        let (status, closed) = call(
            &router,
            REGION,
            "CloseTunnel",
            &json!({"tunnelId": tunnel_id, "delete": true}),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: CloseTunnel answered {closed:?}");
    }
}
