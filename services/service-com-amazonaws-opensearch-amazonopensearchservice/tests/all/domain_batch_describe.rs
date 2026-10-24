// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDomains`, the batch read of the domain records
//! `DescribeDomain` reports one at a time.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test is not one recorded call: `DescribeDomains` answers one
//! `DomainStatus` per DISTINCT requested name that names a stored domain, in the
//! order the request listed them, and — modelling no not-found error, unlike the
//! single-domain read — reports nothing at all for a name no stored domain answers
//! to instead of refusing the call. So the tests span a table of request shapes the
//! rule generalises over (one name, several names in an order the store does not
//! hold them in, every stored name, an unknown name alone, unknown names
//! interleaved with known ones, a repeated name, the shortest and longest names
//! `DomainName` allows, and an empty list), assert each reported entry is
//! member-for-member what `DescribeDomain` answers for that same name, and check
//! the batch read leaves the store exactly as a later caller finds it.

use serde_json::{json, Value};

use super::domain_lifecycle::{Answer, Emulator, PREFIX, SETTLE_WAIT};

/// `POST /2021-01-01/opensearch/domain-info` with `names` as its `DomainNames`.
async fn describe_domains(emulator: &Emulator, names: &[&str]) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain-info"),
            Some(json!({ "DomainNames": names })),
        )
        .await
}

/// The `DomainStatusList` of an accepted `DescribeDomains`, with the case named on
/// failure.
async fn status_list(emulator: &Emulator, case: &str, names: &[&str]) -> Vec<Value> {
    let answer = describe_domains(emulator, names).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeDomains({names:?}) to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error.is_none(),
        "case {case}: expected DescribeDomains({names:?}) to carry no error code, got {:?}",
        answer.error
    );
    answer.body["DomainStatusList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainStatusList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `DomainName` of every entry of a `DomainStatusList`, in the order reported.
fn reported_names(list: &[Value], case: &str) -> Vec<String> {
    list.iter()
        .map(|entry| {
            entry["DomainName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a DomainName, got {entry:?}"))
                .to_owned()
        })
        .collect()
}

/// `CreateDomain` for `name`, asserting it was accepted.
async fn create(emulator: &Emulator, name: &str) {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The shortest and longest names `DomainName` allows — `@length(3, 28)` with pattern
/// `^[a-z][a-z0-9\-]+$` (smithy_query query="shape" shape="DomainName").
const SHORTEST_NAME: &str = "abc";
const LONGEST_NAME: &str = "abcdefghij-klmnopqrst-uvwxyz";

/// The three domains the table below is run against, in the order they are created —
/// deliberately NOT the order the name-keyed store holds them in, so an answer that
/// followed the store's order rather than the request's is visible.
const CREATED: [&str; 3] = ["zeta-domain", "alpha-domain", "mid-domain"];

#[tokio::test]
async fn describe_domains_reports_each_distinct_known_name_in_request_order() {
    let emulator = Emulator::new();
    for name in CREATED {
        create(&emulator, name).await;
    }
    create(&emulator, SHORTEST_NAME).await;
    create(&emulator, LONGEST_NAME).await;

    // (case, requested names, the names expected in the answer, in order)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one known name", vec!["alpha-domain"], vec!["alpha-domain"]),
        (
            "several known names, in neither creation nor store order",
            vec!["mid-domain", "zeta-domain", "alpha-domain"],
            vec!["mid-domain", "zeta-domain", "alpha-domain"],
        ),
        (
            "every stored name",
            vec![
                "zeta-domain",
                "alpha-domain",
                "mid-domain",
                SHORTEST_NAME,
                LONGEST_NAME,
            ],
            vec![
                "zeta-domain",
                "alpha-domain",
                "mid-domain",
                SHORTEST_NAME,
                LONGEST_NAME,
            ],
        ),
        ("one unknown name", vec!["no-such-domain"], vec![]),
        (
            "several unknown names",
            vec!["no-such-domain", "other-missing-domain"],
            vec![],
        ),
        (
            "unknown names interleaved with known ones",
            vec![
                "no-such-domain",
                "mid-domain",
                "other-missing-domain",
                "alpha-domain",
            ],
            vec!["mid-domain", "alpha-domain"],
        ),
        (
            "the shortest name a request may carry, known",
            vec![SHORTEST_NAME],
            vec![SHORTEST_NAME],
        ),
        (
            "the longest name a request may carry, known",
            vec![LONGEST_NAME],
            vec![LONGEST_NAME],
        ),
        (
            "a name of the shortest allowed length that names nothing",
            vec!["xyz"],
            vec![],
        ),
        (
            "one known name repeated",
            vec!["mid-domain", "mid-domain", "mid-domain"],
            vec!["mid-domain"],
        ),
        (
            "known and unknown names both repeated",
            vec![
                "alpha-domain",
                "no-such-domain",
                "alpha-domain",
                "zeta-domain",
                "no-such-domain",
            ],
            vec!["alpha-domain", "zeta-domain"],
        ),
        ("an empty request list", vec![], vec![]),
    ];

    for (case, requested, expected) in rows {
        let list = status_list(&emulator, case, &requested).await;
        let actual = reported_names(&list, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected DescribeDomains({requested:?}) to report {expected:?}, \
             got {actual:?}"
        );
    }
}

