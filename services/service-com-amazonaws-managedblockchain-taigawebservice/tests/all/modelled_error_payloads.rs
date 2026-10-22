// SPDX-License-Identifier: CC0-1.0

//! What this service puts in an error RESPONSE, end to end over HTTP: the shape it
//! names, the status it answers with, and the members it does and does not serialize.
//!
//! Two rules are pinned here, each over a table that spans the class it generalises
//! over rather than the one request that exposed it:
//!
//! 1. A not-found failure names the resource the operation was ASKED FOR — the resource
//!    the request identifies, not an ancestor id present only to scope its lookup — in
//!    `Message`, and leaves `ResourceNotFoundException$ResourceName` unset, because that
//!    member targets `ArnString` (`@length(1, 1011)`, `@pattern ^arn:.+:.+:.+:.+:.+$`)
//!    and a resource this service does not hold has no ARN to name.
//! 2. A request outside a constraint the MODEL declares is refused with a shape the
//!    model declares — `InvalidRequestException`, HTTP 400, which the model binds to the
//!    service so every operation lists it — with prose in its `Message` member naming
//!    the member and the value the request held for it, plus the rule that value broke
//!    where it is a bound or a pattern rather than an enumerated set.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 serialization, the refusal envelope and the handlers
//! together — not the handlers' Rust signatures.

use serde_json::{json, Value};

use super::network_and_node_lifecycle::{
    available_network, call, create_node, member_configuration, new_router, Answer,
};

/// A resource id of the shape this service mints, for a resource nothing holds: the
/// prefix, a hyphen and 26 base-32 characters, within `ResourceIdString`'s
/// `@length(1, 32)`. Taking the shape seriously matters — a malformed id would be
/// refused as a constraint violation (rule 2) long before any lookup (rule 1).
fn unknown_id(prefix: &str, filler: char) -> String {
    format!("{prefix}-{}", std::iter::repeat_n(filler, 26).collect::<String>())
}

/// Assert `answer` is the modelled `ResourceNotFoundException` for `resource_type`
/// `id`: HTTP 404, the modelled shape named in both the body and the error-type header,
/// the resource named in `Message`, and NO `ResourceName` member.
fn assert_not_found(case: &str, answer: &Answer, resource_type: &str, id: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.text(&["__type"]),
        "ResourceNotFoundException",
        "case {case}: expected the body to name the modelled shape, got {:?}",
        answer.body
    );
    let message = answer.text(&["Message"]);
    assert!(
        message.starts_with(resource_type) && message.contains(id),
        "case {case}: expected a Message naming {resource_type} {id}, got {message:?}"
    );
    // `ResourceName` is `ArnString`: a bare id cannot satisfy its pattern, and a
    // resource that was not found has no ARN, so the member must be absent entirely.
    assert_eq!(
        answer.body.get("ResourceName"),
        None,
        "case {case}: expected no ResourceName member, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the modelled `InvalidRequestException` refusing `member`: HTTP
/// 400, the modelled shape named in both the body and the error-type header, and a
/// `Message` naming the member and `expected_in_message`.
fn assert_invalid_request(
    case: &str,
    answer: &Answer,
    member: &str,
    expected_in_message: &str,
    rule_in_message: Option<&str>,
) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case {case}: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.text(&["__type"]),
        "InvalidRequestException",
        "case {case}: expected the body to name the modelled shape, got {:?}",
        answer.body
    );
    let message = answer.text(&["Message"]);
    assert!(
        message.contains(member) && message.contains(expected_in_message),
        "case {case}: expected a Message naming {member} and {expected_in_message:?}, \
         got {message:?}"
    );
    // A value that failed a bound or a pattern is refused with the rule it broke: the
    // value set of an enum is in the model and in every generated client, but a bound is
    // not something the caller can read off the value it sent.
    if let Some(rule) = rule_in_message {
        assert!(
            message.contains(rule),
            "case {case}: expected the Message to state the rule {rule:?}, got {message:?}"
        );
    }
}

/// One row of the not-found table: which request, which resource type the answer must
/// name, and which id it must name.
struct NotFoundCase {
    case: &'static str,
    method: &'static str,
    path: String,
    resource_type: &'static str,
    id: String,
}

/// One row of the refusal table: which request, which member the answer must name,
/// what else its prose must carry (the value the request held, or the requirement a
/// member it omitted states), and — for a class that failed a bound or a pattern — the
/// rule the prose must state as well.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    expected_in_message: String,
    rule_in_message: Option<&'static str>,
}

