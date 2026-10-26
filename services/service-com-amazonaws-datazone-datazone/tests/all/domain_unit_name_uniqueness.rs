// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the uniqueness of a domain unit's name under its parent.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here. The
//! harness (`Emulator`, `Answer`) is the one the other suites already define.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request: a
//! domain unit's name is unique among the DIRECT CHILDREN of one parent domain unit, so
//! a `CreateDomainUnit` whose (parent, name) pair is already taken is refused with a 409
//! `ConflictException` while the same name under another parent, or in another domain, is
//! accepted — and the refusal leaves exactly one unit holding the name, whether the
//! duplicate arrives after the first create or at the same time as several others.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};
use std::sync::Arc;

/// One `CreateDomainUnit` call. The caller decides what the answer has to be, so this
/// serves the accepted and the refused rows alike.
async fn create(dz: &Emulator, domain: &str, parent: &str, name: &str) -> Answer {
    dz.call(
        "POST",
        &format!("/v2/domains/{domain}/domain-units"),
        Some(json!({ "name": name, "parentDomainUnitIdentifier": parent })),
    )
    .await
}

/// Create a domain unit, asserting the create itself succeeded, and answer its id.
async fn create_ok(dz: &Emulator, domain: &str, parent: &str, name: &str, label: &str) -> String {
    let answer = create(dz, domain, parent, name).await;
    assert_eq!(
        answer.status, 201,
        "case {label:?}: expected 201 creating {name:?} under {parent:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("id")
}

/// The domain's root domain unit id.
async fn root_unit(dz: &Emulator, domain: &str) -> String {
    dz.call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId")
}

/// The `(id, name)` of every unit `ListDomainUnitsForParent` reports for `parent`
/// (smithy_query shape="DomainUnitSummary": required `id` and `name`).
async fn children(dz: &Emulator, domain: &str, parent: &str) -> Vec<(String, String)> {
    let answer = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={parent}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "listing the children of {parent:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let member = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or("<absent>")
                            .to_string()
                    };
                    (member("id"), member("name"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One contested name. The rows differ along what the rule generalises over: the form
/// and the length of the name — both published bounds of `DomainUnitName` (len 1..128,
/// pattern `^[\w -]+$`, smithy_query shape="DomainUnitName") and names mixing spaces,
/// digits, hyphens and underscores in between.
fn contested_names() -> Vec<(&'static str, String)> {
    vec![
        ("a single-character name, the modelled minimum", "a".to_string()),
        ("a plain word", "sales".to_string()),
        ("spaces and digits", "Sales Unit 42".to_string()),
        ("hyphens, underscores and a space", "eu-west_sub 1".to_string()),
        (
            "a name at the modelled 128-character maximum",
            "n".repeat(128),
        ),
    ]
}

/// RULE: the (parent, name) pair is what is unique, so a second create of a taken name
/// under the same parent is refused with a 409 `ConflictException`, while the SAME name
/// under a different parent, and in a different domain, is accepted.
#[tokio::test]
async fn a_name_is_unique_among_one_parents_children() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unit-name-uniqueness").await;
    let other_domain = dz.available_domain("unit-name-uniqueness-elsewhere").await;
    let root = root_unit(&dz, &domain).await;
    let other_root = root_unit(&dz, &other_domain).await;

    for (row, (label, name)) in contested_names().into_iter().enumerate() {
        // Each row gets two parents of its own, so a row's contested name is free under
        // both of them before the row runs.
        let held = create_ok(&dz, &domain, &root, &format!("holder {row}"), label).await;
        let sibling_parent = create_ok(&dz, &domain, &root, &format!("neighbour {row}"), label).await;

        let winner = create_ok(&dz, &domain, &held, &name, label).await;

        // The same pair again: refused, and the refusal is the modelled conflict.
        let duplicate = create(&dz, &domain, &held, &name).await;
        assert_eq!(
            duplicate.status, 409,
            "case {label:?}: a second {name:?} under the same parent must be refused with 409, got {} {:?}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code, "ConflictException",
            "case {label:?}: expected the ConflictException error code, got {:?}",
            duplicate.error_code
        );
        assert!(
            duplicate
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {label:?}: the conflict must carry a message, got {:?}",
            duplicate.body
        );

        // A duplicate that differs in every member BUT the (parent, name) pair is still
        // a duplicate: a description, and a clientToken no earlier call used, do not
        // make the name free.
        let dressed_up = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/domain-units"),
                Some(json!({
                    "name": name,
                    "parentDomainUnitIdentifier": held,
                    "description": format!("a second unit called the same thing, row {row}"),
                    "clientToken": format!("fresh-token-{row}"),
                })),
            )
            .await;
        assert_eq!(
            (dressed_up.status, dressed_up.error_code.as_str()),
            (409, "ConflictException"),
            "case {label:?}: a description and a fresh clientToken must not free the name, got {} {:?}",
            dressed_up.status,
            dressed_up.body
        );

        // The same name is free under ANOTHER parent, and in another domain.
        let sibling = create_ok(&dz, &domain, &sibling_parent, &name, label).await;
        assert_ne!(
            sibling, winner,
            "case {label:?}: the unit created under another parent must be its own unit"
        );
        let elsewhere = create_ok(&dz, &other_domain, &other_root, &name, label).await;
        assert_ne!(
            elsewhere, winner,
            "case {label:?}: the unit created in another domain must be its own unit"
        );

        // Exactly one unit holds the name under the contested parent, and it is the
        // winner: the refusals stored nothing.
        assert_eq!(
            children(&dz, &domain, &held).await,
            vec![(winner.clone(), name.clone())],
            "case {label:?}: expected only the winner {winner:?} under the contested parent, got {:?}",
            children(&dz, &domain, &held).await
        );
        let read_back = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/domain-units/{winner}"),
                None,
            )
            .await;
        assert_eq!(
            (
                read_back.status,
                read_back.s("name"),
                read_back.s("parentDomainUnitId")
            ),
            (200, name.clone(), held.clone()),
            "case {label:?}: the winner must read back unchanged after the refusals, got {} {:?}",
            read_back.status,
            read_back.body
        );
    }
}

