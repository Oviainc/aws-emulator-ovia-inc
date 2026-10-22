// SPDX-License-Identifier: CC0-1.0

//! Which resource this service's not-found names, and which ids it refuses as malformed
//! before looking anything up — end to end over HTTP, over tables that span the classes
//! the rules generalise over rather than the one request that exposed each.
//!
//! Three rules are pinned here:
//!
//! 1. A request whose URI nests a resource under a parent id resolves its path labels
//!    LEFT TO RIGHT: the first label that names nothing stored is the resource the 404
//!    is attributed to, and no id further right appears in the answer at all. A member
//!    inside a network that does not exist cannot be reported on, found or missing.
//! 2. An id outside the SPACE its input member's values are drawn from is refused with
//!    the modelled `InvalidRequestException` (400) naming that member and the value,
//!    before any lookup — independently of whether the value happens to sit inside
//!    `ResourceIdString`'s `@length(1, 32)` bound. Only `AccessorId` is validated this
//!    way: Managed Blockchain is recorded refusing an accessor id outside the space for
//!    both a 36-character UUID and a bare 32-character id, while an id of any other
//!    family that resolves to nothing is recorded as a `ResourceNotFoundException`. A
//!    well-formed accessor id nothing holds is therefore still a 404, which is what lets
//!    a caller tell "fix your input" from "that resource is gone".
//! 3. A not-found names the identifier it failed to resolve as this service labels it,
//!    and for an invitation that label is the input member `InvitationId` — the one place
//!    the service echoes back WHICH member it could not resolve. No invitation id is
//!    format-validated, so every unresolvable value, plausible or not, takes that one
//!    path.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 serialization, the refusal envelope and the handlers
//! together.

use serde_json::{json, Value};

use super::modelled_error_payloads::{assert_not_found, unknown_id};
use super::network_and_node_lifecycle::{
    available_network, call, create_node, member_configuration, new_router, Answer,
};

/// The base-32 alphabet Managed Blockchain writes a resource id in (RFC 4648 `A`-`Z`,
/// `2`-`7`), which is the alphabet of the accessor-id space. Source: the ids
/// `CreateAccessor` mints, and the id AWS's own `CreateAccessor` reference prints
/// (`ac-NGQ6QNKXKRGRPCLQ3ZDVZYRWRA`).
const ID_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// How many characters follow `ac-` in an accessor id.
const ACCESSOR_ID_BODY_LEN: usize = 26;

/// A well-formed accessor id whose 26 characters are all `filler`.
fn accessor_id(filler: char) -> String {
    format!(
        "ac-{}",
        std::iter::repeat_n(filler, ACCESSOR_ID_BODY_LEN).collect::<String>()
    )
}

/// The id of one accessor this account holds, minted through the accessor suite's own
/// create helper rather than a second copy of that request.
async fn minted_accessor_id(
    router: &smithy_server_runtime::MultiServiceRouter,
    token: &str,
) -> String {
    let answer =
        super::accessor_lifecycle::create_accessor(router, token, Some("ETHEREUM_MAINNET"), 0)
            .await;
    assert_eq!(
        answer.status, 200,
        "case create {token}: expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["AccessorId"])
}

/// The id of one proposal on `network_id`, minted through the proposal suite's own create
/// helper rather than a second copy of that request.
async fn minted_proposal_id(
    router: &smithy_server_runtime::MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    token: &str,
) -> String {
    let answer = super::proposal_lifecycle::create_proposal(
        router,
        network_id,
        member_id,
        token,
        json!({ "Invitations": [{ "Principal": "111111111111" }] }),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create {token}: expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["ProposalId"])
}

/// One row of the nested-path table: the request, the resource its 404 must name, the id
/// it must name, and the id that must NOT appear anywhere in the answer.
struct NestedCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    names: &'static str,
    id: String,
    silent_about: Option<String>,
}

