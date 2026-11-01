// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a `ListTagsForResource` reply onto a tag set, driven through
//! every operation of this service that leaves a tag set behind.
//!
//! The rule: the `tags` member of a 200 `ListTagsForResource` body reports exactly the
//! pairs the addressed resource holds in state — whichever operation put them there, and
//! whatever the keys and values look like — so reading it through the shared projection
//! (`harness::reported_tags` over a reply, `harness::reported_tags_of` over an ARN) yields
//! the same set on both forms. The suites that used to carry a private copy of that
//! projection now read it from here, which is what this suite pins: it drives the shared
//! path through an operation each of those suites exercises (`TagResource`,
//! `UntagResource`, `RegisterSubscriptionProvider`'s `Tags`, and a record placed straight
//! into the store), and asserts the two forms report the same set as each other and as the
//! set that was written.
//!
//! The rows differ along what the rule generalises over: how many pairs the set holds
//! (none, one, several, the modelled maximum) and the FORM of a key or value (empty value,
//! non-ASCII, URI-significant) — the recorded case is one row of that table, not the rule.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    harness, list_tags, provider_arn, register, reported_tags, reported_tags_of, seed_provider,
    tag, untag, Harness, ACCOUNT, RED_HAT,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::MAX_TAGS_PER_RESOURCE as MAX_TAGS;

/// One row: a label and the tag set the resource is made to carry.
struct Row {
    case: String,
    pairs: Vec<(String, String)>,
}

/// Owned `(key, value)` rows, so a table can hold computed pairs beside written-out ones.
fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct generated pairs, for the row that exercises the modelled maximum.
fn generated_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the empty set".to_string(),
            pairs: Vec::new(),
        },
        Row {
            case: "one pair".to_string(),
            pairs: pairs(&[("env", "prod")]),
        },
        Row {
            case: "several pairs".to_string(),
            pairs: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
        },
        Row {
            case: "a pair whose value is empty".to_string(),
            pairs: pairs(&[("env", ""), ("owner", "platform")]),
        },
        Row {
            case: "non-ASCII keys and values".to_string(),
            pairs: pairs(&[("propriétaire", "équipe-plate-forme"), ("团队", "平台")]),
        },
        Row {
            case: "URI-significant keys and values".to_string(),
            pairs: pairs(&[("a/b:c?d=e&f", "1/2 3"), ("plus+space key", "x%20y")]),
        },
        Row {
            // The largest set a resource may carry (`state::MAX_TAGS_PER_RESOURCE`, the
            // modelled `Tags` maximum of 50).
            case: format!("the modelled maximum of {MAX_TAGS} pairs"),
            pairs: generated_pairs(MAX_TAGS),
        },
    ]
}

