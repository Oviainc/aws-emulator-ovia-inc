// SPDX-License-Identifier: CC0-1.0

//! The per-Region create quota rule EVERY create of this service follows, asserted
//! through each operation that follows it.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the request, the handler runs, and the
//! response is serialized as `restJson1` — so what these tests assert is what a client
//! sees.
//!
//! The rule under test: a create is accepted while the Region the request was signed for
//! holds fewer records of that resource kind than the kind's own quota, and is refused
//! with the modelled `ServiceQuotaExceededException` (HTTP 402) once it holds that many —
//! and a create that would open a record in a Region the store holds no record of that
//! kind in is refused the same way once the store already partitions that kind across the
//! maximum of Regions. Both clauses are decided for `CreateFeed` and `CreateDictionary`
//! alike, each against its own store and its own bound, so the table below spans
//! (resource kind × clause) and the counts each clause is decided at.

use crate::harness::{
    call_in_region, create_body, create_path, message_of, read_resource_in, router, REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_DICTIONARIES, MAX_FEEDS, MAX_REGIONS,
};
use smithy_server_runtime::MultiServiceRouter;

/// One resource kind a create of this service mints, with the bound its own quota is.
struct Kind {
    /// The kind, as the create path and the modelled `resourceArn` pattern spell it.
    label: &'static str,
    /// How many records of this kind one Region may hold.
    max_per_region: usize,
}

/// The two kinds this service creates: `CreateFeed` against [`MAX_FEEDS`] and
/// `CreateDictionary` against [`MAX_DICTIONARIES`] — both declaring
/// `ServiceQuotaExceededException` (HTTP 402) for a create past the quota
/// (`smithy_query(query="operation",operation="CreateFeed,CreateDictionary")`).
const KINDS: [Kind; 2] = [
    Kind {
        label: "feed",
        max_per_region: MAX_FEEDS,
    },
    Kind {
        label: "dictionary",
        max_per_region: MAX_DICTIONARIES,
    },
];

/// Send one create of `kind` through `region`'s endpoint — the smallest body of that kind
/// (`tests/all/harness.rs`), under that kind's own create path.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    kind: &str,
    name: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "POST",
        &create_path(kind),
        Some(create_body(kind, name)),
    )
    .await
}

/// Fill `region` with `count` records of `kind`, each a create of its own (the creates
/// are `@idempotent`, so each body differs in its name), and return the id of the last
/// one.
async fn fill(router: &MultiServiceRouter, region: &str, kind: &str, count: usize) -> String {
    let mut last_id = String::new();
    for index in 0..count {
        let name = format!("{kind}-quota-{index:03}");
        let (status, created) = create_in(router, region, kind, &name).await;
        assert_eq!(
            status, 202,
            "case {kind:?} {index} of {count} in {region}: a create below the quota is accepted, \
             got {status} with {created}"
        );
        last_id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {kind:?} {index}: a created record carries an id, got {created}"))
            .to_string();
    }
    last_id
}

/// Assert `answered` is the modelled refusal a create past a quota earns, carrying
/// `expected_message`.
fn assert_refused(case: &str, answered: (u16, Value), expected_message: &str) {
    let (status, error) = answered;
    assert_eq!(
        status, 402,
        "case {case:?}: a create past the quota is refused 402, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ServiceQuotaExceededException"),
        "case {case:?}: that 402 is the modelled ServiceQuotaExceededException, got {error}"
    );
    assert_eq!(
        message_of(&error),
        expected_message,
        "case {case:?}: expected the sentence for the clause that refused it, got {error}"
    );
}

/// The sentence a create of `kind` is refused with once its Region is full. The figure is
/// the kind's own bound, so the sentence states the bound the refusal was decided from.
fn region_full_message(kind: &str) -> String {
    match kind {
        "feed" => format!(
            "This account already holds the maximum of {MAX_FEEDS} feeds in this Region. \
             Delete unused feeds or request a quota increase."
        ),
        "dictionary" => format!(
            "This account already holds the maximum of {MAX_DICTIONARIES} dictionaries in this \
             Region. Delete unused dictionaries or request a quota increase."
        ),
        other => panic!("no such resource kind: {other}"),
    }
}

/// The sentence a create of `kind` is refused with when it would open a record in one
/// Region too many.
fn new_region_message(kind: &str) -> String {
    match kind {
        "feed" => format!(
            "This account already holds feeds in the maximum of {MAX_REGIONS} Regions. \
             Delete the feeds of a Region you no longer use or request a quota increase."
        ),
        "dictionary" => format!(
            "This account already holds dictionaries in the maximum of {MAX_REGIONS} Regions. \
             Delete the dictionaries of a Region you no longer use or request a quota increase."
        ),
        other => panic!("no such resource kind: {other}"),
    }
}

