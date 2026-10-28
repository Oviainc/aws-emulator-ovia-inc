// SPDX-License-Identifier: CC0-1.0

//! What a `connectivityInfo.privateRouteServerPeerings` entry may NAME: a route server
//! peer the caller's own Amazon EC2 account already holds.
//!
//! The entries are "The unique IDs for private route server peers" and "during environment
//! creation, the route server endpoints peer with the NSX uplink VLAN"
//! (`smithy_query(query="shape",shape="ConnectivityInfo")`), so each id names an Amazon EC2
//! resource of the caller's account that Amazon EVS does not create. The rule under test,
//! one level above the example, is the one `CreateEnvironment` already follows for every
//! other member of the request that names an existing EC2 resource — `vpcId`,
//! `serviceAccessSubnetId`, `placementGroupId`, `dedicatedHostId` and each service access
//! security group: a member naming an Amazon EC2 resource of the caller's account that the
//! account's own EC2 store does not hold is refused with the operation's modelled
//! `ValidationException`, whose `fieldList` names that member and whose clause names the id
//! the request gave, and the refused request creates nothing. So:
//!
//! * a peering whose ids the account holds route server peers for is accepted, and the
//!   stored environment reports them back on every later read;
//! * a peering naming an id the account holds no peer for is refused — one `fieldList`
//!   entry per entry that named one, aggregated with every other failed precondition of
//!   the same request — and no environment is created;
//! * the question is asked only where Amazon EVS INSTALLS VCF, because the peering is part
//!   of that installation and the member is "Not supported when `vcfVersion` is
//!   `SELF_DEPLOYED`", the mode in which it is accepted and does nothing;
//! * a peer taken back out of the account is no longer namable, while an environment
//!   created while it was held keeps reporting the peering it was created with.
//!
//! The rows differ along what the rule generalises over: WHICH of the two entries the
//! account holds (both, either one, neither), the VCF mode that decides whether the
//! peering happens at all, and the LENGTH and character classes of the id the entry
//! carries — the modelled `RouteServerPeering`
//! (`smithy_query(query="shape",shape="RouteServerPeering")`) is a string of length 3..21
//! with no pattern, so the table carries ids at both published bounds beside the ids
//! Amazon EC2's own `CreateRouteServerPeer` mints.

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{
    call, emulator, error_type, register_route_server_peers, Emulator,
};

/// The member path of the peering entry at `index`, as the refusal names it.
fn peering_member(index: usize) -> String {
    format!("connectivityInfo.privateRouteServerPeerings[{index}]")
}

/// Mint `count` route server peers in the account's own Amazon EC2 store, through EC2's
/// own writers, and answer the `RouteServerPeerId` each one was given.
///
/// The peers are built the way `CreateRouteServerPeer` builds them — an endpoint of a route
/// server placed in `subnet_id`, then its peers — so the ids under test are the ids a caller
/// that really created the peers would hold, not strings written into the map by hand.
fn mint_route_server_peers(
    engine: &engine::InMemoryEngine,
    subnet_id: &str,
    count: usize,
) -> Vec<String> {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    let subnet = store
        .subnets
        .get(subnet_id)
        .cloned()
        .unwrap_or_else(|| panic!("the account holds the subnet {subnet_id} the peers live in"));
    let route_server_id = store.next_id(ec2::state::ROUTE_SERVER_ID_PREFIX);
    let eni_address = subnet
        .cidr_block
        .as_deref()
        .and_then(|cidr_block| store.mint_route_server_eni_address(cidr_block));
    let endpoint_id = store.insert_route_server_endpoint(ec2::state::RouteServerEndpointSpec {
        route_server_id,
        subnet_id: subnet_id.to_string(),
        vpc_id: subnet.vpc_id.clone(),
        eni_address,
        tags: Vec::new(),
    });
    (0..count)
        .map(|index| {
            store
                .insert_route_server_peer(
                    &endpoint_id,
                    ec2::state::RouteServerPeerSpec {
                        peer_address: format!("192.0.2.{}", index + 1),
                        // "Valid values are from 1 to 4294967295" (`BgpOptions$PeerAsn`).
                        peer_asn: 65_001,
                        peer_liveness_detection: ec2::state::PEER_LIVENESS_BGP_KEEPALIVE,
                        tags: Vec::new(),
                    },
                )
                .unwrap_or_else(|| panic!("the endpoint {endpoint_id} takes a peer"))
        })
        .collect()
}

