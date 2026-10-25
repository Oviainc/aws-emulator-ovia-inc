// SPDX-License-Identifier: CC0-1.0

//! Which EXCEPTION SHAPE this service refuses an unresolvable resource with, end to end
//! over HTTP.
//!
//! One rule is pinned here: a request naming a resource this service cannot resolve is
//! refused with a shape the OPERATION'S OWN modelled error list declares — the 404
//! `ResourceNotFoundException` only where that operation declares it, and otherwise the
//! 400 `InvalidRequestException` every operation of this service declares — with the
//! identifier the request carried for the unresolvable resource named in `Message`, and
//! never an empty success.
//!
//! The shape is not a free choice: a generated client's error union for an operation holds
//! exactly the shapes that operation declares, so an exception outside that set reaches a
//! caller as an unmodelled error no typed handler can catch, and branching that reads a
//! 404 as "the resource is gone" and a 400 as "fix the request" takes the wrong branch.
//!
//! The table spans the classes the rule generalises over: both halves of it (an operation
//! that declares the not-found and one that does not), the operation kinds that resolve a
//! scope (an id-addressed read and a listing), whether the request identifies the resource
//! by id or by ARN, and which label of a nested path is the one that resolves to nothing
//! (the parent that scopes the call, or the leaf it targets).
//!
//! The catalog each row's expected pair comes from is the operation's own error list in
//! the model (`smithy_query(query="operation", operation="<Name>")`): `ListMembers`,
//! `ListNodes` and `ListProposalVotes` declare AccessDeniedException (403),
//! InternalServiceErrorException (500), InvalidRequestException (400) and
//! ThrottlingException (429) and no not-found, while `GetNetwork`, `GetMember`, `GetNode`,
//! `GetProposal`, `GetAccessor`, `ListProposals` and `ListTagsForResource` declare
//! ResourceNotFoundException (404) as well.

use serde_json::json;

use crate::harness::{available_network, call, new_router, Answer, ACCOUNT, PAST_TRANSITION};
use crate::modelled_error_payloads::unknown_id;
use crate::network_and_node_lifecycle::{create_node, REGION};
use crate::resource_tags::label;

/// The refusal of an operation whose modelled error list declares the not-found.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// The refusal of an operation whose modelled error list does NOT declare it: every
/// operation of this service declares `InvalidRequestException`, so that is the shape an
/// unresolvable scope is reported in there.
const INVALID_REQUEST: (u16, &str) = (400, "InvalidRequestException");

