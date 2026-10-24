// SPDX-License-Identifier: CC0-1.0

//! `InviteAction$Principal` is held to the domain its documentation gives it, end to end
//! over HTTP: "The Amazon Web Services account ID to invite"
//! (`smithy_query(query="shape", shape="InviteAction")`), and an Amazon Web Services
//! account ID is exactly twelve ASCII decimal digits — so `CreateProposal` refuses a
//! `Principal` that is not one with the modelled `InvalidRequestException` (HTTP 400) and
//! stores no proposal for it.
//!
//! The table spans the class the rule generalises over: the ways a string can fail to be
//! an account ID (wrong length on either side of the bound, non-digit characters,
//! separators, surrounding space, non-ASCII digits, an ARN, the empty string) and the ways
//! it can be one (leading zeros, every digit, several invites in one list), plus the
//! position of the offending entry in an `Invitations` list of more than one. Every
//! assertion goes through the router the production emulator mounts.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{available_network, call, is_resource_id, ACCOUNT};
use crate::network_and_node_lifecycle::Answer;

/// One row: the `Principal`s one `CreateProposal` request's `Invitations` carry, and
/// whether an Amazon Web Services account ID is what each of them is.
#[derive(Debug)]
struct PrincipalCase {
    name: &'static str,
    principals: &'static [&'static str],
    /// Whether the create is accepted: true exactly when every `Principal` in the row is
    /// an account ID.
    accepted: bool,
}

/// Create one proposal whose `Invitations` name `principals`, through the one create
/// helper this test target shares.
async fn create_proposal(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
    principals: &[&str],
) -> Answer {
    let invitations: Vec<Value> = principals
        .iter()
        .map(|principal| json!({ "Principal": principal }))
        .collect();
    crate::proposal_lifecycle::create_proposal(
        router,
        network_id,
        member_id,
        token,
        json!({ "Invitations": invitations }),
        &[],
    )
    .await
}

