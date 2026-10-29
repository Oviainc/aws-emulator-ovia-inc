// SPDX-License-Identifier: CC0-1.0

//! The idempotency ledger does not outlive what a create MINTED, whichever call starts
//! the removal.
//!
//! "An idempotent operation completes no more than one time" (`ClientRequestToken`), and
//! what a replay completes is the create's own result: a token replays the ids its first
//! call minted only while one of those resources is still one a caller can use — stored,
//! with its removal not begun. A resource the REQUEST named (the network a node or a
//! member is created in, the member that owns a node) was not minted by that call, so it
//! keeps no token alive. Once nothing a token minted is still usable the token is a first
//! call again and mints afresh, whether the removal was started by the resource's own
//! delete, by the delete of the resource that owns it, or by an approved `RemoveAction`
//! proposal.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures.
//!
//! The tables span the classes the rule generalises over: the create that minted the
//! resource (`CreateNode`, `CreateMember`, `CreateAccessor`), the call that starts the
//! removal (the resource's own delete, the delete of the member that owns it, an approved
//! removal proposal), and both stages a removal is reported in (`DELETING`/
//! `PENDING_DELETION`, then `DELETED`).
//!
//! The sweep a delete makes is also PER RECORD: it releases the token of the create that
//! minted the resource being removed, and leaves every other create's token replaying what
//! IT minted — pinned over the accessor class (every `AccessorNetworkType`, both published
//! `ClientRequestToken` length bounds) and, from the other side, over a create that minted
//! several ids.

use std::time::SystemTime;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::accessor_lifecycle::{create_accessor, delete_accessor, get_accessor};
use crate::harness::{
    call, create_network, delete_member, delete_node, get_member, get_node, member_configuration,
    new_router, router_and_engine, Answer, ACCOUNT, PAST_TRANSITION,
};
use crate::list_members::{seed_members, Seed};
use crate::public_networks_and_membership::invite;
use crate::vote_on_proposal::{create_proposal, network_body, vote};

/// The instance type every node in this suite asks for: one the catalog offers for a
/// `STANDARD` Hyperledger Fabric network, so the create is refused for nothing else.
const INSTANCE_TYPE: &str = "bc.t3.small";

/// Which call starts the removal of what the row's create minted.
#[derive(Debug, Clone, Copy)]
enum Removal {
    /// The resource's own delete: `DeleteNode`, `DeleteMember`, `DeleteAccessor`.
    Own,
    /// `DeleteMember` on the member that OWNS the resource the create minted — the other
    /// path into the very same removal ("The member and all associated resources are in
    /// the process of being deleted", `MemberStatus`).
    OwningMember,
    /// An approved `RemoveAction` proposal, which removes a member "and all associated
    /// resources" the same way: the member the row's create minted, or the member that
    /// owns what it minted.
    ApprovedProposal,
}

/// One `CreateNode`, answered rather than asserted: this suite drives creates it expects
/// to be REFUSED as well as ones it expects to mint, so it needs the answer itself.
async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
) -> Answer {
    let body = json!({
        "ClientRequestToken": token,
        "MemberId": member_id,
        "NodeConfiguration": { "InstanceType": INSTANCE_TYPE },
    });
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body),
    )
    .await
}

/// One `CreateMember` joining `network_id` under `invitation_id` as `name`.
async fn create_member(
    router: &MultiServiceRouter,
    network_id: &str,
    invitation_id: &str,
    token: &str,
    name: &str,
) -> Answer {
    let body = json!({
        "ClientRequestToken": token,
        "InvitationId": invitation_id,
        "MemberConfiguration": member_configuration(name),
    });
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/members"),
        Some(body),
    )
    .await
}

