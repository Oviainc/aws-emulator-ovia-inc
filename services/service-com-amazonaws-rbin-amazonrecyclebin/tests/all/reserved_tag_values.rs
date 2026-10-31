// SPDX-License-Identifier: CC0-1.0

//! The reserved `aws:` namespace read off the VALUE half of a tag, over every door a
//! caller names a tag of its own through.
//!
//! RULE: a tagging API refuses, with its parameter-fault error and without writing
//! anything, a request that would create or change a tag EITHER half of which — its key
//! or its value — claims the `aws:` namespace; a half that merely RESEMBLES it is the
//! caller's own and round-trips like any other. The namespace is reserved however it is
//! cased, on both halves alike.
//!
//! `tests/all/reserved_tag_keys.rs` is the same rule read off the key half, and this
//! suite shares its fixtures and its refusal assertion so the two halves cannot be
//! refused two different ways.
//!
//! The doors carrying a value of the caller's own are `CreateRule$Tags` and
//! `TagResource$Tags`; `UntagResource$TagKeys` carries keys alone, so it has no value half
//! to claim the namespace on. A rule's resource SELECTION (`ResourceTags`,
//! `ExcludeResourceTags`) is not covered: those members name tags the retained resources
//! already carry, so selecting on one creates no tag.
//!
//! The model admits a reserved value — `TagValue` is `@length(0, 256)` with pattern
//! `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which allows a colon anywhere — so the boundary
//! validator cannot refuse one and every assertion here is about what the handlers do.
//!
//! The tables span the class the rule generalises over rather than one value: the casing
//! of the prefix, the prefix alone and with a tail, a value at the modelled maximum
//! length, and — on the accepted side — the empty value the model's minimum admits, the
//! prefix without its separator, the prefix away from the start of the value, and a value
//! whose fourth byte falls inside a multi-byte character.

use serde_json::json;

use crate::reserved_tag_keys::{
    assert_reserved_refusal, assert_same_sentence, rule_count, rule_with_tags, stored_tags,
};
use crate::resource_tags::{pairs, tag_resource};
use crate::retention_rule_lifecycle::{create_rule, retention, router};

/// The reserved prefix and the member path a refused `Tags` is named by, read off the
/// handler-side constants so the suite cannot drift from what the service enforces.
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    claims_reserved_tag_namespace, RESERVED_TAG_PREFIX, TAGS_MEMBER,
};

/// `TagValue` is `@length(min: 0, max: 256)`.
/// Source: smithy_query(query="shape", shape="Tag") — member `Value`.
const MAX_TAG_VALUE_LEN: usize = 256;

/// The resource type the fixture rules retain.
const RESOURCE_TYPE: &str = "EBS_SNAPSHOT";

/// The half of a tag a refusal of this suite names.
const VALUE_HALF: &str = "value";

/// Values that CLAIM the reserved namespace, so no caller may create or change a tag
/// carrying one.
///
/// The rows differ along the casing of the prefix and what follows it: nothing, a system
/// tag's own path, and a tail that brings the value to the modelled maximum length.
fn reserved_values() -> Vec<(&'static str, String)> {
    // The prefix plus a tail that brings the value to exactly `MAX_TAG_VALUE_LEN`, the
    // longest value the model admits.
    let longest = format!(
        "{RESERVED_TAG_PREFIX}{}",
        "v".repeat(MAX_TAG_VALUE_LEN - RESERVED_TAG_PREFIX.len())
    );
    vec![
        ("a system tag's value", "aws:cloudformation:stack".to_string()),
        ("the reserved prefix alone", "aws:".to_string()),
        ("the reserved prefix in upper case", "AWS:managed".to_string()),
        ("the reserved prefix in mixed case", "Aws:Managed".to_string()),
        (
            "a reserved value at the maximum modelled value length",
            longest,
        ),
    ]
}

/// Values that only RESEMBLE the reserved namespace, so they are the caller's own.
///
/// The rows differ along WHERE and HOW the resemblance falls: the empty value the model's
/// minimum length admits, the prefix without its separator, with another separator, with
/// more letters, away from the start of the value, and a value whose fourth byte — where
/// the prefix's colon would sit — is inside a multi-byte character.
fn ordinary_values() -> Vec<(&'static str, String)> {
    vec![
        ("the empty value", String::new()),
        ("the prefix without its separator", "aws".to_string()),
        ("the prefix with more letters after it", "awsome".to_string()),
        ("the prefix with another separator", "aws-managed".to_string()),
        (
            "the prefix away from the start of the value",
            "team:aws:owner".to_string(),
        ),
        (
            "a multi-byte character where the separator would be",
            "awsλ:managed".to_string(),
        ),
        (
            "the prefix after a space the value pattern admits",
            " aws:managed".to_string(),
        ),
    ]
}

