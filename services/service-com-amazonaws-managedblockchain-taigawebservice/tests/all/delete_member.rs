// SPDX-License-Identifier: CC0-1.0

//! `DeleteMember`, end to end over HTTP: the member it names is removed from the stored
//! network together with "all associated resources" (its nodes), so every later read of
//! them is the modelled 404; the network itself is deleted with its last member; and a
//! request this operation cannot carry out is refused with the modelled error rather than
//! changing anything.
//!
//! The tables below span the classes those rules generalise over: the position of the
//! deleted member in the stored ordering (with and without a node of its own), member
//! counts from one to three, every value `MemberStatus` declares, every standing public
//! network (the frameworks this operation does not apply to), and both owning-account
//! cases `MemberSummary$IsOwned` distinguishes.

use serde_json::Value;
use service_com_amazonaws_managedblockchain_taigawebservice::state::PUBLIC_NETWORKS;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_network, create_network_body, router_and_engine, ACCOUNT, PAST_TRANSITION,
};
use crate::list_members::{seed_members, Seed, OTHER_ACCOUNT};
use crate::network_and_node_lifecycle::{create_node, Answer};

/// An id body with the shape every Managed Blockchain resource id takes (26 characters of
/// the base-32 alphabet) that no create ever minted: the rows naming a network or member
/// nothing stored holds use it.
const ABSENT_SUFFIX: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// A node configuration every Fabric member can take.
fn node_configuration() -> Value {
    serde_json::json!({ "InstanceType": "bc.t3.small" })
}

/// One `DeleteMember` call: `DELETE /networks/{NetworkId}/members/{MemberId}`.
async fn delete_member(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/networks/{network_id}/members/{member_id}"),
        None,
    )
    .await
}

/// One `GetMember` call.
async fn get_member(router: &MultiServiceRouter, network_id: &str, member_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/members/{member_id}"),
        None,
    )
    .await
}

/// One `GetNetwork` call.
async fn get_network(router: &MultiServiceRouter, network_id: &str) -> Answer {
    call(router, "GET", &format!("/networks/{network_id}"), None).await
}

/// One `GetNode` call, scoped by the member that owns the node.
async fn get_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await
}

/// The member ids `ListMembers` reports for `network_id`, in the order it reported them.
async fn listed_member_ids(router: &MultiServiceRouter, network_id: &str) -> Vec<String> {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/members"),
        None,
    )
    .await;
    answer.body["Members"]
        .as_array()
        .map(|members| {
            members
                .iter()
                .map(|member| member["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A network that has settled to `AVAILABLE`, holding its founding member plus `extra`
/// seeded members this account owns and the clock owes nothing. Answers the network id and
/// every member id on it, in the stored (id) order `ListMembers` pages on.
async fn network_with_members(token: &str, extra: usize) -> (MultiServiceRouter, String, Vec<String>) {
    let (router, engine) = router_and_engine();
    let (network_id, founder) =
        create_network(&router, create_network_body(token, "delete-member-net")).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    // Each seeded record is a copy of the one `CreateNetwork` stored, with only its name,
    // status and owning account changed, so the rows exercise the handler rather than a
    // record no create could have produced.
    let seeds: Vec<Seed> = (0..extra)
        .map(|_| Seed {
            name: "seeded-org",
            status: "AVAILABLE",
            account: ACCOUNT,
        })
        .collect();
    let mut members = seed_members(&engine, &network_id, &seeds);
    members.push(founder);
    members.sort();
    (router, network_id, members)
}

/// `DeleteMember` "removes the member and all associated resources from the network": the
/// member and its nodes answer 404 afterwards, `ListMembers` reports only the survivors,
/// and every other member and node on the network is untouched.
#[tokio::test]
async fn delete_member_removes_the_member_and_its_nodes() {
    // Rows: the deleted member's position in the stored ordering — first, middle and last
    // — and whether it owned a node of its own.
    let cases: &[(&str, usize, bool)] = &[
        ("first member", 0, true),
        ("middle member", 1, false),
        ("last member", 2, true),
    ];

    for &(case, index, with_node) in cases {
        let (router, network_id, mut members) =
            network_with_members(&format!("remove-{index}"), 2).await;
        let doomed = members.remove(index);
        let survivor = members[0].clone();

        let survivor_node = create_node(
            &router,
            &network_id,
            &survivor,
            &format!("survivor-node-{index}"),
            node_configuration(),
        )
        .await;
        let doomed_node = if with_node {
            Some(
                create_node(
                    &router,
                    &network_id,
                    &doomed,
                    &format!("doomed-node-{index}"),
                    node_configuration(),
                )
                .await,
            )
        } else {
            None
        };
        tokio::time::sleep(PAST_TRANSITION).await;

        let answer = delete_member(&router, &network_id, &doomed).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected DeleteMember 200, got {} {:?}",
            answer.status, answer.body
        );

        let answer = get_member(&router, &network_id, &doomed).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected GetMember 404 ResourceNotFoundException after the delete, \
             got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            listed_member_ids(&router, &network_id).await,
            members,
            "case {case}: expected ListMembers to report only the surviving members"
        );

        // "All associated resources" go with the member: its node is gone, and no other
        // member's node is.
        if let Some(doomed_node) = doomed_node.as_deref() {
            let answer = get_node(&router, &network_id, &doomed, doomed_node).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case}: expected the deleted member's node to be gone, got {} {:?}",
                answer.status,
                answer.body
            );
        }
        let answer = get_node(&router, &network_id, &survivor, &survivor_node).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected another member's node to survive, got {} {:?}",
            answer.status, answer.body
        );
        let answer = get_member(&router, &network_id, &survivor).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the surviving member to read back, got {} {:?}",
            answer.status, answer.body
        );
        // Members are left, so the network stands.
        let answer = get_network(&router, &network_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the network to survive while it holds members, got {} {:?}",
            answer.status, answer.body
        );

        // Deleting it a second time finds nothing to delete.
        let answer = delete_member(&router, &network_id, &doomed).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected a repeated DeleteMember to be a 404, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// "If MemberId is the last member in a network specified by the last Amazon Web Services