/// Take `peer_id` back out of the account's own Amazon EC2 store, as a caller that gives
/// the peer up does: the account then holds no route server peer of that id.
fn remove_route_server_peer(engine: &engine::InMemoryEngine, peer_id: &str) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    ec2_state.lock().route_server_peers.remove(peer_id);
}

/// Create an environment whose `connectivityInfo` names `peerings`, with `overrides`
/// merged into the request.
async fn create_with_peerings(
    router: &Emulator,
    name: &str,
    peerings: &[String],
    overrides: &Value,
) -> (u16, Value) {
    let mut request = router.create_request(Some(name));
    request["connectivityInfo"] = json!({ "privateRouteServerPeerings": peerings });
    for (member, value) in overrides
        .as_object()
        .expect("the overrides are an object")
    {
        request[member] = value.clone();
    }
    call(router, "CreateEnvironment", &request).await
}

/// Whether the account holds a peer for each of the two ids a row names.
#[derive(Debug, Clone, Copy)]
struct Held(bool, bool);

/// One row of the table below: the case, the two ids `connectivityInfo` carries, which of
/// them the account's own EC2 store holds a peer for, what else the request overrides, and
/// the input members the refusal must name — empty for a row that is accepted.
type Row = (&'static str, [&'static str; 2], Held, Value, Vec<String>);

#[tokio::test]
async fn a_peering_the_account_holds_no_route_server_peer_for_is_refused() {
    let rows: Vec<Row> = vec![
        (
            "both peers the account holds",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(true, true),
            json!({}),
            vec![],
        ),
        (
            "neither peer the account holds",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(false, false),
            json!({}),
            vec![peering_member(0), peering_member(1)],
        ),
        (
            "only the first entry names a peer the account holds",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(true, false),
            json!({}),
            vec![peering_member(1)],
        ),
        (
            "only the second entry names a peer the account holds",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(false, true),
            json!({}),
            vec![peering_member(0)],
        ),
        (
            // The shortest value the modelled `RouteServerPeering` admits (length 3..21):
            // the outcome follows the peer existing, not the shape of the id.
            "ids at the shortest length the model admits, both held",
            ["abc", "xyz"],
            Held(true, true),
            json!({}),
            vec![],
        ),
        (
            "ids at the shortest length the model admits, neither held",
            ["abc", "xyz"],
            Held(false, false),
            json!({}),
            vec![peering_member(0), peering_member(1)],
        ),
        (
            // The longest value it admits, 21 characters.
            "ids at the longest length the model admits, both held",
            ["rsp-00112233445566778", "rsp-88990011223344556"],
            Held(true, true),
            json!({}),
            vec![],
        ),
        (
            "ids at the longest length the model admits, neither held",
            ["rsp-00112233445566778", "rsp-88990011223344556"],
            Held(false, false),
            json!({}),
            vec![peering_member(0), peering_member(1)],
        ),
        (
            // The id is the key the store answers by, so an id differing only in case
            // names no peer of the account.
            "ids differing from the held peers in their case alone",
            ["RSP-0123456789ABCDEF0", "RSP-00FEDCBA98765432"],
            Held(false, false),
            json!({}),
            vec![peering_member(0), peering_member(1)],
        ),
        (
            // The peering is part of the VCF installation, and the member is "Not
            // supported when vcfVersion is SELF_DEPLOYED": that mode peers with nothing,
            // so it resolves nothing and the entries are accepted as supplied.
            "SELF_DEPLOYED names two peers the account does not hold",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(false, false),
            json!({ "vcfVersion": "SELF_DEPLOYED" }),
            vec![],
        ),
        (
            "the other installed version resolves them just as well",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(false, false),
            json!({ "vcfVersion": "VCF-5.2.2" }),
            vec![peering_member(0), peering_member(1)],
        ),
        (
            // One refusal reports every failed precondition of the request.
            "two unheld peers together with unaccepted terms",
            ["rsp-0123456789abcdef0", "rsp-00fedcba98765432"],
            Held(false, false),
            json!({ "termsAccepted": false }),
            vec![
                "termsAccepted".to_string(),
                peering_member(0),
                peering_member(1),
            ],
        ),
    ];

    for (case, peerings, held, overrides, expected_members) in rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let mut registered: Vec<&str> = Vec::new();
        if held.0 {
            registered.push(peerings[0]);
        }
        if held.1 {
            registered.push(peerings[1]);
        }
        register_route_server_peers(&router.engine, &registered);

        let ids: Vec<String> = peerings.iter().map(|id| (*id).to_string()).collect();
        let (status, body) = create_with_peerings(&router, "peering-env", &ids, &overrides).await;

        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["environment"]["connectivityInfo"]["privateRouteServerPeerings"],
                json!(ids),
                "case {case:?}: expected the peering the request gave, got {}",
                body["environment"]["connectivityInfo"]["privateRouteServerPeerings"]
            );
            assert_eq!(
                environment_count(&router).await,
                1,
                "case {case:?}: an accepted create must persist exactly one environment"
            );
            continue;
        }

        let expected: Vec<&str> = expected_members.iter().map(String::as_str).collect();
        assert_aggregated_refusal(case, status, &body, &expected);
        // The clause is built from the request: it names the id the entry carried.
        let message = body["message"].as_str().unwrap_or_default();
        for (index, peer_id) in peerings.iter().enumerate() {
            if expected.contains(&peering_member(index).as_str()) {
                assert!(
                    message.contains(peer_id),
                    "case {case:?}: expected the refusal to name {peer_id:?}, got {message:?}"
                );
            }
        }
        assert_eq!(
            environment_count(&router).await,
            0,
            "case {case:?}: a refused create must not persist an environment"
        );
    }
}