/// Start the removal of `member_id` by the path `removal` names, and assert the call that
/// does it succeeded.
async fn remove_member(
    router: &MultiServiceRouter,
    network_id: &str,
    proposer: &str,
    member_id: &str,
    removal: Removal,
    case: &str,
) {
    match removal {
        Removal::Own | Removal::OwningMember => {
            let answer = delete_member(router, network_id, member_id).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected DeleteMember 200 for {member_id}, got {} {:?}",
                answer.status, answer.body
            );
        }
        Removal::ApprovedProposal => {
            // Each row stands up its own store, so one fixed token is a first call here
            // every time, and it is within `ClientRequestTokenString`'s `@length(1, 64)`.
            let proposal_id = create_proposal(
                router,
                network_id,
                proposer,
                "removal-proposal-token",
                json!({ "Removals": [{ "MemberId": member_id }] }),
            )
            .await;
            let answer = vote(router, network_id, &proposal_id, proposer, "YES").await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected VoteOnProposal 200, got {} {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// The status a read reports for one resource, under the member the body nests it in.
fn status_of(answer: &Answer, member: &str) -> String {
    answer.text(&[member, "Status"])
}

/// A two-member Hyperledger Fabric network that has settled to `AVAILABLE`: the founder
/// `CreateNetwork` minted, and one more member this account owns, so removing either
/// leaves the network standing. Answers the router, the network id, the founder and the
/// second member.
///
/// The threshold is 50% `GREATER_THAN_OR_EQUAL_TO` so that, on two members, the founder's
/// single `YES` approves a removal proposal.
async fn two_member_network(name: &str) -> (MultiServiceRouter, String, String, String) {
    let (router, engine) = router_and_engine();
    let (network_id, founder) = create_network(
        &router,
        network_body(name, name, Some(50), Some("GREATER_THAN_OR_EQUAL_TO")),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let mut seeded = seed_members(
        &engine,
        &network_id,
        &[Seed {
            name: "owner-org",
            status: "AVAILABLE",
            account: ACCOUNT,
        }],
    );
    let other = seeded.remove(0);
    (router, network_id, founder, other)
}

/// A `CreateNode` token replays its node only while that node is one a caller can use:
/// once its removal has begun — by `DeleteNode`, by the `DeleteMember` of the member that
/// owns it, or by an approved removal proposal — the same token mints a FRESH node
/// instead of handing back an id on its way out.
#[tokio::test]
async fn a_node_create_token_mints_afresh_once_its_node_is_being_removed() {
    // Rows: every call that starts the removal of a node. They differ only in which call
    // begins it, which is exactly what the rule generalises over.
    let cases: &[(&str, Removal)] = &[
        ("DeleteNode on the node itself", Removal::Own),
        (
            "DeleteMember on the member that owns the node",
            Removal::OwningMember,
        ),
        (
            "an approved proposal removing the member that owns the node",
            Removal::ApprovedProposal,
        ),
    ];

    for (case, removal) in cases {
        let (router, network_id, founder, owner) = two_member_network("nodeledger").await;

        let token = "node-create-token";
        let first = create_node(&router, &network_id, &owner, token).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected CreateNode 200, got {} {:?}",
            first.status, first.body
        );
        let first_id = first.text(&["NodeId"]);

        // While the node stands the token replays it: nothing else can explain a second
        // 200 carrying the same id.
        let replayed = create_node(&router, &network_id, &owner, token).await;
        assert_eq!(
            (replayed.status, replayed.text(&["NodeId"])),
            (200, first_id.clone()),
            "case {case:?}: expected the token to replay {first_id} while the node stands, got \
             {} {:?}",
            replayed.status,
            replayed.body
        );

        // The node settles `AVAILABLE`, so its own delete is not refused for a creation
        // still under way.
        tokio::time::sleep(PAST_TRANSITION).await;

        match removal {
            Removal::Own => {
                let answer = delete_node(&router, &network_id, &owner, &first_id).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected DeleteNode 200, got {} {:?}",
                    answer.status, answer.body
                );
            }
            other => remove_member(&router, &network_id, &founder, &owner, *other, case).await,
        }

        let read = get_node(&router, &network_id, &owner, &first_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Node")),
            (200, "DELETING".to_string()),
            "case {case:?}: expected the node's removal to have begun, got {} {:?}",
            read.status,
            read.body
        );

        // In the rows where the OWNING MEMBER is on its way out, the released token is a
        // first call again — so the retry is judged on its merits, and a node for a member
        // that can no longer take one is the modelled 409 rather than a 200 carrying the
        // dead node id.
        let owner_removed = !matches!(removal, Removal::Own);
        if owner_removed {
            let refused = create_node(&router, &network_id, &owner, token).await;
            assert_eq!(
                (refused.status, refused.error_type.as_str()),
                (409, "ResourceNotReadyException"),
                "case {case:?}: expected the retry naming the removed owner to be refused, got \
                 {} {:?}",
                refused.status,
                refused.body
            );
        }

        // Naming a member that is still in service, the same token mints a FRESH node.
        let retry_member = if owner_removed { &founder } else { &owner };
        let second = create_node(&router, &network_id, retry_member, token).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: expected the released token to mint a node, got {} {:?}",
            second.status, second.body
        );
        let second_id = second.text(&["NodeId"]);
        assert_ne!(
            second_id, first_id,
            "case {case:?}: expected a freshly minted node id, got the removed {first_id:?}"
        );

        // The fresh node reads back as a node of its own...
        let read = get_node(&router, &network_id, retry_member, &second_id).await;
        let status = status_of(&read, "Node");
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetNode 200 for the fresh node, got {} {:?}",
            read.status, read.body
        );
        assert!(
            matches!(status.as_str(), "CREATING" | "AVAILABLE"),
            "case {case:?}: expected the fresh node CREATING or AVAILABLE, got {status:?} {:?}",
            read.body
        );

        // ...and the removed one keeps reporting its own removal: nothing resurrected it.
        let read = get_node(&router, &network_id, &owner, &first_id).await;
        let status = status_of(&read, "Node");
        assert!(
            matches!(status.as_str(), "DELETING" | "DELETED"),
            "case {case:?}: expected the removed node to keep reporting its removal, got \
             {status:?} {:?}",
            read.body
        );
    }
}

