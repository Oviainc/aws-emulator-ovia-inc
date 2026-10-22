// SPDX-License-Identifier: CC0-1.0

//! The removal side of resource tagging: `UntagResource`, end to end over the wire.
//!
//! The `TagResource` / `ListTagsForResource` rules live in the sibling suite
//! `tests/all/resource_tagging.rs`, whose harness and helpers this one reuses — so both
//! suites drive the same request path (`MultiServiceRouter::dispatch` with `awsJson1_0`
//! bodies, `POST /` and `X-Amz-Target: PartnerCentralBenefitsService.<Operation>`) and
//! read the same state back through `ListTagsForResource`.
//!
//! The rules under test:
//!
//! * `UntagResource` removes the keys its `tagKeys` names from the tag set stored on the
//!   resource its `resourceArn` names, and nothing else: a key the resource does not
//!   carry leaves the set as it was, a key it does carry is gone, every key the request
//!   did not name keeps its value, and a second identical call changes nothing.
//! * a `resourceArn` naming no resource the service holds is refused
//!   `ResourceNotFoundException` (404) with the service's catalog-lookup message for the
//!   `Catalog` the request named, which the tagging operations never name — the recorded
//!   answer this issue was filed about.
//! * a `tagKeys` list outside the modelled `TagKeyList` bounds is a 400
//!   `ValidationException`, never a 404 and never a success.
//! * a tag set is resource metadata: a removal moves neither `Revision` nor `UpdatedAt`,
//!   the same rule `TagResource` follows.
//!
//! The tests are tables whose rows differ along what the rules generalise over: how many
//! keys are removed (one, several, all, the modelled minimum of 1 and maximum of 50, and
//! one past each bound), whether a named key is present, the order the keys are named in,
//! duplicates among them, the form of the key (every character class the modelled
//! `TagKey` pattern admits, and its maximum length), the tag value (including the empty
//! one the modelled `TagValue` length admits), the catalog the resource lives in (every
//! entry of `SEEDED_CATALOGS`), and the resource an ARN names. The recorded call — an
//! `UntagResource` naming a benefit that does not exist, from an account with no active
//! partner benefit — is one row of
//! [`untagging_an_arn_that_names_no_resource_is_404`] and the subject of
//! [`an_unentitled_untag_is_answered_by_the_resource_lookup`].

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    catalog_not_found_message, APPLICATION_RESOURCE, BENEFIT_RESOURCE, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, Harness};
use crate::resource_tagging::{
    application_arn, benefit_id, error_message, expected_arn, filler, ok, tags_of, ACCOUNT,
    REGION,
};

/// The published bounds of the modelled `TagKeyList` (`length {min: 1, max: 50}`) and of
/// its member `TagKey` (`length {min: 1, max: 128}`), from
/// `smithy_query(query="operation",operation="UntagResource")` /
/// `smithy_query(query="shape",shape="TagKeyList")`.
const TAG_KEYS_MIN: usize = 1;
const TAG_KEYS_MAX: usize = 50;
const TAG_KEY_MAX_LEN: usize = 128;

/// One row of the removal table: the tag set the resource is created with, and the keys
/// the `UntagResource` call names.
struct Row {
    case: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
}

