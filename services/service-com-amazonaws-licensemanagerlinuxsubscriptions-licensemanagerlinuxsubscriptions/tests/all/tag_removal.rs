// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UntagResource` (`DELETE /tags/{resourceArn}?tagKeys=…`), the
//! rule it follows and the effect a refused call has on the store.
//!
//! Every case drives a real request through the shared harness's `dispatch`, so what is
//! asserted is the wire behaviour an SDK client sees, and the removal is always read back
//! through the implemented `ListTagsForResource`.
//!
//! The rule: a removal deletes exactly the keys the request names from the tag set the
//! addressed resource holds in state and leaves every key it does not name as stored — a
//! key the resource does not carry is nothing to remove rather than a failure, so the
//! operation is `@idempotent`
//! (`smithy_query(query="operation",operation="UntagResource")`) — and the ARN is resolved
//! first, so a removal addressed to an ARN naming no resource of this account changes
//! nothing at all.
//!
//! The table spans the class the rule generalises over rather than one recorded removal:
//! how many of the stored keys the request names (one, several, all, none of them), keys
//! the resource does not carry (alone, mixed with stored ones, named twice, the
//! empty-string key, a key that is a strict PREFIX of a stored one), the key's own form
//! (non-ASCII, URI-significant) and the COUNT — the modelled `tagKeys` maximum of 50
//! (`smithy_query`: `tagKeys` is `len 0..50`) and one below it.
//!
//! `error_taxonomy.rs` owns the wire codes and texts the tag APIs answer over a wider set
//! of ARN classes; what this suite adds there is what the STORE looks like afterwards.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::harness::{
    deregister, get_provider, harness, provider_arn_for, reported_tags_of, seed_provider,
    stored_provider_count, tag, untag, Harness, Reply, ACCOUNT, ACTIVE, RED_HAT, REGION,
};

/// The largest number of keys one removal may name: the modelled `tagKeys` list is
/// `len 0..50` (`smithy_query(query="operation",operation="UntagResource")`), which is
/// also the largest tag set a resource may carry.
const MAX_TAG_KEYS: usize = 50;

/// The wire error code the tag APIs answer for a `resourceArn` addressing no resource of
/// this account, with the HTTP status it carries: 404, because the refusal reports an
/// ABSENT resource (`crate::scaffolded::NOT_FOUND_STATUS`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern, distinct per index.
fn secret_arn(index: usize) -> String {
    format!("arn:aws:secretsmanager:us-west-2:{ACCOUNT}:secret:rhel-token-{index}")
}

/// The provider id of row `index`, in the lowercase-hex UUID shape the service mints.
fn provider_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{:012x}", index + 1)
}

/// Owned `(key, value)` rows, so a table can hold computed pairs beside written-out ones.
fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct generated pairs, for the rows that exercise a key COUNT.
fn generated_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// Owned keys, so a table can hold computed keys beside written-out ones.
fn owned(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|key| (*key).to_string()).collect()
}

/// The keys of `rows`, which is what a removal of every stored key names.
fn keys_of(rows: &[(String, String)]) -> Vec<String> {
    rows.iter().map(|(key, _)| key.clone()).collect()
}