/// A JSON tag map from `(key, value)` pairs, as a write or a create carries it.
fn tag_map(rows: &[(String, String)]) -> Value {
    Value::Object(
        rows.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// The expected set, as the sorted map the shared projection answers.
fn expected(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter().cloned().collect()
}

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern, distinct per index —
/// a registration naming a (source, secret) pair the account already holds resolves to
/// THAT registration, so each create names its own secret.
fn secret_arn(index: usize) -> String {
    format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:rhel-token-{index}")
}

/// The provider id of slot `index`, in the lowercase-hex UUID shape the service mints.
fn provider_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{:012x}", index + 1)
}

/// Seed one provider carrying `tags` through the harness's store seam, and answer its ARN.
fn seed(h: &Harness, index: usize, tags: &[(String, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    seed_provider(h, &provider_id(index), &secret_arn(index), &borrowed)
}

/// Read the set `arn` carries through BOTH forms of the shared projection and answer it,
/// having asserted the two agree: they are one rule stated once, so a reply observed
/// either way reports the same set.
async fn through_both_forms(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let over_arn = reported_tags_of(h, arn, case).await;
    let over_reply = reported_tags(&list_tags(h, arn).await, case);
    assert_eq!(
        over_arn, over_reply,
        "case {case:?}: the two forms of the shared projection reported {over_arn:?} and \
         {over_reply:?}"
    );
    over_arn
}

/// A record placed straight into the store — the path the suites that hold a reply
/// (`resource_tags.rs`, `shared_harness.rs`) read.
#[tokio::test]
async fn a_stored_set_is_reported_through_the_shared_projection() {
    let h = harness();
    for (index, row) in rows().into_iter().enumerate() {
        let case = format!("a seeded resource carrying {}", row.case);
        let arn = seed(&h, index, &row.pairs);
        let got = through_both_forms(&h, &arn, &case).await;
        assert_eq!(
            got,
            expected(&row.pairs),
            "case {case:?}: expected the stored set {:?}, got {got:?}",
            expected(&row.pairs)
        );
    }
}

/// `TagResource`: the path `resource_tags.rs`, `shared_harness.rs`, `shared_tag_merge.rs`
/// and `tag_selection_limit.rs` write their sets through.
#[tokio::test]
async fn a_written_set_is_reported_through_the_shared_projection() {
    let h = harness();
    for (index, row) in rows().into_iter().enumerate() {
        if row.pairs.is_empty() {
            // `TagResource` refuses a tag map naming no pair, so the empty set is not one a
            // WRITE can leave behind (`tag_plane_authorization.rs` pins that refusal); the
            // seeded form above and the removal below project it.
            continue;
        }
        let case = format!("TagResource writing {}", row.case);
        let arn = seed(&h, index, &[]);
        tag(&h, &arn, &tag_map(&row.pairs)).await.ok(&case);
        let got = through_both_forms(&h, &arn, &case).await;
        assert_eq!(
            got,
            expected(&row.pairs),
            "case {case:?}: expected the written set {:?}, got {got:?}",
            expected(&row.pairs)
        );
    }
}

/// `UntagResource`: the path `tag_removal.rs` reads its sets through. Removing every key
/// the row names leaves the empty set, which the projection reports as the empty map
/// rather than as an absent member.
#[tokio::test]
async fn a_removal_is_reported_through_the_shared_projection() {
    let h = harness();
    for (index, row) in rows().into_iter().enumerate() {
        if row.pairs.is_empty() {
            // `tagKeys` is `@required`, so a removal naming no key is a different rule.
            continue;
        }
        let case = format!("UntagResource removing every key of {}", row.case);
        let arn = seed(&h, index, &row.pairs);
        let keys: Vec<&str> = row.pairs.iter().map(|(key, _)| key.as_str()).collect();
        untag(&h, &arn, &keys).await.ok(&case);
        let got = through_both_forms(&h, &arn, &case).await;
        assert_eq!(
            got,
            BTreeMap::new(),
            "case {case:?}: expected the empty set after removing every key, got {got:?}"
        );
    }
}

/// `RegisterSubscriptionProvider`'s `Tags`: the create path `shared_tag_merge.rs` and
/// `tag_selection_limit.rs` drive, where the set reaches the store with the resource.
#[tokio::test]
async fn a_registration_s_tags_are_reported_through_the_shared_projection() {
    for (index, row) in rows().into_iter().enumerate() {
        // An emulator per row: the account holds ONE registration per source, so the set a
        // row's registration carries is the set it created the resource with only when the
        // row's call is that account's first.
        let h = harness();
        let case = format!("RegisterSubscriptionProvider carrying {}", row.case);
        let created = register(
            &h,
            &json!({
                "SubscriptionProviderSource": RED_HAT,
                "SecretArn": secret_arn(index),
                "Tags": tag_map(&row.pairs),
            }),
        )
        .await;
        let arn = provider_arn(&created, &case);
        let got = through_both_forms(&h, &arn, &case).await;
        assert_eq!(
            got,
            expected(&row.pairs),
            "case {case:?}: expected the set the registration carried {:?}, got {got:?}",
            expected(&row.pairs)
        );
    }
}
