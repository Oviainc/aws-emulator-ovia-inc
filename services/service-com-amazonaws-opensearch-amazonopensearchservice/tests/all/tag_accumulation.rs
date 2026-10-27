// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a resource's tag set ACCUMULATES across the calls that
//! write it, and for its independence from the configuration work the resource goes
//! through.
//!
//! The rule these cases assert: a resource's tags are the fold of every tagging
//! write on it, one value per key with the last write to a key winning, so the set a
//! read reports depends only on the pairs written and the order of the writes to the
//! SAME key — never on how those pairs were split across calls — and nothing else
//! the control plane does to the resource changes it.
//!
//! Two dimensions the sibling [`super::resource_tags`] suite does not span:
//!
//! * the SPLIT: one list of pairs versus the same pairs spread over several calls,
//!   in several orders, including a later call that rewrites an earlier call's key.
//!   Every row's expectation is computed by folding the calls, so a row asserts the
//!   rule rather than a hand-written answer, and the set is read back after EVERY
//!   call, so what is pinned is the fold of each prefix as well as of the whole
//!   sequence;
//! * the CLOCK: a tag written while the resource's configuration is still
//!   provisioning survives the settle the clock owes, and one written while an
//!   update is applying survives that settle too.
//!
//! The wire harness comes from [`super::domain_lifecycle`] and the tagging request
//! helpers from [`super::resource_tags`]; this suite adds no second copy of either.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::domain_lifecycle::{change_rows, Emulator, SETTLE_WAIT};
use super::resource_tags::{add_tags, as_set, domain_arn, stored_tags, tag, wire_tags};

/// One tag as the tests carry it: its key and the value under that key.
type Pair = (String, String);

/// The `TagList` of ONE `AddTags` call.
type Call = Vec<Pair>;

/// One table row: what the split is, and the calls it makes in order.
type Row = (&'static str, Vec<Call>);

/// The pairs every call sequence below writes between them, as (key, value).
fn pool() -> Call {
    (0..5)
        .map(|slot| tag(&format!("key-{slot}"), &format!("value-{slot}")))
        .collect()
}

/// `pairs` split into one call per pair, in the order given.
fn one_per_call(pairs: Call) -> Vec<Call> {
    pairs.into_iter().map(|pair| vec![pair]).collect()
}

/// The set a sequence of calls leaves behind, derived from the rule: fold the calls
/// in order into a map keyed by tag key, so a key written twice keeps the value of
/// the LAST write and no key appears twice.
fn folded(calls: &[Call]) -> Call {
    let mut set: BTreeMap<String, String> = BTreeMap::new();
    for call in calls {
        for (key, value) in call {
            set.insert(key.clone(), value.clone());
        }
    }
    set.into_iter().collect()
}

/// One row per way the same pairs can be split across calls.
fn split_rows() -> Vec<Row> {
    let pairs = pool();
    let mut reversed = pairs.clone();
    reversed.reverse();

    vec![
        ("one call carrying every pair", vec![pairs.clone()]),
        ("one call per pair, in order", one_per_call(pairs.clone())),
        // Disjoint keys, so the fold of this sequence is the fold of the one above:
        // the resulting set does not depend on the order of the calls.
        ("one call per pair, in reverse order", one_per_call(reversed)),
        (
            "two calls, three pairs then two",
            vec![pairs[..3].to_vec(), pairs[3..].to_vec()],
        ),
        (
            "two calls, two pairs then four, overlapping in one key",
            vec![pairs[..2].to_vec(), pairs[1..].to_vec()],
        ),
        // The three rows below carry a key written twice. That one value per key
        // wins is the sibling suite's subject; what they add here is that it holds
        // ACROSS calls, at any position in a sequence, which is this suite's.
        (
            "every pair, then a later call rewriting one key",
            vec![pairs.clone(), vec![tag("key-1", "rewritten")]],
        ),
        (
            "a key repeated inside one list, then rewritten by a later call",
            vec![
                vec![tag("key-0", "first"), tag("key-0", "second")],
                vec![tag("key-0", "third"), tag("key-4", "value-4")],
            ],
        ),
        (
            "a key rewritten to the empty value a later call gives it",
            vec![pairs.clone(), vec![tag("key-2", "")]],
        ),
    ]
}

#[tokio::test]
async fn a_tag_set_is_the_fold_of_the_calls_however_they_are_split() {
    for (index, (what, calls)) in split_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let arn = domain_arn(&emulator, &format!("folded-tags-{index}")).await;

        for (step, call) in calls.iter().enumerate() {
            let case = format!("{what} [call {step}]");
            let answer = add_tags(&emulator, &arn, wire_tags(call)).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body,
                Value::Null,
                "case {case}: AddTags answers with an empty body, got {:?}",
                answer.body
            );

            // After every call the stored set is the fold of the calls made SO FAR,
            // so no row depends on only the last write landing.
            let expected = folded(&calls[..=step]);
            let read = stored_tags(&emulator, &arn, &case).await;
            assert_eq!(
                read,
                as_set(&expected),
                "case {case}: expected the fold of the calls so far {expected:?}, got {read:?}"
            );
        }

        let expected = folded(&calls);
        let read = stored_tags(&emulator, &arn, what).await;
        assert_eq!(
            read,
            as_set(&expected),
            "case {what}: expected the fold of the whole sequence {expected:?}, got {read:?}"
        );
    }
}

