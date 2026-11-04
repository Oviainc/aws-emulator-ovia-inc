// SPDX-License-Identifier: CC0-1.0

//! A tunnel is a REGIONAL resource: it belongs to the Region of the endpoint it
//! was opened against and is neither readable nor mutable through another
//! Region's endpoint.
//!
//! Every case is a row in a table whose rows differ along the Region pair, so the
//! rule is asserted over a class of Region labels rather than over the one pair
//! the issue happened to name: both directions of a pair, two Regions of one
//! geography, a Region label that is a prefix of the other (which is what pins
//! the store's key walk), and a request carrying no credential scope at all,
//! which addresses the default Region.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    tunnel_not_found_message, DEFAULT_REGION,
};
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

/// Call one operation the way an SDK does, signed for `region` — or, when
/// `region` is `None`, with no `Authorization` header at all, which is how a
/// request that names no credential scope arrives.
async fn call(
    router: &MultiServiceRouter,
    region: Option<&str>,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"));
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    }
    let response = router
        .dispatch(request.body(bytes::Bytes::from(body)).expect("build the request"))
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The Region field of a resource ARN (`arn:<partition>:<vendor>:<region>:…`).
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

/// Whether a `ListTunnels` body reports `tunnel_id`.
fn lists(body: &Value, tunnel_id: &str) -> bool {
    body["tunnelSummaries"]
        .as_array()
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|summary| summary["tunnelId"].as_str() == Some(tunnel_id))
        })
}

/// One Region pair: the Region a tunnel is opened against, and another Region
/// whose endpoint must not reach it.
struct RegionPair {
    label: &'static str,
    /// The credential scope `OpenTunnel` is signed with; `None` is a request
    /// carrying no scope, which addresses [`DEFAULT_REGION`].
    home: Option<&'static str>,
    /// The Region whose endpoint the same tunnel is then addressed through.
    other: &'static str,
}

/// The table every assertion below runs over.
fn region_pairs() -> Vec<RegionPair> {
    vec![
        RegionPair {
            label: "us-east-1 tunnel addressed from eu-west-1",
            home: Some("us-east-1"),
            other: "eu-west-1",
        },
        // The same pair the other way round: neither Region is privileged.
        RegionPair {
            label: "eu-west-1 tunnel addressed from us-east-1",
            home: Some("eu-west-1"),
            other: "us-east-1",
        },
        // Two Regions of one geography, so isolation is not a continent-level
        // split.
        RegionPair {
            label: "ap-southeast-1 tunnel addressed from ap-southeast-2",
            home: Some("ap-southeast-1"),
            other: "ap-southeast-2",
        },
        // A label that is a strict PREFIX of the other: this is the row that
        // pins the store's Region-keyed walk, which must not spill from one
        // Region's run of records into the next.
        RegionPair {
            label: "us-west-2 tunnel addressed from us-west-20",
            home: Some("us-west-2"),
            other: "us-west-20",
        },
        // A request with no credential scope names no Region, so it addresses
        // the default one; another Region's endpoint still does not hold it.
        RegionPair {
            label: "unsigned (default Region) tunnel addressed from eu-central-1",
            home: None,
            other: "eu-central-1",
        },
    ]
}

