// SPDX-License-Identifier: CC0-1.0

//! What makes a `CreateTransitGatewayPeering` store a peering AT ALL, where that question
//! is about the request's own members: the modelled `@idempotencyToken` and the `Tags` the
//! create carries. Driven end to end over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`), reusing the peering helpers of
//! `crate::transit_gateway_peering`, which is also where what a stored peering REPORTS and
//! reads back as is covered. The two bounds on the store itself — one gateway peered once,
//! whichever core network asks, and the per-core-network peering cap — are covered by
//! `crate::transit_gateway_route_table_attachment`.
//!
//! The rules under test, each asserted over a table that spans it:
//!
//! * the `@idempotencyToken` makes a create a RETRY only of the same request — the same core
//!   network, the same gateway and the same tags, in whatever order those tags arrived — and
//!   the same token asking for anything else is the service's conflict; a token carrying no
//!   characters is no token at all, so it neither joins two creates nor exempts one from the
//!   rules that bound the store;
//! * the `Tags` a create carries obey the same published tag rules a later `TagResource`
//!   does, refused through the error class this operation models — it models no
//!   `ServiceQuotaExceededException` (`smithy_query query="operation"
//!   operation="CreateTransitGatewayPeering"`), so a request past the per-resource tag quota
//!   is the modelled `ValidationException` with the `Other` reason, while a tag no resource
//!   may carry is a field failure naming the member that carried it.
//!
//! Every refusal is also asserted to store NOTHING: the inventory the implemented readers
//! report is unchanged afterwards. Nothing reaches into `crate::state` — every peering is
//! created, counted and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{error_code, expected_arn, Emulator};
use crate::resource_tagging::{list_tags, tag, MAX_KEY_CHARS, MAX_TAGS, MAX_VALUE_CHARS};
use crate::transit_gateway_peering::{
    create_peering, get_peering_ok, own_account, peering_count, peering_id, transit_gateway_arn,
    PEERING,
};

/// The edges every core network in this suite configures, one per Region.
const EDGES: [&str; 2] = ["us-west-2", "eu-central-1"];

/// The `Reason` a `ValidationException` of this service carries when no input member is at
/// fault — the count held is. The other value it reports is [`REASON_FIELD`]
/// (`smithy_query shape=ValidationExceptionReason`).
const REASON_OTHER: &str = "Other";
/// See [`REASON_OTHER`]: the `Reason` of a refusal an input member carried.
const REASON_FIELD: &str = "FieldValidationFailed";

/// A transit gateway of this account in `region`, carrying the widest id its ARN allows.
/// `index` keeps the gateways of two rows apart, since one gateway is peered once.
fn gateway(region: &str, index: usize) -> String {
    transit_gateway_arn("aws", region, &own_account(), index, 17)
}