/// Seed one provider carrying `tags` through the harness's store seam, and answer its ARN.
fn seed(h: &Harness, index: usize, tags: &[(String, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    seed_provider(h, &provider_id(index), &secret_arn(index), &borrowed)
}

/// Remove `keys` from `arn`, passing them as the `&[&str]` the shared helper takes.
async fn remove(h: &Harness, arn: &str, keys: &[String]) -> Reply {
    let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
    untag(h, arn, &borrowed).await
}

/// The expected set, as the same sorted map.
fn expected(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter().cloned().collect()
}

/// A JSON tag map from `(key, value)` pairs, for a write.
fn tag_map(rows: &[(&str, &str)]) -> Value {
    Value::Object(
        rows.iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<Map<String, Value>>(),
    )
}

/// One row: what the resource already carries, which keys the removal names, and the set
/// the resource is left with.
struct Row {
    case: String,
    stored: Vec<(String, String)>,
    removed: Vec<String>,
    want: Vec<(String, String)>,
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "one key of several".to_string(),
            stored: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
            removed: owned(&["owner"]),
            want: pairs(&[("env", "prod"), ("cost-centre", "42")]),
        },
        Row {
            case: "several keys at once".to_string(),
            stored: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
            removed: owned(&["env", "cost-centre"]),
            want: pairs(&[("owner", "platform")]),
        },
        Row {
            case: "every stored key leaves the empty set".to_string(),
            stored: pairs(&[("env", "prod"), ("owner", "platform")]),
            removed: owned(&["env", "owner"]),
            want: Vec::new(),
        },
        Row {
            case: "a key the resource does not carry is a no-op".to_string(),
            stored: pairs(&[("env", "prod")]),
            removed: owned(&["owner"]),
            want: pairs(&[("env", "prod")]),
        },
        Row {
            case: "stored and unstored keys in one request".to_string(),
            stored: pairs(&[("env", "prod"), ("owner", "platform")]),
            removed: owned(&["env", "absent"]),
            want: pairs(&[("owner", "platform")]),
        },
        Row {
            case: "the same key named twice".to_string(),
            stored: pairs(&[("env", "prod"), ("owner", "platform")]),
            removed: owned(&["env", "env"]),
            want: pairs(&[("owner", "platform")]),
        },
        Row {
            case: "the empty-string key, which nothing carries".to_string(),
            stored: pairs(&[("env", "prod")]),
            removed: owned(&[""]),
            want: pairs(&[("env", "prod")]),
        },
        Row {
            case: "a key whose stored value is empty".to_string(),
            stored: pairs(&[("env", ""), ("owner", "platform")]),
            removed: owned(&["env"]),
            want: pairs(&[("owner", "platform")]),
        },
        Row {
            case: "non-ASCII keys".to_string(),
            stored: pairs(&[
                ("propriétaire", "équipe-plate-forme"),
                ("团队", "平台"),
                ("env", "prod"),
            ]),
            removed: owned(&["propriétaire", "团队"]),
            want: pairs(&[("env", "prod")]),
        },
        Row {
            case: "URI-significant keys".to_string(),
            stored: pairs(&[("a/b:c?d=e&f", "1"), ("plus+space key", "2"), ("keep", "3")]),
            removed: owned(&["a/b:c?d=e&f", "plus+space key"]),
            want: pairs(&[("keep", "3")]),
        },
        Row {
            case: "a key that is a prefix of a stored key removes neither".to_string(),
            stored: pairs(&[("environment", "prod")]),
            removed: owned(&["env"]),
            want: pairs(&[("environment", "prod")]),
        },
    ];
    // The modelled maximum: 50 keys is the largest removal one call may name, and it
    // empties a resource carrying exactly those 50.
    let at_max = generated_pairs(MAX_TAG_KEYS);
    rows.push(Row {
        case: format!("the modelled maximum of {MAX_TAG_KEYS} keys"),
        removed: keys_of(&at_max),
        stored: at_max,
        want: Vec::new(),
    });
    // One below the maximum: the largest removal that still leaves something behind.
    let mut all_but_one = generated_pairs(MAX_TAG_KEYS);
    let kept = all_but_one.pop().expect("the generated set is not empty");
    rows.push(Row {
        case: format!(
            "{} of {MAX_TAG_KEYS} keys leaves the last one",
            MAX_TAG_KEYS - 1
        ),
        stored: generated_pairs(MAX_TAG_KEYS),
        removed: keys_of(&all_but_one),
        want: vec![kept],
    });
    rows
}

#[tokio::test]
async fn a_removal_deletes_exactly_the_named_keys_and_nothing_else() {
    let h = harness();
    let mut resources = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case.clone();
        let arn = seed(&h, index, &row.stored);

        remove(&h, &arn, &row.removed).await.ok(&case);
        let got = reported_tags_of(&h, &arn, &case).await;
        assert_eq!(
            got,
            expected(&row.want),
            "case {case:?}: expected the set after removal {:?}, got {got:?}",
            expected(&row.want)
        );

        // `@idempotent`: a key that is no longer there is nothing to remove, so the same
        // removal again answers 200 and leaves the same set.
        remove(&h, &arn, &row.removed).await.ok(&case);
        let again = reported_tags_of(&h, &arn, &case).await;
        assert_eq!(
            again, got,
            "case {case:?}: a repeated removal changed the set to {again:?}"
        );

        resources.push((case, arn, row.want));
    }

    // Per resource: every earlier row still reports its OWN set after every later
    // removal, so a removal addressed to one provider touches no other.
    for (case, arn, want) in &resources {
        let got = reported_tags_of(&h, arn, case).await;
        assert_eq!(
            got,
            expected(want),
            "case {case:?}: after the other rows' removals, {arn} reports {got:?}"
        );
    }
}