/// The `ProposalId`s `ListProposals` reports for `network_id`.
async fn listed_proposal_ids(router: &MultiServiceRouter, network_id: &str) -> Vec<String> {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/proposals"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case listing: expected ListProposals 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Proposals"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["ProposalId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `Principal`s `GetProposal` reports for `proposal_id`, in the order it holds them.
async fn stored_principals(
    router: &MultiServiceRouter,
    network_id: &str,
    proposal_id: &str,
) -> Vec<String> {
    let answer = crate::proposal_lifecycle::get_proposal(router, network_id, proposal_id).await;
    assert_eq!(
        answer.status, 200,
        "case {proposal_id}: expected GetProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Proposal"]["Actions"]["Invitations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Principal"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A `Principal` that is an Amazon Web Services account ID is stored and reads back; one
/// that is not is refused with the modelled `InvalidRequestException` and nothing is stored
/// for it.
#[tokio::test]
async fn create_proposal_holds_every_invite_principal_to_the_account_id_domain() {
    let (router, (network_id, member_id)) = new_router_with_network().await;

    // Rows: every way a value fails to be a twelve-ASCII-digit account ID, and several
    // ways it is one. The `Invitations` list of more than one row puts the offending entry
    // in each position, so the rule is checked over the whole list and not just its head.
    let cases: &[PrincipalCase] = &[
        PrincipalCase {
            name: "the emulator's own account, leading zeros and all",
            principals: &[ACCOUNT],
            accepted: true,
        },
        PrincipalCase {
            name: "another twelve-digit account",
            principals: &["210987654321"],
            accepted: true,
        },
        PrincipalCase {
            name: "every digit the same",
            principals: &["999999999999"],
            accepted: true,
        },
        PrincipalCase {
            name: "two accounts in one proposal",
            principals: &["111111111111", "123456789012"],
            accepted: true,
        },
        PrincipalCase {
            name: "not an account id at all",
            principals: &["not-an-account"],
            accepted: false,
        },
        PrincipalCase {
            name: "the empty string",
            principals: &[""],
            accepted: false,
        },
        PrincipalCase {
            name: "one digit short",
            principals: &["12345678901"],
            accepted: false,
        },
        PrincipalCase {
            name: "one digit long",
            principals: &["1234567890123"],
            accepted: false,
        },
        PrincipalCase {
            name: "twelve digits with separators",
            principals: &["1234-5678-9012"],
            accepted: false,
        },
        PrincipalCase {
            name: "twelve characters, one of them a letter",
            principals: &["12345678901a"],
            accepted: false,
        },
        PrincipalCase {
            name: "twelve digits with surrounding space",
            principals: &[" 123456789012 "],
            accepted: false,
        },
        PrincipalCase {
            name: "twelve non-ASCII digits",
            principals: &["１２３４５６７８９０１２"],
            accepted: false,
        },
        PrincipalCase {
            name: "the account's root ARN instead of its id",
            principals: &["arn:aws:iam::123456789012:root"],
            accepted: false,
        },
        PrincipalCase {
            name: "a member id instead of an account id",
            // The shape every Managed Blockchain resource id takes (a prefix, a hyphen and
            // 26 base-32 characters), which is not an account id.
            principals: &["m-ABCDEFGHIJKLMNOPQRSTUVWXYZ"],
            accepted: false,
        },
        PrincipalCase {
            name: "the second invite of two is malformed",
            principals: &["111111111111", "not-an-account"],
            accepted: false,
        },
        PrincipalCase {
            name: "the first invite of two is malformed",
            principals: &["not-an-account", "111111111111"],
            accepted: false,
        },
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (row, case) in cases.iter().enumerate() {
        let answer = create_proposal(
            &router,
            &network_id,
            &member_id,
            &format!("principal-{row}"),
            case.principals,
        )
        .await;
        let expected: (u16, &str) = if case.accepted {
            (200, "")
        } else {
            (400, "InvalidRequestException")
        };
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            expected,
            "case {:?}: expected {expected:?}, got {} {:?}",
            case.name,
            answer.status,
            answer.body
        );
        let proposal_id = answer.text(&["ProposalId"]);
        if case.accepted {
            assert!(
                is_resource_id(&proposal_id, "p"),
                "case {:?}: expected a minted proposal id, got {:?}",
                case.name,
                answer.body
            );
            let stored = stored_principals(&router, &network_id, &proposal_id).await;
            assert_eq!(
                stored, case.principals,
                "case {:?}: expected the stored invites to name {:?}, got {stored:?}",
                case.name, case.principals
            );
            accepted.push(proposal_id);
            continue;
        }
        assert!(
            proposal_id.is_empty(),
            "case {:?}: a refused create must mint no proposal id, got {:?}",
            case.name,
            answer.body
        );
        // The refusal says which input member was wrong and what the rule is, so the
        // caller can fix it: the prose is in the `Message` member this service's error
        // shapes declare.
        let message = answer.text(&["Message"]);
        assert!(
            message.contains("Principal") && message.contains("account"),
            "case {:?}: expected the refusal Message to name the Principal member and the \
             account-id rule, got {:?}",
            case.name,
            answer.body
        );
    }

    // Nothing but the accepted rows was stored: a refused create leaves no proposal behind.
    let mut listed = listed_proposal_ids(&router, &network_id).await;
    let mut expected = accepted.clone();
    listed.sort();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case the whole table: expected exactly the accepted proposals to be stored, got \
         {listed:?}"
    );
}

/// A refused invite consumes nothing: the `ClientRequestToken` it carried is still free, so
/// the corrected request creates the proposal instead of replaying a refusal, and no
/// invitation was recorded for the value that was refused.
#[tokio::test]
async fn a_refused_invite_principal_consumes_neither_the_token_nor_the_store() {
    let (router, (network_id, member_id)) = new_router_with_network().await;

    let refused = create_proposal(
        &router,
        &network_id,
        &member_id,
        "retry-token",
        &["not-an-account"],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case the refused create: expected 400 InvalidRequestException, got {} {:?}",
        refused.status,
        refused.body
    );

    // The SAME token, now carrying an account id: the create goes through, which it could
    // not if the refused attempt had claimed the token.
    let accepted = create_proposal(
        &router,
        &network_id,
        &member_id,
        "retry-token",
        &[ACCOUNT],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case the corrected retry: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
    let proposal_id = accepted.text(&["ProposalId"]);
    assert!(
        is_resource_id(&proposal_id, "p"),
        "case the corrected retry: expected a minted proposal id, got {:?}",
        accepted.body
    );
    let stored = stored_principals(&router, &network_id, &proposal_id).await;
    assert_eq!(
        stored,
        vec![ACCOUNT.to_string()],
        "case the corrected retry: expected the stored invite to name {ACCOUNT}, got {stored:?}"
    );

    // Nothing was invited by the refused request either: the only invitations this account
    // can see are the ones an APPROVED proposal mints, and this proposal is still open.
    let answer = call(&router, "GET", "/invitations", None).await;
    assert_eq!(
        answer.status, 200,
        "case the invitation listing: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body["Invitations"]
            .as_array()
            .is_some_and(|items| items.is_empty()),
        "case the invitation listing: expected no invitation to have been minted, got {:?}",
        answer.body
    );
}

/// A router with one `AVAILABLE` Hyperledger Fabric network, and that network's id and the
/// id of the member `CreateNetwork` minted with it.
async fn new_router_with_network() -> (MultiServiceRouter, (String, String)) {
    let router = crate::harness::new_router();
    let ids = available_network(&router, "invite-principal", "invite-principal-net").await;
    (router, ids)
}
