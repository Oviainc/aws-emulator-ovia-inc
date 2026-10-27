// SPDX-License-Identifier: CC0-1.0

//! One ARN resolves to ONE stored record, for the read and for the write alike.
//!
//! RULE: a tagging request's `ResourceArn` is resolved by one walk of the store — the
//! stored ARN of each record that carries a tag set, matched in one precedence — so the
//! record a write lands in is the record a read reports and the record a refusal keys its
//! status on.
//!
//! The tables below span what that rule generalises over: EVERY resource type this service
//! tags (the five shapes whose model declares a `Tags` member — `Accessor`, `Network`,
//! `Member`, `Node`, `Proposal`), each resolved while all five are in the store at once, so
//! a resolution that reached a different record than another would show as one kind's tag
//! appearing on another's; and each kind in whichever transient status its own lifecycle
//! gives it, where the status a refusal reports and the tag set the refused write would have
//! reached have to be the same record's.
//!
//! The request harness (`new_router`, `call`, the create bodies) and the three tagging calls
//! are the ones the sibling suites already define.

use std::collections::BTreeMap;

use serde_json::json;

use crate::harness::{
    available_network, call, create_network, create_network_body, new_router, PAST_TRANSITION,
};
use crate::resource_tags::{
    arn, list_tags, tag_map, tag_resource, taggable_resources, tags, untag_resource, BILLING_TOKEN,
};

/// The tag key each case writes, whose VALUE names the record the write was addressed to.
const OWNER_KEY: &str = "owner";

/// The tag set a record holds once the write addressed to it has landed and nothing else
/// has: `owner` -> the case that addressed it.
fn owned_by(case: &str) -> BTreeMap<String, String> {
    [(OWNER_KEY.to_string(), case.to_string())]
        .into_iter()
        .collect()
}

