// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Route 53 Application Recovery Controller readiness CELL
//! operations: `ListCells` and the `CreateCell` / `GetCell` / `UpdateCell` surface a
//! list of cells is computed from.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected: an answer that is not computed from
//! persisted state cannot pass. The account the harness mounts is one that MAY create
//! readiness resources — the account-eligibility gate itself is covered by
//! `account_gate_and_not_found.rs`.
//!
//! The tests are tables. Each row is one input along an axis the rule under test
//! generalises over — nested-cell count, tag count, name form and length, stored record
//! count and page size, and the shape of an update's replacement list. The single
//! `ListCells`-on-an-empty-account call the issue recorded is one row of the pagination
//! table.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    DEFAULT_MAX_RESULTS, MAX_NESTED_CELLS_PER_CELL, MAX_TAGS_PER_RESOURCE,
};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// `MaxResults` is `range 1..1000` on `ListCells` (model:
/// `smithy_query(query="operation", operation="ListCells")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `__stringMax64PatternAAZAZ09Z`, the shape `CellName` is reported through, is
/// `length 0..64 pattern ^\A[a-zA-Z0-9_]+\z$` (model:
/// `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
const NAME_MAX_LENGTH: usize = 64;

/// The constant sentence this service answers a not-found with. Written out here rather
/// than imported, so the test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The constant sentence a list answers for a `NextToken` this service did not mint.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// A list member of a reply, with an omitted member read as the empty list: the wire may
/// carry `[]` or nothing at all for a cell that nests none.
fn strings(reply: &Reply, member: &str) -> Vec<String> {
    reply
        .field(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The same, for one entry of a list response.
fn entry_strings(entry: &Value, member: &str) -> Vec<String> {
    entry
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN AWS documents for a cell. Route 53 Application Recovery Controller readiness
/// is a GLOBAL service, so the region segment is empty:
/// `arn:aws:route53-recovery-readiness::<account>:cell/<name>`. Written out here rather
/// than taken from the handler's helper, so the test pins the format.
fn cell_arn(name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:cell/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// `POST /cells` (model: `CreateCell`).
async fn create_cell(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/cells", Some(body)).await
}

/// `GET /cells/{CellName}` (model: `GetCell`).
async fn get_cell(emulator: &Emulator, name: &str) -> Reply {
    emulator.send("GET", &format!("/cells/{name}"), None).await
}

/// `PUT /cells/{CellName}` (model: `UpdateCell`).
async fn update_cell(emulator: &Emulator, name: &str, cells: &[String]) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/cells/{name}"),
            Some(json!({ "cells": cells })),
        )
        .await
}

/// `GET /cells` (model: `ListCells`).
async fn list_cells(emulator: &Emulator, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/cells".to_string()
    } else {
        format!("/cells?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// Store a cell and assert the create succeeded, returning the ARN it reported.
async fn seed_cell(emulator: &Emulator, name: &str, nested: &[String], label: &str) -> String {
    let reply = create_cell(emulator, json!({ "cellName": name, "cells": nested })).await;
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected CreateCell({name}) to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    reply
        .string("cellArn")
        .unwrap_or_else(|| panic!("case {label:?}: CreateCell({name}) reported no cellArn"))
}

/// Walk every page `ListCells` offers and return the entries it reported, in the order
/// it reported them, asserting each page honours `maxResults` and that only the last
/// page omits `nextToken`.
async fn drain_cells(emulator: &Emulator, max_results: Option<i64>, label: &str) -> Vec<Value> {
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
        let page = list_cells(emulator, &query.join("&")).await;
        assert_eq!(
            page.status, 200,
            "case {label:?}: expected a page to answer HTTP 200, got {} / {}",
            page.status, page.body
        );
        let reported = page
            .field("cells")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected a cells array, got {}", page.body)
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

/// The `cellName` of every entry `ListCells` reports, in the order it reported them.
async fn listed_names(emulator: &Emulator, label: &str) -> Vec<String> {
    drain_cells(emulator, None, label)
        .await
        .iter()
        .map(|item| {
            item.get("cellName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `ListCells` entry reporting `name`, or `None` when no page carries it.
async fn find_cell(emulator: &Emulator, name: &str, label: &str) -> Option<Value> {
    drain_cells(emulator, None, label)
        .await
        .into_iter()
        .find(|item| item.get("cellName").and_then(Value::as_str) == Some(name))
}

fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn tags_json(tags: &BTreeMap<String, String>) -> Option<Value> {
    if tags.is_empty() {
        return None;
    }
    Some(Value::Object(
        tags.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    ))
}

/// One cell row: the name it is created under, how many leaf cells it nests, and its
/// tags. The nested leaves are created first, so the nesting is between cells that
/// really exist in the store.
struct CellCase {
    label: &'static str,
    name: String,
    nested_leaves: usize,
    tags: BTreeMap<String, String>,
}

fn cell_cases() -> Vec<CellCase> {
    vec![
        CellCase {
            label: "no nested cells, no tags",
            name: "cell_1".to_string(),
            nested_leaves: 0,
            tags: BTreeMap::new(),
        },
        CellCase {
            label: "one nested cell",
            name: "parent".to_string(),
            nested_leaves: 1,
            tags: BTreeMap::new(),
        },
        CellCase {
            label: "three nested cells and three tags",
            name: "us_west_2".to_string(),
            nested_leaves: 3,
            tags: tags(&[("env", "prod"), ("owner", "team"), ("tier", "1")]),
        },
        CellCase {
            label: "single-character name",
            name: "c".to_string(),
            nested_leaves: 2,
            tags: tags(&[("k", "v")]),
        },
        CellCase {
            label: "digits and underscores only",
            name: "9_0_1".to_string(),
            nested_leaves: 0,
            tags: tags(&[("Only", "One")]),
        },
        CellCase {
            // The longest name the output shape's `length 0..64` admits.
            label: "name at the 64-character maximum",
            name: "c".repeat(NAME_MAX_LENGTH),
            nested_leaves: 1,
            tags: BTreeMap::new(),
        },
        CellCase {
            label: "tag map at the emulator's per-record ceiling",
            name: "max_tags".to_string(),
            nested_leaves: 0,
            tags: (0..MAX_TAGS_PER_RESOURCE)
                .map(|i| (format!("k{i:02}"), format!("v{i:02}")))
                .collect(),
        },
    ]
}

/// RULE: a created cell is persisted under its `CellName`, and every read of it —
/// `GetCell` and the `ListCells` entry alike — reports the values it was created with,
/// the ARN the service minted, and the parent readiness scopes derived from whichever
/// stored records nest it.
#[tokio::test]
async fn a_created_cell_is_reported_identically_by_get_and_list() {
    for case in cell_cases() {
        let emulator = Emulator::new();
        let label = case.label;

        // The leaves first, so the parent's `cells` names cells that exist.
        let mut leaf_arns = Vec::new();
        for index in 0..case.nested_leaves {
            let leaf = format!("leaf_{index}");
            let arn = seed_cell(&emulator, &leaf, &[], label).await;
            assert_eq!(
                arn,
                cell_arn(&leaf),
                "case {label:?}: expected the minted ARN of {leaf}, got {arn:?}"
            );
            leaf_arns.push(arn);
        }

        let mut body = json!({ "cellName": case.name, "cells": leaf_arns });
        if let Some(value) = tags_json(&case.tags) {
            body["tags"] = value;
        }
        let created = create_cell(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateCell to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("cellArn"),
            Some(cell_arn(&case.name)),
            "case {label:?}: expected the minted cell ARN, got {:?}",
            created.string("cellArn")
        );
        assert_eq!(
            created.string("cellName").as_deref(),
            Some(case.name.as_str()),
            "case {label:?}: expected cellName {:?}, got {:?}",
            case.name,
            created.string("cellName")
        );
        assert_eq!(
            strings(&created, "cells"),
            leaf_arns,
            "case {label:?}: expected the submitted nested cells, got {:?}",
            strings(&created, "cells")
        );
        assert_eq!(
            strings(&created, "parentReadinessScopes"),
            Vec::<String>::new(),
            "case {label:?}: a cell nothing nests has no parent readiness scope, got {:?}",
            strings(&created, "parentReadinessScopes")
        );
        assert_eq!(
            created.field("tags").cloned(),
            tags_json(&case.tags),
            "case {label:?}: expected tags {:?}, got {:?}",
            tags_json(&case.tags),
            created.field("tags")
        );

        // GetCell reports the same record.
        let fetched = get_cell(&emulator, &case.name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected GetCell to answer HTTP 200, got {} / {}",
            fetched.status, fetched.body
        );
        for member in ["cellArn", "cellName", "tags"] {
            assert_eq!(
                fetched.field(member).cloned(),
                created.field(member).cloned(),
                "case {label:?}: expected GetCell to report the created {member}, got {:?}",
                fetched.field(member)
            );
        }
        assert_eq!(
            strings(&fetched, "cells"),
            leaf_arns,
            "case {label:?}: expected GetCell to report the stored nested cells, got {:?}",
            strings(&fetched, "cells")
        );

        // And so does the ListCells entry for it.
        let listed = find_cell(&emulator, &case.name, label)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: ListCells never reported {}", case.name));
        assert_eq!(
            listed.get("cellArn").and_then(Value::as_str),
            Some(cell_arn(&case.name).as_str()),
            "case {label:?}: expected the ListCells entry to carry the minted ARN, got {listed}"
        );
        assert_eq!(
            entry_strings(&listed, "cells"),
            leaf_arns,
            "case {label:?}: expected the ListCells entry to carry the stored nested cells, got {listed}"
        );
        assert_eq!(
            listed.get("tags").cloned().filter(|v| !v.is_null()),
            tags_json(&case.tags),
            "case {label:?}: expected the ListCells entry to carry the stored tags, got {listed}"
        );

        // RULE, derived member: each leaf reports the nesting cell's ARN as its parent
        // readiness scope — on GetCell and in the list alike.
        let parent_arn = cell_arn(&case.name);
        for index in 0..case.nested_leaves {
            let leaf = format!("leaf_{index}");
            let leaf_reply = get_cell(&emulator, &leaf).await;
            assert_eq!(
                strings(&leaf_reply, "parentReadinessScopes"),
                vec![parent_arn.clone()],
                "case {label:?}: expected {leaf} to report its nesting cell as parent scope, got {:?}",
                strings(&leaf_reply, "parentReadinessScopes")
            );
            let leaf_entry = find_cell(&emulator, &leaf, label)
                .await
                .unwrap_or_else(|| panic!("case {label:?}: ListCells never reported {leaf}"));
            assert_eq!(
                entry_strings(&leaf_entry, "parentReadinessScopes"),
                vec![parent_arn.clone()],
                "case {label:?}: expected the ListCells entry for {leaf} to carry its parent scope, got {leaf_entry}"
            );
        }

        // Every stored cell, and nothing else, is reported exactly once.
        let mut reported = listed_names(&emulator, label).await;
        let mut expected: Vec<String> = (0..case.nested_leaves)
            .map(|index| format!("leaf_{index}"))
            .chain(std::iter::once(case.name.clone()))
            .collect();
        reported.sort();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected ListCells to report exactly the stored cells, got {reported:?}"
        );
    }
}

/// RULE: `ListCells` reports every stored cell exactly once in `CellName` order, in
/// pages of at most `MaxResults` entries (a service-chosen default when the caller names
/// none), where each non-final page carries the opaque `NextToken` the next page resumes
/// from; an account with no cells gets a well-formed empty page and no token.
#[tokio::test]
async fn list_cells_pages_every_stored_cell_in_name_order() {
    // Counts either side of the page sizes below, including the empty account the issue
    // recorded, and page sizes at both published bounds of `MaxResults`.
    for count in [0_usize, 1, 2, 5, 7] {
        let emulator = Emulator::new();
        let label = format!("{count} stored cells");
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            // Created out of name order on purpose: the list's order must come from the
            // store's key order, not from insertion order.
            let name = format!("cell_{:02}", count - index);
            seed_cell(&emulator, &name, &[], &label).await;
            expected.push(name);
        }
        expected.sort();

        for max_results in [
            None,
            Some(MAX_RESULTS_MIN),
            Some(2),
            Some(3),
            Some(MAX_RESULTS_MAX),
        ] {
            let label = format!("{label}, maxResults={max_results:?}");
            let names: Vec<String> = drain_cells(&emulator, max_results, &label)
                .await
                .iter()
                .map(|item| {
                    item.get("cellName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                names, expected,
                "case {label:?}: expected every stored cell once in name order, got {names:?}"
            );
        }

        // The first page of an account with no cells is well formed: HTTP 200, an empty
        // list and no token to follow.
        if count == 0 {
            let page = list_cells(&emulator, "").await;
            assert_eq!(
                page.status, 200,
                "case {label:?}: expected HTTP 200 for an empty account, got {} / {}",
                page.status, page.body
            );
            assert_eq!(
                strings(&page, "cells"),
                Vec::<String>::new(),
                "case {label:?}: expected no cells, got {}",
                page.body
            );
            assert_eq!(
                page.string("nextToken"),
                None,
                "case {label:?}: expected no nextToken on a single-page answer, got {:?}",
                page.string("nextToken")
            );
        }
    }
}

/// One update row: what the cell nests to begin with, and the list that replaces it.
struct UpdateCase {
    label: &'static str,
    initial: usize,
    replacement: Vec<usize>,
}

/// RULE: `UpdateCell` stores the submitted `Cells` list as the cell's complete nested
/// list — the previous entries are replaced, never merged — and every later read reports
/// the replacement, including the parent readiness scopes it implies.
#[tokio::test]
async fn update_cell_replaces_the_nested_list_instead_of_merging_it() {
    let cases = [
        UpdateCase {
            label: "empty -> one",
            initial: 0,
            replacement: vec![0],
        },
        UpdateCase {
            label: "one -> empty",
            initial: 1,
            replacement: vec![],
        },
        UpdateCase {
            label: "two -> a disjoint one",
            initial: 2,
            replacement: vec![2],
        },
        UpdateCase {
            label: "three -> an overlapping two",
            initial: 3,
            replacement: vec![0, 3],
        },
        UpdateCase {
            label: "two -> the same two reordered",
            initial: 2,
            replacement: vec![1, 0],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let label = case.label;
        // Four leaves exist throughout, so a replacement can name a leaf the initial
        // list did not.
        let mut leaves = Vec::new();
        for index in 0..4 {
            leaves.push(seed_cell(&emulator, &format!("leaf_{index}"), &[], label).await);
        }
        let initial: Vec<String> = leaves[..case.initial].to_vec();
        seed_cell(&emulator, "parent", &initial, label).await;

        let replacement: Vec<String> = case
            .replacement
            .iter()
            .map(|index| leaves[*index].clone())
            .collect();
        let updated = update_cell(&emulator, "parent", &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected UpdateCell to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_eq!(
            strings(&updated, "cells"),
            replacement,
            "case {label:?}: expected the replacement list verbatim, got {:?}",
            strings(&updated, "cells")
        );
        assert_eq!(
            updated.string("cellArn"),
            Some(cell_arn("parent")),
            "case {label:?}: expected the update to keep the cell's ARN, got {:?}",
            updated.string("cellArn")
        );

        let fetched = get_cell(&emulator, "parent").await;
        assert_eq!(
            strings(&fetched, "cells"),
            replacement,
            "case {label:?}: expected GetCell to report the replacement list, got {:?}",
            strings(&fetched, "cells")
        );
        let listed = find_cell(&emulator, "parent", label)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: ListCells never reported the parent"));
        assert_eq!(
            entry_strings(&listed, "cells"),
            replacement,
            "case {label:?}: expected the ListCells entry to report the replacement list, got {listed}"
        );

        // The derived parent scopes follow the replacement: exactly the leaves it names
        // report the parent, and the ones it dropped report none.
        for (index, leaf_arn) in leaves.iter().enumerate() {
            let leaf = format!("leaf_{index}");
            let expected = if replacement.contains(leaf_arn) {
                vec![cell_arn("parent")]
            } else {
                Vec::new()
            };
            let reply = get_cell(&emulator, &leaf).await;
            assert_eq!(
                strings(&reply, "parentReadinessScopes"),
                expected,
                "case {label:?}: expected {leaf} to report parent scopes {expected:?}, got {:?}",
                strings(&reply, "parentReadinessScopes")
            );
        }
    }
}

/// RULE: an operation addressing a cell the account does not hold is refused with the
/// modelled 404 `ResourceNotFoundException` carrying this service's constant sentence,
/// and refusing it stores nothing — so the answer is the same however often it is asked.
#[tokio::test]
async fn reads_and_updates_of_an_absent_cell_answer_the_modeled_404() {
    let emulator = Emulator::new();
    seed_cell(&emulator, "existing_cell", &[], "seed").await;

    // Names that exist nowhere, including ones that differ from the stored name only in
    // case or in being a prefix of it.
    for name in ["ghost_cell_1", "existing", "Existing_Cell", "a"] {
        for attempt in 1..=2 {
            for (operation, reply) in [
                ("GetCell", get_cell(&emulator, name).await),
                ("UpdateCell", update_cell(&emulator, name, &[]).await),
            ] {
                let label = format!("{operation}({name}) attempt {attempt}");
                assert_eq!(
                    reply.status, 404,
                    "case {label:?}: expected HTTP 404, got {} / {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "ResourceNotFoundException",
                    "case {label:?}: expected ResourceNotFoundException, got {:?}",
                    reply.error_code()
                );
                let message = reply.string("message").unwrap_or_default();
                assert_eq!(
                    message, NOT_FOUND_MESSAGE,
                    "case {label:?}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {message:?}"
                );
            }
        }
    }

    // The refusals left the store as it was: still exactly the one seeded cell.
    let names = listed_names(&emulator, "after the refusals").await;
    assert_eq!(
        names,
        vec!["existing_cell".to_string()],
        "expected the refusals to store nothing, got {names:?}"
    );
}

/// RULE: a `CellName` the service could not report back — one outside the
/// `length 0..64 pattern ^\A[a-zA-Z0-9_]+\z$` shape every output reports it through — is
/// refused with a 400 `ValidationException` and no record is stored; a name inside it is
/// stored and listed.
#[tokio::test]
async fn a_cell_name_outside_the_modeled_shape_is_refused() {
    let emulator = Emulator::new();

    let rejected = [
        ("empty", String::new()),
        ("hyphen", "my-cell".to_string()),
        ("dot", "my.cell".to_string()),
        ("space", "my cell".to_string()),
        ("slash", "a/b".to_string()),
        ("non-ascii", "célula".to_string()),
        // One character past the published maximum.
        ("65 characters", "c".repeat(NAME_MAX_LENGTH + 1)),
    ];
    for (label, name) in &rejected {
        let reply = create_cell(&emulator, json!({ "cellName": name, "cells": [] })).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400 for {name:?}, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException for {name:?}, got {:?}",
            reply.error_code()
        );
    }

    let accepted = [
        ("single character", "c".to_string()),
        ("underscores", "_leading_and_trailing_".to_string()),
        ("digits", "012345".to_string()),
        ("at the maximum length", "c".repeat(NAME_MAX_LENGTH)),
    ];
    for (label, name) in &accepted {
        let reply = create_cell(&emulator, json!({ "cellName": name, "cells": [] })).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 for {name:?}, got {} / {}",
            reply.status, reply.body
        );
    }

    // Only the accepted names reached the store.
    let mut names = listed_names(&emulator, "after the name table").await;
    let mut expected: Vec<String> = accepted.iter().map(|(_, name)| name.clone()).collect();
    names.sort();
    expected.sort();
    assert_eq!(
        names, expected,
        "expected only the accepted names to be stored, got {names:?}"
    );
}

/// RULE: creating a cell whose name is already taken is refused with the modelled 409
/// `ConflictException` and leaves the stored record untouched.
#[tokio::test]
async fn creating_a_cell_twice_is_refused_as_a_conflict() {
    let emulator = Emulator::new();
    let leaf = seed_cell(&emulator, "leaf_0", &[], "conflict").await;
    seed_cell(&emulator, "taken", std::slice::from_ref(&leaf), "conflict").await;

    // The refusal does not depend on what the second request carries.
    for second in [
        json!({ "cellName": "taken", "cells": [] }),
        json!({ "cellName": "taken", "cells": [leaf.clone()] }),
        json!({ "cellName": "taken", "cells": [], "tags": { "k": "v" } }),
    ] {
        let label = format!("second create {second}");
        let reply = create_cell(&emulator, second.clone()).await;
        assert_eq!(
            reply.status, 409,
            "case {label:?}: expected HTTP 409, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {:?}",
            reply.error_code()
        );

        let stored = get_cell(&emulator, "taken").await;
        assert_eq!(
            strings(&stored, "cells"),
            vec![leaf.clone()],
            "case {label:?}: expected the stored cell to be untouched, got {:?}",
            strings(&stored, "cells")
        );
        assert_eq!(
            stored.field("tags").cloned(),
            None,
            "case {label:?}: expected the stored cell to keep its tags, got {:?}",
            stored.field("tags")
        );
    }
}

/// RULE: a `ListCells` request outside the modelled input constraints is refused with a
/// 400 `ValidationException`: `MaxResults` is `range 1..1000`, and a `NextToken` this
/// service did not mint is not a resume point.
#[tokio::test]
async fn list_cells_refuses_a_bad_page_size_or_token() {
    let emulator = Emulator::new();
    seed_cell(&emulator, "cell_1", &[], "paging bounds").await;
    seed_cell(&emulator, "cell_2", &[], "paging bounds").await;

    // Each published bound of `MaxResults` is accepted, one past each is refused.
    for (label, max_results, expected_status) in [
        ("at the minimum", MAX_RESULTS_MIN, 200),
        ("at the maximum", MAX_RESULTS_MAX, 200),
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("one past the maximum", MAX_RESULTS_MAX + 1, 400),
    ] {
        let reply = list_cells(&emulator, &format!("maxResults={max_results}")).await;
        assert_eq!(
            reply.status, expected_status,
            "case {label:?}: expected HTTP {expected_status} for maxResults={max_results}, got {} / {}",
            reply.status, reply.body
        );
        if expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {:?}",
                reply.error_code()
            );
        }
    }

    for (label, token) in [
        ("not hex", "not-a-valid-token!!"),
        ("hex without this service's tag", "6162636465"),
        ("empty", ""),
    ] {
        let reply = list_cells(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400 for nextToken={token:?}, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            reply.error_code()
        );
        let message = reply.string("message").unwrap_or_default();
        assert_eq!(
            message, BAD_TOKEN_MESSAGE,
            "case {label:?}: expected the constant message {BAD_TOKEN_MESSAGE:?}, got {message:?}"
        );
    }

    // A token this service DID mint resumes the walk.
    let first = list_cells(&emulator, "maxResults=1").await;
    let token = first
        .string("nextToken")
        .expect("a full page must carry a nextToken");
    let second = list_cells(&emulator, &format!("maxResults=1&nextToken={token}")).await;
    assert_eq!(
        second.status, 200,
        "expected a minted token to be honoured, got {} / {}",
        second.status, second.body
    );
}

/// RULE: a cell record must not grow past the nested-cell ceiling this emulator stores,
/// on create or on update; a list one entry past it is refused with a 400
/// `ValidationException` and changes nothing.
#[tokio::test]
async fn a_nested_cell_list_past_the_stored_ceiling_is_refused() {
    let emulator = Emulator::new();
    let at_ceiling: Vec<String> = (0..MAX_NESTED_CELLS_PER_CELL)
        .map(|index| cell_arn(&format!("child_{index}")))
        .collect();
    let mut past_ceiling = at_ceiling.clone();
    past_ceiling.push(cell_arn("one_too_many"));

    let accepted =
        create_cell(&emulator, json!({ "cellName": "at_ceiling", "cells": at_ceiling })).await;
    assert_eq!(
        accepted.status, 200,
        "case \"create at the ceiling\": expected HTTP 200, got {} / {}",
        accepted.status, accepted.body
    );

    let refused = create_cell(
        &emulator,
        json!({ "cellName": "past_ceiling", "cells": past_ceiling.clone() }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"create past the ceiling\": expected HTTP 400, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case \"create past the ceiling\": expected ValidationException, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        get_cell(&emulator, "past_ceiling").await.status,
        404,
        "case \"create past the ceiling\": the refused cell must not have been stored"
    );

    let refused = update_cell(&emulator, "at_ceiling", &past_ceiling).await;
    assert_eq!(
        refused.status, 400,
        "case \"update past the ceiling\": expected HTTP 400, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        strings(&get_cell(&emulator, "at_ceiling").await, "cells").len(),
        MAX_NESTED_CELLS_PER_CELL,
        "case \"update past the ceiling\": the refused update must change nothing"
    );
}