/// RULE, from the other side: names that are NOT equal never collide, however close
/// they are — the check compares the whole name, and `DomainUnitName` admits both cases
/// with nothing in the model folding them.
#[tokio::test]
async fn only_an_equal_name_collides() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unit-name-near-misses").await;
    let root = root_unit(&dz, &domain).await;
    let parent = create_ok(&dz, &domain, &root, "near miss parent", "near misses").await;
    let taken = "Sales Unit";
    let holder = create_ok(&dz, &domain, &parent, taken, "the name holder").await;

    // Each row is a name that differs from the taken one somewhere, so each must be
    // accepted: a different case, a longer name, a shorter one, different separators.
    let near_misses = [
        ("lower case throughout", "sales unit"),
        ("upper case throughout", "SALES UNIT"),
        ("one more character", "Sales Units"),
        ("one fewer character", "Sales Uni"),
        ("an underscore where the space is", "Sales_Unit"),
        ("a hyphen where the space is", "Sales-Unit"),
        ("no separator at all", "SalesUnit"),
        ("a trailing space", "Sales Unit "),
    ];

    let mut minted = vec![(holder.clone(), taken.to_string())];
    for (label, name) in near_misses {
        let answer = create(&dz, &domain, &parent, name).await;
        assert_eq!(
            answer.status, 201,
            "case {label:?}: {name:?} is not {taken:?}, so it must be accepted, got {} {:?}",
            answer.status, answer.body
        );
        minted.push((answer.s("id"), name.to_string()));
    }

    minted.sort();
    let mut listed = children(&dz, &domain, &parent).await;
    listed.sort();
    assert_eq!(
        listed, minted,
        "expected every accepted near miss stored beside the taken name, got {listed:?}"
    );
}

/// How many callers race to create one name under one parent. More than two, so the
/// test is about ANY number of concurrent creates rather than a single pair.
const RACERS: usize = 5;

/// RULE, reached concurrently: of N callers creating the same name under one parent at
/// the same time exactly ONE succeeds, every other is refused with the 409
/// `ConflictException`, and the parent is left holding exactly one unit of that name —
/// the winner's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_of_many_concurrent_same_name_creates_wins() {
    let dz = Arc::new(Emulator::new());
    let domain = dz.available_domain("unit-name-race").await;
    let root = root_unit(&dz, &domain).await;

    // The rows differ along the one thing a race generalises over: how many racers
    // there are, and the form of the contested name.
    for (row, (label, racers, name)) in [
        ("two racers, a plain name", 2usize, "race one".to_string()),
        ("five racers, a plain name", RACERS, "race two".to_string()),
        (
            "five racers, a name at the modelled maximum length",
            RACERS,
            "r".repeat(128),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let parent = create_ok(&dz, &domain, &root, &format!("race parent {row}"), label).await;

        let mut pending = Vec::new();
        for racer in 0..racers {
            let dz = Arc::clone(&dz);
            let domain = domain.clone();
            let parent = parent.clone();
            let name = name.clone();
            pending.push(tokio::spawn(async move {
                (racer, create(&dz, &domain, &parent, &name).await)
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        let mut refused = 0usize;
        for task in pending {
            let (racer, answer) = task.await.expect("the racing create task ran");
            match answer.status {
                201 => winners.push(answer.s("id")),
                409 => {
                    assert_eq!(
                        answer.error_code, "ConflictException",
                        "case {label:?} racer {racer}: a refused racer must name a ConflictException, got {:?}",
                        answer.error_code
                    );
                    refused += 1;
                }
                status => panic!(
                    "case {label:?} racer {racer}: expected 201 or 409, got {status} {:?}",
                    answer.body
                ),
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {label:?}: expected exactly 1 winner of {racers} racers, got {winners:?}"
        );
        assert_eq!(
            refused,
            racers - 1,
            "case {label:?}: expected the other {} racers refused, got {refused}",
            racers - 1
        );

        let listed = children(&dz, &domain, &parent).await;
        assert_eq!(
            listed,
            vec![(winners[0].clone(), name.clone())],
            "case {label:?}: expected only the winner {:?} under the contested parent, got {listed:?}",
            winners[0]
        );
    }
}