/// The `CreateTransitGatewayPeering` body peering `gateway_arn` from `core_network`.
fn peer_request(core_network: &str, gateway_arn: &str) -> Value {
    json!({
        "CoreNetworkId": core_network,
        "TransitGatewayArn": gateway_arn,
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

/// A create that must be accepted, answering the minted peering id.
async fn create_ok(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, created) = create_peering(emulator, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTransitGatewayPeering expected 200, got {status} with {created}"
    );
    peering_id(case, &created["TransitGatewayPeering"])
}

/// A create that must be refused with `status` and `code`, answering the error body.
async fn create_refused(
    emulator: &Emulator,
    case: &str,
    body: Value,
    status: u16,
    code: &str,
) -> Value {
    let (actual, refusal) = create_peering(emulator, body).await;
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

#[tokio::test]
async fn a_client_token_is_a_retry_only_of_the_same_request() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "retries", &EDGES).await;
    let (_, other_core) = core_network_at(&emulator, "retries-elsewhere", &EDGES).await;
    let token = "token-retry";
    let tags = json!([tag("Name", "oregon"), tag("Tier", "edge")]);
    let first = with(
        with(
            peer_request(&core_network, &gateway(EDGES[0], 1)),
            "ClientToken",
            json!(token),
        ),
        "Tags",
        tags.clone(),
    );
    let id = create_ok(&emulator, "the first create", first.clone()).await;

    // Rows that ARE the same request: each answers the record the first call minted rather
    // than minting a second peering — including the one carrying the same tags in the other
    // order, since a resource's tags are keys with values and not a sequence.
    for (case, body) in [
        ("the identical request", first.clone()),
        (
            "the same tags in the other order",
            with(
                first.clone(),
                "Tags",
                json!([tag("Tier", "edge"), tag("Name", "oregon")]),
            ),
        ),
    ] {
        let (status, again) = create_peering(&emulator, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry expected 200, got {status} with {again}"
        );
        assert_eq!(
            peering_id(case, &again["TransitGatewayPeering"]),
            id,
            "case {case:?}: a retry minted a second peering, got {again}"
        );
    }

    // Rows that are a DIFFERENT request under the same token: refused as a conflict, each
    // differing in ONE thing the request carries — the gateway, the core network, a tag
    // value, a tag that is gone, or every tag.
    for (case, body) in [
        (
            "another gateway",
            with(
                first.clone(),
                "TransitGatewayArn",
                json!(gateway(EDGES[1], 2)),
            ),
        ),
        (
            "another core network",
            with(first.clone(), "CoreNetworkId", json!(other_core)),
        ),
        (
            "a changed tag value",
            with(
                first.clone(),
                "Tags",
                json!([tag("Name", "oregon"), tag("Tier", "core")]),
            ),
        ),
        (
            "a dropped tag",
            with(first.clone(), "Tags", json!([tag("Name", "oregon")])),
        ),
        ("no tags at all", with(first, "Tags", json!([]))),
    ] {
        create_refused(&emulator, case, body, 409, "ConflictException").await;
    }

    // Neither the retries nor the conflicts changed the one stored peering, which still
    // reports the tags the first create carried.
    let stored = get_peering_ok(&emulator, "the first create", &id).await;
    assert_eq!(
        stored["Peering"]["Tags"], tags,
        "a retry or a conflicted token changed the stored tags, got {stored}"
    );
    assert_eq!(
        peering_count(&emulator, &core_network).await,
        1,
        "a retry or a conflicted token stored a second peering"
    );
}

#[tokio::test]
async fn a_client_token_with_no_characters_is_no_token() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "empty-token", &EDGES).await;
    let empty = json!("");

    // Two creates carrying the same EMPTY token and two gateways: were the empty string
    // taken for an idempotency token, the second would be that token asking for something
    // else and so a conflict. It is no token, so each create mints its own peering.
    let first = create_ok(
        &emulator,
        "the first create under an empty token",
        with(
            peer_request(&core_network, &gateway(EDGES[0], 1)),
            "ClientToken",
            empty.clone(),
        ),
    )
    .await;
    let second = create_ok(
        &emulator,
        "a second gateway under the same empty token",
        with(
            peer_request(&core_network, &gateway(EDGES[1], 2)),
            "ClientToken",
            empty.clone(),
        ),
    )
    .await;
    assert_ne!(
        first, second,
        "an empty client token must not make the second create a retry, got {first} twice"
    );

    // Nor does it make a repeat of the first request a retry: with no token to resolve it
    // against, the rule that one gateway is peered once refuses it.
    create_refused(
        &emulator,
        "the first request repeated under the same empty token",
        with(
            peer_request(&core_network, &gateway(EDGES[0], 1)),
            "ClientToken",
            empty,
        ),
        409,
        "ConflictException",
    )
    .await;

    assert_eq!(
        peering_count(&emulator, &core_network).await,
        2,
        "the refused repeat stored a third peering"
    );
}

#[tokio::test]
async fn the_tags_a_create_carries_obey_the_published_tag_rules() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "create-tags", &EDGES).await;

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
    ];
    let accepted = at_the_bound.len();
    for (index, (case, tags)) in at_the_bound.into_iter().enumerate() {
        let id = create_ok(
            &emulator,
            case,
            with(
                peer_request(&core_network, &gateway(EDGES[index % EDGES.len()], index + 1)),
                "Tags",
                tags.clone(),
            ),
        )
        .await;
        let (status, listed) = list_tags(&emulator, &expected_arn(PEERING, &id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, tags,
            "case {case:?}: expected the tags the create carried, got {listed}"
        );
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
    for (index, (case, tags, reason)) in refusals.into_iter().enumerate() {
        let refusal = create_refused(
            &emulator,
            case,
            with(
                peer_request(&core_network, &gateway(EDGES[0], 100 + index)),
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
                Some("Tags"),
                "case {case:?}: the refusal must name the member at fault, got {refusal}"
            );
        }
    }

    // No refused create stored a peering: the inventory is exactly the accepted rows.
    assert_eq!(
        peering_count(&emulator, &core_network).await,
        accepted,
        "a create refused over its tags stored a peering"
    );
}
