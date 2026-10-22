// SPDX-License-Identifier: CC0-1.0

//! The Managed Blockchain tagging surface, end to end over HTTP: `ListTagsForResource`
//! reports the tag set held by the resource its `ResourceArn` names, and `TagResource`
//! and `UntagResource` are the two writes to that one set.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization — including the percent-encoded
//! `@httpLabel` an ARN travels in — the handlers and the shared engine together, not the
//! handlers' Rust signatures. The request harness (`new_router`, `call`, the network
//! body) is the one the network/node suite beside this file already defines.
//!
//! The tables below span the classes the rules generalise over: EVERY resource type this
//! service tags (the five shapes whose model declares a `Tags` member — `Network`,
//! `Member`, `Node`, `Proposal`, `Accessor`), tag counts from none to the documented
//! maximum of 50 and one past it, and each way a `ResourceArn` can name no tag set of
//! this service.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::network_and_node_lifecycle::{
    call, create_network_body, new_router, Answer, ACCOUNT, PAST_TRANSITION, REGION,
};

/// The documented per-resource maximum: "A resource can have up to 50 tags. If you try to
/// create more than 50 tags for a resource, your request fails and returns an error"
/// (`TagResource`), which is also `InputTagMap`'s `@length(0, 50)`.
const MAX_TAGS: usize = 50;

/// The only `AccessorType` the model declares (smithy_query: shape AccessorType =
/// BILLING_TOKEN).
const BILLING_TOKEN: &str = "BILLING_TOKEN";

/// `count` distinct tags, `key-<i>` -> `value-<i>`.
fn tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|i| (format!("key-{i}"), json!(format!("value-{i}"))))
            .collect(),
    )
}

/// The same `count` tags as a map, to compare a listing against.
fn expected_tags(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|i| (format!("key-{i}"), format!("value-{i}")))
        .collect()
}

/// A JSON tag map as a sorted map, so two answers compare regardless of member order.
fn tag_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN of one of this service's resources, as the create that minted it stored it:
/// `arn:aws:managedblockchain:<region>:<account>:<type>/<id>`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// A `ResourceArn` as a non-greedy `@httpLabel` carries it: every reserved character
/// percent-encoded, which is how an SDK sends one.
fn label(resource_arn: &str) -> String {
    resource_arn.replace(':', "%3A").replace('/', "%2F")
}

/// `ListTagsForResource` (`GET /tags/{ResourceArn}`).
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `TagResource` (`POST /tags/{ResourceArn}`).
async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `UntagResource` (`DELETE /tags/{ResourceArn}?tagKeys=…`, one query value per key).
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[String],
) -> Answer {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={key}")).collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// One taggable resource of this service: how a case names it, the ARN that identifies
/// it, and the route (plus body member) its own read answers its `Tags` on.
#[derive(Debug)]
struct Resource {
    case: &'static str,
    resource_arn: String,
    read_path: String,
    shape: &'static str,
}

/// The `Tags` the resource's OWN read reports, so a listing can be checked against the
/// record every other reader projects.
async fn tags_of_read(router: &MultiServiceRouter, resource: &Resource) -> BTreeMap<String, String> {
    let answer = call(router, "GET", &resource.read_path, None).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected the resource's own read to answer 200, got {} {:?}",
        resource.case, answer.status, answer.body
    );
    tag_map(&answer.body[resource.shape]["Tags"])
}

