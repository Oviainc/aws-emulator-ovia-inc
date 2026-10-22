// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecoveryGroups`, and for the create/get pair whose
//! persisted records it reports.
//!
//! The rule under test: a recovery group is a named account-level resource, so
//! `ListRecoveryGroups` reports every record this account's store holds — and only
//! those — in `RecoveryGroupName` order, in pages bounded by `MaxResults` and resumed by
//! a `NextToken` this service itself minted, with each entry carrying exactly the ARN,
//! name, member cells and tags the create persisted.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected: an answer that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables. Rows differ along what the rule generalises over — cell count
//! and cell order, name form and length, tag count and tag value, how many records the
//! store holds, the page size asked for, and which token a page resumes from. The
//! recorded unit's own call (a list against a store nothing was written to) is one row.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    DEFAULT_MAX_RESULTS, MAX_NESTED_CELLS_PER_CELL, MAX_RECORDS_PER_STORE, MAX_TAGS_PER_RESOURCE,
};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// `MaxResults` is `range 1..1000` on `ListRecoveryGroups` (model:
/// `smithy_query(query="operation", operation="ListRecoveryGroups")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `__stringMax64PatternAAZAZ09Z`, the shape `RecoveryGroupName` targets on every
/// response, is `length 0..64 pattern ^\A[a-zA-Z0-9_]+\z$` (model:
/// `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
const NAME_MAX_LENGTH: usize = 64;

/// The collection URI `ListRecoveryGroups` and `CreateRecoveryGroup` are bound to, and
/// the prefix of the `GetRecoveryGroup` instance URI (model `@http`: `/recoverygroups`,
/// `/recoverygroups/{RecoveryGroupName}`).
const PATH: &str = "/recoverygroups";

/// The list response's items member (`@paginated items`, `jsonName: recoveryGroups`).
const ITEMS_FIELD: &str = "recoveryGroups";

/// The sentence a list answers for a `NextToken` this service did not mint. Written out
/// here rather than imported, so the test pins the string.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The constant, resource-agnostic sentence every not-found answer of this service's
/// resource-addressed operations carries.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

async fn create(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", PATH, Some(body)).await
}

async fn get(emulator: &Emulator, name: &str) -> Reply {
    emulator.send("GET", &format!("{PATH}/{name}"), None).await
}

async fn list(emulator: &Emulator, query: &str) -> Reply {
    let uri = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// Walk every page the list offers and return the entries it reported, in the order it
/// reported them, asserting each page honours `maxResults` and that only the last page
/// omits `nextToken`.
async fn drain_pages(emulator: &Emulator, max_results: Option<i64>, label: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let mut token: Option<String> = None;
    // More rounds than there can be pages, so a non-terminating cursor fails loudly
    // instead of looping forever.
    for _ in 0..64 {
        let mut query = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(cursor) = &token {
            query.push(format!("nextToken={cursor}"));
        }
        let page = list(emulator, &query.join("&")).await;
        assert_eq!(
            page.status, 200,
            "case {label:?}: expected a page to answer HTTP 200, got {} / {}",
            page.status, page.body
        );
        let reported = page
            .field(ITEMS_FIELD)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a {ITEMS_FIELD} array, got {}",
                    page.body
                )
            });
        if let Some(max) = max_results {
            assert!(
                reported.len() as i64 <= max,
                "case {label:?}: expected at most {max} entries on a page, got {}",
                reported.len()
            );
        }
        items.extend(reported.clone());
        match page.string("nextToken") {
            Some(next) => {
                assert_eq!(
                    reported.len() as i64,
                    max_results.unwrap_or(DEFAULT_MAX_RESULTS as i64),
                    "case {label:?}: a page that carries nextToken must be full, got {} entries",
                    reported.len()
                );
                token = Some(next);
            }
            None => return items,
        }
    }
    panic!("case {label:?}: the cursor never terminated after 64 pages");
}

