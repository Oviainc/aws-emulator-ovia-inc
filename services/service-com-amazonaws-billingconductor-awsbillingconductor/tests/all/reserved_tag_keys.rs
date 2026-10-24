// SPDX-License-Identifier: CC0-1.0

//! The reserved `aws:` tag-key namespace, over every path of this service that takes a
//! caller-supplied tag key, driven end to end over the real `restJson1` wire.
//!
//! RULE under test: the `aws:` prefix names the tags Amazon Web Services generates
//! itself, which a caller may neither create, nor edit, nor delete — so a request
//! carrying such a key is refused with the modelled `ValidationException` (HTTP 400,
//! reason `FIELD_VALIDATION_FAILED`, the input member that carried the key in `Fields`)
//! and the store is left exactly as it was. The prefix is reserved however it is cased,
//! and reserved only as a PREFIX: a key that merely resembles or contains it is an
//! ordinary key this service stores.
//!
//! The model cannot express the rule — `TagKey` carries only `@length(1..128)` and no
//! pattern (`smithy_query(query="shape",shape="TagKey")`) — so the boundary walk admits
//! such a key and each handler refuses it.
//!
//! The rows differ along: the operation (`TagResource`, `UntagResource`, and all four
//! creates that persist `Tags`), the letter case of the prefix, what follows it (nothing,
//! an ASCII tail, a multi-byte tail), how many reserved keys one request carries, whether
//! a legal pair travels beside the reserved one, whether the addressed resource exists,
//! and — for the accepted rows — the near-miss spellings (`aws` alone, `aws-`, `awsx`,
//! the prefix away from the start, a space before it).

use crate::tagging_surface::{error_code, expected, tag_map, Emulator, TAGGING_STATUS};
use serde_json::{json, Value};

/// The status the modelled `ValidationException` is served on
/// (`smithy_query(query="shape",shape="ValidationException")`: `httpError` 400).
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` this service answers a failed field constraint with
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")` enumerates it).
const VALIDATION_REASON: &str = "FIELD_VALIDATION_FAILED";

/// Assert one refusal: the modelled 400 `ValidationException`, the enumerated reason,
/// `member` named in `Fields`, and a message that names every offending key.
fn assert_reserved_refusal(case: &str, member: &str, offenders: &[&str], answered: &(u16, Value)) {
    let (status, body) = answered;
    assert_eq!(
        *status, VALIDATION_STATUS,
        "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(VALIDATION_REASON),
        "case {case:?}: expected the reason {VALIDATION_REASON:?}, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(member),
        "case {case:?}: expected the refusal to name the member {member:?}, got {body}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: the model marks Message @required, got {body}"
    );
    for offender in offenders {
        assert!(
            message.contains(offender),
            "case {case:?}: expected the message to name the reserved key {offender:?}, \
             got {message:?}"
        );
    }
}