impl Row {
    /// The tag set that must remain: the created set minus every named key.
    fn expected(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .filter(|(key, _)| !self.remove.contains(key))
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn created(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn keys(&self) -> Vec<&'static str> {
        self.remove.clone()
    }
}

/// The rows of the removal table.
///
/// The key forms span every character class the modelled `TagKey` pattern admits
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from `smithy_query(query="shape",shape="Tag")`):
/// letters (including non-ASCII ones), separators, numbers and `_ . : / = + - @`; the
/// values include the empty string, which the modelled `TagValue` length (`max 256`, no
/// minimum) admits.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one key of several",
            tags: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3")],
            remove: vec!["Beta"],
        },
        Row {
            case: "several keys of several",
            tags: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3"), ("Delta", "4")],
            remove: vec!["Alpha", "Gamma"],
        },
        Row {
            case: "every key the resource carries",
            tags: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Beta"],
        },
        Row {
            case: "a key the resource does not carry",
            tags: vec![("Alpha", "1")],
            remove: vec!["Absent"],
        },
        Row {
            case: "a present key and an absent one",
            tags: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Absent"],
        },
        Row {
            case: "the same key named twice",
            tags: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Alpha"],
        },
        Row {
            case: "keys named in reverse order",
            tags: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3")],
            remove: vec!["Gamma", "Beta"],
        },
        Row {
            case: "keys of every modelled character class",
            tags: vec![
                ("Ünïcødé", "letters"),
                ("проект", "letters"),
                ("cost center", "separator"),
                ("a.b:c/d=e+f-g@h", "punctuation"),
                ("123", "numbers"),
                ("_kept_", "underscore"),
            ],
            remove: vec!["Ünïcødé", "cost center", "a.b:c/d=e+f-g@h", "123"],
        },
        Row {
            case: "a key whose value is the empty string",
            tags: vec![("Empty", ""), ("Kept", "")],
            remove: vec!["Empty"],
        },
        Row {
            case: "the minimum list of one key",
            tags: vec![("Only", "1")],
            remove: vec!["Only"],
        },
    ]
}

// ---------------------------------------------------------------------------
// The removal rule
// ---------------------------------------------------------------------------

/// Every row of the removal table, in every catalog the service serves: the create's tags
/// are read back, the named keys are removed, and the read-back reports exactly the
/// created set minus those keys. A second identical call reports the same set, because
/// removing a key twice is removing it once.
///
/// The expectation is computed by replaying the same set difference over a `BTreeMap`, so
/// what is pinned is the rule rather than one recorded answer; the row set for the catalog
/// is `SEEDED_CATALOGS`, so it cannot drift from the store.
#[tokio::test]
async fn untag_removes_exactly_the_named_keys() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{catalog}: {}", row.case);
            let arn = application_arn(
                &harness,
                &case,
                catalog,
                &format!("tok-untag-{catalog}-{index}"),
                &row.tags,
            )
            .await;

            // The round trip of the create, which the removal is measured against.
            assert_eq!(
                tags_of(&harness, &case, &arn).await,
                row.created(),
                "case {case:?}: the created tag set must read back unchanged"
            );

            let expected = row.expected();
            for attempt in 1..=2 {
                let case = format!("{case} (call {attempt})");
                let out = ok(
                    &harness,
                    &case,
                    "UntagResource",
                    &json!({ "resourceArn": arn, "tagKeys": row.keys() }),
                )
                .await;
                assert_eq!(
                    out,
                    json!({}),
                    "case {case:?}: UntagResource models an empty response, got {out:?}"
                );
                let got = tags_of(&harness, &case, &arn).await;
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected exactly the unnamed keys to remain after \
                     removing {:?}, got {got:?}",
                    row.remove
                );
            }
        }
    }
}

/// A removal touches ONE resource: other applications carrying the same keys, in the same
/// catalog and across catalogs, keep their tag sets.
#[tokio::test]
async fn untag_one_resource_does_not_touch_another() {
    let harness = Harness::new();
    let shared = [("shared", "1"), ("also.shared", "2")];
    let mut arns: Vec<(String, String)> = Vec::new();
    for catalog in SEEDED_CATALOGS {
        for index in 0..2 {
            let case = format!("{catalog}-{index}");
            let arn = application_arn(
                &harness,
                &case,
                catalog,
                &format!("tok-untag-scope-{catalog}-{index}"),
                &shared,
            )
            .await;
            arns.push((case, arn));
        }
    }

    let (target_case, target_arn) = arns[0].clone();
    ok(
        &harness,
        &target_case,
        "UntagResource",
        &json!({ "resourceArn": target_arn, "tagKeys": ["shared", "also.shared"] }),
    )
    .await;

    let untouched: BTreeMap<String, String> = shared
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    for (case, arn) in &arns {
        let got = tags_of(&harness, case, arn).await;
        if arn == &target_arn {
            assert!(
                got.is_empty(),
                "case {case:?}: the named resource must lose the named keys, got {got:?}"
            );
        } else {
            assert_eq!(
                got, untouched,
                "case {case:?}: a resource the request did not name must keep its tags, \
                 got {got:?}"
            );
        }
    }
}