/// The list entry reporting `name`, or `None` when no page carries it.
async fn find_in_list(emulator: &Emulator, name: &str, label: &str) -> Option<Value> {
    drain_pages(emulator, None, label)
        .await
        .into_iter()
        .find(|item| item.get("recoveryGroupName").and_then(Value::as_str) == Some(name))
}

/// The names the list reports, in the order it reported them.
async fn listed_names(emulator: &Emulator, max_results: Option<i64>, label: &str) -> Vec<String> {
    drain_pages(emulator, max_results, label)
        .await
        .into_iter()
        .map(|item| {
            item.get("recoveryGroupName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The ARN AWS documents for a recovery group. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty:
/// `arn:aws:route53-recovery-readiness::<account>:recovery-group/<name>`. Written out
/// here rather than taken from the handler's helper, so the test pins the format.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:recovery-group/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// A cell ARN of the form `CreateRecoveryGroup`'s `Cells` member documents ("a list of
/// the cell Amazon Resource Names (ARNs) in the recovery group").
fn cell_arn(name: &str) -> String {
    format!("arn:aws:route53-recovery-readiness::111122223333:cell/{name}")
}

fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// `value`'s `cells` member as a vector, with an absent member reading as no cells.
fn cells_of(value: &Value) -> Vec<String> {
    value
        .get("cells")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `value`'s `tags` member as a map, with an absent member reading as no tags.
fn tags_of(value: &Value) -> BTreeMap<String, String> {
    value
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// One recovery-group row: the request, and therefore exactly what every read of it must
/// report back.
struct Case {
    label: String,
    name: String,
    cells: Vec<String>,
    tags: BTreeMap<String, String>,
}

impl Case {
    fn body(&self) -> Value {
        let mut body = json!({ "recoveryGroupName": self.name });
        body["cells"] = Value::Array(
            self.cells
                .iter()
                .map(|cell| Value::String(cell.clone()))
                .collect(),
        );
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            );
        }
        body
    }

    /// Assert one reported recovery group — a create answer, a get answer or a list
    /// entry — carries exactly what this row stored.
    fn assert_reported(&self, reported: &Value, source: &str) {
        let label = &self.label;
        assert_eq!(
            reported.get("recoveryGroupName").and_then(Value::as_str),
            Some(self.name.as_str()),
            "case {label:?} via {source}: expected the stored name, got {reported}"
        );
        assert_eq!(
            reported.get("recoveryGroupArn").and_then(Value::as_str),
            Some(expected_arn(&self.name).as_str()),
            "case {label:?} via {source}: expected the minted ARN, got {reported}"
        );
        assert_eq!(
            cells_of(reported),
            self.cells,
            "case {label:?} via {source}: expected the stored cells in submitted order, got {reported}"
        );
        assert_eq!(
            tags_of(reported),
            self.tags,
            "case {label:?} via {source}: expected the stored tags, got {reported}"
        );
    }
}

/// The table the round-trip assertions run over. Rows differ along cell count, cell
/// order, name form, name length, tag count and tag value.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no cells, no tags".to_string(),
            name: "my_recovery_group".to_string(),
            cells: Vec::new(),
            tags: BTreeMap::new(),
        },
        Case {
            label: "one cell, one tag".to_string(),
            name: "single_cell".to_string(),
            cells: vec![cell_arn("cell_a")],
            tags: tags(&[("stage", "prod")]),
        },
        Case {
            label: "cells in a non-sorted order".to_string(),
            name: "Group_42".to_string(),
            cells: vec![cell_arn("zulu"), cell_arn("alpha"), cell_arn("mike")],
            tags: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        Case {
            label: "name at the published length bound".to_string(),
            name: "n".repeat(NAME_MAX_LENGTH),
            cells: vec![cell_arn("one"), cell_arn("two")],
            tags: BTreeMap::new(),
        },
        Case {
            label: "single-underscore name, empty tag value".to_string(),
            name: "_".to_string(),
            cells: vec![cell_arn("only_cell")],
            tags: tags(&[("owner", "")]),
        },
        Case {
            label: "all-digit name".to_string(),
            name: "0123456789".to_string(),
            cells: Vec::new(),
            tags: tags(&[("team", "payments"), ("env", "test")]),
        },
        Case {
            label: "cells at the emulator's ceiling".to_string(),
            name: "cells_at_ceiling".to_string(),
            cells: (0..MAX_NESTED_CELLS_PER_CELL)
                .map(|index| cell_arn(&format!("cell_{index:04}")))
                .collect(),
            tags: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn every_created_recovery_group_round_trips_through_get_and_list() {
    let emulator = Emulator::new();
    let table = cases();

    for case in &table {
        let created = create(&emulator, case.body()).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected HTTP 200 from the create, got {} / {}",
            case.label,
            created.status,
            created.body
        );
        case.assert_reported(&created.body, "CreateRecoveryGroup");

        let fetched = get(&emulator, &case.name).await;
        assert_eq!(
            fetched.status,
            200,
            "case {:?}: expected HTTP 200 from the get, got {} / {}",
            case.label,
            fetched.status,
            fetched.body
        );
        case.assert_reported(&fetched.body, "GetRecoveryGroup");

        let listed = find_in_list(&emulator, &case.name, &case.label)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the list reported no such group", case.label));
        case.assert_reported(&listed, "ListRecoveryGroups");
    }

    // And the list reports every row exactly once, in `RecoveryGroupName` order.
    let mut expected: Vec<String> = table.iter().map(|case| case.name.clone()).collect();
    expected.sort();
    let reported = listed_names(&emulator, None, "whole table").await;
    assert_eq!(
        reported, expected,
        "case \"whole table\": expected every stored group once, in key order, got {reported:?}"
    );
}

#[tokio::test]
async fn an_empty_store_reports_an_empty_recovery_group_list() {
    // The recorded unit's own case: a list against a store nothing was written to must
    // answer an empty collection and no cursor, not a refusal. Rows differ along the
    // page size asked for, including both published bounds of `MaxResults`.
    let emulator = Emulator::new();
    for query in [
        String::new(),
        format!("maxResults={MAX_RESULTS_MIN}"),
        format!("maxResults={MAX_RESULTS_MAX}"),
    ] {
        let reply = list(&emulator, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.field(ITEMS_FIELD).and_then(Value::as_array),
            Some(&Vec::new()),
            "case {query:?}: expected an empty {ITEMS_FIELD} array, got {}",
            reply.body
        );
        assert!(
            reply.string("nextToken").is_none(),
            "case {query:?}: expected no cursor on a single-page answer, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn every_stored_recovery_group_is_reported_once_in_key_order() {
    // Rows differ along how many records the store holds and how large a page the caller
    // asks for, including both published bounds of `MaxResults`.
    for stored in [0_usize, 1, 2, 5, 11] {
        let emulator = Emulator::new();
        let mut expected = Vec::new();
        for index in (0..stored).rev() {
            // Created in DESCENDING name order, so a list that reports insertion order
            // rather than key order fails.
            let name = format!("group_{index:03}");
            let created = create(&emulator, json!({ "recoveryGroupName": name })).await;
            assert_eq!(
                created.status,
                200,
                "case {stored} stored: expected HTTP 200 creating {name}, got {} / {}",
                created.status,
                created.body
            );
            expected.push(name);
        }
        // The store is a BTreeMap, so a list walks names in sorted order.
        expected.sort();

        for max_results in [
            Some(MAX_RESULTS_MIN),
            Some(2),
            Some(3),
            Some(7),
            Some(MAX_RESULTS_MAX),
            None,
        ] {
            let label = format!("{stored} stored, maxResults={max_results:?}");
            let reported = listed_names(&emulator, max_results, &label).await;
            assert_eq!(
                reported, expected,
                "case {label:?}: expected every stored group once, in key order, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    create(&emulator, json!({ "recoveryGroupName": "one" })).await;

    // One past each published bound of `MaxResults` (range 1..1000).
    for max in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let reply = list(&emulator, &format!("maxResults={max}")).await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={max}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
    }
    // And each published bound itself is accepted.
    for max in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let reply = list(&emulator, &format!("maxResults={max}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={max}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for name in ["one", "two"] {
        create(&emulator, json!({ "recoveryGroupName": name })).await;
    }

    // Tokens differing in why they are not cursors: not hex at all, odd-length hex, hex
    // that decodes to bytes that are not text, and hex of a stored key the service never
    // minted a cursor for (so it carries no cursor tag).
    for token in [
        "not-a-cursor".to_string(),
        "abc".to_string(),
        "ff".to_string(),
        hex_of("one"),
    ] {
        let reply = list(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        let message = reply.string("message").unwrap_or_default();
        assert_eq!(
            message, BAD_TOKEN_MESSAGE,
            "case {token:?}: expected the constant invalid-token sentence, got {message:?}"
        );
    }

    // A cursor the service DID mint resumes the walk where the page stopped.
    let first = list(&emulator, "maxResults=1").await;
    let cursor = first
        .string("nextToken")
        .expect("case \"minted cursor\": expected a cursor for the record the page left out");
    let second = list(&emulator, &format!("maxResults=1&nextToken={cursor}")).await;
    assert_eq!(
        second.status, 200,
        "case \"minted cursor\": expected HTTP 200, got {} / {}",
        second.status, second.body
    );
    assert_eq!(
        second
            .field(ITEMS_FIELD)
            .and_then(Value::as_array)
            .map(|items| items
                .iter()
                .filter_map(|item| item.get("recoveryGroupName").and_then(Value::as_str))
                .collect::<Vec<_>>()),
        Some(vec!["two"]),
        "case \"minted cursor\": expected the page after the first, got {}",
        second.body
    );
}

/// `value`'s bytes as lowercase hex — the encoding this service's cursors use, written
/// out here so the test does not borrow the handler's codec.
fn hex_of(value: &str) -> String {
    value.as_bytes().iter().fold(String::new(), |mut out, byte| {
        out.push_str(&format!("{byte:02x}"));
        out
    })
}

#[tokio::test]
async fn a_recovery_group_name_outside_the_modelled_pattern_is_refused_and_stores_nothing() {
    // Rows differ in how they break `^\A[a-zA-Z0-9_]+\z$` / `length 0..64`: a space, a
    // dash, a dot, a slash, emptiness, non-ASCII, and one character past the bound.
    for name in [
        "bad name!".to_string(),
        "with-dash".to_string(),
        "with.dot".to_string(),
        "with/slash".to_string(),
        String::new(),
        "ünïcode".to_string(),
        "n".repeat(NAME_MAX_LENGTH + 1),
    ] {
        let emulator = Emulator::new();
        let reply = create(&emulator, json!({ "recoveryGroupName": name })).await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        let message = reply.string("message").unwrap_or_default();
        assert!(
            message.contains("failed to satisfy constraint"),
            "case {name:?}: expected a constraint message, got {message:?}"
        );
        let listed = drain_pages(&emulator, None, &name).await;
        assert!(
            listed.is_empty(),
            "case {name:?}: a refused create must store nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_second_create_of_a_live_recovery_group_name_is_refused_as_a_conflict() {
    let emulator = Emulator::new();
    let name = "duplicated_group";
    let first = create(
        &emulator,
        json!({ "recoveryGroupName": name, "cells": [cell_arn("first")] }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case {name:?}: expected the first create to answer HTTP 200, got {} / {}",
        first.status, first.body
    );

    let second = create(
        &emulator,
        json!({ "recoveryGroupName": name, "cells": [cell_arn("second")] }),
    )
    .await;
    assert_eq!(
        second.status, 409,
        "case {name:?}: expected HTTP 409 for a second create, got {} / {}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "ConflictException",
        "case {name:?}: expected ConflictException, got {:?} / {}",
        second.error_code(),
        second.body
    );

    // The refused create left the stored record exactly as the first one wrote it, and
    // added no second entry to the list.
    let fetched = get(&emulator, name).await;
    assert_eq!(
        cells_of(&fetched.body),
        vec![cell_arn("first")],
        "case {name:?}: a refused create must not overwrite the stored cells, got {}",
        fetched.body
    );
    assert_eq!(
        listed_names(&emulator, None, name).await,
        vec![name.to_string()],
        "case {name:?}: a refused create must not add a second entry"
    );
}

#[tokio::test]
async fn getting_a_recovery_group_that_was_never_created_is_a_404() {
    let emulator = Emulator::new();
    create(&emulator, json!({ "recoveryGroupName": "live_group" })).await;

    // Rows differ in how the addressed name relates to what is stored: never created, a
    // prefix of a stored name, a suffix of one, and a case-different spelling.
    for name in ["missing_group", "live", "group", "LIVE_GROUP"] {
        let reply = get(&emulator, name).await;
        assert_eq!(
            reply.status, 404,
            "case {name:?}: expected HTTP 404, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        let message = reply.string("message").unwrap_or_default();
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {name:?}: expected the constant not-found sentence, got {message:?}"
        );
    }
    // The list still reports only the one record that was created.
    assert_eq!(
        listed_names(&emulator, None, "after the misses").await,
        vec!["live_group".to_string()],
        "case \"after the misses\": a miss must not change what the store reports"
    );
}

#[tokio::test]
async fn a_recovery_group_larger_than_the_emulator_stores_is_refused() {
    // Rows differ in which caller-supplied collection is one entry over this emulator's
    // ceiling for it.
    let too_many_cells: Vec<Value> = (0..=MAX_NESTED_CELLS_PER_CELL)
        .map(|index| Value::String(cell_arn(&format!("cell_{index:04}"))))
        .collect();
    let too_many_tags: serde_json::Map<String, Value> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key_{index:04}"), Value::String("v".to_string())))
        .collect();

    for (label, body) in [
        (
            format!("{} cells", too_many_cells.len()),
            json!({ "recoveryGroupName": "oversized", "cells": too_many_cells }),
        ),
        (
            format!("{} tags", too_many_tags.len()),
            json!({
                "recoveryGroupName": "oversized",
                "cells": [],
                "tags": Value::Object(too_many_tags.clone()),
            }),
        ),
    ] {
        let emulator = Emulator::new();
        let reply = create(&emulator, body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
        assert!(
            drain_pages(&emulator, None, &label).await.is_empty(),
            "case {label:?}: a refused create must store nothing"
        );
    }
}

#[tokio::test]
async fn a_recovery_group_store_at_its_ceiling_refuses_another_record() {
    // The emulator keeps at most MAX_RECORDS_PER_STORE records per store, so a caller
    // varying the name cannot grow the process without bound.
    let emulator = Emulator::new();
    for index in 0..MAX_RECORDS_PER_STORE {
        let name = format!("record_{index:06}");
        let created = create(&emulator, json!({ "recoveryGroupName": name })).await;
        assert_eq!(
            created.status,
            200,
            "case record {index}: expected HTTP 200 below the ceiling, got {} / {}",
            created.status,
            created.body
        );
    }

    let refused = create(&emulator, json!({ "recoveryGroupName": "one_too_many" })).await;
    assert_eq!(
        refused.status, 400,
        "case \"ceiling\": a store at {MAX_RECORDS_PER_STORE} records must refuse another, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case \"ceiling\": expected ValidationException, got {:?} / {}",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        get(&emulator, "one_too_many").await.status,
        404,
        "case \"ceiling\": the refused create must have stored nothing"
    );
    // And the list reports exactly the records the store holds, over several page sizes.
    for max_results in [Some(MAX_RESULTS_MAX), None] {
        let label = format!("ceiling, maxResults={max_results:?}");
        let reported = listed_names(&emulator, max_results, &label).await;
        assert_eq!(
            reported.len(),
            MAX_RECORDS_PER_STORE,
            "case {label:?}: expected every stored record, got {}",
            reported.len()
        );
    }
}

#[tokio::test]
async fn a_page_the_caller_sized_nothing_carries_the_default_page_size() {
    let emulator = Emulator::new();
    for index in 0..=DEFAULT_MAX_RESULTS {
        create(
            &emulator,
            json!({ "recoveryGroupName": format!("record_{index:06}") }),
        )
        .await;
    }

    let page = list(&emulator, "").await;
    let reported = page
        .field(ITEMS_FIELD)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        reported, DEFAULT_MAX_RESULTS,
        "case \"default page size\": expected {DEFAULT_MAX_RESULTS} entries, got {reported}"
    );
    assert!(
        page.string("nextToken").is_some(),
        "case \"default page size\": expected a cursor for the records the page left out"
    );
}

#[tokio::test]
async fn reading_recovery_groups_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    for case in cases().iter().take(3) {
        create(&emulator, case.body()).await;
    }

    let first = drain_pages(&emulator, None, "first read").await;
    for _ in 0..3 {
        get(&emulator, "my_recovery_group").await;
        get(&emulator, "missing_group").await;
        list(&emulator, "maxResults=1").await;
    }
    let last = drain_pages(&emulator, None, "last read").await;
    assert_eq!(
        last, first,
        "case \"repeated reads\": repeated reads must not change what the store reports"
    );
}

#[tokio::test]
async fn the_recovery_group_list_reports_only_recovery_groups() {
    // Each resource kind this service stores is its own collection, so a record written
    // through one kind's create is never reported by another kind's list — even under the
    // same name.
    let emulator = Emulator::new();
    let shared_name = "shared_name";
    for (label, path, body) in [
        (
            "recovery group",
            PATH,
            json!({ "recoveryGroupName": shared_name }),
        ),
        ("cell", "/cells", json!({ "cellName": shared_name })),
        (
            "resource set",
            "/resourcesets",
            json!({
                "resourceSetName": shared_name,
                "resourceSetType": "AWS::DynamoDB::Table",
                "resources": [],
            }),
        ),
        (
            "readiness check",
            "/readinesschecks",
            json!({ "readinessCheckName": shared_name, "resourceSetName": shared_name }),
        ),
    ] {
        let created = emulator.send("POST", path, Some(body)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from the create, got {} / {}",
            created.status, created.body
        );
    }

    let groups = drain_pages(&emulator, None, "only recovery groups").await;
    assert_eq!(
        groups.len(),
        1,
        "case \"only recovery groups\": expected exactly the one recovery group, got {groups:?}"
    );
    assert_eq!(
        groups[0].get("recoveryGroupArn").and_then(Value::as_str),
        Some(expected_arn(shared_name).as_str()),
        "case \"only recovery groups\": expected the recovery-group ARN, got {}",
        groups[0]
    );
    // And the other collections still report their own record under the same name.
    for (path, items, arn_field) in [
        ("/resourcesets", "resourceSets", "resourceSetArn"),
        ("/readinesschecks", "readinessChecks", "readinessCheckArn"),
    ] {
        let reply = emulator.send("GET", path, None).await;
        let reported = reply
            .field(items)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {path:?}: expected exactly its own record, got {reported:?}"
        );
        let arn = reported[0]
            .get(arn_field)
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !arn.contains("recovery-group/"),
            "case {path:?}: expected its own ARN, got {arn:?}"
        );
    }
}
