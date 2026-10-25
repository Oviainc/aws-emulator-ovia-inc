// SPDX-License-Identifier: CC0-1.0

//! What makes a `CreateVpcAttachment` store an attachment AT ALL, where that question is
//! about the request's own members: the `SubnetArns` list it carries, the `Tags` it carries
//! and the modelled `@idempotencyToken`. Driven end to end over HTTP through the real router
//! (the harness lives in `crate::global_network_lifecycle`), reusing the ARN, policy and
//! core-network helpers of `crate::attachment_lifecycle`, which is also where what a stored
//! VPC attachment REPORTS, and the references it must resolve, are covered.
//!
//! The rules under test, each asserted over a table that spans it:
//!
//! * a VPC attachment reaches its VPC through a BOUNDED, repetition-free list of subnets of
//!   that VPC's own Region, which it stores in the order the request listed them: a list at
//!   either published bound is accepted and read back entry for entry through
//!   `GetVpcAttachment`, and a list outside those bounds is refused with the modelled
//!   `ValidationException` naming the member that carried it;
//! * the `Tags` a create carries obey the same published tag rules a later `TagResource`
//!   does, refused through the error class this operation models — it models no
//!   `ServiceQuotaExceededException` (`smithy_query query="operation"
//!   operation="CreateVpcAttachment"`), so a request past the per-resource tag quota is the
//!   modelled `ValidationException` with the `Other` reason, while a tag no resource may
//!   carry is a field failure naming the member that carried it;
//! * the `@idempotencyToken` joins two creates only where it carries characters: a token of
//!   no characters is no token at all, so it neither makes a second create a retry nor
//!   exempts one from the rule that a VPC is attached once.
//!
//! Every refusal is also asserted to store NOTHING: the attachments `ListAttachments`
//! reports for the core network afterwards are exactly the accepted ones. Nothing reaches
//! into `crate::state` — every attachment is created, listed and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_vpc_attachment, subnet_arn, vpc_arn, ATTACHMENT,
};
use crate::global_network_lifecycle::{error_code, expected_arn, ids_of, Emulator};
use crate::resource_tagging::{list_tags, tag, MAX_KEY_CHARS, MAX_TAGS, MAX_VALUE_CHARS};

/// The one Region every core network here configures an edge location in, and so the Region
/// of every VPC and subnet these cases name.
const REGION: &str = "us-west-2";

/// How many subnets one VPC attachment may hold: the bound
/// `state::MAX_SUBNETS_PER_VPC_ATTACHMENT` fixes, the same number
/// `crate::vpc_attachment_lifecycle` holds an update to.
const MAX_SUBNETS: usize = 50;

/// The `Reason` a `ValidationException` of this service carries when an input member is at
/// fault, and the one it carries when none is — the request is invalid against a count or
/// against stored state (`smithy_query shape=ValidationExceptionReason`).
const REASON_FIELD: &str = "FieldValidationFailed";
/// See [`REASON_FIELD`].
const REASON_OTHER: &str = "Other";

/// The `CreateVpcAttachment` member that carries the subnet list.
const SUBNET_ARNS: &str = "subnetArns";
/// The member the tag rules blame, as `TagResource` blames it too.
const TAGS: &str = "Tags";

/// The `CreateVpcAttachment` body attaching the VPC of `vpc_index` through `subnets`.
fn create_request(core_network: &str, vpc_index: usize, subnets: &[String]) -> Value {
    json!({
        "CoreNetworkId": core_network,
        "VpcArn": vpc_arn(REGION, vpc_index),
        "SubnetArns": subnets,
    })
}

/// `body` with `member` set to `value`.
fn with(mut body: Value, member: &str, value: Value) -> Value {
    let object = body
        .as_object_mut()
        .expect("the request body of every row here is a JSON object");
    object.insert(member.to_string(), value);
    body
}

/// `count` subnet ARNs of [`REGION`] out of the block `base` reserves for one row, in
/// DESCENDING index order — an order no sort would produce, so a reader answering them in
/// the order the request listed them is answering what was stored.
fn descending_block(base: usize, count: usize) -> Vec<String> {
    (0..count)
        .rev()
        .map(|index| subnet_arn(REGION, base + index))
        .collect()
}