#[tokio::test]
async fn peers_amazon_ec2_minted_are_accepted_and_reported_back() {
    let router = emulator();
    let mut request = router.create_request(Some("peering-roundtrip"));
    let subnet_id = request["serviceAccessSubnetId"]
        .as_str()
        .expect("the request names the service access subnet")
        .to_string();
    // Two peers of the account, minted by Amazon EC2's own writers: the ids a caller that
    // created them through `CreateRouteServerPeer` would name here.
    let peers = mint_route_server_peers(&router.engine, &subnet_id, 2);
    assert_eq!(
        peers.len(),
        2,
        "the fixture mints the two peers Amazon EVS requires, got {peers:?}"
    );
    request["connectivityInfo"] = json!({ "privateRouteServerPeerings": peers });

    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "peers Amazon EC2 minted: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment_id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("peers Amazon EC2 minted: no environmentId in {body}"))
        .to_string();

    // Read the stored environment back: the peering it reports is the one it was created
    // with, which is what the create persisted.
    let (status, got) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(status, 200, "read back: expected 200, got {status} {got}");
    assert_eq!(
        got["environment"]["connectivityInfo"]["privateRouteServerPeerings"],
        json!(peers),
        "read back: expected the peering the create stored ({peers:?}), got {}",
        got["environment"]["connectivityInfo"]["privateRouteServerPeerings"]
    );

    // A peer the account gives up is no longer namable by a NEW environment, while the
    // environment created while it was held keeps reporting the peering it was created
    // with: the resolution happens at create time, against the store as it is then.
    remove_route_server_peer(&router.engine, &peers[0]);
    let (status, body) =
        create_with_peerings(&router, "peering-after-removal", &peers, &json!({})).await;
    assert_aggregated_refusal(
        "a peer taken back out of the account",
        status,
        &body,
        &[peering_member(0).as_str()],
    );
    let (status, got) = call(
        &router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "read back after the removal: expected 200, got {status} {got}"
    );
    assert_eq!(
        got["environment"]["connectivityInfo"]["privateRouteServerPeerings"],
        json!(peers),
        "read back after the removal: expected the stored peering ({peers:?}), got {}",
        got["environment"]["connectivityInfo"]["privateRouteServerPeerings"]
    );
}

#[tokio::test]
async fn a_request_with_no_peering_at_all_is_accepted() {
    // `connectivityInfo` is optional, so a request that carries none names no peer and
    // nothing is resolved — in either VCF mode.
    for (case, vcf_version) in [
        ("an installed version with no connectivityInfo", "VCF-5.2.1"),
        ("SELF_DEPLOYED with no connectivityInfo", "SELF_DEPLOYED"),
    ] {
        let router = emulator();
        let mut request = router.create_request(Some("no-peering"));
        request["vcfVersion"] = json!(vcf_version);
        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            body["environment"]["connectivityInfo"],
            Value::Null,
            "case {case:?}: expected no connectivityInfo to be reported, got {}",
            body["environment"]["connectivityInfo"]
        );
    }
}
