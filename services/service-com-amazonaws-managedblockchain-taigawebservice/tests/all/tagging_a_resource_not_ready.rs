// SPDX-License-Identifier: CC0-1.0

//! Tagging a resource that is not in a status that can complete the operation, end to end
//! over HTTP.
//!
//! RULE: `TagResource`, `UntagResource` and `ListTagsForResource` are served only while the
//! resource their `ResourceArn` names is SETTLED. A resource in a transient status — one
//! the service itself moves it out of, a creation still running or a deletion already begun
//! — exists but cannot complete the operation, so each of the three is refused with the
//! modelled `ResourceNotReadyException` (HTTP 409) and the stored tag set is left exactly as
//! it was; once the resource settles, the same three calls are served.
//!
//! The tables span what that rule generalises over: every value `MemberStatus` declares (so
//! each operation is judged against a transient and a settled status of the same record),
//! and every resource type this service tags, each in whichever transient status its own
//! lifecycle puts it in — `CREATING` for a network, member and node, `PENDING_DELETION` for
//! an accessor, and a proposal, whose `ProposalStatus` declares no transient value at all.
//!
//! The request harness (`new_router`, `call`, the create bodies) and the three tagging
//! calls are the ones the sibling suites already define.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_network, create_network_body, new_router, router_and_engine, Answer, ACCOUNT,
    PAST_TRANSITION,
};
use crate::list_members::{seed_members, Seed};
use crate::resource_tags::{arn, list_tags, tag_map, tag_resource, untag_resource, BILLING_TOKEN};

/// The tag key every resource in these cases is created with, so a refused `UntagResource`
/// has something real to fail to remove.
const CREATED_KEY: &str = "created";

/// The tag key the `TagResource` row writes, so a refused write is visible by its absence.
const WRITTEN_KEY: &str = "written";

/// The `InstanceType` the node row is created with, as the other suites use.
const INSTANCE_TYPE: &str = "bc.t3.small";

/// The tag set every resource in these cases holds when it is created.
fn created_tags() -> Value {
    json!({ CREATED_KEY: "tag" })
}

/// The tag set a resource holds while no tagging write has been served against it.
fn created_tag_map() -> BTreeMap<String, String> {
    [(CREATED_KEY.to_string(), "tag".to_string())]
        .into_iter()
        .collect()
}

/// The tag set a resource holds once the write and the removal below have BOTH been served:
/// the created key removed, the written one added.
fn rewritten_tag_map() -> BTreeMap<String, String> {
    [(WRITTEN_KEY.to_string(), "v".to_string())]
        .into_iter()
        .collect()
}

/// The three tagging operations against one `ResourceArn`, in the order that leaves a
/// served resource holding [`rewritten_tag_map`]: the read, then the write, then the
/// removal of the key its create stored.
async fn tagging_calls(
    router: &MultiServiceRouter,
    resource_arn: &str,
) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "ListTagsForResource",
            list_tags(router, resource_arn).await,
        ),
        (
            "TagResource",
            tag_resource(router, resource_arn, json!({ WRITTEN_KEY: "v" })).await,
        ),
        (
            "UntagResource",
            untag_resource(router, resource_arn, &[CREATED_KEY.to_string()]).await,
        ),
    ]
}

/// The `Tags` the resource's OWN read reports, so the tag set can be checked even while the
/// tagging operations are refusing to report it.
async fn tags_of_read(
    router: &MultiServiceRouter,
    case: &str,
    read_path: &str,
    shape: &str,
) -> BTreeMap<String, String> {
    let answer = call(router, "GET", read_path, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the resource's own read to answer 200, got {} {:?}",
        answer.status, answer.body
    );
    tag_map(&answer.body[shape]["Tags"])
}

/// Assert one answer is the modelled not-ready refusal.
fn assert_not_ready(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}/{operation}: expected 409 while the resource is not ready, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "ResourceNotReadyException",
        "case {case:?}/{operation}: expected ResourceNotReadyException, got {:?} {:?}",
        answer.error_type, answer.body
    );
    assert!(
        answer.body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}/{operation}: expected the refusal to say what was wrong, got {:?}",
        answer.body
    );
}

