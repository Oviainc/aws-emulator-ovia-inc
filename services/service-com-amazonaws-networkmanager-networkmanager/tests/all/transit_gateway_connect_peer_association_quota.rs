// SPDX-License-Identifier: CC0-1.0

//! The per-global-network bound on transit gateway Connect peer associations, driven over
//! HTTP through `AssociateTransitGatewayConnectPeer`.
//!
//! The rule under test: whether an associate is accepted depends ONLY on how many ties the
//! named global network holds in the store at that moment, judged against the store's own
//! bound. A request naming a peer the global network already holds a tie for reports that
//! stored tie and adds nothing, so it is accepted at the bound too; a request past the bound
//! is the operation's modelled quota refusal (HTTP 402) and stores nothing; the ties the
//! disassociate removes give their slots back; and the count is per global network, so a
//! sibling of a full one still has room. The refusal names the global network that is full,
//! the kind of tie that filled it, the quota and the service.
//!
//! The bound is the store's own constant, imported rather than spelled out, so these cases
//! follow it if it ever moves — what they pin is the SHAPE of the bound (accepted strictly
//! below it, refused at it, counted over stored ties only), not the number.
//!
//! The lifecycle suite beside this one covers what one association reports and how it is
//! read back; this file only drives its boundary, reusing that suite's request helpers so
//! both go through the same routes a client hits. Nothing reaches into `crate::state` for a
//! stored tie: every tie is read back through `GetTransitGatewayConnectPeerAssociations`.

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state::MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK as MAX_TIES;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::transit_gateway_connect_peer_association_lifecycle::{
    arns_of, associate, connect_peer_arn, disassociate, get_associations, register_gateway,
    ASSOCIATION_KIND,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The remaining wire values a quota refusal of this operation carries, written out here
/// rather than imported from the handler's own constants, so a rename is caught instead of
/// followed: the service code is the model's own `arnNamespace`/signing name for Network
/// Manager (`smithy_query query="service_info"`), and the limit code names the quota that was
/// reached. The resource type is the lifecycle suite's `ASSOCIATION_KIND`.
const SERVICE_CODE: &str = "networkmanager";
const LIMIT_CODE: &str = "transit-gateway-connect-peer-associations-per-global-network";

/// The partition and Region every peer of these cases lives in: one registered transit
/// gateway of that Region and account is what lets its peers be associated at all, so
/// holding them fixed keeps the fixture to a single registration. The account is the one
/// that owns the global network, the only account a gateway is registered from.
const PARTITION: &str = "aws";
const REGION: &str = "us-west-2";

/// A global network with one device and one registered transit gateway, which is everything
/// an associate of these cases needs. `gateway` offsets the registered gateway's id, since a
/// gateway is registered with at most one global network.
struct QuotaFixture {
    global_network: String,
    device: String,
}

async fn fixture(emulator: &Emulator, name: &str, gateway: usize) -> QuotaFixture {
    let global_network = emulator.create_global_network(name).await;
    let device = create_device_ok(emulator, &global_network, json!({ "Description": name })).await;
    register_gateway(
        emulator,
        &global_network,
        &transit_gateway_arn(PARTITION, REGION, &own_account(), gateway, 17),
    )
    .await;
    QuotaFixture {
        global_network,
        device,
    }
}

/// The ARN of the `index`th Connect peer these cases tie, each a distinct peer of the one
/// registered gateway's Region and account.
fn peer(index: usize) -> String {
    connect_peer_arn(PARTITION, REGION, &own_account(), index, 17)
}

/// Associate `arn` with the fixture's device, answering the call's status and body.
async fn tie(emulator: &Emulator, fixture: &QuotaFixture, arn: &str) -> (u16, Value) {
    associate(
        emulator,
        &fixture.global_network,
        json!({ "TransitGatewayConnectPeerArn": arn, "DeviceId": fixture.device }),
    )
    .await
}

/// The ARNs the global network holds a tie for, sorted: the SET the store holds, which is
/// what the bound counts. The listing's own order is the lifecycle suite's subject.
async fn stored_arns(emulator: &Emulator, case: &str, network: &str) -> Vec<String> {
    let (status, body) = get_associations(emulator, network, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTransitGatewayConnectPeerAssociations expected 200, got {status} with {body}"
    );
    let mut listed = arns_of(case, &body);
    listed.sort();
    listed
}

#[tokio::test]
async fn an_associate_is_accepted_below_the_bound_and_refused_at_it_without_storing_anything() {
    let emulator = Emulator::new();
    let full = fixture(&emulator, "full", 0).await;
    let sibling = fixture(&emulator, "sibling", 1).await;

    // Strictly below the bound every associate is accepted, and the store grows by exactly
    // the tie it made.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..MAX_TIES {
        let arn = peer(index);
        let (status, created) = tie(&emulator, &full, &arn).await;
        assert_eq!(
            status, 200,
            "case tie {index} of {MAX_TIES}: expected 200, got {status} with {created}"
        );
        expected.push(arn);
    }
    expected.sort();
    assert_eq!(
        stored_arns(&emulator, "at the bound", &full.global_network).await,
        expected,
        "case at the bound: the store must hold exactly the {MAX_TIES} ties that were made"
    );

    // A peer the global network ALREADY holds a tie for is reported rather than counted
    // again, so the same request is accepted at the bound and leaves the set as it was.
    let (status, repeated) = tie(&emulator, &full, &peer(0)).await;
    assert_eq!(
        status, 200,
        "case a stored tie re-asked at the bound: expected 200, got {status} with {repeated}"
    );
    assert_eq!(
        repeated["TransitGatewayConnectPeerAssociation"]["TransitGatewayConnectPeerArn"],
        json!(peer(0)),
        "case a stored tie re-asked at the bound: expected the stored tie, got {repeated}"
    );
    assert_eq!(
        stored_arns(&emulator, "after the repeat", &full.global_network).await,
        expected,
        "case after the repeat: a repeat must add no tie"
    );

    // One tie past the bound is the modelled quota refusal, which names the global network
    // that is full, the kind of tie, the quota and the service.
    let past = peer(MAX_TIES);
    let (status, refused) = tie(&emulator, &full, &past).await;
    assert_eq!(
        status, 402,
        "case one past the bound: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case one past the bound: expected ServiceQuotaExceededException, got {refused}"
    );
    assert_eq!(
        refused["ResourceId"],
        json!(full.global_network),
        "case one past the bound: the refusal must name the full global network, got {refused}"
    );
    assert_eq!(
        refused["ResourceType"],
        json!(ASSOCIATION_KIND),
        "case one past the bound: the refusal must name the kind of tie, got {refused}"
    );
    assert_eq!(
        refused["LimitCode"],
        json!(LIMIT_CODE),
        "case one past the bound: the refusal must name the quota, got {refused}"
    );
    // The refusal's prose is a TEMPLATE filled from the request and the store, not a fixed
    // sentence: it names the global network that is full and the number of ties that filled
    // it, so both are asserted as substrings rather than the whole string being pinned.
    let message = refused["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case one past the bound: expected a message in {refused}"));
    assert!(
        message.contains(&full.global_network),
        "case one past the bound: the message must name the full global network, got {message:?}"
    );
    assert!(
        message.contains(&MAX_TIES.to_string()),
        "case one past the bound: the message must name the bound {MAX_TIES}, got {message:?}"
    );
    assert_eq!(
        refused["ServiceCode"],
        json!(SERVICE_CODE),
        "case one past the bound: the refusal must name the service, got {refused}"
    );
    assert_eq!(
        stored_arns(&emulator, "after the refusal", &full.global_network).await,
        expected,
        "case after the refusal: a refused associate must store nothing"
    );

    // The count is per global network, so the very peer a full global network refused is
    // accepted by a sibling that holds no tie at all.
    let (status, accepted) = tie(&emulator, &sibling, &past).await;
    assert_eq!(
        status, 200,
        "case a sibling of a full global network: expected 200, got {status} with {accepted}"
    );
    assert_eq!(
        stored_arns(&emulator, "the sibling", &sibling.global_network).await,
        vec![past.clone()],
        "case the sibling: the sibling must hold exactly its own tie"
    );
}