/// A lookup that finds nothing names the resource the operation was asked for, and
/// never puts a non-ARN id in `ResourceName` — over a table spanning the operations
/// that look a resource up, the resource types they name, and whether it is the leaf or
/// an ancestor of the request's path that is missing.
#[tokio::test]
async fn a_lookup_that_finds_nothing_names_the_resource_it_was_asked_for() {
    let router = new_router();
    let (network_id, member_id) = available_network(&router, "errors-1", "errors-net").await;
    let node_id = create_node(
        &router,
        &network_id,
        &member_id,
        "errors-node",
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;

    let gone_network = unknown_id("n", 'A');
    let other_gone_network = unknown_id("n", 'B');
    let gone_member = unknown_id("m", 'C');
    let gone_node = unknown_id("nd", 'D');
    let gone_invitation = unknown_id("i", 'E');
    let gone_proposal = unknown_id("p", 'F');
    let gone_accessor = unknown_id("ac", 'G');

    // Rows differ along the operation, the resource type the answer must name, and
    // WHICH id of the request's path names nothing: the leaf the request targets, or an
    // ancestor that only scopes it. The ancestor rows are the point of the rule — a
    // caller asking for a node is told about the node, whatever else is missing.
    let cases: Vec<NotFoundCase> = vec![
        NotFoundCase {
            case: "GetNetwork/unknown",
            method: "GET",
            path: format!("/networks/{gone_network}"),
            resource_type: "Network",
            id: gone_network.clone(),
        },
        NotFoundCase {
            case: "GetNetwork/another-unknown",
            method: "GET",
            path: format!("/networks/{other_gone_network}"),
            resource_type: "Network",
            id: other_gone_network.clone(),
        },
        NotFoundCase {
            case: "GetMember/unknown-member",
            method: "GET",
            path: format!("/networks/{network_id}/members/{gone_member}"),
            resource_type: "Member",
            id: gone_member.clone(),
        },
        NotFoundCase {
            case: "GetMember/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/members/{member_id}"),
            resource_type: "Member",
            id: member_id.clone(),
        },
        NotFoundCase {
            case: "GetNode/unknown-node",
            method: "GET",
            path: format!("/networks/{network_id}/nodes/{gone_node}?memberId={member_id}"),
            resource_type: "Node",
            id: gone_node.clone(),
        },
        NotFoundCase {
            case: "GetNode/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/nodes/{node_id}?memberId={member_id}"),
            resource_type: "Node",
            id: node_id.clone(),
        },
        NotFoundCase {
            case: "GetNode/other-member",
            method: "GET",
            path: format!("/networks/{network_id}/nodes/{node_id}?memberId={gone_member}"),
            resource_type: "Node",
            id: node_id.clone(),
        },
        NotFoundCase {
            case: "DeleteNode/unknown-node",
            method: "DELETE",
            path: format!("/networks/{network_id}/nodes/{gone_node}?memberId={member_id}"),
            resource_type: "Node",
            id: gone_node.clone(),
        },
        NotFoundCase {
            case: "DeleteNode/unknown-network",
            method: "DELETE",
            path: format!("/networks/{gone_network}/nodes/{node_id}?memberId={member_id}"),
            resource_type: "Node",
            id: node_id.clone(),
        },
        NotFoundCase {
            case: "GetProposal/unknown-proposal",
            method: "GET",
            path: format!("/networks/{network_id}/proposals/{gone_proposal}"),
            resource_type: "Proposal",
            id: gone_proposal.clone(),
        },
        NotFoundCase {
            case: "GetProposal/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/proposals/{gone_proposal}"),
            resource_type: "Proposal",
            id: gone_proposal.clone(),
        },
        // A resource that is not part of the network tree at all, to show the rule is
        // about the exception shape and not about one resource hierarchy: an invitation
        // this account holds none of, and an accessor nothing holds, are named the same
        // way, and their ARNs are derivable from the request yet still not emitted — the
        // member names the ARN OF A RESOURCE, and there is no such resource.
        NotFoundCase {
            case: "RejectInvitation/unknown",
            method: "DELETE",
            path: format!("/invitations/{gone_invitation}"),
            resource_type: "Invitation",
            id: gone_invitation.clone(),
        },
        NotFoundCase {
            case: "GetAccessor/unknown",
            method: "GET",
            path: format!("/accessors/{gone_accessor}"),
            resource_type: "Accessor",
            id: gone_accessor.clone(),
        },
        NotFoundCase {
            case: "DeleteAccessor/unknown",
            method: "DELETE",
            path: format!("/accessors/{gone_accessor}"),
            resource_type: "Accessor",
            id: gone_accessor.clone(),
        },
        // A LISTING that models the exception answers it the same way: the scope it
        // could not resolve is named in `Message`, with no `ResourceName`.
        NotFoundCase {
            case: "ListProposals/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/proposals"),
            resource_type: "Network",
            id: gone_network.clone(),
        },
        NotFoundCase {
            case: "ListProposals/another-unknown-network",
            method: "GET",
            path: format!("/networks/{other_gone_network}/proposals?maxResults=5"),
            resource_type: "Network",
            id: other_gone_network.clone(),
        },
        NotFoundCase {
            case: "ListMembers/unknown-network",
            method: "GET",
            path: format!("/networks/{gone_network}/members"),
            resource_type: "Network",
            id: gone_network.clone(),
        },
    ];

    for row in cases {
        let answer = call(&router, row.method, &row.path, None).await;
        assert_not_found(row.case, &answer, row.resource_type, &row.id);
    }

    // The rows that carry a request BODY, so the table above can stay bodyless. An
    // `UpdateNode` of a node nothing holds names the node, exactly as the reads do — the
    // rule is about which resource the request identifies, not about the method.
    for (case, path, id) in [
        (
            "UpdateNode/unknown-node",
            format!("/networks/{network_id}/nodes/{gone_node}"),
            gone_node.clone(),
        ),
        (
            "UpdateNode/unknown-network",
            format!("/networks/{gone_network}/nodes/{node_id}"),
            node_id.clone(),
        ),
    ] {
        let answer = call(&router, "PATCH", &path, Some(json!({}))).await;
        assert_not_found(case, &answer, "Node", &id);
    }

    // A create does not act on the resource it is about to mint — that one does not
    // exist yet — so what a `CreateNode` can fail to find is the network it creates the
    // node in, or the member that will own it. Both still carry no `ResourceName`.
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{gone_network}/nodes"),
        Some(json!({
            "ClientRequestToken": "errors-create-unknown-network",
            "MemberId": member_id,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" }
        })),
    )
    .await;
    assert_not_found(
        "CreateNode/unknown-network",
        &answer,
        "Network",
        &gone_network,
    );

    let answer = call(
        &router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(json!({
            "ClientRequestToken": "errors-create-unknown-member",
            "MemberId": gone_member,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" }
        })),
    )
    .await;
    assert_not_found("CreateNode/unknown-member", &answer, "Member", &gone_member);

    // `CreateMember` resolves TWO things the request names by id — the network it joins
    // and the invitation that authorises the join — and each one it cannot find names
    // itself, never the other. The invitation rows matter most: the network in the
    // request path exists, so a `ResourceName` defaulted to the path resource would tell
    // a client a healthy network is the missing resource.
    for (case, network, invitation, resource_type, id) in [
        (
            "CreateMember/unknown-invitation",
            network_id.clone(),
            gone_invitation.clone(),
            "Invitation",
            gone_invitation.clone(),
        ),
        (
            "CreateMember/another-unknown-invitation",
            network_id.clone(),
            unknown_id("i", 'H'),
            "Invitation",
            unknown_id("i", 'H'),
        ),
        (
            "CreateMember/unknown-network",
            gone_network.clone(),
            gone_invitation.clone(),
            "Network",
            gone_network.clone(),
        ),
    ] {
        let answer = call(
            &router,
            "POST",
            &format!("/networks/{network}/members"),
            Some(json!({
                "ClientRequestToken": format!("errors-{case}"),
                "InvitationId": invitation,
                "MemberConfiguration": member_configuration("errors-org"),
            })),
        )
        .await;
        assert_not_found(case, &answer, resource_type, &id);
    }
}