/// account, the network is deleted also": the network stands while any member is left and
/// goes with the last one.
#[tokio::test]
async fn deleting_the_last_member_deletes_the_network() {
    // Rows: member counts from one (the founding member alone) to three, each emptied one
    // member at a time.
    for extra in 0..3_usize {
        let case = format!("{} member(s)", extra + 1);
        let (router, network_id, members) =
            network_with_members(&format!("empty-{extra}"), extra).await;

        for (position, doomed) in members.iter().enumerate() {
            let last = position + 1 == members.len();
            let answer = delete_member(&router, &network_id, doomed).await;
            assert_eq!(
                answer.status, 200,
                "case {case}/{position}: expected DeleteMember 200, got {} {:?}",
                answer.status, answer.body
            );

            let expected = if last { 404 } else { 200 };
            let answer = get_network(&router, &network_id).await;
            assert_eq!(
                answer.status, expected,
                "case {case}/{position}: expected GetNetwork {expected} after deleting \
                 {doomed}, got {} {:?}",
                answer.status, answer.body
            );
        }

        // The network the last member took with it is reported by no listing either.
        let answer = call(&router, "GET", "/networks", None).await;
        let listed: Vec<String> = answer.body["Networks"]
            .as_array()
            .map(|networks| {
                networks
                    .iter()
                    .map(|network| network["Id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !listed.contains(&network_id),
            "case {case}: expected ListNetworks not to report the deleted {network_id}, got {:?}",
            answer.body
        );
    }
}

/// Only a member whose creation has not finished is refused as not ready: every settled
/// `MemberStatus` can be deleted.
#[tokio::test]
async fn a_member_still_creating_cannot_be_deleted_yet() {
    // Rows: every value `MemberStatus` declares (smithy_query: shape MemberStatus =
    // CREATING | AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED |
    // INACCESSIBLE_ENCRYPTION_KEY). `CREATING` is "the process of creating a member"
    // (`Member$Status`) and is the one status the delete cannot complete from;
    // `INACCESSIBLE_ENCRYPTION_KEY` is documented as a state to delete the resource from.
    let cases: &[(&str, u16)] = &[
        ("CREATING", 409),
        ("AVAILABLE", 200),
        ("CREATE_FAILED", 200),
        ("UPDATING", 200),
        ("DELETING", 200),
        ("DELETED", 200),
        ("INACCESSIBLE_ENCRYPTION_KEY", 200),
    ];

    for &(status, expected) in cases {
        let (router, engine) = router_and_engine();
        let (network_id, _founder) = create_network(
            &router,
            create_network_body(&format!("status-{status}"), "status-net"),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;
        let seeded = seed_members(
            &engine,
            &network_id,
            &[Seed {
                name: "status-org",
                status,
                account: ACCOUNT,
            }],
        );
        let member_id = seeded[0].clone();

        let answer = delete_member(&router, &network_id, &member_id).await;
        assert_eq!(
            answer.status, expected,
            "case {status}: expected DeleteMember {expected}, got {} {:?}",
            answer.status, answer.body
        );
        let read = get_member(&router, &network_id, &member_id).await;
        if expected == 200 {
            assert_eq!(
                (read.status, read.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {status}: expected the deleted member to be gone, got {} {:?}",
                read.status,
                read.body
            );
        } else {
            assert_eq!(
                answer.error_type, "ResourceNotReadyException",
                "case {status}: expected the modelled not-ready error, got {:?} {:?}",
                answer.error_type, answer.body
            );
            assert_eq!(
                read.status, 200,
                "case {status}: expected the refused member to be untouched, got {} {:?}",
                read.status, read.body
            );
        }
    }
}

/// A request `DeleteMember` cannot carry out is refused with the modelled error, and
/// nothing it named is changed.
#[tokio::test]
async fn delete_member_refuses_what_it_cannot_delete() {
    let (router, engine) = router_and_engine();
    let (network_id, founder) = create_network(
        &router,
        create_network_body("refusals-1", "refusals-net"),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let seeded = seed_members(
        &engine,
        &network_id,
        &[Seed {
            name: "other-account-org",
            status: "AVAILABLE",
            // "a different Amazon Web Services account" (`MemberSummary$IsOwned`): such a
            // member is removed by an approved proposal, never by this call.
            account: OTHER_ACCOUNT,
        }],
    );
    let other_account_member = seeded[0].clone();

    let mut cases: Vec<(String, String, u16, &str)> = vec![
        (
            "a network id nothing holds".to_string(),
            format!("/networks/n-{ABSENT_SUFFIX}/members/{founder}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a member id the network does not hold".to_string(),
            format!("/networks/{network_id}/members/m-{ABSENT_SUFFIX}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a member another account owns".to_string(),
            format!("/networks/{network_id}/members/{other_account_member}"),
            403,
            "AccessDeniedException",
        ),
    ];
    // Every standing public network: `DeleteMember` "Applies only to Hyperledger Fabric",
    // and these are the networks of another framework this service holds. Catalog source:
    // `crate::state::PUBLIC_NETWORKS`.
    for public in PUBLIC_NETWORKS {
        cases.push((
            format!("the public {} network", public.id),
            format!("/networks/{}/members/{founder}", public.id),
            400,
            "InvalidRequestException",
        ));
    }

    for (case, path, expected_status, expected_error) in &cases {
        let answer = call(&router, "DELETE", path, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (*expected_status, *expected_error),
            "case {case}: expected {expected_status} {expected_error}, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // Not one refusal changed what a later caller reads.
    for member_id in [&founder, &other_account_member] {
        let answer = get_member(&router, &network_id, member_id).await;
        assert_eq!(
            answer.status, 200,
            "case refused {member_id}: expected the member to be untouched, got {} {:?}",
            answer.status, answer.body
        );
    }
    let mut expected = vec![founder.clone(), other_account_member.clone()];
    expected.sort();
    assert_eq!(
        listed_member_ids(&router, &network_id).await,
        expected,
        "case after the refusals: expected both members still listed"
    );
    let answer = get_network(&router, &network_id).await;
    assert_eq!(
        answer.status, 200,
        "case after the refusals: expected the network to be untouched, got {} {:?}",
        answer.status, answer.body
    );
}

/// The idempotency ledger does not outlive the resources it describes: a create token
/// whose every minted resource has been deleted mints afresh, while one whose network
/// still stands keeps replaying its original ids.
#[tokio::test]
async fn a_create_token_is_released_only_once_nothing_it_minted_is_left() {
    let (router, engine) = router_and_engine();
    let (network_id, founder) =
        create_network(&router, create_network_body("kept-token", "token-net")).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // One more member, so deleting the founding member leaves the network standing: the
    // token's network is still held, so the token still replays.
    let seeded = seed_members(
        &engine,
        &network_id,
        &[Seed {
            name: "survivor-org",
            status: "AVAILABLE",
            account: ACCOUNT,
        }],
    );
    let answer = delete_member(&router, &network_id, &founder).await;
    assert_eq!(
        answer.status, 200,
        "case delete one of two members: expected DeleteMember 200, got {} {:?}",
        answer.status, answer.body
    );
    let (replayed_network, replayed_member) =
        create_network(&router, create_network_body("kept-token", "token-net")).await;
    assert_eq!(
        (replayed_network.as_str(), replayed_member.as_str()),
        (network_id.as_str(), founder.as_str()),
        "case replay while the network stands: expected the token to replay its original ids"
    );

    // Now the last member goes, so the network goes with it and the token names nothing:
    // replaying it creates a new network instead of handing back ids nothing holds.
    let answer = delete_member(&router, &network_id, &seeded[0]).await;
    assert_eq!(
        answer.status, 200,
        "case delete the last member: expected DeleteMember 200, got {} {:?}",
        answer.status, answer.body
    );
    let (fresh_network, fresh_member) =
        create_network(&router, create_network_body("kept-token", "token-net")).await;
    assert_ne!(
        fresh_network, network_id,
        "case replay after the network was deleted: expected a freshly minted network id, \
         got the deleted {network_id:?}"
    );
    assert_ne!(
        fresh_member, founder,
        "case replay after the network was deleted: expected a freshly minted member id, \
         got the deleted {founder:?}"
    );
    let answer = get_network(&router, &fresh_network).await;
    assert_eq!(
        answer.status, 200,
        "case replay after the network was deleted: expected the new network to read back, \
         got {} {:?}",
        answer.status, answer.body
    );
    let answer = get_network(&router, &network_id).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case replay after the network was deleted: expected the deleted network to stay \
         gone, got {} {:?}",
        answer.status,
        answer.body
    );
}
