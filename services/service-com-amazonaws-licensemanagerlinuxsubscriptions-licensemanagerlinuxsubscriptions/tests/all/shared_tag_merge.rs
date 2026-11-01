// SPDX-License-Identifier: CC0-1.0

//! The ONE tag-merge rule, driven through BOTH operations that merge a tag map into a
//! resource's stored set: `TagResource` (`PUT /tags/{resourceArn}`) and the `@idempotent`
//! repeat path of `RegisterSubscriptionProvider` (a registration naming a (source, secret)
//! pair the account already holds).
//!
//! The rule: the pairs a request carries are merged into the set the addressed resource
//! already carries — a key the request names takes the request's value, a key it does not
//! name keeps the value it had — and the write is refused, with nothing stored, when the
//! RESULTANT set would be larger than [`MAX_TAGS`] (so a key already stored is an
//! overwrite that does not grow the set, while a key the resource does not carry does).
//!
//! Both operations are driven with the SAME stored set and the SAME requested map in each
//! row, and the two are asserted to answer the same thing and leave the same set behind:
//! that is what pins the rule to one implementation rather than to one operation's copy of
//! it.
//!
//! The rows differ along what the rule generalises over: how many pairs are stored, how
//! many of the requested keys are already stored (overwrites) and how many are new, taking
//! the resultant set below the bound, exactly to it, and past it by one and by more.
//!
//! A request whose map names NO pair is not a row: `TagResource` refuses it as invalid input
//! before it resolves the resource (`tag_plane_authorization.rs`), so it is not a merge the
//! two paths share.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    harness, provider_arn, register, reported_tags_of, seed_provider, tag, Reply, RED_HAT,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::MAX_TAGS_PER_RESOURCE as MAX_TAGS;

/// The HTTP status, wire error code and message a write past [`MAX_TAGS`] is refused with,
/// whichever operation carried the tag map.
const TAG_LIMIT: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "The resultant tag set must not have more than 50 user tags.",
);

/// One row: the set the resource already carries, and the map the request names — some of
/// whose keys are already stored (`overwrites`) and some of which are new (`added`).
struct Row {
    label: &'static str,
    stored: usize,
    overwrites: usize,
    added: usize,
    accepted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one new key onto an empty set",
            stored: 0,
            overwrites: 0,
            added: 1,
            accepted: true,
        },
        Row {
            label: "new keys onto an empty set",
            stored: 0,
            overwrites: 0,
            added: 3,
            accepted: true,
        },
        Row {
            label: "half the bound plus the other half",
            stored: MAX_TAGS / 2,
            overwrites: 0,
            added: MAX_TAGS / 2,
            accepted: true,
        },
        Row {
            label: "one below the bound plus one new key",
            stored: MAX_TAGS - 1,
            overwrites: 0,
            added: 1,
            accepted: true,
        },
        Row {
            label: "a set at the bound taking overwrites only",
            stored: MAX_TAGS,
            overwrites: 5,
            added: 0,
            accepted: true,
        },
        Row {
            label: "a set at the bound taking one new key",
            stored: MAX_TAGS,
            overwrites: 0,
            added: 1,
            accepted: false,
        },
        Row {
            label: "one below the bound plus two new keys",
            stored: MAX_TAGS - 1,
            overwrites: 0,
            added: 2,
            accepted: false,
        },
        Row {
            label: "overwrites do not make room for the new keys",
            stored: MAX_TAGS - 1,
            overwrites: 3,
            added: 2,
            accepted: false,
        },
        Row {
            label: "well past the bound",
            stored: MAX_TAGS - 5,
            overwrites: 0,
            added: 10,
            accepted: false,
        },
    ]
}

/// The `count` pairs a row's resource starts out carrying.
fn stored_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// The map a row's request names: `overwrites` keys the resource already carries, under
/// NEW values, plus `added` keys it does not carry.
fn requested_pairs(overwrites: usize, added: usize) -> Vec<(String, String)> {
    (0..overwrites)
        .map(|index| (format!("key-{index:02}"), format!("rewritten-{index:02}")))
        .chain(
            (0..added).map(|index| (format!("added-{index:02}"), format!("value-added-{index:02}"))),
        )
        .collect()
}