/// RULE, on `TagResource`: a key in the reserved namespace is refused and NONE of the
/// request's pairs is stored — not even the legal ones travelling beside it.
///
/// The rows differ along the letter case of the prefix, what follows it (nothing, an
/// ASCII tail, a multi-byte tail), how many reserved keys the request carries, and
/// whether a legal pair travels with them.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_stores_none_of_the_request() {
    let em = Emulator::new();
    let plan = em
        .create_plan("ReservedTagsPlan", Some(tag_map(&[("team", "finance")])))
        .await;

    // (case, the Tags map sent, the keys the refusal must name)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "the reserved prefix with an ASCII tail",
            tag_map(&[("aws:cost-center", "reserved")]),
            vec!["aws:cost-center"],
        ),
        (
            "the reserved prefix alone",
            tag_map(&[("aws:", "reserved")]),
            vec!["aws:"],
        ),
        (
            "the prefix in upper case",
            tag_map(&[("AWS:owner", "reserved")]),
            vec!["AWS:owner"],
        ),
        (
            "the prefix in mixed case",
            tag_map(&[("Aws:Created-By", "reserved")]),
            vec!["Aws:Created-By"],
        ),
        (
            "a multi-byte tail after the prefix",
            tag_map(&[("aws:коды", "reserved")]),
            vec!["aws:коды"],
        ),
        (
            "a reserved key beside a legal one that would overwrite a stored pair",
            tag_map(&[("aws:cost-center", "reserved"), ("team", "overwritten")]),
            vec!["aws:cost-center"],
        ),
        (
            "two reserved keys in one request",
            tag_map(&[("aws:a", "1"), ("AWS:b", "2")]),
            vec!["aws:a", "AWS:b"],
        ),
    ];

    for (case, tags, offenders) in &rows {
        let answered = em.tag(&plan, tags).await;
        assert_reserved_refusal(case, "Tags", offenders, &answered);

        // The resource keeps exactly the tags its create persisted: the refusal stored
        // nothing, so the legal pair of a mixed request did not land either.
        let stored = em.stored_tags(case, &plan).await;
        assert_eq!(
            stored,
            expected(&[("team", "finance")]),
            "case {case:?}: expected the stored tags to be unchanged, got {stored:?}"
        );
    }

    // The key is refused from the REQUEST alone, so it does not matter whether the ARN
    // resolves: an unknown resource with a reserved key is the same 400, not the 404 the
    // tagging surface answers an unknown resource with.
    let missing = format!(
        "arn:aws:billingconductor::{}:pricingplan/MISSINGPLN",
        ::engine::emulator_account_id()
    );
    let answered = em.tag(&missing, &tag_map(&[("aws:owner", "x")])).await;
    assert_reserved_refusal(
        "a reserved key on an ARN naming no resource",
        "Tags",
        &["aws:owner"],
        &answered,
    );
}

/// RULE, the other half: only the PREFIX is reserved, so a key that resembles it, or
/// carries it anywhere but the start, is an ordinary key `TagResource` stores and
/// `ListTagsForResource` reads back.
#[tokio::test]
async fn tag_resource_stores_a_key_that_only_resembles_the_reserved_prefix() {
    let em = Emulator::new();
    let plan = em.create_plan("ResemblingKeysPlan", None).await;

    // Each row is a key the reservation does NOT cover: the prefix is `aws:`, with the
    // colon, at the start of the key.
    let accepted: [(&str, &str); 6] = [
        ("the bare word, with no colon", "aws"),
        ("the word with another separator", "aws-cost-center"),
        ("a longer word starting with the letters", "awsx"),
        ("the letters in upper case, no colon", "AWSOME"),
        ("the prefix away from the start", "team:aws:owner"),
        ("a space before the prefix", " aws:owner"),
    ];

    let mut stored_so_far: Vec<(&str, &str)> = Vec::new();
    for (case, key) in &accepted {
        let answered = em.tag(&plan, &tag_map(&[(key, "ok")])).await;
        let (status, body) = &answered;
        assert_eq!(
            *status, TAGGING_STATUS,
            "case {case:?}: TagResource with the key {key:?} expected {TAGGING_STATUS}, \
             got {status} with {body}"
        );
        stored_so_far.push((key, "ok"));
        let stored = em.stored_tags(case, &plan).await;
        assert_eq!(
            stored,
            expected(&stored_so_far),
            "case {case:?}: expected the key {key:?} to be stored, got {stored:?}"
        );
    }
}

