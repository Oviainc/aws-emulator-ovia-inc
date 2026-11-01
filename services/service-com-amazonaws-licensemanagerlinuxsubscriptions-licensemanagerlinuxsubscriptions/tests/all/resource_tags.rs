// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for this service's tagging slice: `TagResource`
//! (`PUT /tags/{resourceArn}`) and `ListTagsForResource` (`GET /tags/{resourceArn}`).
//!
//! Every case drives a real request through the shared harness's `dispatch`, so what is
//! asserted is the wire behaviour an SDK client sees: deserialize → constraint check →
//! handle → serialize, with this suite stating no second copy of what that request looks
//! like on the wire.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a tag write MERGES the pairs it names into the tag set the addressed resource
//!   already carries in state — a key the request names takes the request's value, a key
//!   it does not name keeps the value it had — and the read reports exactly that stored
//!   set, so two writes of different keys converge on the union of both and a repeat of
//!   one write changes nothing (`TagResource` is `@idempotent`);
//! * the set is per RESOURCE: a write addressed to one subscription provider leaves
//!   every other provider's set alone;
//! * `ListTagsForResource` is `@readonly` — reading tags changes nothing a later caller
//!   sees, including the rest of the provider record `GetRegisteredSubscriptionProvider`
//!   reports;
//! * a `resourceArn` addressing no resource of this account is refused by BOTH
//!   operations with the code and status the tag APIs answer for an ABSENT resource
//!   (`NotFoundException`, HTTP 404), and the refused write stores nothing;
//! * a write whose RESULTANT set would be larger than a resource may carry
//!   ([`MAX_TAGS`], the modelled `Tags` maximum) is refused with `BadRequestException`
//!   (HTTP 400 — a malformed request, not an absent resource) and leaves the stored set
//!   as it was.
//!
//! The tables span the class each rule covers rather than the one recorded tag set: the
//! number of pairs written (one, three, the modelled maximum of 50), keys and
//! values that are non-ASCII or URI-significant, a write that overwrites a stored key, a
//! write disjoint from the stored keys, and — for the maximum — the published bound, one
//! below it and one past it.
//!
//! A write naming NO pair at all is not a row here: the surface refuses it as invalid input
//! before the resource is resolved, which `tag_plane_authorization.rs` owns — as it does the
//! denial of a `resourceArn` in another account.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::harness::{
    deregister, get_provider, harness, list_tags, provider_arn_for, reported_tags, seed_provider,
    stored_provider_count, tag, Harness, Reply, ACCOUNT, RED_HAT, REGION, SIGNING_NAME,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState as ServiceState;

/// The largest tag set a resource may carry: the modelled `Tags` map is `len 0..50`,
/// both as `TagResource`'s input and as `ListTagsForResource`'s output
/// (`smithy_query(query="shape",shape="Tags")`).
const MAX_TAGS: usize = 50;

/// The HTTP status, wire error code and message the tag APIs answer for a `resourceArn`
/// that addresses no resource of this account.
const NOT_FOUND: (u16, &str, &str) = (404, "NotFoundException", "Resource was not found");

/// The HTTP status, wire error code and message a write past [`MAX_TAGS`] is refused
/// with.
const TAG_LIMIT: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "The resultant tag set must not have more than 50 user tags.",
);

/// Assert `reply` is the HTTP status, wire error code and message of `refusal`.
fn assert_refused(reply: &Reply, case: &str, refusal: (u16, &str, &str)) {
    let (status, code, message) = refusal;
    reply.error(case, status, code);
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected message {message:?}, got body {:?}",
        reply.body
    );
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