#[tokio::test]
async fn a_removed_key_can_be_written_again_and_the_record_is_otherwise_untouched() {
    let h = harness();
    let case = "remove the only key, then write it again";
    let arn = seed(&h, 0, &pairs(&[("env", "prod")]));

    // A removed key is gone from what the reader reports...
    remove(&h, &arn, &owned(&["env"])).await.ok(case);
    let after_removal = reported_tags_of(&h, &arn, case).await;
    assert_eq!(
        after_removal,
        BTreeMap::new(),
        "case {case:?}: expected the empty set after removing the only key, got {after_removal:?}"
    );

    // ...and a later write of the same key stores the new value, rather than the removal
    // leaving a tombstone the write cannot pass.
    tag(
        &h,
        &arn,
        &tag_map(&[("env", "dev"), ("owner", "platform")]),
    )
    .await
    .ok(case);
    let after_rewrite = reported_tags_of(&h, &arn, case).await;
    let want = expected(&pairs(&[("env", "dev"), ("owner", "platform")]));
    assert_eq!(
        after_rewrite, want,
        "case {case:?}: expected {want:?} after re-writing the removed key, got {after_rewrite:?}"
    );

    // The removal touched the tag set and nothing else: the rest of the record still
    // reads back as it was stored.
    let provider = get_provider(&h, &arn).await;
    let body = provider.ok(case).clone();
    for (member, value) in [
        ("SubscriptionProviderArn", arn.as_str()),
        ("SubscriptionProviderSource", RED_HAT),
        ("SecretArn", secret_arn(0).as_str()),
        ("SubscriptionProviderStatus", ACTIVE),
    ] {
        assert_eq!(
            body.get(member).and_then(Value::as_str),
            Some(value),
            "case {case:?}: expected {member} {value:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_removal_leaves_every_stored_set_as_it_was() {
    let h = harness();
    let stored = pairs(&[("env", "prod"), ("owner", "platform")]);
    let live = seed(&h, 0, &stored);
    // A provider whose record existed and is gone: its ARN is well-formed and addresses
    // nothing.
    let gone = seed(&h, 1, &pairs(&[("env", "prod")]));
    deregister(&h, &gone).await.ok("deregister the second provider");

    // The ARN classes whose refusal could still have touched the store: one this account
    // never held, one whose record is gone, and one differing from the LIVE resource's
    // ARN only in the region segment — the case a lookup keyed on anything less than the
    // whole ARN would answer from the live record. (An ARN differing only in the ACCOUNT
    // segment is denied before it is resolved, so it is a row of
    // `tag_plane_authorization.rs` rather than of this rule.)
    let unknown = [
        (
            "an id nothing was ever registered under",
            provider_arn_for("66666666-6666-4666-8666-666666666666"),
        ),
        ("a deregistered provider", gone),
        (
            "the live provider's id in another region",
            live.replace(REGION, "eu-west-1"),
        ),
    ];

    let (status, code) = NOT_FOUND;
    for (label, arn) in &unknown {
        let case = format!("UntagResource on {label}");
        // The refusal does not depend on whether the named keys exist anywhere: the ARN
        // is resolved first, so a key named by a refused removal survives it.
        remove(&h, arn, &owned(&["env", "absent"]))
            .await
            .error(&case, status, code);
    }

    // The refused removals removed nothing.
    let got = reported_tags_of(&h, &live, "the live provider").await;
    assert_eq!(
        got,
        expected(&stored),
        "expected the live provider's stored tags {:?}, got {got:?}",
        expected(&stored)
    );
    assert_eq!(
        stored_provider_count(&h),
        1,
        "expected only the live provider in the store, got {} record(s)",
        stored_provider_count(&h)
    );
}