/// The `index`th Region this suite signs a request for. A Region is whatever the request's
/// SigV4 credential scope names, so these only have to be distinct scopes; [`REGION`] is
/// deliberately not among them, so a suite can add one Region beyond the ones it filled.
fn nth_region(index: usize) -> String {
    format!("ap-southeast-{}", index + 1)
}

/// BOTH creates are held to the first clause of the one quota rule: a Region takes records
/// of a kind up to that kind's own bound and refuses the next one with the modelled
/// `ServiceQuotaExceededException`, whichever kind it is — and the refusal leaves the
/// Region's records exactly as they were, so a refused create spends nothing.
#[tokio::test]
async fn every_create_refuses_once_its_region_holds_its_quota() {
    for kind in &KINDS {
        let router = router();
        let label = kind.label;

        // Each count below the bound is accepted: the first record of an empty Region,
        // and every one up to the last the quota admits.
        let last_id = fill(&router, REGION, label, kind.max_per_region).await;

        let case = format!("{label} number {} in {REGION}", kind.max_per_region + 1);
        let answered = create_in(&router, REGION, label, "past-the-quota").await;
        assert_refused(&case, answered, &region_full_message(label));

        // The refusal did not disturb what the Region holds.
        let (status, fetched) = read_resource_in(&router, REGION, label, &last_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the records already stored still read back, got {status} with {fetched}"
        );

        // Another Region's quota is its own, so the clause is decided on what THIS
        // endpoint holds and the next Region still takes its first record.
        let other = nth_region(0);
        let (status, created) = create_in(&router, &other, label, "first-of-this-region").await;
        assert_eq!(
            status, 202,
            "case {case:?}: another Region still takes its first {label}, got {status} with \
             {created}"
        );
    }
}

/// BOTH creates are held to the second clause of the same rule: while the store already
/// partitions a kind across the maximum of Regions, a create that would open a further
/// partition for that kind is refused — and a Region that already holds a record of the
/// kind is no new partition, so it keeps taking records.
#[tokio::test]
async fn every_create_refuses_a_region_past_the_partition_bound() {
    for kind in &KINDS {
        let router = router();
        let label = kind.label;

        for index in 0..MAX_REGIONS {
            let region = nth_region(index);
            let (status, created) = create_in(&router, &region, label, "partition").await;
            assert_eq!(
                status, 202,
                "case {label:?} partition {} of {MAX_REGIONS}: a create in a Region the store has \
                 room for is accepted, got {status} with {created}",
                index + 1
            );
        }

        let case = format!("{label} in Region number {} ({REGION})", MAX_REGIONS + 1);
        let answered = create_in(&router, REGION, label, "one-region-too-many").await;
        assert_refused(&case, answered, &new_region_message(label));

        // A Region the store already holds this kind in is NOT a new partition: it keeps
        // taking records, up to its own quota.
        let filled = nth_region(0);
        let (status, created) = create_in(&router, &filled, label, "second-of-this-region").await;
        assert_eq!(
            status, 202,
            "case {case:?}: a Region already holding a {label} still takes another, got {status} \
             with {created}"
        );
    }
}

/// The rule is read against EACH kind's own store and bound: the Regions a feed create
/// has partitioned across neither consume the dictionary partitions nor are consumed by
/// them, and a Region full of one kind still takes the other.
#[tokio::test]
async fn each_kinds_quota_is_decided_against_its_own_records() {
    let partitioned = router();

    // Every partition the store admits, opened with feeds alone.
    for index in 0..MAX_REGIONS {
        let region = nth_region(index);
        let (status, created) = create_in(&partitioned, &region, "feed", "partition").await;
        assert_eq!(
            status, 202,
            "case feed partition {} of {MAX_REGIONS}: accepted, got {status} with {created}",
            index + 1
        );
    }
    let answered = create_in(&partitioned, REGION, "feed", "one-region-too-many").await;
    assert_refused(
        "a feed in one Region too many",
        answered,
        &new_region_message("feed"),
    );

    // The dictionary store holds nothing, so its own partition count is zero and the very
    // Region the feed create was refused in takes a dictionary.
    let (status, created) = create_in(&partitioned, REGION, "dictionary", "unconsumed").await;
    assert_eq!(
        status, 202,
        "case a dictionary in a Region no feed create may open: the dictionary partitions are \
         its own, got {status} with {created}"
    );

    // And a Region filled with one kind's whole quota still takes the other kind, which
    // a store of its own says without the partitions above standing in the way.
    let second_router = router();
    fill(&second_router, REGION, "dictionary", MAX_DICTIONARIES).await;
    let answered = create_in(&second_router, REGION, "dictionary", "past-the-quota").await;
    assert_refused(
        "a dictionary past this Region's dictionary quota",
        answered,
        &region_full_message("dictionary"),
    );
    let (status, created) = create_in(&second_router, REGION, "feed", "unconsumed").await;
    assert_eq!(
        status, 202,
        "case a feed in a Region full of dictionaries: the feed quota is its own, got {status} \
         with {created}"
    );
}