/// A `CreateMember` token replays its member only while that member is one a caller can
/// use: once its removal has begun — by `DeleteMember` or by an approved removal proposal
/// — the same token mints a FRESH member, even though the network its record also named is
/// still standing.
#[tokio::test]
async fn a_member_create_token_mints_afresh_once_its_member_is_being_removed() {
    // Rows: both calls that start a member's removal.
    let cases: &[(&str, Removal)] = &[
        ("DeleteMember on the joined member", Removal::Own),
        (
            "an approved proposal removing the joined member",
            Removal::ApprovedProposal,
        ),
    ];

    for (case, removal) in cases {
        let (router, engine) = router_and_engine();
        let (network_id, founder) = create_network(
            &router,
            network_body(
                "memberledger",
                "memberledger",
                Some(50),
                Some("GREATER_THAN_OR_EQUAL_TO"),
            ),
        )
        .await;
        tokio::time::sleep(PAST_TRANSITION).await;

        let token = "member-create-token";
        let joined_name = "joined-org";
        let first = create_member(
            &router,
            &network_id,
            &invite(&engine, &network_id, ACCOUNT, SystemTime::now()),
            token,
            joined_name,
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected CreateMember 200, got {} {:?}",
            first.status, first.body
        );
        let first_id = first.text(&["MemberId"]);

        // While the member stands the token replays it, under a DIFFERENT invitation: the
        // token, not the request body, is what makes this a repeat.
        let replayed = create_member(
            &router,
            &network_id,
            &invite(&engine, &network_id, ACCOUNT, SystemTime::now()),
            token,
            joined_name,
        )
        .await;
        assert_eq!(
            (replayed.status, replayed.text(&["MemberId"])),
            (200, first_id.clone()),
            "case {case:?}: expected the token to replay {first_id} while the member stands, got \
             {} {:?}",
            replayed.status,
            replayed.body
        );

        // The member settles `AVAILABLE`, so its removal is not refused for a creation
        // still under way.
        tokio::time::sleep(PAST_TRANSITION).await;
        remove_member(&router, &network_id, &founder, &first_id, *removal, case).await;

        let read = get_member(&router, &network_id, &first_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Member")),
            (200, "DELETING".to_string()),
            "case {case:?}: expected the member's removal to have begun, got {} {:?}",
            read.status,
            read.body
        );

        // The network the first call's request named still stands, and that is NOT what
        // keeps the token alive: nothing the create minted is usable, so the token is a
        // first call again and mints a fresh member.
        let second = create_member(
            &router,
            &network_id,
            &invite(&engine, &network_id, ACCOUNT, SystemTime::now()),
            token,
            joined_name,
        )
        .await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: expected the released token to mint a member, got {} {:?}",
            second.status, second.body
        );
        let second_id = second.text(&["MemberId"]);
        assert_ne!(
            second_id, first_id,
            "case {case:?}: expected a freshly minted member id, got the removed {first_id:?}"
        );

        let read = get_member(&router, &network_id, &second_id).await;
        let status = status_of(&read, "Member");
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetMember 200 for the fresh member, got {} {:?}",
            read.status, read.body
        );
        assert!(
            matches!(status.as_str(), "CREATING" | "AVAILABLE"),
            "case {case:?}: expected the fresh member CREATING or AVAILABLE, got {status:?} {:?}",
            read.body
        );

        let read = get_member(&router, &network_id, &first_id).await;
        let status = status_of(&read, "Member");
        assert!(
            matches!(status.as_str(), "DELETING" | "DELETED"),
            "case {case:?}: expected the removed member to keep reporting its removal, got \
             {status:?} {:?}",
            read.body
        );
    }
}