/// A JSON tag map from `(key, value)` pairs.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// The set the resource is left with: the union when the write is accepted (the requested
/// value winning for every key the request names), and the stored set untouched when it is
/// refused.
fn expected_set(row: &Row) -> BTreeMap<String, String> {
    let mut want: BTreeMap<String, String> = stored_pairs(row.stored).into_iter().collect();
    if row.accepted {
        want.extend(requested_pairs(row.overwrites, row.added));
    }
    want
}

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern.
fn secret_arn() -> String {
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token-shared".to_string()
}

/// Drive the row's merge through `TagResource` on a resource already carrying its stored
/// set, and answer what the call said and what the resource carries afterwards.
async fn through_tag_resource(row: &Row, case: &str) -> (Reply, BTreeMap<String, String>) {
    let h = harness();
    let stored = stored_pairs(row.stored);
    let stored_refs: Vec<(&str, &str)> = stored
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let arn = seed_provider(
        &h,
        "00000000-0000-4000-8000-000000000001",
        &secret_arn(),
        &stored_refs,
    );
    let reply = tag(&h, &arn, &tag_map(&requested_pairs(row.overwrites, row.added))).await;
    let carried = reported_tags_of(&h, &arn, case).await;
    (reply, carried)
}

/// Drive the SAME merge through the `@idempotent` repeat of `RegisterSubscriptionProvider`:
/// the registration is created carrying the row's stored set, then repeated naming the same
/// (source, secret) pair with the row's requested map.
async fn through_registration_repeat(row: &Row, case: &str) -> (Reply, BTreeMap<String, String>) {
    let h = harness();
    let created = register(
        &h,
        &json!({
            "SubscriptionProviderSource": RED_HAT,
            "SecretArn": secret_arn(),
            "Tags": tag_map(&stored_pairs(row.stored)),
        }),
    )
    .await;
    let arn = provider_arn(&created, case);
    let reply = register(
        &h,
        &json!({
            "SubscriptionProviderSource": RED_HAT,
            "SecretArn": secret_arn(),
            "Tags": tag_map(&requested_pairs(row.overwrites, row.added)),
        }),
    )
    .await;
    let carried = reported_tags_of(&h, &arn, case).await;
    (reply, carried)
}

#[tokio::test]
async fn both_tag_merge_paths_obey_the_one_resultant_set_bound() {
    for row in rows() {
        let case = format!(
            "{}: {} stored, {} overwritten, {} new",
            row.label, row.stored, row.overwrites, row.added
        );
        let want = expected_set(&row);

        let (tagged, after_tagging) = through_tag_resource(&row, &case).await;
        let (repeated, after_repeat) = through_registration_repeat(&row, &case).await;

        for (path, reply) in [
            ("TagResource", &tagged),
            ("a repeated registration", &repeated),
        ] {
            if row.accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {path} must accept the write, got HTTP {} with body {:?}",
                    reply.status, reply.body
                );
            } else {
                let (status, code, message) = TAG_LIMIT;
                reply.error(&format!("{case} via {path}"), status, code);
                assert_eq!(
                    reply.message(),
                    message,
                    "case {case:?}: {path} must state the resultant-set bound, got {:?}",
                    reply.message()
                );
            }
        }

        for (path, carried) in [
            ("TagResource", &after_tagging),
            ("a repeated registration", &after_repeat),
        ] {
            assert_eq!(
                carried, &want,
                "case {case:?}: after {path} the resource must carry {want:?}, got {carried:?}"
            );
            assert!(
                carried.len() <= MAX_TAGS,
                "case {case:?}: after {path} the resource carries {} tags, past the maximum of \
                 {MAX_TAGS}",
                carried.len()
            );
        }

        // The two paths implement ONE rule, so they cannot answer differently about the
        // same merge.
        assert_eq!(
            tagged.status, repeated.status,
            "case {case:?}: TagResource answered HTTP {} and the repeated registration HTTP {}",
            tagged.status, repeated.status
        );
        assert_eq!(
            after_tagging, after_repeat,
            "case {case:?}: the two paths left different sets behind: {after_tagging:?} and \
             {after_repeat:?}"
        );
    }
}