#[tokio::test]
async fn a_disassociate_gives_its_slot_back_so_the_bound_counts_stored_ties_only() {
    let emulator = Emulator::new();
    let full = fixture(&emulator, "reuse", 0).await;

    // Every peer this case ties gets an id of its own, so no round can reuse one an earlier
    // round removed.
    let mut next = 0usize;
    let mut stored: Vec<String> = Vec::new();
    while stored.len() < MAX_TIES {
        let arn = peer(next);
        next += 1;
        let (status, created) = tie(&emulator, &full, &arn).await;
        assert_eq!(
            status,
            200,
            "case tie {} of {MAX_TIES}: expected 200, got {status} with {created}",
            stored.len()
        );
        stored.push(arn);
    }

    // Freeing N slots admits exactly N further ties: the slot count is the stored tie count,
    // not how many associates the global network has ever served. The rows span freeing one
    // slot and freeing several.
    for freed in [1usize, 3] {
        let removed: Vec<String> = stored.split_off(stored.len() - freed);
        for arn in &removed {
            let (status, body) = disassociate(&emulator, &full.global_network, arn).await;
            assert_eq!(
                status, 200,
                "case freeing {freed}: DisassociateTransitGatewayConnectPeer({arn}) expected 200, got {status} with {body}"
            );
        }

        // Each freed slot takes a peer the full global network would have refused.
        let mut fresh: Vec<String> = Vec::new();
        for _ in 0..freed {
            let arn = peer(next);
            next += 1;
            let (status, body) = tie(&emulator, &full, &arn).await;
            assert_eq!(
                status, 200,
                "case freeing {freed}: the freed slot must take {arn}, expected 200, got {status} with {body}"
            );
            fresh.push(arn);
        }

        // And the global network is full again: one more is refused.
        let past = peer(next);
        next += 1;
        let (status, body) = tie(&emulator, &full, &past).await;
        assert_eq!(
            status, 402,
            "case freeing {freed}: refilled to the bound, expected 402, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case freeing {freed}: refilled to the bound, expected ServiceQuotaExceededException, got {body}"
        );

        let listed = stored_arns(&emulator, "refilled", &full.global_network).await;
        assert_eq!(
            listed.len(),
            MAX_TIES,
            "case freeing {freed}: expected {MAX_TIES} stored ties, got {}",
            listed.len()
        );
        for arn in &removed {
            assert!(
                !listed.contains(arn),
                "case freeing {freed}: the removed tie {arn} must be gone, got {listed:?}"
            );
        }
        for arn in &fresh {
            assert!(
                listed.contains(arn),
                "case freeing {freed}: the new tie {arn} must be stored, got {listed:?}"
            );
        }
        stored.extend(fresh);
    }
}