/// A `CreateAccessor` token replays its accessor only while that accessor is one a caller
/// can use: `DeleteAccessor` starts the removal, and from then on — `PENDING_DELETION`
/// and, on the clock, `DELETED` — the same token mints a FRESH accessor with its own
/// billing token.
#[tokio::test]
async fn an_accessor_create_token_mints_afresh_once_its_accessor_is_being_removed() {
    // Rows: both stages a removal is reported in (`AccessorStatus` =
    // AVAILABLE | PENDING_DELETION | DELETED), so the retry is made once while the
    // deletion is still running and once after the clock has finished it.
    let cases: &[(&str, bool)] = &[
        ("retried while the accessor is PENDING_DELETION", false),
        ("retried once the accessor has settled DELETED", true),
    ];

    for (case, settled) in cases {
        let router = new_router();
        let token = "accessor-create-token";

        let first = create_accessor(&router, token, Some("ETHEREUM_MAINNET"), 0).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected CreateAccessor 200, got {} {:?}",
            first.status, first.body
        );
        let first_id = first.text(&["AccessorId"]);

        // While the accessor stands the token replays it, under a different body.
        let replayed = create_accessor(&router, token, Some("POLYGON_MAINNET"), 1).await;
        assert_eq!(
            (replayed.status, replayed.text(&["AccessorId"])),
            (200, first_id.clone()),
            "case {case:?}: expected the token to replay {first_id} while the accessor stands, \
             got {} {:?}",
            replayed.status,
            replayed.body
        );

        let answer = delete_accessor(&router, &first_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteAccessor 200, got {} {:?}",
            answer.status, answer.body
        );
        if *settled {
            tokio::time::sleep(PAST_TRANSITION).await;
        }
        let expected_status = if *settled {
            "DELETED"
        } else {
            "PENDING_DELETION"
        };
        let read = get_accessor(&router, &first_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Accessor")),
            (200, expected_status.to_string()),
            "case {case:?}: expected the accessor {expected_status}, got {} {:?}",
            read.status,
            read.body
        );

        let second = create_accessor(&router, token, Some("ETHEREUM_MAINNET"), 0).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: expected the released token to mint an accessor, got {} {:?}",
            second.status, second.body
        );
        let second_id = second.text(&["AccessorId"]);
        assert_ne!(
            second_id, first_id,
            "case {case:?}: expected a freshly minted accessor id, got the removed {first_id:?}"
        );
        assert_ne!(
            second.text(&["BillingToken"]),
            first.text(&["BillingToken"]),
            "case {case:?}: expected the fresh accessor to carry its own billing token, got the \
             removed accessor's {:?}",
            second.body
        );

        let read = get_accessor(&router, &second_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Accessor")),
            (200, "AVAILABLE".to_string()),
            "case {case:?}: expected GetAccessor 200 AVAILABLE for the fresh accessor, got {} \
             {:?}",
            read.status,
            read.body
        );

        let read = get_accessor(&router, &first_id).await;
        let status = status_of(&read, "Accessor");
        assert!(
            matches!(status.as_str(), "PENDING_DELETION" | "DELETED"),
            "case {case:?}: expected the removed accessor to keep reporting its removal, got \
             {status:?} {:?}",
            read.body
        );
    }
}

/// Every `AccessorNetworkType` the model declares (smithy_query: shape
/// AccessorNetworkType = ETHEREUM_GOERLI | ETHEREUM_MAINNET | ETHEREUM_MAINNET_AND_GOERLI
/// | POLYGON_MAINNET | POLYGON_MUMBAI).
const ACCESSOR_NETWORK_TYPES: &[&str] = &[
    "ETHEREUM_GOERLI",
    "ETHEREUM_MAINNET",
    "ETHEREUM_MAINNET_AND_GOERLI",
    "POLYGON_MAINNET",
    "POLYGON_MUMBAI",
];