/// The rule over every resource type this service tags, with one resource of each in the
/// store at once: the write `TagResource` makes lands in exactly the record
/// `ListTagsForResource` and the resource's own read report, and in no other record —
/// `UntagResource` then removes from that same one.
#[tokio::test]
async fn a_tagging_write_lands_in_the_record_the_read_resolves_for_every_taggable_kind() {
    let router = new_router();

    // One settled resource of every taggable kind, each created holding NO tags, so the
    // only tag any of them carries afterwards is the one a write addressed to it put there.
    let resources = taggable_resources(&router, &tags(0)).await;

    // Each write names its own case in the tag's value.
    for resource in &resources {
        let case = resource.case;
        let written = tag_resource(
            &router,
            &resource.resource_arn,
            json!({ OWNER_KEY: case }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected TagResource 200, got {} {:?}",
            written.status, written.body
        );
    }

    // Every write landed in the record its ARN named: the read resolution reports that
    // case's own value, the resource's own read projects the same record, and no other
    // kind's record was reached.
    for resource in &resources {
        let case = resource.case;
        let expected = owned_by(case);
        let listed = list_tags(&router, &resource.resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {:?}",
            listed.body
        );
        let own = call(&router, "GET", &resource.read_path, None).await;
        assert_eq!(
            own.status, 200,
            "case {case:?}: expected the resource's own read to answer 200, got {} {:?}",
            own.status, own.body
        );
        assert_eq!(
            tag_map(&own.body[resource.shape]["Tags"]),
            expected,
            "case {case:?}: expected the resource's own read to report {expected:?}, got {:?}",
            own.body
        );
    }

    // The removal resolves the same record the write did: each kind loses only its own tag.
    for (removed, resource) in resources.iter().enumerate() {
        let case = resource.case;
        let answer = untag_resource(&router, &resource.resource_arn, &[OWNER_KEY.to_string()]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UntagResource 200, got {} {:?}",
            answer.status, answer.body
        );
        for (index, other) in resources.iter().enumerate() {
            let expected = if index <= removed {
                BTreeMap::new()
            } else {
                owned_by(other.case)
            };
            let listed = list_tags(&router, &other.resource_arn).await;
            assert_eq!(
                tag_map(&listed.body["Tags"]),
                expected,
                "case {case:?}: after removing from {case:?}, expected {:?} to report \
                 {expected:?}, got {:?}",
                other.case,
                listed.body
            );
        }
    }
}

/// The same rule where the two resolutions could drift apart: a resource in a transient
/// status is refused, and the status the refusal reports is the status the SAME record's own
/// read reports, while the tag set that record holds is left as it was; once the clock
/// settles it, the write lands in that record and the read reports it.
#[tokio::test]
async fn the_refusal_status_and_the_refused_write_name_the_same_record() {
    let router = new_router();

    // A settled network to create the node on: `CreateNode` requires an `AVAILABLE`
    // network. Its own records are not rows here.
    let (host_network, host_member) = available_network(&router, "resolve-host", "resolvehost").await;

    // Rows: the case, the ARN, the resource's own read, that read's body member. Each row
    // is created just before the assertions, so every one is still inside the window its
    // create opened — `CREATING` for the network, its founding member and the node, and
    // `PENDING_DELETION` for the accessor, whose deletion has already begun.
    let mut rows: Vec<(&'static str, String, String, &'static str)> = Vec::new();

    let mut body = create_network_body("resolve-network", "resolvenet");
    body["Tags"] = json!({ OWNER_KEY: "created" });
    body["MemberConfiguration"]["Tags"] = json!({ OWNER_KEY: "created" });
    let (network_id, member_id) = create_network(&router, body).await;
    rows.push((
        "network",
        arn("networks", &network_id),
        format!("/networks/{network_id}"),
        "Network",
    ));
    rows.push((
        "member",
        arn("members", &member_id),
        format!("/networks/{network_id}/members/{member_id}"),
        "Member",
    ));

    let answer = call(
        &router,
        "POST",
        &format!("/networks/{host_network}/nodes"),
        Some(json!({
            "ClientRequestToken": "resolve-node",
            "MemberId": host_member,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" },
            "Tags": { OWNER_KEY: "created" },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"node\": expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);
    rows.push((
        "node",
        arn("nodes", &node_id),
        format!("/networks/{host_network}/nodes/{node_id}?memberId={host_member}"),
        "Node",
    ));

    let answer = call(
        &router,
        "POST",
        "/accessors",
        Some(json!({
            "ClientRequestToken": "resolve-accessor",
            "AccessorType": BILLING_TOKEN,
            "Tags": { OWNER_KEY: "created" },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"accessor\": expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    let accessor_id = answer.text(&["AccessorId"]);
    let deleted = call(&router, "DELETE", &format!("/accessors/{accessor_id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case \"accessor\": expected DeleteAccessor 200, got {} {:?}",
        deleted.status, deleted.body
    );
    rows.push((
        "accessor",
        arn("accessors", &accessor_id),
        format!("/accessors/{accessor_id}"),
        "Accessor",
    ));

    let created = owned_by("created");
    for (case, resource_arn, read_path, shape) in &rows {
        let own = call(&router, "GET", read_path, None).await;
        assert_eq!(
            own.status, 200,
            "case {case:?}: expected the resource's own read to answer 200, got {} {:?}",
            own.status, own.body
        );
        let status = own.text(&[shape, "Status"]);

        let refused = tag_resource(&router, resource_arn, json!({ OWNER_KEY: "written" })).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409 while the resource is {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ResourceNotReadyException",
            "case {case:?}: expected ResourceNotReadyException, got {:?} {:?}",
            refused.error_type, refused.body
        );
        // The refusal keys on the record the write was addressed to: it reports that
        // record's ARN and the status that record's own read reports.
        let message = refused.text(&["Message"]);
        assert!(
            message.contains(resource_arn.as_str()) && message.contains(status.as_str()),
            "case {case:?}: expected the refusal to name {resource_arn} and its status \
             {status}, got {message:?}"
        );
        // The removal addressed to the same record is refused on the same status.
        let refused = untag_resource(&router, resource_arn, &[OWNER_KEY.to_string()]).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected UntagResource 409 while the resource is {status}, got {} {:?}",
            refused.status, refused.body
        );

        // Neither refused write reached the record's tag set: read AFTER both of them, so a
        // write that had landed would show here.
        let after = call(&router, "GET", read_path, None).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected the read after the refusals to answer 200, got {} {:?}",
            after.status, after.body
        );
        assert_eq!(
            tag_map(&after.body[shape]["Tags"]),
            created,
            "case {case:?}: expected the record to still hold {created:?} after the refusals, \
             got {:?}",
            after.body
        );
    }

    // Past every transition the clock owed: the same ARNs now resolve to records that can
    // complete the operation, and the write lands in the record both reads report.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (case, resource_arn, read_path, shape) in &rows {
        // Still only what its create stored: the refused writes above left the record as it
        // was, which the write below then changes.
        let before = call(&router, "GET", read_path, None).await;
        assert_eq!(
            tag_map(&before.body[shape]["Tags"]),
            created,
            "case {case:?}: expected the settled record to still hold {created:?}, got {:?}",
            before.body
        );

        let written = tag_resource(&router, resource_arn, json!({ OWNER_KEY: case })).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected TagResource 200 once the record settled, got {} {:?}",
            written.status, written.body
        );
        let expected = owned_by(case);
        let listed = list_tags(&router, resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {:?}",
            listed.body
        );
        let own = call(&router, "GET", read_path, None).await;
        assert_eq!(
            tag_map(&own.body[shape]["Tags"]),
            expected,
            "case {case:?}: expected the resource's own read to report {expected:?}, got {:?}",
            own.body
        );
    }
}