/// Create one resource of EVERY type this service tags, each with `create_tags`.
///
/// The network's first member comes from `CreateNetwork`'s `MemberConfiguration`, which
/// carries its own `Tags`; the node and the proposal are created on that network once it
/// has settled to `AVAILABLE`.
async fn taggable_resources(router: &MultiServiceRouter, create_tags: &Value) -> Vec<Resource> {
    let answer = call(
        router,
        "POST",
        "/accessors",
        Some(json!({
            "ClientRequestToken": "tagging-accessor",
            "AccessorType": BILLING_TOKEN,
            "Tags": create_tags,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    let accessor_id = answer.text(&["AccessorId"]);

    let mut body = create_network_body("tagging-network", "tagnet");
    body["Tags"] = create_tags.clone();
    body["MemberConfiguration"]["Tags"] = create_tags.clone();
    let answer = call(router, "POST", "/networks", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "expected CreateNetwork 200, got {} {:?}",
        answer.status, answer.body
    );
    let network_id = answer.text(&["NetworkId"]);
    let member_id = answer.text(&["MemberId"]);

    // A node and a proposal can only be created on an `AVAILABLE` network.
    tokio::time::sleep(PAST_TRANSITION).await;

    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(json!({
            "ClientRequestToken": "tagging-node",
            "MemberId": member_id,
            "NodeConfiguration": { "InstanceType": "bc.t3.small" },
            "Tags": create_tags,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);

    let answer = call(
        router,
        "POST",
        &format!("/networks/{network_id}/proposals"),
        Some(json!({
            "ClientRequestToken": "tagging-proposal",
            "MemberId": member_id,
            "Actions": { "Invitations": [{ "Principal": "100000000001" }] },
            "Tags": create_tags,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateProposal 200, got {} {:?}",
        answer.status, answer.body
    );
    let proposal_id = answer.text(&["ProposalId"]);

    vec![
        Resource {
            case: "accessor",
            resource_arn: arn("accessors", &accessor_id),
            read_path: format!("/accessors/{accessor_id}"),
            shape: "Accessor",
        },
        Resource {
            case: "network",
            resource_arn: arn("networks", &network_id),
            read_path: format!("/networks/{network_id}"),
            shape: "Network",
        },
        Resource {
            case: "member",
            resource_arn: arn("members", &member_id),
            read_path: format!("/networks/{network_id}/members/{member_id}"),
            shape: "Member",
        },
        Resource {
            case: "node",
            resource_arn: arn("nodes", &node_id),
            read_path: format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
            shape: "Node",
        },
        Resource {
            case: "proposal",
            resource_arn: arn("proposals", &proposal_id),
            read_path: format!("/networks/{network_id}/proposals/{proposal_id}"),
            shape: "Proposal",
        },
    ]
}

/// The rule, over every resource type this service tags: `ListTagsForResource` reports
/// the tag set the named resource holds — what its create stored, as `TagResource` added
/// to and overwrote it and `UntagResource` removed from it — and that set is the one the
/// resource's own read projects.
#[tokio::test]
async fn tags_round_trip_through_every_taggable_resource_type() {
    let router = new_router();

    // Two tags at create time, so a later write has both a key to overwrite and a key to
    // leave alone.
    let created = 2;
    let resources = taggable_resources(&router, &tags(created)).await;

    for resource in &resources {
        let case = resource.case;
        let resource_arn = resource.resource_arn.as_str();

        // What the create stored is what the listing reports.
        let listed = list_tags(&router, resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {} {:?}",
            listed.status, listed.body
        );
        let mut expected = expected_tags(created);
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected the created tags back, got {:?}",
            listed.body
        );
        assert_eq!(
            tags_of_read(&router, resource).await,
            expected,
            "case {case:?}: expected the resource's own read to report the same tag set as \
             ListTagsForResource"
        );

        // A read changes nothing a later caller sees: the second listing is the first.
        let again = list_tags(&router, resource_arn).await;
        assert_eq!(
            again.body, listed.body,
            "case {case:?}: expected ListTagsForResource to answer identically twice, got {:?} \
             then {:?}",
            listed.body, again.body
        );

        // "Adds or overwrites the specified tags": a fresh key is added and a key that
        // already exists takes the new value, while the untouched key stays as it was.
        let written = tag_resource(
            &router,
            resource_arn,
            json!({ "key-0": "overwritten", "added": "new" }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected TagResource 200, got {} {:?}",
            written.status, written.body
        );
        expected.insert("key-0".to_string(), "overwritten".to_string());
        expected.insert("added".to_string(), "new".to_string());
        let listed = list_tags(&router, resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected the overwritten and added tags, got {:?}",
            listed.body
        );
        assert_eq!(
            tags_of_read(&router, resource).await,
            expected,
            "case {case:?}: expected the resource's own read to report the written tag set"
        );

        // "Removes the specified tags": the named key goes, every other stays — and a key
        // the resource does not hold is not there to remove, so naming it is no failure.
        let removed = untag_resource(
            &router,
            resource_arn,
            &["key-1".to_string(), "never-held".to_string()],
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected UntagResource 200, got {} {:?}",
            removed.status, removed.body
        );
        expected.remove("key-1");
        let listed = list_tags(&router, resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected,
            "case {case:?}: expected the removed key gone and the rest kept, got {:?}",
            listed.body
        );

        // Removing it again is the same answer: the key is already not there.
        let removed = untag_resource(&router, resource_arn, &["key-1".to_string()]).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected the repeated UntagResource 200, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            tag_map(&list_tags(&router, resource_arn).await.body["Tags"]),
            expected,
            "case {case:?}: expected the repeated removal to change nothing"
        );
        assert_eq!(
            tags_of_read(&router, resource).await,
            expected,
            "case {case:?}: expected the resource's own read to report the remaining tag set"
        );
    }

    // Each resource holds its OWN set: tagging one never writes another's.
    let listed = list_tags(&router, &resources[0].resource_arn).await;
    assert_eq!(
        tag_map(&listed.body["Tags"]).len(),
        2,
        "expected the accessor to still hold its own two tags, got {:?}",
        listed.body
    );
}

/// Tag counts span the whole published range: a resource holding none, one, several and
/// the documented maximum of 50 reports exactly that set, and the write that would take it
/// past 50 is refused with `TooManyTagsException` (HTTP 400) without changing it.
#[tokio::test]
async fn tag_counts_span_none_to_the_documented_maximum_and_one_past_it() {
    let router = new_router();

    // Rows: no tags at all, one, a handful, and `InputTagMap`'s published maximum of 50.
    for created in [0usize, 1, 7, MAX_TAGS] {
        let answer = call(
            &router,
            "POST",
            "/accessors",
            Some(json!({
                "ClientRequestToken": format!("count-{created}"),
                "AccessorType": BILLING_TOKEN,
                "Tags": tags(created),
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {created} tags: expected CreateAccessor 200, got {} {:?}",
            answer.status, answer.body
        );
        let resource_arn = arn("accessors", &answer.text(&["AccessorId"]));

        let listed = list_tags(&router, &resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]),
            expected_tags(created),
            "case {created} tags: expected exactly the created tags back, got {:?}",
            listed.body
        );

        // Fill the resource to the documented maximum through `TagResource`.
        let fill = Value::Object(
            (created..MAX_TAGS)
                .map(|i| (format!("key-{i}"), json!(format!("value-{i}"))))
                .collect(),
        );
        let written = tag_resource(&router, &resource_arn, fill).await;
        assert_eq!(
            written.status, 200,
            "case {created} tags: expected TagResource up to the maximum to answer 200, got {} \
             {:?}",
            written.status, written.body
        );
        let at_maximum = expected_tags(MAX_TAGS);
        assert_eq!(
            tag_map(&list_tags(&router, &resource_arn).await.body["Tags"]),
            at_maximum,
            "case {created} tags: expected {MAX_TAGS} tags on the resource"
        );

        // One key PAST the maximum is refused, and the refusal stores nothing.
        let refused = tag_resource(&router, &resource_arn, json!({ "one-too-many": "v" })).await;
        assert_eq!(
            refused.status, 400,
            "case {created} tags: expected the 51st tag to be refused with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "TooManyTagsException",
            "case {created} tags: expected TooManyTagsException, got {:?} {:?}",
            refused.error_type, refused.body
        );
        assert_eq!(
            refused.body["ResourceName"].as_str(),
            Some(resource_arn.as_str()),
            "case {created} tags: expected the refusal to name the resource ARN, got {:?}",
            refused.body
        );
        assert_eq!(
            tag_map(&list_tags(&router, &resource_arn).await.body["Tags"]),
            at_maximum,
            "case {created} tags: expected the refused write to leave the tag set untouched"
        );

        // Overwriting a key the resource already holds does not grow the set, so it is
        // accepted at the maximum.
        let written = tag_resource(&router, &resource_arn, json!({ "key-0": "rewritten" })).await;
        assert_eq!(
            written.status, 200,
            "case {created} tags: expected an overwrite at the maximum to answer 200, got {} {:?}",
            written.status, written.body
        );
        let mut rewritten = at_maximum.clone();
        rewritten.insert("key-0".to_string(), "rewritten".to_string());
        assert_eq!(
            tag_map(&list_tags(&router, &resource_arn).await.body["Tags"]),
            rewritten,
            "case {created} tags: expected the overwrite to replace only that key's value"
        );
    }
}

/// A `ResourceArn` of a taggable type that names nothing this store holds is refused with
/// `ResourceNotFoundException` (HTTP 404) by all three tagging operations, and the
/// resource that DOES exist keeps its tags.
#[tokio::test]
async fn an_arn_naming_nothing_stored_is_not_found() {
    let router = new_router();

    let created = 2;
    let resources = taggable_resources(&router, &tags(created)).await;
    let accessor_arn = resources[0].resource_arn.clone();
    let stored_id = accessor_arn
        .rsplit('/')
        .next()
        .expect("the stored ARN names a resource id")
        .to_string();

    // Rows: an unminted id of EVERY taggable resource type, plus the stored accessor's own
    // id in another Region and under another account — an ARN is matched as the create
    // stored it, so neither names this store's resource.
    let mut cases: Vec<(String, String)> = Vec::new();
    for (resource_type, _) in
        service_com_amazonaws_managedblockchain_taigawebservice::state::TAGGABLE_RESOURCE_TYPES
    {
        cases.push((
            format!("unminted {resource_type}"),
            arn(resource_type, "x-ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ));
    }
    cases.push((
        "the stored accessor in another Region".to_string(),
        format!("arn:aws:managedblockchain:us-east-2:{ACCOUNT}:accessors/{stored_id}"),
    ));
    cases.push((
        "the stored accessor under another account".to_string(),
        format!("arn:aws:managedblockchain:{REGION}:111122223333:accessors/{stored_id}"),
    ));

    for (case, resource_arn) in &cases {
        for answer in [
            list_tags(&router, resource_arn).await,
            tag_resource(&router, resource_arn, json!({ "k": "v" })).await,
            untag_resource(&router, resource_arn, &["k".to_string()]).await,
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404 for {resource_arn}, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?} {:?}",
                answer.error_type, answer.body
            );
            assert_eq!(
                answer.body["ResourceName"].as_str(),
                Some(resource_arn.as_str()),
                "case {case:?}: expected the refusal to name the ARN the request sent, got {:?}",
                answer.body
            );
        }
    }

    // Nothing above touched the resources that do exist.
    for resource in &resources {
        assert_eq!(
            tag_map(&list_tags(&router, &resource.resource_arn).await.body["Tags"]),
            expected_tags(created),
            "case {:?}: expected the stored resource to keep its tags",
            resource.case
        );
    }
}

/// A `ResourceArn` that names no tag set of this service AT ALL — another service's
/// resource, or one of this service's resource types that holds no tags — asks for
/// something this service cannot do, whatever the store holds, so all three tagging
/// operations refuse it with `InvalidRequestException` (HTTP 400).
#[tokio::test]
async fn an_arn_that_names_no_taggable_resource_is_an_invalid_request() {
    let router = new_router();

    // Rows: another service's resource; this service's `invitations`, whose `Invitation`
    // shape declares no `Tags` member at all (smithy_query: shape Invitation), so no
    // invitation has a tag set to read or write whether or not one exists; a resource type
    // this service does not have; and a resource part with no id after the type.
    let cases: &[(&str, String)] = &[
        (
            "another service's resource",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0"),
        ),
        (
            "an invitation, whose shape declares no Tags",
            arn("invitations", "i-ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ),
        (
            "a resource type this service does not have",
            arn("channels", "c-ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ),
        ("no id after the resource type", arn("accessors", "")),
    ];

    for (case, resource_arn) in cases {
        for answer in [
            list_tags(&router, resource_arn).await,
            tag_resource(&router, resource_arn, json!({ "k": "v" })).await,
            untag_resource(&router, resource_arn, &["k".to_string()]).await,
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400 for {resource_arn}, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {:?} {:?}",
                answer.error_type, answer.body
            );
            assert!(
                answer.body["Message"].as_str().is_some_and(|m| !m.is_empty()),
                "case {case:?}: expected the refusal to say what was wrong, got {:?}",
                answer.body
            );
        }
    }
}