/// One row of the accessor-sweep table: the accessor whose delete releases its create's
/// token, and the SIBLING accessor standing beside it whose own token must survive that
/// delete untouched.
#[derive(Debug)]
struct AccessorSweepRow {
    /// The token that minted the accessor this row deletes.
    deleted_token: String,
    /// The token that minted the accessor this row leaves alone.
    sibling_token: String,
    /// The `NetworkType` the deleted accessor was created with.
    network_type: &'static str,
    /// A different `NetworkType`, named by the retry, so a 200 answering from the stored
    /// record rather than minting afresh is visible in the body as well as in the id.
    retry_network_type: &'static str,
}

/// The ledger sweep a delete makes is PER RECORD: `DeleteAccessor` releases the token of
/// the create that minted THAT accessor, and every other create's token keeps replaying
/// the accessor it minted.
///
/// The companion of the accessor case above, over the class the rule generalises over:
/// every `AccessorNetworkType` the model declares and `ClientRequestTokenString`'s
/// published `@length(1, 64)` bounds, so neither the released token nor the surviving one
/// depends on the shape of the request that minted it.
#[tokio::test]
async fn deleting_an_accessor_releases_only_its_own_create_token() {
    // Token lengths span `ClientRequestTokenString`'s `@length(1, 64)`: the
    // one-character minimum and the 64-character maximum are rows, on both the released
    // and the surviving side, and the remaining rows are ordinary tokens in between.
    let rows: Vec<AccessorSweepRow> = ACCESSOR_NETWORK_TYPES
        .iter()
        .enumerate()
        .map(|(index, network_type)| AccessorSweepRow {
            deleted_token: match index {
                0 => "d".to_string(),
                1 => "d".repeat(64),
                _ => format!("deleted-accessor-token-{index}"),
            },
            sibling_token: match index {
                0 => "s".repeat(64),
                1 => "s".to_string(),
                _ => format!("sibling-accessor-token-{index}"),
            },
            network_type,
            retry_network_type: ACCESSOR_NETWORK_TYPES[(index + 1) % ACCESSOR_NETWORK_TYPES.len()],
        })
        .collect();

    for row in &rows {
        // A store of its own per row, so a row's tokens are first calls whatever the rows
        // before it did.
        let router = new_router();

        let sibling =
            create_accessor(&router, &row.sibling_token, Some(row.retry_network_type), 1).await;
        assert_eq!(
            sibling.status, 200,
            "case {row:?}: expected the sibling CreateAccessor 200, got {} {:?}",
            sibling.status, sibling.body
        );
        let sibling_id = sibling.text(&["AccessorId"]);

        let first = create_accessor(&router, &row.deleted_token, Some(row.network_type), 0).await;
        assert_eq!(
            first.status, 200,
            "case {row:?}: expected CreateAccessor 200, got {} {:?}",
            first.status, first.body
        );
        let first_id = first.text(&["AccessorId"]);

        let answer = delete_accessor(&router, &first_id).await;
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected DeleteAccessor 200, got {} {:?}",
            answer.status, answer.body
        );

        // The deleted accessor stays readable, on its way out, with the billing token its
        // create minted: the delete released a LEDGER record, not the resource's data.
        let read = get_accessor(&router, &first_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Accessor")),
            (200, "PENDING_DELETION".to_string()),
            "case {row:?}: expected the deleted accessor PENDING_DELETION, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.text(&["Accessor", "BillingToken"]),
            first.text(&["BillingToken"]),
            "case {row:?}: expected the deleted accessor to keep its own billing token, got {:?}",
            read.body
        );

        // Its token is a first call again: a fresh accessor, with its own id and billing
        // token, carrying the NETWORK TYPE THIS request named rather than the stored
        // record's.
        let second =
            create_accessor(&router, &row.deleted_token, Some(row.retry_network_type), 2).await;
        assert_eq!(
            second.status, 200,
            "case {row:?}: expected the released token to mint an accessor, got {} {:?}",
            second.status, second.body
        );
        let second_id = second.text(&["AccessorId"]);
        assert_ne!(
            second_id, first_id,
            "case {row:?}: expected a freshly minted accessor id, got the deleted {first_id:?}"
        );
        assert_ne!(
            second.text(&["BillingToken"]),
            first.text(&["BillingToken"]),
            "case {row:?}: expected the fresh accessor to carry its own billing token, got the \
             deleted accessor's {:?}",
            second.body
        );
        assert_eq!(
            second.text(&["NetworkType"]),
            row.retry_network_type,
            "case {row:?}: expected the fresh accessor to report the NetworkType this request \
             named, got {:?}",
            second.body
        );
        let read = get_accessor(&router, &second_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Accessor")),
            (200, "AVAILABLE".to_string()),
            "case {row:?}: expected GetAccessor 200 AVAILABLE for the fresh accessor, got {} {:?}",
            read.status,
            read.body
        );

        // The sibling's record was not swept with it: its token still replays the accessor
        // IT minted, under a body naming a different network type and tag count, which is
        // what makes a replay distinguishable from a fresh create.
        let replayed = create_accessor(&router, &row.sibling_token, Some(row.network_type), 0).await;
        assert_eq!(
            (replayed.status, replayed.text(&["AccessorId"])),
            (200, sibling_id.clone()),
            "case {row:?}: expected the sibling token to keep replaying {sibling_id}, got {} {:?}",
            replayed.status,
            replayed.body
        );
        assert_eq!(
            replayed.text(&["BillingToken"]),
            sibling.text(&["BillingToken"]),
            "case {row:?}: expected the sibling replay to answer with its first billing token, \
             got {:?}",
            replayed.body
        );
        assert_eq!(
            replayed.text(&["NetworkType"]),
            row.retry_network_type,
            "case {row:?}: expected the sibling replay to report the STORED NetworkType, got {:?}",
            replayed.body
        );
        let read = get_accessor(&router, &sibling_id).await;
        assert_eq!(
            (read.status, status_of(&read, "Accessor")),
            (200, "AVAILABLE".to_string()),
            "case {row:?}: expected the sibling accessor untouched and AVAILABLE, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// The ids a create's REQUEST named keep no token alive, and the ids it MINTED do: a
/// `CreateNetwork` token keeps replaying while the network it minted stands, even after
/// the member it minted with it has been removed.
///
/// The companion of the three cases above: the same liveness test, read from the other
/// side, so a fix that simply forgot every token on any delete would fail here.
#[tokio::test]
async fn a_create_token_keeps_replaying_while_one_minted_resource_stands() {
    let (router, network_id, founder, other) = two_member_network("standingledger").await;

    // A node on the surviving member: its own token must still replay, since nothing it
    // minted was removed by the founder's removal below.
    let node_token = "standing-node-token";
    let node = create_node(&router, &network_id, &other, node_token).await;
    assert_eq!(
        node.status, 200,
        "case the surviving member's node: expected CreateNode 200, got {} {:?}",
        node.status, node.body
    );
    let node_id = node.text(&["NodeId"]);

    // The founding member goes; the network it was minted with still stands, because the
    // second member still participates in it.
    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = delete_member(&router, &network_id, &founder).await;
    assert_eq!(
        answer.status, 200,
        "case the founder's removal: expected DeleteMember 200, got {} {:?}",
        answer.status, answer.body
    );

    // `CreateNetwork`'s token minted the network AND the founding member: the network is
    // still usable, so the token replays both ids rather than creating a second network.
    let replayed = call(
        &router,
        "POST",
        "/networks",
        Some(network_body(
            "standingledger",
            "standingledger",
            Some(50),
            Some("GREATER_THAN_OR_EQUAL_TO"),
        )),
    )
    .await;
    assert_eq!(
        (
            replayed.status,
            replayed.text(&["NetworkId"]),
            replayed.text(&["MemberId"])
        ),
        (200, network_id.clone(), founder.clone()),
        "case the network's own token: expected it to keep replaying its ids while the network \
         stands, got {} {:?}",
        replayed.status,
        replayed.body
    );

    // And the untouched node's token still replays its node.
    let replayed = create_node(&router, &network_id, &other, node_token).await;
    assert_eq!(
        (replayed.status, replayed.text(&["NodeId"])),
        (200, node_id.clone()),
        "case the untouched node's token: expected it to replay {node_id}, got {} {:?}",
        replayed.status,
        replayed.body
    );
}