/// RULE: the classification is about the NAMESPACE a half of a tag claims, not about one
/// spelling of it and not about which half it is — so the handler-side predicate and the
/// value tables below agree on every row.
#[test]
fn the_reserved_namespace_is_claimed_by_a_value_as_it_is_by_a_key() {
    for (case, value) in reserved_values() {
        assert!(
            claims_reserved_tag_namespace(&value),
            "case {case:?}: expected the value {value:?} to claim the reserved namespace"
        );
    }
    for (case, value) in ordinary_values() {
        assert!(
            !claims_reserved_tag_namespace(&value),
            "case {case:?}: expected the value {value:?} to be the caller's own"
        );
    }
}

/// RULE: `CreateRule` refuses a reserved VALUE in `Tags` and mints no rule — the same
/// namespace rule the key half is held to, on the other half of the same member.
#[tokio::test]
async fn create_rule_refuses_a_reserved_tag_value_and_mints_nothing() {
    let router = router();
    let before = rule_count(&router, "before any refused create").await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, value) in reserved_values() {
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPE,
                "RetentionPeriod": retention(7),
                "Tags": [
                    { "Key": "env", "Value": value },
                    { "Key": "Also", "Value": "valid" },
                ],
            }),
        )
        .await;
        refusals.push((
            case,
            assert_reserved_refusal(case, TAGS_MEMBER, VALUE_HALF, status, &body),
        ));
        assert_eq!(
            rule_count(&router, case).await,
            before,
            "case {case:?}: expected the refused create to mint no rule"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: `TagResource` refuses a reserved VALUE and assigns nothing — not even the
/// well-formed tag that travelled beside it.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_value_and_assigns_nothing() {
    let router = router();
    let held = pairs(&[("env", "prod")]);
    let (_, rule_arn) = rule_with_tags(&router, "tagged beside a reserved value", &held).await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, value) in reserved_values() {
        let assigned = vec![
            ("owner".to_string(), value.clone()),
            ("Also".to_string(), "valid".to_string()),
        ];
        let (status, body) = tag_resource(&router, &rule_arn, &assigned).await;
        refusals.push((
            case,
            assert_reserved_refusal(case, TAGS_MEMBER, VALUE_HALF, status, &body),
        ));
        // Nothing of the refused call is stored: neither the reserved value nor the
        // acceptable tag beside it.
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            held,
            "case {case:?}: expected the rule's tags to be left untouched"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: `TagResource` refuses a reserved value when it would REPLACE the value of a key
/// the rule already carries, so the namespace cannot be reached by editing either.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_value_for_a_key_the_rule_already_carries() {
    let router = router();
    let held = pairs(&[("env", "prod")]);
    let (_, rule_arn) = rule_with_tags(&router, "re-tagged with a reserved value", &held).await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, value) in reserved_values() {
        let (status, body) =
            tag_resource(&router, &rule_arn, &pairs(&[("env", value.as_str())])).await;
        refusals.push((
            case,
            assert_reserved_refusal(case, TAGS_MEMBER, VALUE_HALF, status, &body),
        ));
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            held,
            "case {case:?}: expected the stored value to be left as it was"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: a tag claiming the namespace on BOTH halves is refused by its KEY — the half a
/// caller names a tag by — so one tag is never reported as two faults.
#[tokio::test]
async fn a_tag_reserved_on_both_halves_is_refused_by_its_key() {
    let router = router();
    let (status, body) = create_rule(
        &router,
        json!({
            "ResourceType": RESOURCE_TYPE,
            "RetentionPeriod": retention(7),
            "Tags": [{ "Key": "aws:team", "Value": "aws:managed" }],
        }),
    )
    .await;
    assert_reserved_refusal(
        "both halves reserved",
        TAGS_MEMBER,
        "key",
        status,
        &body,
    );
}

/// RULE: a value that only RESEMBLES the reserved namespace is the caller's own, so it is
/// created, read back and changed like any other — through both doors that carry a value.
#[tokio::test]
async fn a_value_that_only_resembles_the_reserved_prefix_round_trips() {
    let router = router();

    for (case, value) in ordinary_values() {
        // Assigned at CreateRule time, and reported by the tag reader.
        let created = vec![("env".to_string(), value.clone())];
        let (_, rule_arn) = rule_with_tags(&router, case, &created).await;
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            created,
            "case {case:?}: expected CreateRule to assign the value"
        );

        // And changed by TagResource, which upserts it onto another key.
        let (status, body) =
            tag_resource(&router, &rule_arn, &pairs(&[("owner", value.as_str())])).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected TagResource to accept the value {value:?}, \
             got {status} / {body:?}"
        );
        let mut expected = created.clone();
        expected.push(("owner".to_string(), value.clone()));
        expected.sort();
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            expected,
            "case {case:?}: expected both tags to carry the value"
        );
    }
}