#[tokio::test]
async fn each_reported_entry_is_what_the_single_domain_read_answers() {
    let emulator = Emulator::new();
    for name in CREATED {
        super::domain_lifecycle::create_baseline(&emulator, name).await;
    }
    // Read the domains once settled, so both paths report the configuration the clock
    // has already made due rather than racing the provisioning span.
    tokio::time::sleep(SETTLE_WAIT).await;

    let case = "batch entry matches the single read";
    let list = status_list(&emulator, case, &CREATED).await;
    assert_eq!(
        list.len(),
        CREATED.len(),
        "case {case:?}: expected {} entries, got {list:?}",
        CREATED.len()
    );

    for (name, entry) in CREATED.iter().zip(list.iter()) {
        let single = emulator.describe(name).await;
        assert_eq!(
            single.status, 200,
            "case {case:?} ({name:?}): expected DescribeDomain to answer 200, got {} with {:?}",
            single.status, single.body
        );
        assert_eq!(
            entry, &single.body["DomainStatus"],
            "case {case:?} ({name:?}): expected the batch entry to equal the single read's \
             DomainStatus {:?}, got {entry:?}",
            single.body["DomainStatus"]
        );
    }
}

#[tokio::test]
async fn a_batch_read_reports_the_configuration_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let name = "clock-domain";
    create(&emulator, name).await;

    let case = "straight after the create";
    let list = status_list(&emulator, case, &[name]).await;
    assert_eq!(
        list[0]["Processing"].as_bool(),
        Some(true),
        "case {case:?}: expected the domain to still be Processing, got {:?}",
        list[0]["Processing"]
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    let case = "once the provisioning span has fallen due";
    let list = status_list(&emulator, case, &[name]).await;
    assert_eq!(
        list[0]["Processing"].as_bool(),
        Some(false),
        "case {case:?}: expected the domain to have settled, got {:?}",
        list[0]["Processing"]
    );
    assert_eq!(
        list[0]["DomainProcessingStatus"].as_str(),
        Some("Active"),
        "case {case:?}: expected DomainProcessingStatus Active, got {:?}",
        list[0]["DomainProcessingStatus"]
    );
}

#[tokio::test]
async fn a_batch_read_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    for name in CREATED {
        create(&emulator, name).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    let case = "repeated batch read";
    let first = status_list(&emulator, case, &CREATED).await;
    let before = emulator.describe(CREATED[0]).await.body;
    let second = status_list(&emulator, case, &CREATED).await;
    let after = emulator.describe(CREATED[0]).await.body;

    assert_eq!(
        first, second,
        "case {case:?}: expected two reads to report the same list, got {first:?} then {second:?}"
    );
    assert_eq!(
        before, after,
        "case {case:?}: expected the single read to be unchanged by a batch read, got {before:?} \
         then {after:?}"
    );
    assert_eq!(
        emulator.store().lock().domains.len(),
        CREATED.len(),
        "case {case:?}: expected the batch read to create and remove no domain"
    );
}

#[tokio::test]
async fn a_deleted_domain_is_omitted_rather_than_refused() {
    let emulator = Emulator::new();
    for name in CREATED {
        create(&emulator, name).await;
    }

    let removed = CREATED[1];
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{removed}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected DeleteDomain({removed:?}) to answer 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    let case = "a deleted name among known ones";
    let list = status_list(&emulator, case, &CREATED).await;
    let actual = reported_names(&list, case);
    let expected: Vec<String> = CREATED
        .iter()
        .filter(|name| **name != removed)
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}