#[tokio::test]
async fn a_tunnel_is_reachable_only_through_the_region_it_was_opened_against() {
    for RegionPair {
        label,
        home,
        other,
    } in region_pairs()
    {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        let (status, opened) = call(
            &router,
            home,
            "OpenTunnel",
            &json!({
                "description": "regional",
                "destinationConfig": {"thingName": "thing-region", "services": ["SSH"]},
                "tags": [{"key": "env", "value": "test"}],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let tunnel_arn = opened["tunnelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The tunnel belongs to the Region it was opened against, which is the
        // Region its ARN names.
        let expected_region = home.unwrap_or(DEFAULT_REGION);
        assert_eq!(
            arn_region(&tunnel_arn),
            expected_region,
            "case {label:?}: the ARN must name the Region the tunnel was opened against, got {tunnel_arn:?}"
        );

        // Its own Region's endpoint reports it, and reports the Region its ARN
        // names.
        let (status, described) =
            call(&router, home, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the home endpoint must describe the tunnel, answered {described:?}"
        );
        assert_eq!(
            described["tunnel"]["tunnelArn"].as_str(),
            Some(tunnel_arn.as_str()),
            "case {label:?}: the home endpoint reported arn {:?}",
            described["tunnel"]["tunnelArn"]
        );

        // Another Region's endpoint holds no such tunnel.
        let (status, refused) = call(
            &router,
            Some(other),
            "DescribeTunnel",
            &json!({"tunnelId": tunnel_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: DescribeTunnel from {other} answered {status} {refused:?}"
        );
        assert_eq!(
            refused["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: DescribeTunnel from {other} answered {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(tunnel_not_found_message(&tunnel_id).as_str()),
            "case {label:?}: DescribeTunnel from {other} reported {:?}",
            refused["message"]
        );
        assert!(
            refused.get("tunnel").is_none(),
            "case {label:?}: a refusal must carry no tunnel, got {:?}",
            refused.get("tunnel")
        );

        // The listing is scoped the same way: the home endpoint lists it, the
        // other Region's endpoint lists nothing at all.
        let (status, listed_home) = call(&router, home, "ListTunnels", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTunnels answered {listed_home:?}"
        );
        assert!(
            lists(&listed_home, &tunnel_id),
            "case {label:?}: the home listing must report the tunnel, got {listed_home:?}"
        );
        let (status, listed_other) = call(&router, Some(other), "ListTunnels", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTunnels from {other} answered {listed_other:?}"
        );
        assert!(
            !lists(&listed_other, &tunnel_id),
            "case {label:?}: the {other} listing must report no tunnel, got {listed_other:?}"
        );

        // The three ARN-addressed tag operations resolve the ARN only in its own
        // Region, even though the caller hands them the tunnel's real ARN.
        for (operation, input) in [
            (
                "TagResource",
                json!({"resourceArn": tunnel_arn, "tags": [{"key": "cross", "value": "region"}]}),
            ),
            (
                "UntagResource",
                json!({"resourceArn": tunnel_arn, "tagKeys": ["env"]}),
            ),
            (
                "ListTagsForResource",
                json!({"resourceArn": tunnel_arn}),
            ),
        ] {
            let (status, refused) = call(&router, Some(other), operation, &input).await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} from {other} answered {status} {refused:?}"
            );
            assert_eq!(
                refused["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {label:?}: {operation} from {other} answered {refused:?}"
            );
        }

        // And the mutating operations: refused from the other Region, and the
        // record they would have changed is untouched.
        let before = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(expected_region, &tunnel_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the opened tunnel was not persisted"));
        for (operation, input) in [
            (
                "RotateTunnelAccessToken",
                json!({"tunnelId": tunnel_id, "clientMode": "ALL"}),
            ),
            ("CloseTunnel", json!({"tunnelId": tunnel_id})),
            (
                "CloseTunnel",
                json!({"tunnelId": tunnel_id, "delete": true}),
            ),
        ] {
            let (status, refused) = call(&router, Some(other), operation, &input).await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} from {other} answered {status} {refused:?}"
            );
            assert_eq!(
                refused["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {label:?}: {operation} from {other} answered {refused:?}"
            );
        }
        let after = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(expected_region, &tunnel_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the tunnel vanished on a refused call"));
        assert_eq!(
            (
                after.status,
                after.source_access_token.clone(),
                after.destination_access_token.clone(),
                after.last_updated_at
            ),
            (
                before.status,
                before.source_access_token.clone(),
                before.destination_access_token.clone(),
                before.last_updated_at
            ),
            "case {label:?}: a call refused from {other} must leave the record exactly as it was"
        );

        // The home endpoint still serves every one of those calls, so the
        // refusals above are about the Region and nothing else.
        let (status, rotated) = call(
            &router,
            home,
            "RotateTunnelAccessToken",
            &json!({"tunnelId": tunnel_id, "clientMode": "ALL"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the home endpoint must rotate the tunnel, answered {rotated:?}"
        );
        let (status, closed) = call(
            &router,
            home,
            "CloseTunnel",
            &json!({"tunnelId": tunnel_id, "delete": true}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the home endpoint must close the tunnel, answered {closed:?}"
        );
    }
}

#[tokio::test]
async fn two_regions_hold_their_own_record_of_one_tunnel_id() {
    // The store addresses a tunnel by Region AND id, so the same id in two
    // Regions is two resources — each Region's endpoint reports its own.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    const HOME: &str = "us-east-1";
    const AWAY: &str = "eu-west-1";

    let (status, opened) = call(
        &router,
        Some(HOME),
        "OpenTunnel",
        &json!({"description": "home", "destinationConfig": {"thingName": "thing-both", "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(status, 200, "OpenTunnel answered {opened:?}");
    let tunnel_id = opened["tunnelId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The same id, as a resource of the other Region: its own record, with its
    // own ARN and description.
    {
        let state = engine.state::<TunnelState>();
        let mut store = state.lock();
        let mut away = store
            .tunnel(HOME, &tunnel_id)
            .cloned()
            .expect("the opened tunnel was not persisted");
        away.description = Some("away".to_string());
        away.tunnel_arn = format!("arn:aws:iot:{AWAY}:000000000000:tunnel/{tunnel_id}");
        store.insert_tunnel(AWAY, away);
    }

    for (region, expected_description) in [(HOME, "home"), (AWAY, "away")] {
        let (status, described) = call(
            &router,
            Some(region),
            "DescribeTunnel",
            &json!({"tunnelId": tunnel_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: DescribeTunnel answered {described:?}"
        );
        assert_eq!(
            described["tunnel"]["description"].as_str(),
            Some(expected_description),
            "case {region}: an endpoint must report its own Region's record, got {:?}",
            described["tunnel"]["description"]
        );
        assert_eq!(
            arn_region(described["tunnel"]["tunnelArn"].as_str().unwrap_or_default()),
            region,
            "case {region}: the reported ARN must name the Region being called, got {:?}",
            described["tunnel"]["tunnelArn"]
        );
        // And the listing reports exactly that one record.
        let (_, listed) = call(&router, Some(region), "ListTunnels", &json!({})).await;
        assert_eq!(
            listed["tunnelSummaries"].as_array().map(Vec::len),
            Some(1),
            "case {region}: the listing must report only this Region's tunnel, got {listed:?}"
        );
        assert!(
            lists(&listed, &tunnel_id),
            "case {region}: the listing must report the tunnel, got {listed:?}"
        );
    }

    // Deleting one Region's record leaves the other Region's alone.
    let (status, closed) = call(
        &router,
        Some(HOME),
        "CloseTunnel",
        &json!({"tunnelId": tunnel_id, "delete": true}),
    )
    .await;
    assert_eq!(status, 200, "CloseTunnel answered {closed:?}");
    let (status, described) = call(
        &router,
        Some(HOME),
        "DescribeTunnel",
        &json!({"tunnelId": tunnel_id}),
    )
    .await;
    assert_eq!(status, 400, "a deleted tunnel answered {described:?}");
    let (status, described) = call(
        &router,
        Some(AWAY),
        "DescribeTunnel",
        &json!({"tunnelId": tunnel_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region's record must survive, answered {described:?}"
    );
    assert_eq!(
        described["tunnel"]["description"].as_str(),
        Some("away"),
        "the other Region's record must be unchanged, got {:?}",
        described["tunnel"]["description"]
    );
}