/// A tag set is resource metadata, so a removal moves neither `Revision` nor `UpdatedAt`
/// — the rule `TagResource` follows — while the tags themselves do change.
#[tokio::test]
async fn untag_leaves_the_application_revision_and_updated_at_alone() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let arn = application_arn(
        &harness,
        "metadata",
        catalog,
        "tok-untag-metadata",
        &[("env", "test"), ("kept", "yes")],
    )
    .await;
    let read = || async {
        ok(
            &harness,
            "metadata",
            "GetBenefitApplication",
            &json!({ "Catalog": catalog, "Identifier": arn }),
        )
        .await
    };

    let before = read().await;
    ok(
        &harness,
        "metadata",
        "UntagResource",
        &json!({ "resourceArn": arn, "tagKeys": ["env"] }),
    )
    .await;
    let after = read().await;

    for member in ["Revision", "UpdatedAt", "Status"] {
        assert_eq!(
            before[member], after[member],
            "case \"metadata\": untagging must leave {member} as it was, got {:?} then {:?}",
            before[member], after[member]
        );
    }
    let got = tags_of(&harness, "metadata", &arn).await;
    assert_eq!(
        got,
        [("kept".to_string(), "yes".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"metadata\": the named key must be gone and the other kept, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// The resource lookup
// ---------------------------------------------------------------------------

/// A well-formed `resourceArn` that names no resource this service holds is a 404
/// `ResourceNotFoundException` carrying the catalog-lookup message, and nothing is
/// removed from the resource that does exist.
///
/// The rows span every way an ARN of the modelled form can fail to resolve: an id nothing
/// holds, a resource type that holds no tag set (a benefit — the model binds `Tags` on no
/// benefit shape), a catalog the service does not serve, an existing resource's ARN in
/// another region, account or partition, and a sub-resource path the modelled pattern
/// admits.
#[tokio::test]
async fn untagging_an_arn_that_names_no_resource_is_404() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let existing = application_arn(
        &harness,
        "seed",
        catalog,
        "tok-untag-lookup",
        &[("env", "test")],
    )
    .await;
    let benefit = benefit_id(&harness, "seed", catalog).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an application id nothing holds",
            expected_arn(catalog, APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            // The recorded instance of the rule.
            "a benefit that does not exist",
            expected_arn(catalog, BENEFIT_RESOURCE, "ben-00000000000000"),
        ),
        (
            "a benefit that does exist, which holds no tag set",
            expected_arn(catalog, BENEFIT_RESOURCE, &benefit),
        ),
        (
            "a catalog the service does not serve",
            expected_arn("NotACatalog", APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            "the application's ARN in another region",
            existing.replace(REGION, "eu-west-1"),
        ),
        (
            "the application's ARN under another account",
            existing.replace(ACCOUNT, "210987654321"),
        ),
        (
            "the application's ARN in another partition",
            existing.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "a sub-resource of the existing application",
            format!("{existing}/attachment/1"),
        ),
    ];

    for (name, resource_arn) in &rows {
        let case = format!("UntagResource: {name}");
        let (status, out) = harness
            .call(
                "UntagResource",
                &json!({ "resourceArn": resource_arn, "tagKeys": ["env"] }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {resource_arn:?}, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
        assert_eq!(
            error_message(&out),
            catalog_not_found_message(None),
            "case {case:?}: unexpected not-found message in {out:?}"
        );
    }

    // None of those refusals removed anything: the one real resource still carries exactly
    // the tags its create supplied.
    let got = tags_of(&harness, "after the refusals", &existing).await;
    assert_eq!(
        got,
        [("env".to_string(), "test".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"after the refusals\": a refused UntagResource must remove nothing, got {got:?}"
    );
}

/// The recorded shape of the issue's own call: the tag-MUTATING path is NOT behind the
/// partner-entitlement gate, so an account with no active partner benefit — every caller
/// from the wire — is answered by the resource lookup, not by the gate.
#[tokio::test]
async fn an_unentitled_untag_is_answered_by_the_resource_lookup() {
    let harness = Harness::unentitled();
    let resource_arn = expected_arn(SEEDED_CATALOGS[0], BENEFIT_RESOURCE, "ben-00000000000000");

    let (status, out) = harness
        .call(
            "UntagResource",
            &json!({ "resourceArn": resource_arn, "tagKeys": ["env"] }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"unentitled\": expected the lookup's 404, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ResourceNotFoundException",
        "case \"unentitled\": expected ResourceNotFoundException, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        catalog_not_found_message(None),
        "case \"unentitled\": unexpected not-found message in {out:?}"
    );
}

// ---------------------------------------------------------------------------
// The modelled bounds of `tagKeys`
// ---------------------------------------------------------------------------

/// Each published bound of the modelled `tagKeys` list and of its member, and one past
/// it: a list of 1 and of 50 keys is accepted, and an empty list, a list of 51 keys and a
/// key longer than 128 characters are refused 400 `ValidationException` — never with a
/// 404, a 501 or a success — leaving the resource's tag set exactly as it was.
#[tokio::test]
async fn untag_honours_the_modelled_tag_key_bounds() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];

    // A resource carrying one tag per key the accepted rows name, plus one the removals
    // never name, so what each bound row removed is visible.
    let owned_keys: Vec<String> = (0..TAG_KEYS_MAX).map(|n| format!("key{n:02}")).collect();
    let mut seeded: Vec<(&str, &str)> = owned_keys.iter().map(|key| (key.as_str(), "v")).collect();
    seeded.push(("kept", "v"));

    for (case, count) in [
        ("the minimum of one key", TAG_KEYS_MIN),
        ("the maximum of fifty keys", TAG_KEYS_MAX),
    ] {
        let arn = application_arn(
            &harness,
            case,
            catalog,
            &format!("tok-untag-bound-{count}"),
            &seeded,
        )
        .await;
        let named: Vec<&str> = owned_keys.iter().take(count).map(String::as_str).collect();
        ok(
            &harness,
            case,
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": named }),
        )
        .await;
        let remaining = tags_of(&harness, case, &arn).await;
        // Everything the row did not name, plus the key no row names.
        let expected = seeded.len() - count;
        assert_eq!(
            remaining.len(),
            expected,
            "case {case:?}: expected {expected} keys to remain, got {remaining:?}"
        );
        assert!(
            remaining.contains_key("kept"),
            "case {case:?}: a key the request did not name must survive, got {remaining:?}"
        );
    }

    let arn = application_arn(
        &harness,
        "bounds",
        catalog,
        "tok-untag-bound-refusals",
        &seeded,
    )
    .await;
    let too_many: Vec<String> = (0..=TAG_KEYS_MAX).map(|n| format!("extra{n:02}")).collect();
    let refusals: Vec<(&str, Value)> = vec![
        ("an empty list, under the 1..50 length", json!([])),
        ("one key past the 1..50 length", json!(too_many)),
        (
            "a key past the 1..128 TagKey length",
            json!([filler(TAG_KEY_MAX_LEN + 1)]),
        ),
        ("an empty key, under the TagKey minimum", json!([""])),
    ];

    for (case, tag_keys) in refusals {
        let (status, out) = harness
            .call(
                "UntagResource",
                &json!({ "resourceArn": arn, "tagKeys": tag_keys }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
    }

    // And a key AT the maximum length is accepted, so the refusal above is the bound and
    // not the length itself.
    let long_key = filler(TAG_KEY_MAX_LEN);
    let tagged = application_arn(
        &harness,
        "the maximum key length",
        catalog,
        "tok-untag-long-key",
        &[(long_key.as_str(), "v"), ("kept", "v")],
    )
    .await;
    ok(
        &harness,
        "the maximum key length",
        "UntagResource",
        &json!({ "resourceArn": tagged, "tagKeys": [long_key] }),
    )
    .await;
    let remaining = tags_of(&harness, "the maximum key length", &tagged).await;
    assert_eq!(
        remaining,
        [("kept".to_string(), "v".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"the maximum key length\": expected only the unnamed key to remain, \
         got {remaining:?}"
    );

    // None of the refusals touched the resource they named.
    let after = tags_of(&harness, "bounds", &arn).await;
    assert_eq!(
        after.len(),
        seeded.len(),
        "case \"bounds\": a refused request must leave the tag set alone, got {after:?}"
    );
}
