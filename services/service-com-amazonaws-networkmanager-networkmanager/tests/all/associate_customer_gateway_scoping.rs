// SPDX-License-Identifier: CC0-1.0

//! `AssociateCustomerGateway` pinned where the association meets the boundary of its
//! global network, driven over HTTP.
//!
//! The rule under test: a customer gateway association is addressed THROUGH the global
//! network in the request's URI, so every resource the request names must belong to THAT
//! global network — one stored under another is not addressable and is the missing-resource
//! answer — and the association's identity is the global network plus the gateway's ARN, so
//! the same gateway may be tied in two global networks independently and each network
//! reports only its own ties. The number of ties one global network holds is bounded, and an
//! associate past that bound answers the operation's modelled quota error and stores
//! nothing.
//!
//! `customer_gateway_association_lifecycle` covers the slice's round trip, its conflict and
//! its field-level refusals; this suite covers the global-network boundary those tables do
//! not cross. Every call goes through the real `MultiServiceRouter`, and nothing reaches
//! into `crate::state` to read a tie back: `GetCustomerGatewayAssociations` is the only
//! reader used.

use serde_json::json;

use crate::customer_gateway_association_lifecycle::{
    arns_of, associate, device_with_link, disassociate, get_associations, plain_cgw_arn, read_one,
};
use crate::global_network_lifecycle::{error_code, Emulator};

/// Every resource an associate names is resolved inside the global network of its URI, so a
/// device or a link of ANOTHER global network names nothing here — even though both exist.
#[tokio::test]
async fn resources_of_another_global_network_are_not_addressable() {
    let emulator = Emulator::new();
    let here = emulator.create_global_network("here").await;
    let elsewhere = emulator.create_global_network("elsewhere").await;
    let mine = device_with_link(&emulator, &here, "mine").await;
    let theirs = device_with_link(&emulator, &elsewhere, "theirs").await;
    let arn = plain_cgw_arn("0123456789abcdef1");

    // The rows span which of the two references crosses the boundary: the device alone, the
    // link alone, and both at once.
    let rows = [
        (
            "a device of another global network",
            json!({ "CustomerGatewayArn": arn, "DeviceId": theirs.device }),
        ),
        (
            "a link of another global network",
            json!({
                "CustomerGatewayArn": arn,
                "DeviceId": mine.device,
                "LinkId": theirs.link,
            }),
        ),
        (
            "a device and a link of another global network",
            json!({
                "CustomerGatewayArn": arn,
                "DeviceId": theirs.device,
                "LinkId": theirs.link,
            }),
        ),
    ];
    for (case, body) in &rows {
        let (status, refused) = associate(&emulator, &here, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {refused}"
        );
    }

    // The same request is accepted when it is sent to the global network those resources
    // DO belong to, so the refusals above are about the boundary and nothing else.
    let (status, accepted) = associate(
        &emulator,
        &elsewhere,
        json!({
            "CustomerGatewayArn": arn,
            "DeviceId": theirs.device,
            "LinkId": theirs.link,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the owning global network: expected 200, got {status} with {accepted}"
    );

    // Not one refusal stored a tie in the global network that issued it.
    let (status, page) = get_associations(&emulator, &here, "").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {page}"
    );
    assert_eq!(
        arns_of(&page),
        Vec::<String>::new(),
        "case read after the refusals: a refused associate must store nothing, got {page}"
    );
}

/// The identity of a tie is the global network plus the gateway ARN, so one gateway is tied
/// once per global network, independently in each.
#[tokio::test]
async fn one_gateway_is_tied_independently_in_each_global_network() {
    let emulator = Emulator::new();
    let first = emulator.create_global_network("first").await;
    let second = emulator.create_global_network("second").await;
    let in_first = device_with_link(&emulator, &first, "first").await;
    let in_second = device_with_link(&emulator, &second, "second").await;

    // The rows differ in whether the tie names a link, so the pair of global networks is
    // exercised with both forms of the association.
    let rows = [
        ("without a link", plain_cgw_arn("0123abcd"), false),
        ("with a link", plain_cgw_arn("fedcba98765432100"), true),
    ];
    for (case, arn, with_link) in &rows {
        for (network, subject) in [(&first, &in_first), (&second, &in_second)] {
            let mut body = json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device });
            if *with_link {
                body["LinkId"] = json!(subject.link);
            }
            let (status, created) = associate(&emulator, network, body).await;
            assert_eq!(
                status, 200,
                "case {case} in {network}: expected 200, got {status} with {created}"
            );
            assert_eq!(
                created["CustomerGatewayAssociation"]["GlobalNetworkId"],
                json!(network),
                "case {case} in {network}: the tie must name the global network asked, got {created}"
            );
        }

        // Each global network answers its OWN tie for the same ARN: the device differs.
        for (network, subject) in [(&first, &in_first), (&second, &in_second)] {
            let stored = read_one(&emulator, network, arn)
                .await
                .unwrap_or_else(|| panic!("case {case} in {network}: the tie must be listed"));
            assert_eq!(
                stored["DeviceId"],
                json!(subject.device),
                "case {case} in {network}: expected the device of this global network, got {stored}"
            );
        }
    }

    // Removing a tie in one global network leaves the other global network's tie for the
    // same gateway exactly where it was.
    let shared = &rows[0].1;
    let (status, removed) = disassociate(&emulator, &first, shared).await;
    assert_eq!(
        status, 200,
        "case disassociate in the first global network: expected 200, got {status} with {removed}"
    );
    assert!(
        read_one(&emulator, &first, shared).await.is_none(),
        "case disassociate in the first global network: the removed tie must not be listed"
    );
    let survivor = read_one(&emulator, &second, shared)
        .await
        .unwrap_or_else(|| panic!("case the second global network: its tie must survive"));
    assert_eq!(
        survivor["DeviceId"],
        json!(in_second.device),
        "case the second global network: its tie must be untouched, got {survivor}"
    );
}