/// Assert where the domain's configuration stands, so a case that writes a tag
/// mid-change shows that a change really was applying at that point.
async fn assert_processing(emulator: &Emulator, name: &str, case: &str, processing: bool) {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from DescribeDomain, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["DomainStatus"]["Processing"].as_bool(),
        Some(processing),
        "case {case}: expected Processing {processing}, got {:?}",
        answer.body["DomainStatus"]
    );
}

#[tokio::test]
async fn tags_written_while_a_change_applies_survive_the_settle_the_clock_owes() {
    let emulator = Emulator::new();
    let name = "tagged-through-settle";
    let arn = domain_arn(&emulator, name).await;

    // The tag below is written while the configuration the create started is still
    // provisioning, so the case really spans the settle rather than degrading to a
    // plain write-then-read.
    assert_processing(&emulator, name, "while provisioning", true).await;

    let provisioning = vec![tag("written", "while-provisioning")];
    let answer = add_tags(&emulator, &arn, wire_tags(&provisioning)).await;
    assert_eq!(
        answer.status, 200,
        "case AddTags(while provisioning): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let read = stored_tags(&emulator, &arn, "while provisioning").await;
    assert_eq!(
        read,
        as_set(&provisioning),
        "case AddTags(while provisioning): expected the attached set back, got {read:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    assert_processing(&emulator, name, "after the create settles", false).await;
    let read = stored_tags(&emulator, &arn, "after the create settles").await;
    assert_eq!(
        read,
        as_set(&provisioning),
        "case after the create settles: settling the configuration must not touch the tags, \
         got {read:?}"
    );

    // An update starts another configuration change; a tag written while it applies
    // survives that settle too, and the earlier tag stays where it was.
    let (option, value, _) = change_rows()
        .into_iter()
        .next()
        .expect("the change table has at least one row");
    let updated = emulator.update_config(name, json!({ option: value })).await;
    assert_eq!(
        updated.status, 200,
        "case UpdateDomainConfig({option}): expected 200, got {} with {:?}",
        updated.status, updated.body
    );

    // One key the resource already carries, rewritten, and one it does not, written
    // while that change is still applying: the expectation is the fold of both
    // calls, so the rewritten key keeps one value.
    assert_processing(&emulator, name, "while updating", true).await;

    let while_updating = vec![
        tag("written", "while-updating"),
        tag("added", "while-updating"),
    ];
    let answer = add_tags(&emulator, &arn, wire_tags(&while_updating)).await;
    assert_eq!(
        answer.status, 200,
        "case AddTags(while updating): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let expected = folded(&[provisioning.clone(), while_updating]);
    let read = stored_tags(&emulator, &arn, "while updating").await;
    assert_eq!(
        read,
        as_set(&expected),
        "case AddTags(while updating): expected {expected:?}, got {read:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    assert_processing(&emulator, name, "after the update settles", false).await;
    let read = stored_tags(&emulator, &arn, "after the update settles").await;
    assert_eq!(
        read,
        as_set(&expected),
        "case after the update settles: settling the change must not touch the tags, got {read:?}"
    );
}