/// The subnet ARNs a `VpcAttachment` wrapper reports, in the order it reports them.
fn subnets_of(wrapper: &Value) -> Vec<String> {
    wrapper["SubnetArns"]
        .as_array()
        .map_or_else(Vec::new, |listed| {
            listed
                .iter()
                .map(|arn| arn.as_str().unwrap_or_default().to_string())
                .collect()
        })
}

/// A create that must be accepted, answering the minted attachment id.
async fn create_ok(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, created) = create_vpc_attachment(emulator, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    attachment_id(case, &created["VpcAttachment"])
}

/// A create that must be refused with `status` and `code`, answering the error body.
async fn create_refused(
    emulator: &Emulator,
    case: &str,
    body: Value,
    status: u16,
    code: &str,
) -> Value {
    let (actual, refusal) = create_vpc_attachment(emulator, body).await;
    assert_eq!(
        actual, status,
        "case {case:?}: expected {status}, got {actual} with {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        code,
        "case {case:?}: expected {code}, got {refusal}"
    );
    refusal
}

/// The ids `ListAttachments` reports for `core_network` — the stored inventory, read through
/// an operation.
async fn stored_ids(emulator: &Emulator, core_network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/attachments?coreNetworkId={core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "ListAttachments expected 200, got {status} with {body}"
    );
    ids_of(&body, "Attachments", "AttachmentId")
}

/// The `VpcAttachment` `GetVpcAttachment` answers for `attachment`.
async fn read_back(emulator: &Emulator, case: &str, attachment: &str) -> Value {
    let (status, body) = emulator
        .get(&format!("/vpc-attachments/{attachment}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {body}"
    );
    body["VpcAttachment"].clone()
}

/// One row of the subnet-list table: how many subnets the create names, and whether such a
/// list may be stored.
struct SubnetCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// How many subnets the request's `SubnetArns` names.
    count: usize,
    /// The `Reason` the refusal must carry, or `None` for a row that must be accepted.
    refused_with: Option<&'static str>,
}

/// The table: both published bounds on the list exactly, a length between them, and one step
/// outside each bound.
const SUBNET_CASES: &[SubnetCase] = &[
    SubnetCase {
        name: "one subnet, the fewest a VPC attachment may reach its VPC through",
        count: 1,
        refused_with: None,
    },
    SubnetCase {
        name: "two subnets",
        count: 2,
        refused_with: None,
    },
    SubnetCase {
        name: "subnets up to the maximum one attachment holds",
        count: MAX_SUBNETS,
        refused_with: None,
    },
    SubnetCase {
        // A member at fault: the list satisfies every modelled constraint, and names no
        // subnet to reach the VPC through.
        name: "no subnets at all, one short of the fewest",
        count: 0,
        refused_with: Some(REASON_FIELD),
    },
    SubnetCase {
        // The COUNT is what cannot be stored, so this one carries the other reason.
        name: "one subnet past the maximum",
        count: MAX_SUBNETS + 1,
        refused_with: Some(REASON_OTHER),
    },
];