/// RULE, on `UntagResource`: a caller may not DELETE a tag in the reserved namespace
/// either, so a request naming one is refused and removes nothing — including the legal
/// keys it names beside it.
///
/// The rows differ along the letter case of the prefix, what follows it, how many
/// reserved keys the request names, and whether a legal key travels with them; the last
/// row shows the refusal is specific by removing a legal key afterwards.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let em = Emulator::new();
    let plan = em
        .create_plan(
            "ReservedUntagPlan",
            Some(tag_map(&[("team", "finance"), ("env", "prod")])),
        )
        .await;

    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one reserved key",
            vec!["aws:cost-center"],
            vec!["aws:cost-center"],
        ),
        ("the reserved prefix alone", vec!["aws:"], vec!["aws:"]),
        (
            "the prefix in upper case",
            vec!["AWS:owner"],
            vec!["AWS:owner"],
        ),
        (
            "a reserved key beside a key the resource carries",
            vec!["env", "aws:cost-center"],
            vec!["aws:cost-center"],
        ),
        (
            "two reserved keys in one request",
            vec!["aws:a", "Aws:b"],
            vec!["aws:a", "Aws:b"],
        ),
    ];

    for (case, keys, offenders) in &rows {
        let answered = em.untag(&plan, keys).await;
        assert_reserved_refusal(case, "TagKeys", offenders, &answered);

        let stored = em.stored_tags(case, &plan).await;
        assert_eq!(
            stored,
            expected(&[("env", "prod"), ("team", "finance")]),
            "case {case:?}: expected the stored tags to be unchanged, got {stored:?}"
        );
    }

    // Only the reserved namespace is out of reach: an ordinary key is still removed.
    let (status, body) = em.untag(&plan, &["env"]).await;
    assert_eq!(
        status, TAGGING_STATUS,
        "UntagResource on an ordinary key expected {TAGGING_STATUS}, got {status} with {body}"
    );
    let stored = em.stored_tags("an ordinary key is removed", &plan).await;
    assert_eq!(
        stored,
        expected(&[("team", "finance")]),
        "expected the ordinary key to be removed, got {stored:?}"
    );
}

/// RULE, on the create paths: `Tags` at create time is the same caller-supplied tag map,
/// so a reserved key is refused there too and NOTHING is minted — proved by the create
/// that follows: it carries the SAME name (no conflict, so the refused create stored no
/// resource) and reads back ONLY its legal pair through `ListTagsForResource`.
///
/// The rows are all four operations of this service that persist `Tags`.
#[tokio::test]
async fn every_create_that_persists_tags_refuses_a_reserved_key_and_mints_nothing() {
    let em = Emulator::new();
    // Setup for the custom-line-item row, created with no tags of its own.
    let host_plan = em.create_plan("CreateReservedHostPlan", None).await;
    let host_group = em
        .create_group("CreateReservedHostGroup", &host_plan, None)
        .await;

    // (case, path, the request body WITHOUT its Tags member)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "CreatePricingPlan",
            "/create-pricing-plan",
            json!({ "Name": "ReservedCreatePlan" }),
        ),
        (
            "CreateBillingGroup",
            "/create-billing-group",
            json!({
                "Name": "ReservedCreateGroup",
                // A different linked account from the setup group's: an account belongs
                // to at most one billing group.
                "AccountGrouping": { "LinkedAccountIds": ["444455556666"] },
                "ComputationPreference": { "PricingPlanArn": host_plan.clone() },
                "PrimaryAccountId": "444455556666",
            }),
        ),
        (
            "CreatePricingRule",
            "/create-pricing-rule",
            json!({
                "Name": "ReservedCreateRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
        ),
        (
            "CreateCustomLineItem",
            "/create-custom-line-item",
            json!({
                "Name": "ReservedCreateItem",
                "Description": "the reserved-key custom line item",
                "BillingGroupArn": host_group.clone(),
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            }),
        ),
    ];

    for (case, path, body) in &rows {
        let mut refused = body.clone();
        refused["Tags"] = tag_map(&[("aws:cost-center", "reserved"), ("team", "ok")]);
        let answered = em.call("POST", path, Some(&refused)).await;
        assert_reserved_refusal(case, "Tags", &["aws:cost-center"], &answered);

        // The same name is still free, which it would not be had the refused create
        // stored a resource.
        let mut accepted = body.clone();
        accepted["Tags"] = tag_map(&[("team", "ok")]);
        let (status, created) = em.call("POST", path, Some(&accepted)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create after the refusal expected 200, got {status} \
             with {created}"
        );
        let arn = created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));

        // Round trip: the only pair that reached the store is the legal one.
        let stored = em.stored_tags(case, arn).await;
        assert_eq!(
            stored,
            expected(&[("team", "ok")]),
            "case {case:?}: expected only the legal pair to be stored, got {stored:?}"
        );
    }
}
