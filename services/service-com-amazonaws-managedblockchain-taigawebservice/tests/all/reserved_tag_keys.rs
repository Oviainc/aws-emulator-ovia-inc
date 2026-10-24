// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, end to end over HTTP.
//!
//! RULE: the tag-key prefix Amazon Web Services keeps for its own tags
//! (`state::RESERVED_TAG_KEY_PREFIX`) is not a namespace a caller writes: every request
//! that would create, overwrite or remove a key carrying it — `TagResource`,
//! `UntagResource`, and the `Tags` of every create — is refused with
//! `InvalidRequestException` (HTTP 400) and leaves the stored tag set exactly as it was,
//! while a key that merely resembles the prefix is an ordinary caller key that is stored.
//!
//! `TagKey` carries only `@length(1, 128)` and no `@pattern` (smithy_query: shape TagKey),
//! so the namespace is service behaviour these cases pin rather than a modelled constraint.
//!
//! The tables span what the rule generalises over: every spelling of the reserved prefix
//! (alone, with a real AWS system key after it, upper-cased, mixed-case, with a non-ASCII
//! name, and filled to `TagKey`'s published 128-character maximum), every resource type
//! this service tags, a reserved key alone and mixed in with keys the caller may write, and
//! every `Tags` member a create carries. The prefix itself is read from the crate rather
//! than re-spelled here, so the rows and the handler cannot drift apart.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::RESERVED_TAG_KEY_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, create_network_body, member_configuration, new_router,
    router_and_engine, Answer, ACCOUNT,
};
use crate::public_networks_and_membership::invite;
use crate::resource_tags::{
    arn, expected_tags, list_tags, tag_map, tag_resource, taggable_resources, tags, tags_of_read,
    untag_resource, BILLING_TOKEN,
};

/// `TagKey`'s published maximum (smithy_query: shape TagKey = @length(1, 128)), so one row
/// can fill a reserved key right up to it.
const MAX_TAG_KEY_CHARS: usize = 128;

/// The `InstanceType` the node rows are created with, as the other suites use.
const INSTANCE_TYPE: &str = "bc.t3.small";

/// The reserved prefix with its first character upper-cased, derived from the prefix itself
/// rather than written out: the namespace is reserved however it is cased.
fn mixed_case_prefix() -> String {
    let mut chars = RESERVED_TAG_KEY_PREFIX.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Keys in the reserved namespace: every spelling of the prefix, each with what a caller
/// might put after it.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        (
            "the prefix with nothing after it".to_string(),
            prefix.to_string(),
        ),
        (
            "a key AWS itself applies".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "the prefix upper-cased".to_string(),
            format!("{}owner", prefix.to_uppercase()),
        ),
        (
            "the prefix mixed-case".to_string(),
            format!("{}created-by", mixed_case_prefix()),
        ),
        (
            "the prefix with a non-ASCII name".to_string(),
            format!("{prefix}Проект-Опс"),
        ),
        (
            "the prefix filled to the published key maximum".to_string(),
            format!(
                "{prefix}{}",
                "k".repeat(MAX_TAG_KEY_CHARS - prefix.chars().count())
            ),
        ),
    ]
}

/// The ASCII subset of [`reserved_keys`], for the rows that travel in a query string — an
/// `UntagResource` `tagKeys` value — where a non-ASCII key would be about encoding rather
/// than about the namespace.
fn reserved_keys_ascii() -> Vec<(String, String)> {
    reserved_keys()
        .into_iter()
        .filter(|(_, key)| key.is_ascii())
        .collect()
}