/// Assert one answer is served.
fn assert_served(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{operation}: expected 200 for a settled resource, got {} {:?}",
        answer.status, answer.body
    );
}

/// The rule over every value `MemberStatus` declares: a member in a transient status
/// refuses all three tagging operations with `ResourceNotReadyException` and keeps the tag
/// set its create stored, while a member settled in any status serves them.
#[tokio::test]
async fn tagging_a_member_is_refused_only_while_its_status_is_transient() {
    // Rows: every value of the status enum (smithy_query: shape MemberStatus = CREATING |
    // AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED |
    // INACCESSIBLE_ENCRYPTION_KEY). `CREATING` is a creation still running and `DELETING` a
    // removal already under way — the two transient ones; every other value is one the
    // record is settled in, whether in service (`AVAILABLE`, `UPDATING`) or ended
    // (`CREATE_FAILED`, `DELETED`, `INACCESSIBLE_ENCRYPTION_KEY`).
    let cases: &[(&str, bool)] = &[
        ("CREATING", true),
        ("AVAILABLE", false),
        ("CREATE_FAILED", false),
        ("UPDATING", false),
        ("DELETING", true),
        ("DELETED", false),
        ("INACCESSIBLE_ENCRYPTION_KEY", false),
    ];

    for &(status, transient) in cases {
        let (router, engine) = router_and_engine();
        let mut body = create_network_body(&format!("tag-status-{status}"), "tagstatusnet");
        body["MemberConfiguration"]["Tags"] = created_tags();
        let (network_id, _founder) = create_network(&router, body).await;
        // Past the create's transition, so the row's own seeded status is the only
        // transient thing in the store.
        tokio::time::sleep(PAST_TRANSITION).await;

        // Each seeded member is a copy of the one `CreateNetwork` stored — the created tag
        // set included — with only its status changed.
        let member_id = seed_members(
            &engine,
            &network_id,
            &[Seed {
                name: "tagstatusorg",
                status,
                account: ACCOUNT,
            }],
        )
        .remove(0);
        let resource_arn = arn("members", &member_id);
        let read_path = format!("/networks/{network_id}/members/{member_id}");

        for (operation, answer) in tagging_calls(&router, &resource_arn).await {
            if transient {
                assert_not_ready(status, operation, &answer);
            } else {
                assert_served(status, operation, &answer);
            }
        }

        let expected = if transient {
            created_tag_map()
        } else {
            rewritten_tag_map()
        };
        assert_eq!(
            tags_of_read(&router, status, &read_path, "Member").await,
            expected,
            "case {status:?}: expected the member's tag set to be {expected:?}"
        );
    }
}