#[tokio::test]
async fn a_create_stores_a_bounded_subnet_list_in_the_order_it_was_given() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "subnet bounds", &[REGION]).await;
    let mut accepted: Vec<String> = Vec::new();

    for (row, case) in SUBNET_CASES.iter().enumerate() {
        // Each row attaches its OWN VPC out of its own subnet block, since a VPC is attached
        // once and a subnet belongs to one attachment.
        let base = row * 1_000;
        let subnets = descending_block(base, case.count);
        let request = create_request(&core_network, base, &subnets);

        match case.refused_with {
            None => {
                let id = create_ok(&emulator, case.name, request).await;
                let stored = read_back(&emulator, case.name, &id).await;
                assert_eq!(
                    subnets_of(&stored),
                    subnets,
                    "case {:?}: expected the subnets the create named, in that order, got {stored}",
                    case.name
                );
                assert_eq!(
                    stored["Attachment"]["EdgeLocation"],
                    json!(REGION),
                    "case {:?}: the attachment sits at the Region of its VPC, got {stored}",
                    case.name
                );
                assert_eq!(
                    stored["Attachment"]["ResourceArn"],
                    json!(vpc_arn(REGION, base)),
                    "case {:?}: expected the ARN of the attached VPC, got {stored}",
                    case.name
                );
                accepted.push(id);
            }
            Some(reason) => {
                let refusal =
                    create_refused(&emulator, case.name, request, 400, "ValidationException").await;
                assert_eq!(
                    refusal["Reason"].as_str(),
                    Some(reason),
                    "case {:?}: expected the {reason} reason, got {refusal}",
                    case.name
                );
                assert_eq!(
                    refusal["Fields"][0]["Name"].as_str(),
                    Some(SUBNET_ARNS),
                    "case {:?}: the refusal must name the member at fault, got {refusal}",
                    case.name
                );
            }
        }
    }

    // A list the attachment could hold, whose entries cannot: one named twice, and one
    // outside the VPC's Region. Both are the same member's failure, and both store nothing.
    for (case, subnets) in [
        (
            "the same subnet named twice",
            vec![subnet_arn(REGION, 90_001), subnet_arn(REGION, 90_001)],
        ),
        (
            "a subnet outside the VPC's Region",
            vec![subnet_arn(REGION, 90_002), subnet_arn("eu-west-1", 90_003)],
        ),
        (
            "an empty subnet ARN, which the modelled pattern admits",
            vec![String::new()],
        ),
    ] {
        let refusal = create_refused(
            &emulator,
            case,
            create_request(&core_network, 90_000, &subnets),
            400,
            "ValidationException",
        )
        .await;
        assert_eq!(
            refusal["Fields"][0]["Name"].as_str(),
            Some(SUBNET_ARNS),
            "case {case:?}: the refusal must name the member at fault, got {refusal}"
        );
    }

    // Exactly the accepted rows are stored: no refused create left an attachment behind.
    let mut listed = stored_ids(&emulator, &core_network).await;
    listed.sort();
    accepted.sort();
    assert_eq!(
        listed, accepted,
        "a refused create stored an attachment: expected {accepted:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn the_tags_a_create_carries_obey_the_published_tag_rules() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "create tags", &[REGION]).await;

    // Accepted rows: each published bound EXACTLY — a key of the maximum length, a value of
    // the maximum length, and the maximum number of distinct keys — stored and read back
    // through `ListTagsForResource`.
    let at_the_bound: Vec<(&str, Value)> = vec![
        (
            "a key of the maximum length",
            json!([tag(&"k".repeat(MAX_KEY_CHARS), "x")]),
        ),
        (
            "a value of the maximum length",
            json!([tag("k", &"v".repeat(MAX_VALUE_CHARS))]),
        ),
        (
            "the maximum number of distinct keys",
            json!((0..MAX_TAGS)
                .map(|index| tag(&format!("key-{index:02}"), "x"))
                .collect::<Vec<Value>>()),
        ),
        ("no tags at all", json!([])),
    ];
    let mut accepted: Vec<String> = Vec::new();
    for (row, (case, tags)) in at_the_bound.into_iter().enumerate() {
        let base = row * 1_000;
        let id = create_ok(
            &emulator,
            case,
            with(
                create_request(&core_network, base, &descending_block(base, 2)),
                "Tags",
                tags.clone(),
            ),
        )
        .await;
        let (status, listed) = list_tags(&emulator, &expected_arn(ATTACHMENT, &id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, tags,
            "case {case:?}: expected the tags the create carried, got {listed}"
        );
        let stored = read_back(&emulator, case, &id).await;
        assert_eq!(
            stored["Attachment"]["Tags"], tags,
            "case {case:?}: the attachment's own reader must answer the same tags, got {stored}"
        );
        accepted.push(id);
    }

    // Refused rows: each ONE step outside a bound, plus both shapes of a key AWS reserves.
    // The per-resource tag quota is the count the request would leave rather than a member
    // at fault, so it carries the `Other` reason; every other row names `Tags`.
    let refusals: Vec<(&str, Value, &str)> = vec![
        ("an empty key", json!([tag("", "x")]), REASON_FIELD),
        (
            "a key one character past the bound",
            json!([tag(&"k".repeat(MAX_KEY_CHARS + 1), "x")]),
            REASON_FIELD,
        ),
        (
            "a value one character past the bound",
            json!([tag("k", &"v".repeat(MAX_VALUE_CHARS + 1))]),
            REASON_FIELD,
        ),
        (
            "a key carrying the prefix AWS reserves",
            json!([tag("aws:cloudformation:stack-name", "x")]),
            REASON_FIELD,
        ),
        (
            "that reserved prefix in another case",
            json!([tag("AWS:billing", "x")]),
            REASON_FIELD,
        ),
        (
            "one distinct key past the maximum number",
            json!((0..=MAX_TAGS)
                .map(|index| tag(&format!("key-{index:02}"), "x"))
                .collect::<Vec<Value>>()),
            REASON_OTHER,
        ),
    ];
    for (row, (case, tags, reason)) in refusals.into_iter().enumerate() {
        let base = 50_000 + row * 1_000;
        let refusal = create_refused(
            &emulator,
            case,
            with(
                create_request(&core_network, base, &descending_block(base, 1)),
                "Tags",
                tags,
            ),
            400,
            "ValidationException",
        )
        .await;
        assert_eq!(
            refusal["Reason"].as_str(),
            Some(reason),
            "case {case:?}: expected the {reason} reason, got {refusal}"
        );
        if reason == REASON_FIELD {
            assert_eq!(
                refusal["Fields"][0]["Name"].as_str(),
                Some(TAGS),
                "case {case:?}: the refusal must name the member at fault, got {refusal}"
            );
        }
    }

    // No refused create stored an attachment: the inventory is exactly the accepted rows.
    let mut listed = stored_ids(&emulator, &core_network).await;
    listed.sort();
    accepted.sort();
    assert_eq!(
        listed, accepted,
        "a create refused over its tags stored an attachment: expected {accepted:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_client_token_with_no_characters_is_no_token() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "empty token", &[REGION]).await;
    let empty = json!("");

    // Two creates carrying the same EMPTY token and two VPCs: were the empty string taken
    // for an idempotency token, the second would be that token asking for something else and
    // so a conflict. It is no token, so each create mints its own attachment.
    let first_request = with(
        create_request(&core_network, 1, &descending_block(1, 1)),
        "ClientToken",
        empty.clone(),
    );
    let first = create_ok(
        &emulator,
        "the first create under an empty token",
        first_request.clone(),
    )
    .await;
    let second = create_ok(
        &emulator,
        "a second VPC under the same empty token",
        with(
            create_request(&core_network, 2, &descending_block(10, 1)),
            "ClientToken",
            empty,
        ),
    )
    .await;
    assert_ne!(
        first, second,
        "an empty client token must not make the second create a retry, got {first} twice"
    );

    // Nor does it make a repeat of the first request a retry: with no token to resolve it
    // against, the rule that a VPC is attached once refuses it, and names the attachment
    // already holding that VPC.
    let refusal = create_refused(
        &emulator,
        "the first request repeated under the same empty token",
        first_request,
        409,
        "ConflictException",
    )
    .await;
    assert_eq!(
        refusal["ResourceId"],
        json!(first),
        "the conflict must name the attachment already holding the VPC, got {refusal}"
    );
    assert_eq!(
        refusal["ResourceType"],
        json!(ATTACHMENT),
        "the conflict must name the resource type, got {refusal}"
    );

    // A token that DOES carry characters, at the widest length the model admits
    // (`ClientToken`, len 0..256: `smithy_query query="operation"
    // operation="CreateVpcAttachment"`), makes the identical request a retry of the one it
    // first created rather than a second attachment.
    let token = "t".repeat(256);
    let tokened = with(
        create_request(&core_network, 3, &descending_block(20, 2)),
        "ClientToken",
        json!(token),
    );
    let third = create_ok(&emulator, "a create under the widest token", tokened.clone()).await;
    let retried = create_ok(&emulator, "the identical request retried", tokened.clone()).await;
    assert_eq!(
        retried, third,
        "a retry under the same token minted a second attachment, got {retried} after {third}"
    );

    // The same token asking for something else — one subnet more — is the service's
    // conflict, and stores nothing.
    create_refused(
        &emulator,
        "the same token asking for another subnet list",
        with(tokened, "SubnetArns", json!(descending_block(20, 3))),
        409,
        "ConflictException",
    )
    .await;

    let mut listed = stored_ids(&emulator, &core_network).await;
    listed.sort();
    let mut expected = vec![first, second, third];
    expected.sort();
    assert_eq!(
        listed, expected,
        "a retry or a refused create changed the stored inventory: expected {expected:?}, got {listed:?}"
    );
}