/// Keys that merely RESEMBLE the reserved namespace and are therefore the caller's own: the
/// prefix letters without the colon that makes the namespace, that stem extended, the
/// prefix appearing later in the key, and the stem upper-cased.
fn lookalike_keys() -> Vec<(String, String)> {
    let stem = RESERVED_TAG_KEY_PREFIX.trim_end_matches(':');
    vec![
        (
            "the prefix letters without the colon".to_string(),
            stem.to_string(),
        ),
        ("that stem extended".to_string(), format!("{stem}ome")),
        (
            "the stem as a word, upper-cased".to_string(),
            stem.to_uppercase(),
        ),
        (
            "the prefix later in the key".to_string(),
            format!("mine-{RESERVED_TAG_KEY_PREFIX}thing"),
        ),
        (
            "the stem before a hyphen".to_string(),
            format!("{stem}-tier"),
        ),
    ]
}

/// Assert one answer is the modelled refusal for a reserved key.
fn assert_reserved_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a reserved tag key, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {:?} {:?}",
        answer.error_type, answer.body
    );
    assert!(
        answer.body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected the refusal to say what was wrong, got {:?}",
        answer.body
    );
}

/// How many items one listing reports.
async fn listed(router: &MultiServiceRouter, path: &str, member: &str) -> usize {
    let answer = call(router, "GET", path, None).await;
    assert_eq!(
        answer.status, 200,
        "case listing {path}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body[member].as_array().map_or(0, Vec::len)
}

/// `TagResource` refuses a reserved key on EVERY resource type this service tags, whether
/// the key comes alone or beside keys the caller may write, and the resource keeps the tag
/// set it had.
#[tokio::test]
async fn tag_resource_refuses_every_reserved_key_spelling_and_stores_nothing() {
    let router = new_router();

    let created = 2;
    let resources = taggable_resources(&router, &tags(created)).await;
    let untouched = expected_tags(created);

    for resource in &resources {
        let resource_arn = resource.resource_arn.as_str();
        for (spelling, key) in reserved_keys() {
            // The key alone, then the same key beside one the caller may write: the
            // request is judged whole, so the writable key is not stored either.
            for (shape, body) in [
                ("alone", json!({ key.clone(): "x" })),
                (
                    "beside a key the caller may write",
                    json!({ key.clone(): "x", "mine": "v" }),
                ),
            ] {
                let case = format!("{} / {spelling} / {shape}", resource.case);
                let answer = tag_resource(&router, resource_arn, body).await;
                assert_reserved_refusal(&case, &answer);

                assert_eq!(
                    tag_map(&list_tags(&router, resource_arn).await.body["Tags"]),
                    untouched,
                    "case {case:?}: expected the refused write to leave the tag set untouched"
                );
                assert_eq!(
                    tags_of_read(&router, resource).await,
                    untouched,
                    "case {case:?}: expected the resource's own read to report the untouched \
                     tag set"
                );
            }
        }
    }
}

/// A key that only RESEMBLES the reserved prefix is the caller's own: `TagResource` stores
/// it, `ListTagsForResource` reports it and `UntagResource` removes it again.
#[tokio::test]
async fn keys_that_merely_resemble_the_reserved_prefix_are_the_callers_own() {
    let router = new_router();

    let answer = call(
        &router,
        "POST",
        "/accessors",
        Some(json!({
            "ClientRequestToken": "lookalike-keys",
            "AccessorType": BILLING_TOKEN,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    let resource_arn = arn("accessors", &answer.text(&["AccessorId"]));

    for (case, key) in lookalike_keys() {
        let written = tag_resource(&router, &resource_arn, json!({ key.clone(): "v" })).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected TagResource to accept {key:?}, got {} {:?}",
            written.status, written.body
        );
        let listed = list_tags(&router, &resource_arn).await;
        assert_eq!(
            tag_map(&listed.body["Tags"]).get(&key).map(String::as_str),
            Some("v"),
            "case {case:?}: expected {key:?} to be stored, got {:?}",
            listed.body
        );

        let removed = untag_resource(&router, &resource_arn, std::slice::from_ref(&key)).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected UntagResource to accept {key:?}, got {} {:?}",
            removed.status, removed.body
        );
        let listed = list_tags(&router, &resource_arn).await;
        assert!(
            !tag_map(&listed.body["Tags"]).contains_key(&key),
            "case {case:?}: expected {key:?} to be removed, got {:?}",
            listed.body
        );
    }
}

/// `UntagResource` refuses a reserved key — a caller may no more remove one than create it
/// — and removes NOTHING when it does, including the writable keys the same request named.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let router = new_router();

    let created = 2;
    let resources = taggable_resources(&router, &tags(created)).await;
    let untouched = expected_tags(created);

    for resource in &resources {
        let resource_arn = resource.resource_arn.as_str();
        for (spelling, key) in reserved_keys_ascii() {
            for (shape, keys) in [
                ("alone", vec![key.clone()]),
                (
                    "beside a key the resource holds",
                    vec!["key-0".to_string(), key.clone()],
                ),
            ] {
                let case = format!("{} / {spelling} / {shape}", resource.case);
                let answer = untag_resource(&router, resource_arn, &keys).await;
                assert_reserved_refusal(&case, &answer);

                assert_eq!(
                    tag_map(&list_tags(&router, resource_arn).await.body["Tags"]),
                    untouched,
                    "case {case:?}: expected the refused removal to leave the tag set untouched"
                );
            }
        }

        // The same request without the reserved key removes exactly what it names, so the
        // refusals above were about the namespace and nothing else.
        let removed = untag_resource(&router, resource_arn, &["key-0".to_string()]).await;
        assert_eq!(
            removed.status, 200,
            "case {:?}: expected UntagResource 200 for a key the caller owns, got {} {:?}",
            resource.case, removed.status, removed.body
        );
        let mut remaining = untouched.clone();
        remaining.remove("key-0");
        assert_eq!(
            tag_map(&list_tags(&router, resource_arn).await.body["Tags"]),
            remaining,
            "case {:?}: expected only the named key removed",
            resource.case
        );
    }
}

/// How one row builds its request body: the idempotency token to send, and the `Tags`
/// value to place in the member the row is about.
type BodyFor = Box<dyn Fn(&str, Value) -> Value>;

/// One create row: the request this `Tags` member travels in, the listing that proves
/// nothing was created, and how the accepted request's resource is named.
struct CreateCase {
    case: &'static str,
    path: String,
    listing_path: String,
    listing_member: &'static str,
    id_member: &'static str,
    arn_type: &'static str,
    body: BodyFor,
}

/// Every `Tags` member a create of this service carries refuses a reserved key the same
/// way: the modelled 400, nothing created, and not even the request's idempotency token
/// recorded — the same token then creates the resource once the reserved key is gone.
#[tokio::test]
async fn every_create_refuses_a_reserved_tag_key_and_creates_nothing() {
    let (router, engine) = router_and_engine();

    // A settled network for the rows created on one, and the invitation the join row uses.
    let (network_id, member_id) = available_network(&router, "reserved-tags-network", "resnet").await;
    let invitation_id = invite(&engine, &network_id, ACCOUNT, SystemTime::now());

    let node_member = member_id.clone();
    let proposal_member = member_id.clone();
    let join_invitation = invitation_id.clone();

    let cases: Vec<CreateCase> = vec![
        CreateCase {
            case: "CreateAccessor Tags",
            path: "/accessors".to_string(),
            listing_path: "/accessors".to_string(),
            listing_member: "Accessors",
            id_member: "AccessorId",
            arn_type: "accessors",
            body: Box::new(|token, tags| {
                json!({
                    "ClientRequestToken": token,
                    "AccessorType": BILLING_TOKEN,
                    "Tags": tags,
                })
            }),
        },
        CreateCase {
            case: "CreateNetwork Tags",
            path: "/networks".to_string(),
            listing_path: "/networks".to_string(),
            listing_member: "Networks",
            id_member: "NetworkId",
            arn_type: "networks",
            body: Box::new(|token, tags| {
                let mut body = create_network_body(token, "reserved-network-tags");
                body["Tags"] = tags;
                body
            }),
        },
        CreateCase {
            case: "CreateNetwork MemberConfiguration.Tags",
            path: "/networks".to_string(),
            listing_path: "/networks".to_string(),
            listing_member: "Networks",
            // The member the network's own create founds: its tags are the ones this row
            // writes, so the ARN to read back is the member's.
            id_member: "MemberId",
            arn_type: "members",
            body: Box::new(|token, tags| {
                let mut body = create_network_body(token, "reserved-member-tags");
                body["MemberConfiguration"]["Tags"] = tags;
                body
            }),
        },
        CreateCase {
            case: "CreateNode Tags",
            path: format!("/networks/{network_id}/nodes"),
            listing_path: format!("/networks/{network_id}/nodes?memberId={member_id}"),
            listing_member: "Nodes",
            id_member: "NodeId",
            arn_type: "nodes",
            body: Box::new(move |token, tags| {
                json!({
                    "ClientRequestToken": token,
                    "MemberId": node_member,
                    "NodeConfiguration": { "InstanceType": INSTANCE_TYPE },
                    "Tags": tags,
                })
            }),
        },
        CreateCase {
            case: "CreateProposal Tags",
            path: format!("/networks/{network_id}/proposals"),
            listing_path: format!("/networks/{network_id}/proposals"),
            listing_member: "Proposals",
            id_member: "ProposalId",
            arn_type: "proposals",
            body: Box::new(move |token, tags| {
                json!({
                    "ClientRequestToken": token,
                    "MemberId": proposal_member,
                    "Actions": { "Invitations": [{ "Principal": "100000000001" }] },
                    "Tags": tags,
                })
            }),
        },
        CreateCase {
            case: "CreateMember MemberConfiguration.Tags",
            path: format!("/networks/{network_id}/members"),
            listing_path: format!("/networks/{network_id}/members"),
            listing_member: "Members",
            id_member: "MemberId",
            arn_type: "members",
            body: Box::new(move |token, tags| {
                let mut configuration = member_configuration("org2");
                configuration["Tags"] = tags;
                json!({
                    "ClientRequestToken": token,
                    "InvitationId": join_invitation,
                    "MemberConfiguration": configuration,
                })
            }),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let before = listed(&router, &case.listing_path, case.listing_member).await;
        let token = format!("reserved-create-{index}");

        for (spelling, key) in reserved_keys() {
            let row = format!("{} / {spelling}", case.case);
            let body = (case.body)(&token, json!({ key.clone(): "x", "mine": "v" }));
            let answer = call(&router, "POST", &case.path, Some(body)).await;
            assert_reserved_refusal(&row, &answer);

            assert_eq!(
                listed(&router, &case.listing_path, case.listing_member).await,
                before,
                "case {row:?}: expected the refused create to create nothing"
            );
        }

        // The SAME idempotency token, now without the reserved key: a refusal records no
        // token, so this is a first call and it creates the resource.
        let accepted = call(
            &router,
            "POST",
            &case.path,
            Some((case.body)(&token, json!({ "mine": "v" }))),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the same request without the reserved key to answer 200, got {} \
             {:?}",
            case.case, accepted.status, accepted.body
        );
        assert_eq!(
            listed(&router, &case.listing_path, case.listing_member).await,
            before + 1,
            "case {:?}: expected exactly one resource created",
            case.case
        );

        let created_arn = arn(case.arn_type, &accepted.text(&[case.id_member]));
        let listing = list_tags(&router, &created_arn).await;
        assert_eq!(
            tag_map(&listing.body["Tags"])
                .get("mine")
                .map(String::as_str),
            Some("v"),
            "case {:?}: expected the accepted create to store the caller's own tag on \
             {created_arn}, got {:?}",
            case.case,
            listing.body
        );
        assert!(
            tag_map(&listing.body["Tags"])
                .keys()
                .all(|key| !key.to_ascii_lowercase().starts_with(RESERVED_TAG_KEY_PREFIX)),
            "case {:?}: expected no reserved key on {created_arn}, got {:?}",
            case.case,
            listing.body
        );
    }
}