/// Owned `(key, value)` rows, so a table can hold computed keys and values beside
/// written-out ones.
fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct generated pairs, for the rows that exercise a tag COUNT.
fn generated_pairs(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// Put a subscription provider carrying `tags` in the store, under the id `index` names,
/// and return its ARN.
///
/// The provider is seeded DIRECTLY, through the shared harness's one seeding routine (the
/// same `engine.state::<…>()` seam the handlers use), because these rules are about the
/// TAG slice of a provider that is already usable:
/// `RegisterSubscriptionProvider` creates one over the wire and it becomes usable
/// on the clock (the create path is covered by
/// `tests/all/subscription_provider_registration.rs`), so a suite about tags would
/// otherwise wait out that span once per row. The id and the secret keep each seeded
/// record distinct.
fn register(h: &Harness, case: &str, index: usize, tags: &[(String, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let before = stored_provider_count(h);
    let arn = seed_provider(
        h,
        &format!("00000000-0000-4000-8000-{:012x}", index + 1),
        &secret_arn(index),
        &borrowed,
    );
    assert_eq!(
        stored_provider_count(h),
        before + 1,
        "case {case:?}: the seeded ARN {arn} was already in the store"
    );
    arn
}

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern, distinct per index.
fn secret_arn(index: usize) -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:rhel-token-{index}")
}

async fn tag_resource(h: &Harness, arn: &str, tags: &[(String, String)]) -> Reply {
    tag(h, arn, &tag_map(tags)).await
}

/// The expected set, as the same sorted map.
fn expected_tags(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter().cloned().collect()
}

/// One row of the merge table: what the resource already carries, what the write names,
/// and the set the resource is left with.
struct MergeCase {
    case: String,
    stored: Vec<(String, String)>,
    written: Vec<(String, String)>,
    want: Vec<(String, String)>,
}

fn merge_cases() -> Vec<MergeCase> {
    let mut cases = vec![
        MergeCase {
            case: "one pair leaves the stored key it does not name alone".to_string(),
            stored: pairs(&[("owner", "platform")]),
            written: pairs(&[("env", "prod")]),
            want: pairs(&[("owner", "platform"), ("env", "prod")]),
        },
        MergeCase {
            case: "one pair onto an empty set".to_string(),
            stored: Vec::new(),
            written: pairs(&[("env", "prod")]),
            want: pairs(&[("env", "prod")]),
        },
        MergeCase {
            case: "three pairs onto an empty set".to_string(),
            stored: Vec::new(),
            written: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
            want: pairs(&[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")]),
        },
        MergeCase {
            case: "a stored key takes the written value".to_string(),
            stored: pairs(&[("env", "dev"), ("owner", "platform")]),
            written: pairs(&[("env", "prod")]),
            want: pairs(&[("env", "prod"), ("owner", "platform")]),
        },
        MergeCase {
            case: "disjoint keys converge on the union".to_string(),
            stored: pairs(&[("a", "1")]),
            written: pairs(&[("b", "2"), ("c", "3")]),
            want: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        MergeCase {
            case: "an empty value overwrites a non-empty one".to_string(),
            stored: pairs(&[("env", "prod")]),
            written: pairs(&[("env", "")]),
            want: pairs(&[("env", "")]),
        },
        MergeCase {
            case: "non-ASCII keys and values".to_string(),
            stored: Vec::new(),
            written: pairs(&[("propriétaire", "équipe-plate-forme"), ("团队", "平台")]),
            want: pairs(&[("propriétaire", "équipe-plate-forme"), ("团队", "平台")]),
        },
        MergeCase {
            case: "URI-significant keys and values".to_string(),
            stored: pairs(&[("path", "a/b")]),
            written: pairs(&[
                ("path", "a/b:c?d=e&f"),
                ("arn", "arn:aws:iam::000000000000:role/x"),
            ]),
            want: pairs(&[
                ("path", "a/b:c?d=e&f"),
                ("arn", "arn:aws:iam::000000000000:role/x"),
            ]),
        },
    ];
    // The modelled maximum, written in one call onto an empty set.
    cases.push(MergeCase {
        case: format!("the modelled maximum of {MAX_TAGS} pairs onto an empty set"),
        stored: Vec::new(),
        written: generated_pairs(MAX_TAGS),
        want: generated_pairs(MAX_TAGS),
    });
    // A resource already AT the maximum: a write that only overwrites stored keys keeps
    // the resultant set at the bound, so it is accepted.
    let mut at_max = generated_pairs(MAX_TAGS);
    let mut overwritten = at_max.clone();
    at_max[0].1 = "replaced".to_string();
    overwritten.truncate(1);
    overwritten[0].1 = "replaced".to_string();
    cases.push(MergeCase {
        case: format!("a set already at {MAX_TAGS} accepts an overwrite of a stored key"),
        stored: generated_pairs(MAX_TAGS),
        written: overwritten,
        want: at_max,
    });
    cases
}

#[tokio::test]
async fn a_tag_write_merges_into_the_stored_set_the_read_reports() {
    let h = harness();
    let mut resources = Vec::new();

    for (index, row) in merge_cases().into_iter().enumerate() {
        let case = row.case.clone();
        let arn = register(&h, &case, index, &row.stored);

        tag_resource(&h, &arn, &row.written).await.ok(&case);
        let got = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            got,
            expected_tags(&row.want),
            "case {case:?}: expected the merged set {:?}, got {got:?}",
            expected_tags(&row.want)
        );

        // `TagResource` is `@idempotent`: the same write again leaves the same set.
        tag_resource(&h, &arn, &row.written).await.ok(&case);
        let again = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            again, got,
            "case {case:?}: a repeated write changed the set to {again:?}"
        );

        resources.push((case, arn, row.want));
    }

    // Per resource: every earlier row still reports its OWN set after every later write.
    for (case, arn, want) in &resources {
        let got = reported_tags(&list_tags(&h, arn).await, case);
        assert_eq!(
            got,
            expected_tags(want),
            "case {case:?}: after the other rows' writes, {arn} reports {got:?}"
        );
    }
}

#[tokio::test]
async fn reading_tags_changes_nothing_a_later_caller_sees() {
    let h = harness();
    let case = "read a provider's tags twice";
    let stored = pairs(&[("env", "prod"), ("owner", "platform")]);
    let arn = register(&h, case, 0, &stored);

    let first = reported_tags(&list_tags(&h, &arn).await, case);
    let second = reported_tags(&list_tags(&h, &arn).await, case);
    assert_eq!(
        first,
        expected_tags(&stored),
        "case {case:?}: expected the registration's tags {:?}, got {first:?}",
        expected_tags(&stored)
    );
    assert_eq!(
        second, first,
        "case {case:?}: a second read reported {second:?} after {first:?}"
    );

    // The rest of the record is untouched too: the reader is `@readonly`.
    let provider = get_provider(&h, &arn).await;
    let body = provider.ok(case);
    for (member, want) in [
        ("SubscriptionProviderArn", arn.as_str()),
        ("SubscriptionProviderSource", RED_HAT),
        ("SecretArn", secret_arn(0).as_str()),
    ] {
        assert_eq!(
            body.get(member).and_then(Value::as_str),
            Some(want),
            "case {case:?}: expected {member} {want:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_addressing_no_resource_is_refused_by_both_tag_operations() {
    let h = harness();
    // One registered provider, so a refusal cannot be confused with an empty store, and
    // so the test can show a refused write stored nothing.
    let stored = pairs(&[("env", "prod")]);
    let live = register(&h, "the registered provider", 0, &stored);
    // A provider that WAS registered and is not any more: its ARN is well-formed and
    // addresses nothing.
    let deregistered = register(&h, "a provider to deregister", 1, &[]);
    deregister(&h, &deregistered)
        .await
        .ok("deregister the second provider");

    let unknown: Vec<(String, String)> = vec![
        (
            "an id nothing was ever registered under".to_string(),
            provider_arn_for("55555555-5555-4555-8555-555555555555"),
        ),
        ("a deregistered provider".to_string(), deregistered),
        (
            "the live provider's id in another region".to_string(),
            live.replace(REGION, "eu-west-1"),
        ),
        (
            "another resource kind".to_string(),
            live.replace("subscription-provider/", "linux-subscription/"),
        ),
        (
            "another service's namespace".to_string(),
            live.replace(&format!(":{SIGNING_NAME}:"), ":license-manager:"),
        ),
    ];

    for (label, arn) in &unknown {
        let case = format!("TagResource on {label}");
        assert_refused(
            &tag_resource(&h, arn, &pairs(&[("env", "prod")])).await,
            &case,
            NOT_FOUND,
        );

        let case = format!("ListTagsForResource on {label}");
        assert_refused(&list_tags(&h, arn).await, &case, NOT_FOUND);
    }

    // Nothing the refused writes carried was stored: the store still holds exactly the
    // one live provider, with exactly the tags its registration carried.
    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    assert_eq!(
        guard.providers.len(),
        1,
        "expected only the live provider in the store, got {:?}",
        guard.providers.keys().collect::<Vec<_>>()
    );
    let got: BTreeMap<String, String> = guard
        .resource_tags(&live)
        .unwrap_or_else(|| panic!("the live provider {live} is gone from the store"))
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(
        got,
        expected_tags(&stored),
        "expected the live provider's tags {:?}, got {got:?}",
        expected_tags(&stored)
    );
}

#[tokio::test]
async fn a_write_past_the_tag_maximum_is_refused_and_stores_nothing() {
    let h = harness();

    // The bound, one below it, and one past it: a resource holding `stored` keys taking
    // `added` new keys is accepted exactly while the resultant set fits in MAX_TAGS.
    let rows = [
        (MAX_TAGS - 1, 1, true),
        (MAX_TAGS, 1, false),
        (MAX_TAGS - 1, 2, false),
        (MAX_TAGS / 2, MAX_TAGS / 2, true),
    ];

    for (index, (stored_count, added, accepted)) in rows.into_iter().enumerate() {
        let case = format!("{stored_count} stored tags plus {added} new keys");
        let stored = generated_pairs(stored_count);
        let arn = register(&h, &case, index, &stored);
        let added_pairs: Vec<(String, String)> = (0..added)
            .map(|offset| {
                (
                    format!("added-{offset:02}"),
                    format!("value-{offset:02}"),
                )
            })
            .collect();

        let reply = tag_resource(&h, &arn, &added_pairs).await;
        let mut want = stored.clone();
        if accepted {
            reply.ok(&case);
            want.extend(added_pairs.iter().cloned());
        } else {
            assert_refused(&reply, &case, TAG_LIMIT);
        }

        let got = reported_tags(&list_tags(&h, &arn).await, &case);
        assert_eq!(
            got,
            expected_tags(&want),
            "case {case:?}: expected the stored set {:?}, got {got:?}",
            expected_tags(&want)
        );
        assert!(
            got.len() <= MAX_TAGS,
            "case {case:?}: the resource carries {} tags, past the maximum of {MAX_TAGS}",
            got.len()
        );
    }
}