/// The store bound on ties per global network: honoured at the bound, refused one past it
/// with the operation's modelled quota error, storing nothing — and per global network, so a
/// sibling of a full one still has room.
#[tokio::test]
async fn associating_past_the_per_global_network_bound_is_refused_and_stores_nothing() {
    use service_com_amazonaws_networkmanager_networkmanager::state::MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK as MAX_TIES;

    let emulator = Emulator::new();
    let network = emulator.create_global_network("bound").await;
    let sibling = emulator.create_global_network("sibling").await;
    let subject = device_with_link(&emulator, &network, "hq").await;
    let neighbour = device_with_link(&emulator, &sibling, "hq").await;

    // One well-formed ARN per tie, distinguished by an 8-hex-digit gateway id.
    let arn_of = |index: usize| plain_cgw_arn(&format!("{index:08x}"));

    let mut stored: Vec<String> = Vec::new();
    for index in 0..MAX_TIES {
        let arn = arn_of(index);
        let (status, created) = associate(
            &emulator,
            &network,
            json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case tie {index} of {MAX_TIES}: expected 200, got {status} with {created}"
        );
        stored.push(arn);
    }

    let past = arn_of(MAX_TIES);
    let (status, refused) = associate(
        &emulator,
        &network,
        json!({ "CustomerGatewayArn": past, "DeviceId": subject.device }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the bound: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case one past the bound: expected ServiceQuotaExceededException, got {refused}"
    );

    let (status, page) = get_associations(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        status, 200,
        "case read at the bound: expected 200, got {status} with {page}"
    );
    // The page's ORDER is the lifecycle suite's subject, not this one's, so both sides are
    // sorted here: what this case asserts is the SET the store holds at the bound.
    let mut expected = stored.clone();
    expected.sort();
    let mut listed = arns_of(&page);
    listed.sort();
    assert_eq!(
        listed, expected,
        "case read at the bound: a refused associate must store nothing, got {page}"
    );

    // The bound is per global network, so a sibling of a full one still accepts a tie.
    let (status, accepted) = associate(
        &emulator,
        &sibling,
        json!({ "CustomerGatewayArn": past, "DeviceId": neighbour.device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a sibling of a full global network: expected 200, got {status} with {accepted}"
    );
}