/// A request outside a constraint the model declares is refused with the modelled
/// `InvalidRequestException` and a `Message` built from the request — over a table
/// spanning the constraint CLASSES the model expresses (an enumerated value set, a
/// string length, a numeric range, a pattern, and a required member's absence), on both
/// implemented and still-scaffolded operations and on query, label and body bindings.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_the_modelled_invalid_request() {
    let router = new_router();
    let (network_id, _member_id) = available_network(&router, "refusals-1", "refusals-net").await;

    // A `ResourceIdString` is `@length(1, 32)`; 33 characters is one past the bound.
    let too_long_id = format!("n-{}", "A".repeat(31));

    let cases: Vec<RefusalCase> = vec![
        // `ListMembers$Status` targets the `MemberStatus` enum, bound as httpQuery
        // `status` (smithy_query: operation ListMembers).
        RefusalCase {
            case: "ListMembers/status-outside-the-enum",
            method: "GET",
            path: format!("/networks/{network_id}/members?status=NOT_A_REAL_STATUS"),
            body: None,
            member: "Status",
            expected_in_message: "NOT_A_REAL_STATUS".to_string(),
            rule_in_message: None,
        },
        RefusalCase {
            case: "ListMembers/status-wrong-case",
            method: "GET",
            path: format!("/networks/{network_id}/members?status=available"),
            body: None,
            member: "Status",
            expected_in_message: "available".to_string(),
            rule_in_message: None,
        },
        // `@length` on a label-bound member, one character past the maximum.
        RefusalCase {
            case: "GetNetwork/network-id-too-long",
            method: "GET",
            path: format!("/networks/{too_long_id}"),
            body: None,
            member: "NetworkId",
            expected_in_message: too_long_id.clone(),
            rule_in_message: Some("length less than or equal to 32"),
        },
        // `@range` on a query-bound member: `ListNodes$MaxResults` is 1..20 and
        // `ListNetworks$MaxResults` is 1..10, so each bound is tested one past it.
        RefusalCase {
            case: "ListNodes/max-results-below-the-range",
            method: "GET",
            path: format!("/networks/{network_id}/nodes?maxResults=0"),
            body: None,
            member: "MaxResults",
            expected_in_message: "0".to_string(),
            rule_in_message: Some("greater than or equal to 1"),
        },
        RefusalCase {
            case: "ListNodes/max-results-above-the-range",
            method: "GET",
            path: format!("/networks/{network_id}/nodes?maxResults=21"),
            body: None,
            member: "MaxResults",
            expected_in_message: "21".to_string(),
            rule_in_message: Some("less than or equal to 20"),
        },
        RefusalCase {
            case: "ListNetworks/max-results-above-the-range",
            method: "GET",
            path: "/networks?maxResults=11".to_string(),
            body: None,
            member: "MaxResults",
            expected_in_message: "11".to_string(),
            rule_in_message: Some("less than or equal to 10"),
        },
        // `@pattern`: `ResourceArn` is `ArnString`, `^arn:.+:.+:.+:.+:.+$`.
        RefusalCase {
            case: "TagResource/resource-arn-not-an-arn",
            method: "POST",
            path: "/tags/not-an-arn".to_string(),
            body: Some(json!({ "Tags": { "k": "v" } })),
            member: "ResourceArn",
            expected_in_message: "not-an-arn".to_string(),
            rule_in_message: Some("^arn:.+:.+:.+:.+:.+$"),
        },
        // A `@required` body member the request omits: absence is found while READING
        // the request, and to a caller it is the same refusal.
        RefusalCase {
            case: "CreateNetwork/name-absent",
            method: "POST",
            path: "/networks".to_string(),
            body: Some(json!({
                "ClientRequestToken": "refusals-no-name",
                "Framework": "HYPERLEDGER_FABRIC",
                "FrameworkVersion": "2.2",
                "VotingPolicy": {
                    "ApprovalThresholdPolicy": {
                        "ThresholdPercentage": 50,
                        "ProposalDurationInHours": 24,
                        "ThresholdComparator": "GREATER_THAN"
                    }
                },
                "MemberConfiguration": {
                    "Name": "org1",
                    "FrameworkConfiguration": {
                        "Fabric": { "AdminUsername": "AdminUser", "AdminPassword": "MyPassword1" }
                    }
                }
            })),
            member: "Name",
            expected_in_message: "is required".to_string(),
            rule_in_message: None,
        },
    ];

    for row in cases {
        let answer = call(&router, row.method, &row.path, row.body).await;
        assert_invalid_request(
            row.case,
            &answer,
            row.member,
            &row.expected_in_message,
            row.rule_in_message,
        );
    }
}

/// Every value the modelled enum declares is ACCEPTED, so the refusal above is about
/// the value set and not about the member being present.
///
/// The catalog is `com.amazonaws.managedblockchain#MemberStatus`'s own value list
/// (`smithy_query(query="shape", shape="com.amazonaws.managedblockchain#MemberStatus")`),
/// which includes values the reported request never named. What is asserted is only that
/// the request is NOT refused as invalid — true while `ListMembers` answered 501 as a
/// scaffold, and true now that it answers the listing, because the refusal is decided
/// before any handler runs.
#[tokio::test]
async fn every_value_in_the_modelled_enum_is_accepted() {
    let router = new_router();
    let (network_id, _member_id) = available_network(&router, "enum-1", "enum-net").await;

    let statuses = [
        "CREATING",
        "AVAILABLE",
        "CREATE_FAILED",
        "UPDATING",
        "DELETING",
        "DELETED",
        "INACCESSIBLE_ENCRYPTION_KEY",
    ];

    for status in statuses {
        let answer = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members?status={status}"),
            None,
        )
        .await;
        assert_ne!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case ListMembers/status={status}: a value the enum declares must not be \
             refused, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