/// The same rule over EVERY resource type this service tags, each put in whichever
/// transient status its own lifecycle has: the three tagging operations are refused while it
/// is transient and leave its tag set alone, and are served once the clock has settled it.
#[tokio::test]
async fn every_taggable_resource_type_refuses_tagging_until_it_settles() {
    let router = new_router();

    // One settled network to create the node and the proposal on: both creates require an
    // `AVAILABLE` network.
    let (host_network, host_member) = create_network(
        &router,
        create_network_body("settle-host", "settlehostnet"),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Each row: the case name, the ARN, the read that reports its `Tags`, that read's body
    // member, and whether the action just taken left the resource in a transient status.
    // The rows are driven one at a time, so each resource's three tagging calls happen
    // inside the window the action that put it there opened.
    let mut rows: Vec<(&'static str, String, String, &'static str, bool)> = Vec::new();

    // A network and its founding member are both `CREATING` the moment `CreateNetwork`
    // answers.
    let mut body = create_network_body("settle-network", "settlenet");
    body["Tags"] = created_tags();
    body["MemberConfiguration"]["Tags"] = created_tags();
    let (network_id, member_id) = create_network(&router, body).await;
    rows.push((
        "network CREATING",
        arn("networks", &network_id),
        format!("/networks/{network_id}"),
        "Network",
        true,
    ));
    rows.push((
        "member CREATING",
        arn("members", &member_id),
        format!("/networks/{network_id}/members/{member_id}"),
        "Member",
        true,
    ));

    // A node is `CREATING` the moment `CreateNode` answers.
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{host_network}/nodes"),
        Some(json!({
            "ClientRequestToken": "settle-node",
            "MemberId": host_member,
            "NodeConfiguration": { "InstanceType": INSTANCE_TYPE },
            "Tags": created_tags(),
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected CreateNode 200, got {} {:?}",
        "node CREATING", answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);
    rows.push((
        "node CREATING",
        arn("nodes", &node_id),
        format!("/networks/{host_network}/nodes/{node_id}?memberId={host_member}"),
        "Node",
        true,
    ));

    // A proposal is in service from the moment it is minted: `ProposalStatus` has no
    // transient value, so this row is served straight away.
    let answer = call(
        &router,
        "POST",
        &format!("/networks/{host_network}/proposals"),
        Some(json!({
            "ClientRequestToken": "settle-proposal",
            "MemberId": host_member,
            "Actions": { "Invitations": [{ "Principal": "100000000001" }] },
            "Tags": created_tags(),
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected CreateProposal 200, got {} {:?}",
        "proposal IN_PROGRESS", answer.status, answer.body
    );
    let proposal_id = answer.text(&["ProposalId"]);
    rows.push((
        "proposal IN_PROGRESS",
        arn("proposals", &proposal_id),
        format!("/networks/{host_network}/proposals/{proposal_id}"),
        "Proposal",
        false,
    ));

    // An accessor's transient status is the other kind: a deletion already under way
    // (`DeleteAccessor` "changes the status of the accessor … from AVAILABLE to
    // PENDING_DELETION").
    let answer = call(
        &router,
        "POST",
        "/accessors",
        Some(json!({
            "ClientRequestToken": "settle-accessor",
            "AccessorType": BILLING_TOKEN,
            "Tags": created_tags(),
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected CreateAccessor 200, got {} {:?}",
        "accessor PENDING_DELETION", answer.status, answer.body
    );
    let accessor_id = answer.text(&["AccessorId"]);
    let deleted = call(
        &router,
        "DELETE",
        &format!("/accessors/{accessor_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {:?}: expected DeleteAccessor 200, got {} {:?}",
        "accessor PENDING_DELETION", deleted.status, deleted.body
    );
    rows.push((
        "accessor PENDING_DELETION",
        arn("accessors", &accessor_id),
        format!("/accessors/{accessor_id}"),
        "Accessor",
        true,
    ));

    // While each row is in the status its action left it in.
    for (case, resource_arn, read_path, shape, transient) in &rows {
        for (operation, answer) in tagging_calls(&router, resource_arn).await {
            if *transient {
                assert_not_ready(case, operation, &answer);
            } else {
                assert_served(case, operation, &answer);
            }
        }
        let expected = if *transient {
            created_tag_map()
        } else {
            rewritten_tag_map()
        };
        assert_eq!(
            tags_of_read(&router, case, read_path, shape).await,
            expected,
            "case {case:?}: expected the tag set to be {expected:?} after the three calls"
        );
    }

    // Past every transition the clock owed: a network, member and node have settled to
    // `AVAILABLE` and the accessor to `DELETED`, so all three operations are served on
    // every row — and the rows that were refused still hold the tag set their create
    // stored, which the served write and removal now change.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (case, resource_arn, read_path, shape, transient) in &rows {
        let before = tags_of_read(&router, case, read_path, shape).await;
        let expected_before = if *transient {
            created_tag_map()
        } else {
            rewritten_tag_map()
        };
        assert_eq!(
            before, expected_before,
            "case {case:?}: expected the settled resource to still hold {expected_before:?}"
        );

        for (operation, answer) in tagging_calls(&router, resource_arn).await {
            assert_served(case, operation, &answer);
        }
        assert_eq!(
            tags_of_read(&router, case, read_path, shape).await,
            rewritten_tag_map(),
            "case {case:?}: expected the served write and removal to leave only the written tag"
        );
        let listed = list_tags(&router, resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            rewritten_tag_map(),
            "case {case:?}: expected ListTagsForResource to report the same tag set as the \
             resource's own read, got {:?}",
            listed.body
        );
    }
}
