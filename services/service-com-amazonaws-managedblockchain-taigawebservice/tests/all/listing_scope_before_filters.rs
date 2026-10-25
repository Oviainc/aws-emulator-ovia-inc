// SPDX-License-Identifier: CC0-1.0

//! What a listing nested under the `/networks/{NetworkId}` label answers when that label
//! names no stored network — end to end over HTTP, over a table that spans every such
//! listing, the filter combinations each one's own query members accept, and the id forms
//! a caller can put in the label.
//!
//! The rule pinned here: a listing resolves the resource scope its URI names BEFORE it
//! reports anything, and answers a scope it cannot resolve with the refusal for the
//! resource the listing is about — the service's `ResourceNotFoundException` (HTTP 404)
//! where that resource is the network itself (`ListMembers`, `ListProposals`), and the
//! recorded `InvalidRequestException` (400) naming the in-network resource where the
//! listing reports on one (`ListNodes`, on the nodes of one member; `ListProposalVotes`,
//! on the votes of one proposal). The answer is decided by the SCOPE alone: every filter
//! combination the operation accepts takes the same refusal, and none of them turns it
//! into a request-validation answer about the filters, or into a page.
//!
//! Why the filter dimension is the one spanned: a caller branches on the two conditions
//! differently — a poller retries while a network is not visible yet, a teardown swallows
//! the not-found — while SDK error classification reads a request-validation code as a
//! permanent caller bug. A request that carries filters and is answered as filter
//! validation therefore sends such a caller down the wrong branch, and leaves "that
//! network does not exist" indistinguishable from "your request was wrong".
//!
//! Each query string in the table is proved well-formed by the second pass, which sends
//! the SAME query against the network that does exist and requires a 200 — so a refusal in
//! the first pass can only have been about resolving the scope.

use serde_json::json;

use service_com_amazonaws_managedblockchain_taigawebservice::state::encode_cursor;

use crate::harness::{available_network, call, new_router};
use crate::modelled_error_payloads::unknown_id;

/// The refusal a listing whose subject IS the network answers an unresolvable label with.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// The refusal the listings whose subject is a resource INSIDE the network answer it with,
/// as the service is recorded answering an unresolvable member scope on `ListNodes`.
const INVALID_REQUEST: (u16, &str) = (400, "InvalidRequestException");

/// Which identifier the refusal's `Message` must name.
enum Names {
    /// The network the request named: this listing's subject is that network.
    Network,
    /// The resource inside the network that this listing reports on, named alongside the
    /// network the request scoped it by.
    Scope(String),
}