/// Rule 1: a nested request's path labels resolve left to right, so the FIRST label that
/// names nothing stored is the resource the 404 is attributed to — over a table spanning
/// every implemented operation whose URI nests a resource under a parent id, both HTTP
/// methods that read and the two that mutate, and which label of the path is missing
/// (the parent, the leaf, or both).
#[tokio::test]
async fn a_nested_request_names_the_first_path_label_it_cannot_resolve() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "nested-1", "nested-net").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "nested-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    let proposal_id =
        minted_proposal_id(&router, &network_id, &member_id, "nested-proposal").await;

    let gone_network = unknown_id("n", 'A');
    let other_gone_network = unknown_id("n", 'B');
    let gone_member = unknown_id("m", 'C');
    let gone_node = unknown_id("nd", 'D');
    let gone_proposal = unknown_id("p", 'F');

    let cases: Vec<NestedCase> = vec![
        // The parent label names nothing: the network is the missing resource, and the
        // child id the request also carried is not mentioned at all.
        NestedCase {
            case: "GetMember/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/members/{member_id}"),
            body: None,
            names: "Network",
            id: gone_network.clone(),
            silent_about: Some(member_id.clone()),
        },
        NestedCase {
            case: "GetMember/both-unknown",
            method: "GET",
            path: format!("/networks/{other_gone_network}/members/{gone_member}"),
            body: None,
            names: "Network",
            id: other_gone_network.clone(),
            silent_about: Some(gone_member.clone()),
        },
        NestedCase {
            case: "GetNode/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/nodes/{node_id}?memberId={member_id}"),
            body: None,
            names: "Network",
            id: gone_network.clone(),
            silent_about: Some(node_id.clone()),
        },
        NestedCase {
            case: "DeleteNode/unknown-network",
            method: "DELETE",
            path: format!("/networks/{gone_network}/nodes/{node_id}?memberId={member_id}"),
            body: None,
            names: "Network",
            id: gone_network.clone(),
            silent_about: Some(node_id.clone()),
        },
        NestedCase {
            case: "UpdateNode/unknown-network",
            method: "PATCH",
            path: format!("/networks/{gone_network}/nodes/{node_id}"),
            body: Some(json!({ "MemberId": member_id })),
            names: "Network",
            id: gone_network.clone(),
            silent_about: Some(node_id.clone()),
        },
        NestedCase {
            case: "GetProposal/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/proposals/{proposal_id}"),
            body: None,
            names: "Network",
            id: gone_network.clone(),
            silent_about: Some(proposal_id.clone()),
        },
        // The parent resolves, so the leaf label is next and IT is the missing resource.
        NestedCase {
            case: "GetMember/unknown-member",
            method: "GET",
            path: format!("/networks/{network_id}/members/{gone_member}"),
            body: None,
            names: "Member",
            id: gone_member.clone(),
            silent_about: None,
        },
        NestedCase {
            case: "GetNode/unknown-node",
            method: "GET",
            path: format!("/networks/{network_id}/nodes/{gone_node}?memberId={member_id}"),
            body: None,
            names: "Node",
            id: gone_node.clone(),
            silent_about: None,
        },
        NestedCase {
            case: "DeleteNode/unknown-node",
            method: "DELETE",
            path: format!("/networks/{network_id}/nodes/{gone_node}?memberId={member_id}"),
            body: None,
            names: "Node",
            id: gone_node.clone(),
            silent_about: None,
        },
        NestedCase {
            case: "UpdateNode/unknown-node",
            method: "PATCH",
            path: format!("/networks/{network_id}/nodes/{gone_node}"),
            body: Some(json!({ "MemberId": member_id })),
            names: "Node",
            id: gone_node.clone(),
            silent_about: None,
        },
        NestedCase {
            case: "GetProposal/unknown-proposal",
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{gone_proposal}"),
            body: None,
            names: "Proposal",
            id: gone_proposal.clone(),
            silent_about: None,
        },
    ];

    for row in cases {
        let answer = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_not_found(row.case, &answer, row.names, &row.id);
        if let Some(unmentioned) = &row.silent_about {
            let message = answer.text(&["Message"]);
            assert!(
                !message.contains(unmentioned.as_str()),
                "case {}: a failure about the parent must not name {unmentioned:?}, got \
                 {message:?}",
                row.case
            );
        }
    }

    // Every resource the refusals were about is untouched: the reads still answer, and
    // the mutating rows wrote nothing.
    for (case, path) in [
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
    ] {
        let answer = call(&router, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case} after the refusals: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// Rule 2: an `AccessorId` outside the accessor-id space is the modelled
/// `InvalidRequestException` (400) naming the member and the value, whether or not the
/// value also breaks `ResourceIdString`'s `@length(1, 32)` — over a table spanning the
/// ways a value leaves that space (a wrong prefix, no prefix, a body of the wrong length
/// on either side, a character outside the alphabet, the wrong case, and lengths from one
/// character to past the modelled bound).
#[tokio::test]
async fn an_accessor_id_outside_the_accessor_id_space_is_the_modelled_invalid_request() {
    let router = new_router();
    let minted = minted_accessor_id(&router, "malformed-control").await;

    // Rows: each a value no `CreateAccessor` could have issued. The 36-character UUID and
    // the 33-character row are past `@length(1, 32)` and so are refused at the boundary;
    // every other row sits INSIDE that bound and so is refused by the handler. Both paths
    // must word the refusal identically, which is what the shared expectation asserts.
    let malformed: Vec<(&str, String)> = vec![
        ("a uuid", "58f96631-35ad-4946-b2be-1e06e85c8536".to_string()),
        ("a bare 32-character id", "a".repeat(32)),
        ("32 hex digits", "0123456789abcdef0123456789abcdef".to_string()),
        ("33 characters", "A".repeat(33)),
        ("one character", "x".to_string()),
        ("another family's prefix", unknown_id("n", 'B')),
        ("no prefix at all", "AAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()),
        ("the prefix alone", "ac-".to_string()),
        (
            "one character short",
            format!("ac-{}", "A".repeat(ACCESSOR_ID_BODY_LEN - 1)),
        ),
        (
            "one character long",
            format!("ac-{}", "A".repeat(ACCESSOR_ID_BODY_LEN + 1)),
        ),
        (
            "a character outside the base-32 alphabet",
            format!("ac-{}1", "A".repeat(ACCESSOR_ID_BODY_LEN - 1)),
        ),
        (
            "the alphabet lower-cased",
            format!("ac-{}", "a".repeat(ACCESSOR_ID_BODY_LEN)),
        ),
        ("the minted id without its prefix", minted[3..].to_string()),
    ];

    for (case, id) in &malformed {
        let answer = call(&router, "GET", &format!("/accessors/{id}"), None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected 400 InvalidRequestException for {id:?}, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.text(&["Message"]),
            format!("AccessorId parameter {id} is malformed"),
            "case {case}: expected the refusal to name the member and the value, got {:?}",
            answer.body
        );
    }

    // The other half of the rule: an id INSIDE the space is looked up, so one nothing
    // holds is the modelled 404 and the one that was minted still reads back. Rows: the
    // first and the last id of the space, and one of every alphabet character in turn, so
    // the whole alphabet is accepted as well-formed.
    let mut well_formed: Vec<String> = vec![accessor_id('A'), accessor_id('7')];
    well_formed.extend(ID_ALPHABET.chars().map(accessor_id));
    for id in &well_formed {
        if *id == minted {
            continue;
        }
        let answer = call(&router, "GET", &format!("/accessors/{id}"), None).await;
        assert_not_found(
            "GetAccessor/well-formed-but-unminted",
            &answer,
            "Accessor",
            id,
        );
    }

    let answer = call(&router, "GET", &format!("/accessors/{minted}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case the minted id {minted}: expected GetAccessor 200 after the refusals, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.text(&["Accessor", "Id"]),
        minted,
        "case the minted id {minted}: expected the stored accessor, got {:?}",
        answer.body
    );
}

/// Rule 3: an `InvitationId` that resolves to nothing is the modelled 404 naming the
/// INPUT MEMBER and the value, for every shape of id — over a table spanning a
/// service-shaped id, an id of another resource family, prose that is not an id at all,
/// the one-character minimum and the 32-character maximum of `ResourceIdString`.
#[tokio::test]
async fn an_invitation_id_that_resolves_to_nothing_names_the_invitation_id_member() {
    let router = new_router();
    // A network exists, so these refusals are about the invitation and not about an empty
    // store; `CreateMember` reaches the same wording through its own `InvitationId`.
    let (network_id, _member_id) = available_network(&router, "inv-404", "inv-net").await;

    // No invitation id is format-validated, so the rows deliberately include values no
    // mint could produce; each must still be the SAME 404, never a 400.
    let ids: Vec<(&str, String)> = vec![
        ("a service-shaped id", unknown_id("i", 'A')),
        ("a plausible id", format!("in-{}", "0".repeat(29))),
        ("prose, not an id", "not-an-invitation-id".to_string()),
        ("one character", "x".to_string()),
        ("the 32-character maximum", "z".repeat(32)),
        ("another family's id", network_id.clone()),
        ("an uppercase id", "I-ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string()),
    ];

    for (case, id) in &ids {
        let answer = call(&router, "DELETE", &format!("/invitations/{id}"), None).await;
        assert_not_found(case, &answer, "InvitationId", id);
        assert_eq!(
            answer.text(&["Message"]),
            format!("InvitationId {id} not found."),
            "case {case}: expected the not-found to name the InvitationId member, got {:?}",
            answer.body
        );
    }

    // `CreateMember` resolves an `InvitationId` of its own, and labels it the same way:
    // the rule is about the member, not about the operation that reads it.
    for (case, id) in &ids {
        let answer = call(
            &router,
            "POST",
            &format!("/networks/{network_id}/members"),
            Some(json!({
                "ClientRequestToken": format!("inv-404-{case}"),
                "InvitationId": id,
                "MemberConfiguration": member_configuration("inv-404-org"),
            })),
        )
        .await;
        assert_not_found(case, &answer, "InvitationId", id);
    }
}

/// The two id members this suite covers answer DIFFERENTLY for the same value, which is
/// what rule 2 being scoped to the accessor-id space means: `not-an-invitation-id` is a
/// 404 as an `InvitationId` and the same string is a malformed-id refusal as an
/// `AccessorId`.
#[tokio::test]
async fn only_the_accessor_id_member_is_validated_against_an_id_space() {
    let router = new_router();

    let shared = "not-a-real-id";
    let as_invitation: Answer =
        call(&router, "DELETE", &format!("/invitations/{shared}"), None).await;
    assert_not_found(
        "the value as an InvitationId",
        &as_invitation,
        "InvitationId",
        shared,
    );

    let as_accessor = call(&router, "GET", &format!("/accessors/{shared}"), None).await;
    assert_eq!(
        (
            as_accessor.status,
            as_accessor.error_type.as_str(),
            as_accessor.text(&["Message"])
        ),
        (
            400,
            "InvalidRequestException",
            format!("AccessorId parameter {shared} is malformed")
        ),
        "case the value as an AccessorId: expected the malformed-id refusal, got {} {:?}",
        as_accessor.status,
        as_accessor.body
    );
}