/// One row: the request, the (status, error code) pair the operation's modelled error list
/// allows for an unresolvable resource, and the identifier its `Message` must name.
struct ScopeCase {
    case: &'static str,
    method: &'static str,
    path: String,
    refusal: (u16, &'static str),
    names: String,
}

/// An unresolvable resource is refused with a shape the operation's own modelled error
/// list declares, naming the identifier the request carried for it.
#[tokio::test]
async fn an_unresolvable_resource_is_refused_with_a_shape_the_operation_declares() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "error-sets-1", "error-sets-net").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "error-sets-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    let proposals = crate::proposal_lifecycle::create_proposal(
        &router,
        &network_id,
        &member_id,
        "error-sets-proposal",
        json!({ "Invitations": [{ "Principal": "111111111111" }] }),
        &[],
    )
    .await;
    assert_eq!(
        proposals.status, 200,
        "case setup: expected CreateProposal 200, got {} {:?}",
        proposals.status, proposals.body
    );
    let proposal_id = proposals.text(&["ProposalId"]);
    let accessors = crate::accessor_lifecycle::create_accessor(
        &router,
        "error-sets-accessor",
        Some("ETHEREUM_MAINNET"),
        0,
    )
    .await;
    assert_eq!(
        accessors.status, 200,
        "case setup: expected CreateAccessor 200, got {} {:?}",
        accessors.status, accessors.body
    );
    let accessor_id = accessors.text(&["AccessorId"]);

    let gone_network = unknown_id("n", 'A');
    let other_gone_network = unknown_id("n", 'B');
    let gone_member = unknown_id("m", 'C');
    let gone_node = unknown_id("nd", 'D');
    let gone_proposal = unknown_id("p", 'F');
    let gone_accessor = unknown_id("ac", 'G');

    let cases: Vec<ScopeCase> = vec![
        // The operations whose model declares NO not-found: the refusal is the invalid
        // request, whatever the listing was scoped by.
        ScopeCase {
            case: "ListMembers/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/members"),
            refusal: INVALID_REQUEST,
            names: gone_network.clone(),
        },
        ScopeCase {
            case: "ListMembers/unknown-network-with-filters",
            method: "GET",
            path: format!(
                "/networks/{other_gone_network}/members?status=AVAILABLE&isOwned=true&maxResults=5"
            ),
            refusal: INVALID_REQUEST,
            names: other_gone_network.clone(),
        },
        // `ListNodes` is scoped to the member that owns the nodes, so the scope it reports
        // as unresolvable is that member.
        ScopeCase {
            case: "ListNodes/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/nodes?memberId={member_id}"),
            refusal: INVALID_REQUEST,
            names: member_id.clone(),
        },
        ScopeCase {
            case: "ListNodes/unknown-member",
            method: "GET",
            path: format!("/networks/{network_id}/nodes?memberId={gone_member}"),
            refusal: INVALID_REQUEST,
            names: gone_member.clone(),
        },
        // `ListProposalVotes` reports on the proposal, so that is the scope it names —
        // including when it is the network label that resolves to nothing.
        ScopeCase {
            case: "ListProposalVotes/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/proposals/{proposal_id}/votes"),
            refusal: INVALID_REQUEST,
            names: proposal_id.clone(),
        },
        ScopeCase {
            case: "ListProposalVotes/unknown-proposal",
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{gone_proposal}/votes"),
            refusal: INVALID_REQUEST,
            names: gone_proposal.clone(),
        },
        // The other half of the rule: an operation that DOES declare the not-found answers
        // it, so a caller can still tell "that resource is gone" from "fix the request".
        ScopeCase {
            case: "ListProposals/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/proposals"),
            refusal: NOT_FOUND,
            names: gone_network.clone(),
        },
        ScopeCase {
            case: "GetNetwork/unknown",
            method: "GET",
            path: format!("/networks/{gone_network}"),
            refusal: NOT_FOUND,
            names: gone_network.clone(),
        },
        ScopeCase {
            case: "GetMember/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/members/{member_id}"),
            refusal: NOT_FOUND,
            names: gone_network.clone(),
        },
        ScopeCase {
            case: "GetMember/unknown-member",
            method: "GET",
            path: format!("/networks/{network_id}/members/{gone_member}"),
            refusal: NOT_FOUND,
            names: gone_member.clone(),
        },
        ScopeCase {
            case: "GetNode/unknown-node",
            method: "GET",
            path: format!("/networks/{network_id}/nodes/{gone_node}?memberId={member_id}"),
            refusal: NOT_FOUND,
            names: gone_node.clone(),
        },
        ScopeCase {
            case: "GetProposal/unknown-proposal",
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{gone_proposal}"),
            refusal: NOT_FOUND,
            names: gone_proposal.clone(),
        },
        ScopeCase {
            case: "GetAccessor/unknown",
            method: "GET",
            path: format!("/accessors/{gone_accessor}"),
            refusal: NOT_FOUND,
            names: gone_accessor.clone(),
        },
    ];

    for row in cases {
        let answer = call(&router, row.method, &row.path, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            row.refusal,
            "case {}: expected {} {}, got {} {:?}",
            row.case,
            row.refusal.0,
            row.refusal.1,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.text(&["__type"]),
            row.refusal.1,
            "case {}: expected the body to name the modelled shape, got {:?}",
            row.case,
            answer.body
        );
        assert!(
            answer.text(&["Message"]).contains(row.names.as_str()),
            "case {}: expected a Message naming {:?}, got {:?}",
            row.case,
            row.names,
            answer.body
        );
        // Neither shape carries an identifier the request named by ID: `Message` is all
        // `InvalidRequestException` declares, and
        // `ResourceNotFoundException$ResourceName` is an `ArnString`
        // (`@pattern ^arn:.+:.+:.+:.+:.+$`) with no ARN to name.
        assert_eq!(
            answer.body.get("ResourceName"),
            None,
            "case {}: expected no ResourceName member, got {:?}",
            row.case,
            answer.body
        );
    }

    // A resource identified BY ARN takes the same rule: `ListTagsForResource` declares the
    // not-found, so an ARN nothing holds is the 404 — and there the ARN the request sent IS
    // reportable, so `ResourceName` carries it.
    let gone_arn =
        format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:networks/{other_gone_network}");
    let answer = call(&router, "GET", &format!("/tags/{}", label(&gone_arn)), None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        NOT_FOUND,
        "case ListTagsForResource/unknown-arn: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.text(&["ResourceName"]),
        gone_arn,
        "case ListTagsForResource/unknown-arn: expected the ARN the request sent, got {:?}",
        answer.body
    );

    // Every refusal above left the store as it was: each operation still answers for the
    // resource that does exist, so the refusals were about resolving the scope and not
    // about the operation being broken or the page being empty.
    tokio::time::sleep(PAST_TRANSITION).await;
    for (case, path) in [
        ("ListMembers", format!("/networks/{network_id}/members")),
        (
            "ListNodes",
            format!("/networks/{network_id}/nodes?memberId={member_id}"),
        ),
        (
            "ListProposalVotes",
            format!("/networks/{network_id}/proposals/{proposal_id}/votes"),
        ),
        ("ListProposals", format!("/networks/{network_id}/proposals")),
        ("GetNetwork", format!("/networks/{network_id}")),
        (
            "GetMember",
            format!("/networks/{network_id}/members/{member_id}"),
        ),
        (
            "GetNode",
            format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        ),
        (
            "GetProposal",
            format!("/networks/{network_id}/proposals/{proposal_id}"),
        ),
        ("GetAccessor", format!("/accessors/{accessor_id}")),
    ] {
        let answer: Answer = call(&router, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case} after the refusals: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And the listing whose refusal this suite is about reports the network's own member,
    // which is what makes the 400 a refusal and not a disguised empty page.
    let answer = call(&router, "GET", &format!("/networks/{network_id}/members"), None).await;
    assert_eq!(
        answer.body["Members"].as_array().map(Vec::len),
        Some(1),
        "case ListMembers on the stored network: expected its one member, got {:?}",
        answer.body
    );
}