/// One listing nested under the `/networks/{NetworkId}` label.
struct Listing {
    /// The operation, for the case label.
    op: &'static str,
    /// The path after the network label.
    suffix: String,
    /// The output member a 200 answer pages under, which a refusal must not carry.
    page_member: &'static str,
    /// Query strings built only from this operation's own filters.
    queries: Vec<String>,
    /// The (status, error code) pair an unresolvable network label takes.
    refusal: (u16, &'static str),
    names: Names,
}

/// An unresolvable listing scope is refused on the scope alone — the same status, error
/// code and sentence whatever filters the request carries, and never a page.
#[tokio::test]
async fn an_unresolvable_listing_scope_is_refused_before_any_filter_is_applied() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "scope-1", "scope-net").await;
    let created = crate::proposal_lifecycle::create_proposal(
        &router,
        &network_id,
        &member_id,
        "scope-proposal",
        json!({ "Invitations": [{ "Principal": "111111111111" }] }),
        &[],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateProposal 200, got {} {:?}",
        created.status, created.body
    );
    let proposal_id = created.text(&["ProposalId"]);

    // Pagination tokens of the form this service issues, so the `NextToken` rows carry a
    // token a listing accepts rather than one it refuses as forged — the refusal under
    // test has to be the scope's, not the token's.
    let member_token = encode_cursor(&member_id);
    let proposal_token = encode_cursor(&proposal_id);

    // The filters and `MaxResults` bounds of each row come from the model
    // (`smithy_query(query="operation", operation="ListMembers,ListNodes,ListProposals,
    // ListProposalVotes")`): `MemberListMaxResults` and `NodeListMaxResults` are
    // `@range(1, 20)`, `ProposalListMaxResults` is `@range(1, 100)`, `MemberStatus` and
    // `NodeStatus` are the enums the two `status` filters target, and `NextToken` is
    // `@length(0, 128)` — the empty string included.
    let listings = vec![
        Listing {
            op: "ListMembers",
            suffix: "/members".to_string(),
            page_member: "Members",
            queries: vec![
                String::new(),
                "nextToken=".to_string(),
                "status=AVAILABLE".to_string(),
                "status=INACCESSIBLE_ENCRYPTION_KEY".to_string(),
                "name=org1".to_string(),
                "isOwned=false".to_string(),
                "maxResults=1".to_string(),
                "maxResults=20".to_string(),
                format!(
                    "name=org1&status=CREATING&isOwned=true&maxResults=3&nextToken={member_token}"
                ),
            ],
            refusal: NOT_FOUND,
            names: Names::Network,
        },
        Listing {
            op: "ListProposals",
            suffix: "/proposals".to_string(),
            page_member: "Proposals",
            queries: vec![
                String::new(),
                "maxResults=1".to_string(),
                "maxResults=100".to_string(),
                format!("maxResults=2&nextToken={proposal_token}"),
            ],
            refusal: NOT_FOUND,
            names: Names::Network,
        },
        Listing {
            // Every row names the member whose nodes the listing reports on: that member
            // is the scope, and it is what the refusal names.
            op: "ListNodes",
            suffix: "/nodes".to_string(),
            page_member: "Nodes",
            queries: vec![
                format!("memberId={member_id}"),
                format!("memberId={member_id}&status=AVAILABLE"),
                format!("memberId={member_id}&status=INACCESSIBLE_ENCRYPTION_KEY"),
                format!("memberId={member_id}&maxResults=1"),
                format!("memberId={member_id}&maxResults=20"),
                format!("memberId={member_id}&maxResults=4&nextToken={member_token}"),
            ],
            refusal: INVALID_REQUEST,
            names: Names::Scope(member_id.clone()),
        },
        Listing {
            op: "ListProposalVotes",
            suffix: format!("/proposals/{proposal_id}/votes"),
            page_member: "ProposalVotes",
            queries: vec![
                String::new(),
                "maxResults=1".to_string(),
                "maxResults=100".to_string(),
                format!("maxResults=2&nextToken={proposal_token}"),
            ],
            refusal: INVALID_REQUEST,
            names: Names::Scope(proposal_id.clone()),
        },
    ];

    // The id forms a caller can put in the label for a network nothing holds: the shape
    // this service mints, both ends of `ResourceIdString`'s `@length(1, 32)`, and a value
    // that is no id of this service at all. None of them is format-validated, so all four
    // reach the lookup.
    let unknown_networks = [
        ("a service-shaped id nothing holds", unknown_id("n", 'A')),
        ("the one-character minimum", "x".to_string()),
        ("the 32-character maximum", "Z".repeat(32)),
        ("a value that is no id of this service", "no-such-network".to_string()),
    ];

    for listing in &listings {
        for (form, unknown) in &unknown_networks {
            for query in &listing.queries {
                let case = format!("{} over {form} ?{query}", listing.op);
                let answer = call(&router, "GET", &path(unknown, listing, query), None).await;
                assert_eq!(
                    (answer.status, answer.error_type.as_str()),
                    listing.refusal,
                    "case {case}: expected {} {}, got {} {:?}",
                    listing.refusal.0,
                    listing.refusal.1,
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.text(&["__type"]),
                    listing.refusal.1,
                    "case {case}: expected the body to name the same shape as the error-type \
                     header, got {:?}",
                    answer.body
                );
                let message = answer.text(&["Message"]);
                match &listing.names {
                    Names::Network => assert_eq!(
                        message,
                        format!("Network {unknown} not found."),
                        "case {case}: expected the service's not-found sentence for the network, \
                         got {:?}",
                        answer.body
                    ),
                    Names::Scope(scope) => assert!(
                        message.contains(scope.as_str()) && message.contains(unknown.as_str()),
                        "case {case}: expected a Message naming {scope:?} and {unknown:?}, got \
                         {:?}",
                        answer.body
                    ),
                }
                // A refusal is not a disguised page: no listing member and no cursor.
                assert_eq!(
                    answer.body.get(listing.page_member),
                    None,
                    "case {case}: expected no {} member on a refusal, got {:?}",
                    listing.page_member,
                    answer.body
                );
                assert_eq!(
                    answer.body.get("NextToken"),
                    None,
                    "case {case}: expected no NextToken on a refusal, got {:?}",
                    answer.body
                );
            }
        }
    }

    // `MemberId` "is required for Hyperledger Fabric", so the one scope `ListNodes` can be
    // missing is also refused on the scope: the network it could not resolve is what the
    // sentence names, and it is still not a page.
    for (form, unknown) in &unknown_networks {
        let case = format!("ListNodes with no memberId over {form}");
        let answer = call(&router, "GET", &format!("/networks/{unknown}/nodes"), None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            INVALID_REQUEST,
            "case {case}: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.text(&["Message"]).contains(unknown.as_str()),
            "case {case}: expected a Message naming {unknown:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("Nodes"),
            None,
            "case {case}: expected no Nodes member on a refusal, got {:?}",
            answer.body
        );
    }

    // Second pass: the SAME query strings against the network that DOES exist all answer
    // 200, so every one of them is a request these operations accept — which is what makes
    // the first pass a statement about resolving the scope and nothing else. It also shows
    // the refusals wrote nothing: each listing still reports for the stored network.
    for listing in &listings {
        for query in &listing.queries {
            let case = format!("{} on the stored network ?{query}", listing.op);
            let answer = call(&router, "GET", &path(&network_id, listing, query), None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        }
    }

    // And the two listings whose subject is the network report what it holds: its founding
    // member and the proposal created above. A 404 for an id nothing holds is a refusal,
    // not the empty page an existing-but-empty network would answer with.
    for (case, suffix, page_member) in [
        ("ListMembers", "/members", "Members"),
        ("ListProposals", "/proposals", "Proposals"),
    ] {
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}{suffix}"),
            None,
        )
        .await;
        assert_eq!(
            answer.body[page_member].as_array().map(Vec::len),
            Some(1),
            "case {case} on the stored network: expected the one record it holds, got {:?}",
            answer.body
        );
    }
}

/// The request path for `listing` scoped by `network_id`, with `query` (empty for none).
fn path(network_id: &str, listing: &Listing, query: &str) -> String {
    let base = format!("/networks/{network_id}{}", listing.suffix);
    if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    }
}
