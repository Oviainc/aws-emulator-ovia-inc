// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONTAINMENT CARDINALITY of a cell: a cell is contained by
//! at most one readiness scope — one parent cell or one recovery group — so every
//! handler that reports a cell reports at most one `parentReadinessScopes` element, and
//! a write that would place an already-contained cell into a second scope is refused
//! with the modelled `ValidationException` (HTTP 400) and stores nothing.
//!
//! The scope's own documentation is where the cardinality comes from: it "can be a cell
//! Amazon Resource Name (ARN) or a recovery group ARN. This is a list but currently can
//! have only one element" (model: `smithy_query(query="doc", shape="CellOutput",
//! member="ParentReadinessScopes")`, the identical sentence on
//! `CreateCellResponse` / `GetCellResponse` / `UpdateCellResponse`).
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected: an answer that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables. Rows differ along what the rule generalises over — which kind
//! of scope holds the cell first (a parent cell, a recovery group), which write tries to
//! hold it second (`CreateCell`, `CreateRecoveryGroup`, `UpdateCell`), how many free
//! cells travel in the same list and where the contained one sits in it, whether it is
//! named twice, how the first scope releases it, and how deep the containment nests.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::MAX_NESTED_CELLS_PER_CELL;

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out here
/// rather than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// A list member of a reply, with an omitted member read as the empty list: the wire may
/// carry `[]` or nothing at all for a cell nothing contains.
fn strings(reply: &Reply, member: &str) -> Vec<String> {
    values(reply.field(member))
}

fn entry_strings(entry: &Value, member: &str) -> Vec<String> {
    values(entry.get(member))
}

fn values(member: Option<&Value>) -> Vec<String> {
    member
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn create_cell(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/cells", Some(body)).await
}

async fn get_cell(emulator: &Emulator, name: &str) -> Reply {
    emulator.send("GET", &format!("/cells/{name}"), None).await
}

async fn update_cell(emulator: &Emulator, name: &str, cells: &[String]) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/cells/{name}"),
            Some(json!({ "cells": cells })),
        )
        .await
}

async fn create_group(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/recoverygroups", Some(body)).await
}

/// The `ListCells` entry for `name`, walking every page the list offers.
async fn listed_cell(emulator: &Emulator, name: &str) -> Option<Value> {
    let mut token: Option<String> = None;
    for _ in 0..32 {
        let uri = match &token {
            Some(cursor) => format!("/cells?nextToken={cursor}"),
            None => "/cells".to_string(),
        };
        let page = emulator.send("GET", &uri, None).await;
        assert_eq!(
            page.status, 200,
            "ListCells: expected HTTP 200, got {} / {}",
            page.status, page.body
        );
        let entries = page
            .field("cells")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in entries {
            if entry.get("cellName").and_then(Value::as_str) == Some(name) {
                return Some(entry);
            }
        }
        token = page.string("nextToken");
        token.as_ref()?;
    }
    None
}

/// Assert a seeding write succeeded, so a fixture that fails to store fails where it
/// was written rather than later as a wrong scope.
fn seed(reply: Reply, label: &str, what: &str) {
    assert_eq!(
        reply.status, 200,
        "fixture {label:?}: expected {what} to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

/// Assert `reply` is the modelled refusal of a containment that would give a cell a
/// second scope: HTTP 400 and the `ValidationException` error code.
fn assert_refused(label: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {label:?}: expected HTTP 400, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ValidationException",
        "case {label:?}: expected the error code ValidationException, got {} / {}",
        reply.error_code(),
        reply.body
    );
    assert!(
        reply.string("message").is_some_and(|m| !m.is_empty()),
        "case {label:?}: expected a refusal message, got {}",
        reply.body
    );
}

/// Assert every path that reports a cell reports exactly `expected` as its readiness
/// scope — which is at most one element.
async fn assert_scope(emulator: &Emulator, label: &str, cell: &str, expected: &[String]) {
    assert!(
        expected.len() <= 1,
        "case {label:?}: the expectation itself must hold at most one scope, got {expected:?}"
    );
    let fetched = get_cell(emulator, cell).await;
    assert_eq!(
        fetched.status, 200,
        "case {label:?}: expected GetCell({cell}) to answer HTTP 200, got {} / {}",
        fetched.status, fetched.body
    );
    assert_eq!(
        strings(&fetched, "parentReadinessScopes"),
        expected.to_vec(),
        "case {label:?}: expected GetCell({cell}) to report scopes {expected:?}, got {:?}",
        strings(&fetched, "parentReadinessScopes")
    );
    let listed = listed_cell(emulator, cell)
        .await
        .unwrap_or_else(|| panic!("case {label:?}: ListCells never reported {cell}"));
    assert_eq!(
        entry_strings(&listed, "parentReadinessScopes"),
        expected.to_vec(),
        "case {label:?}: expected the ListCells entry for {cell} to carry scopes {expected:?}, got {listed}"
    );
}

/// Which kind of scope holds the cell first.
#[derive(Clone, Copy, Debug)]
enum Holder {
    ParentCell,
    RecoveryGroup,
}

/// Which write tries to hold it a second time.
#[derive(Clone, Copy, Debug)]
enum Second {
    CreateCell,
    CreateGroup,
    UpdateCell,
}

#[derive(Debug)]
struct Case {
    label: &'static str,
    first: Holder,
    second: Second,
    /// How many cells nothing contains travel in the second write's list.
    free_cells: usize,
    /// Whether the already-contained cell is named twice in that list.
    repeated: bool,
    /// Whether the already-contained cell sits at the FRONT of that list.
    first_in_list: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a parent cell, then a second parent cell",
            first: Holder::ParentCell,
            second: Second::CreateCell,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "a parent cell, then a recovery group",
            first: Holder::ParentCell,
            second: Second::CreateGroup,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "a recovery group, then a parent cell",
            first: Holder::RecoveryGroup,
            second: Second::CreateCell,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "a recovery group, then a second recovery group",
            first: Holder::RecoveryGroup,
            second: Second::CreateGroup,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "a parent cell, then an existing cell updated to take it",
            first: Holder::ParentCell,
            second: Second::UpdateCell,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "a recovery group, then an existing cell updated to take it",
            first: Holder::RecoveryGroup,
            second: Second::UpdateCell,
            free_cells: 0,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "the contained cell last in a list of free cells",
            first: Holder::ParentCell,
            second: Second::CreateCell,
            free_cells: 3,
            repeated: false,
            first_in_list: false,
        },
        Case {
            label: "the contained cell first in a list of free cells",
            first: Holder::RecoveryGroup,
            second: Second::CreateGroup,
            free_cells: 3,
            repeated: false,
            first_in_list: true,
        },
        Case {
            label: "the contained cell named twice",
            first: Holder::ParentCell,
            second: Second::UpdateCell,
            free_cells: 1,
            repeated: true,
            first_in_list: false,
        },
        Case {
            label: "a list at the emulator's nested-cell ceiling",
            first: Holder::RecoveryGroup,
            second: Second::CreateCell,
            free_cells: MAX_NESTED_CELLS_PER_CELL - 1,
            repeated: false,
            first_in_list: false,
        },
    ]
}

/// RULE: a cell is contained by at most one readiness scope, so a write that would place
/// an already-contained cell into a second scope is refused with the modelled
/// `ValidationException` (400), stores nothing, and leaves the one scope the cell had.
#[tokio::test]
async fn a_cell_cannot_be_placed_into_a_second_readiness_scope() {
    for case in cases() {
        let label = case.label;
        let emulator = Emulator::new();

        // The cell every row is about, and the cells nothing contains that travel beside
        // it in the second write's list.
        seed(
            create_cell(&emulator, json!({ "cellName": "child_cell" })).await,
            label,
            "CreateCell(child_cell)",
        );
        let child = arn("cell", "child_cell");
        let mut free: Vec<String> = Vec::new();
        for index in 0..case.free_cells {
            let name = format!("free_{index:04}");
            seed(
                create_cell(&emulator, json!({ "cellName": name })).await,
                label,
                "CreateCell(a free cell)",
            );
            free.push(arn("cell", &name));
        }

        // The ONE scope that holds the child.
        let holder = match case.first {
            Holder::ParentCell => {
                seed(
                    create_cell(
                        &emulator,
                        json!({ "cellName": "scope_one", "cells": [child.clone()] }),
                    )
                    .await,
                    label,
                    "CreateCell(scope_one)",
                );
                arn("cell", "scope_one")
            }
            Holder::RecoveryGroup => {
                seed(
                    create_group(
                        &emulator,
                        json!({ "recoveryGroupName": "scope_one", "cells": [child.clone()] }),
                    )
                    .await,
                    label,
                    "CreateRecoveryGroup(scope_one)",
                );
                arn("recovery-group", "scope_one")
            }
        };
        assert_scope(&emulator, label, "child_cell", std::slice::from_ref(&holder)).await;

        // The list the second write submits.
        let mut cells: Vec<String> = Vec::new();
        if case.first_in_list {
            cells.push(child.clone());
        }
        cells.extend(free.iter().cloned());
        if !case.first_in_list || case.repeated {
            cells.push(child.clone());
        }

        // The second write, which must be refused.
        let refused = match case.second {
            Second::CreateCell => {
                create_cell(&emulator, json!({ "cellName": "scope_two", "cells": cells })).await
            }
            Second::CreateGroup => {
                create_group(
                    &emulator,
                    json!({ "recoveryGroupName": "scope_two", "cells": cells }),
                )
                .await
            }
            Second::UpdateCell => {
                seed(
                    create_cell(&emulator, json!({ "cellName": "scope_two", "cells": [] })).await,
                    label,
                    "CreateCell(scope_two)",
                );
                update_cell(&emulator, "scope_two", &cells).await
            }
        };
        assert_refused(label, &refused);

        // Nothing the refused write named was stored: the create left no record, and the
        // update left the previous list in place.
        match case.second {
            Second::CreateCell => {
                let ghost = get_cell(&emulator, "scope_two").await;
                assert_eq!(
                    ghost.status, 404,
                    "case {label:?}: a refused create must store no cell, got {} / {}",
                    ghost.status, ghost.body
                );
            }
            Second::CreateGroup => {
                let ghost = emulator
                    .send("GET", "/recoverygroups/scope_two", None)
                    .await;
                assert_eq!(
                    ghost.status, 404,
                    "case {label:?}: a refused create must store no recovery group, got {} / {}",
                    ghost.status, ghost.body
                );
            }
            Second::UpdateCell => {
                let kept = get_cell(&emulator, "scope_two").await;
                assert_eq!(
                    strings(&kept, "cells"),
                    Vec::<String>::new(),
                    "case {label:?}: a refused update must not replace the stored list, got {:?}",
                    strings(&kept, "cells")
                );
            }
        }

        // And the child still reports exactly the one scope it had.
        assert_scope(&emulator, label, "child_cell", &[holder]).await;
        for index in 0..free.len() {
            let name = format!("free_{index:04}");
            assert_scope(&emulator, label, &name, &[]).await;
        }
    }
}

/// RULE, the other side of it: a containment a scope already has may be re-written, and
/// a cell the holding scope RELEASES may then be taken by another scope — the cell still
/// reporting exactly one scope throughout. Rows differ along how the release happens.
#[tokio::test]
async fn a_released_cell_can_be_taken_by_another_scope() {
    #[derive(Clone, Copy, Debug)]
    enum Release {
        /// The parent cell's list is replaced with one that no longer names the child.
        UpdateParent,
        /// The parent cell is deleted.
        DeleteParent,
        /// The holding recovery group is deleted.
        DeleteGroup,
    }

    for release in [
        Release::UpdateParent,
        Release::DeleteParent,
        Release::DeleteGroup,
    ] {
        let label = format!("{release:?}");
        let label = label.as_str();
        let emulator = Emulator::new();
        seed(
            create_cell(&emulator, json!({ "cellName": "child_cell" })).await,
            label,
            "CreateCell(child_cell)",
        );
        let child = arn("cell", "child_cell");

        let holder = match release {
            Release::UpdateParent | Release::DeleteParent => {
                seed(
                    create_cell(
                        &emulator,
                        json!({ "cellName": "scope_one", "cells": [child.clone()] }),
                    )
                    .await,
                    label,
                    "CreateCell(scope_one)",
                );
                arn("cell", "scope_one")
            }
            Release::DeleteGroup => {
                seed(
                    create_group(
                        &emulator,
                        json!({ "recoveryGroupName": "scope_one", "cells": [child.clone()] }),
                    )
                    .await,
                    label,
                    "CreateRecoveryGroup(scope_one)",
                );
                arn("recovery-group", "scope_one")
            }
        };
        assert_scope(&emulator, label, "child_cell", std::slice::from_ref(&holder)).await;

        // Re-writing the containment the holder already has is not a second scope.
        if matches!(release, Release::UpdateParent | Release::DeleteParent) {
            let again = update_cell(&emulator, "scope_one", std::slice::from_ref(&child)).await;
            assert_eq!(
                again.status, 200,
                "case {label:?}: re-writing the same containment must be accepted, got {} / {}",
                again.status, again.body
            );
            assert_scope(&emulator, label, "child_cell", std::slice::from_ref(&holder)).await;
        }

        // Before the release, another scope cannot take the child.
        let refused = create_cell(
            &emulator,
            json!({ "cellName": "scope_two", "cells": [child.clone()] }),
        )
        .await;
        assert_refused(label, &refused);

        // The release.
        match release {
            Release::UpdateParent => {
                let emptied = update_cell(&emulator, "scope_one", &[]).await;
                assert_eq!(
                    emptied.status, 200,
                    "case {label:?}: expected the release to answer HTTP 200, got {} / {}",
                    emptied.status, emptied.body
                );
            }
            Release::DeleteParent => {
                let deleted = emulator.send("DELETE", "/cells/scope_one", None).await;
                assert_eq!(
                    deleted.status, 204,
                    "case {label:?}: expected the delete to answer HTTP 204, got {} / {}",
                    deleted.status, deleted.body
                );
            }
            Release::DeleteGroup => {
                let deleted = emulator
                    .send("DELETE", "/recoverygroups/scope_one", None)
                    .await;
                assert_eq!(
                    deleted.status, 204,
                    "case {label:?}: expected the delete to answer HTTP 204, got {} / {}",
                    deleted.status, deleted.body
                );
            }
        }
        assert_scope(&emulator, label, "child_cell", &[]).await;

        // And now another scope may take it, which the create reports itself.
        let taken = create_cell(
            &emulator,
            json!({ "cellName": "scope_two", "cells": [child.clone()] }),
        )
        .await;
        assert_eq!(
            taken.status, 200,
            "case {label:?}: expected the create after the release to answer HTTP 200, got {} / {}",
            taken.status, taken.body
        );
        assert_eq!(
            strings(&taken, "parentReadinessScopes"),
            Vec::<String>::new(),
            "case {label:?}: the new scope itself is contained by nothing, got {:?}",
            strings(&taken, "parentReadinessScopes")
        );
        assert_scope(
            &emulator,
            label,
            "child_cell",
            &[arn("cell", "scope_two")],
        )
        .await;
    }
}

/// RULE, over a whole containment tree: however deep the nesting goes, every cell in it
/// reports exactly the one scope that holds it — the recovery group at the root for the
/// top cell, the cell above for each nested one — on `CreateCell`, `UpdateCell`,
/// `GetCell` and `ListCells` alike. Rows differ along the depth of the chain.
#[tokio::test]
async fn every_cell_of_a_containment_tree_reports_the_one_scope_that_holds_it() {
    for depth in [1_usize, 2, 3, 5] {
        let label = format!("a chain of {depth} cells under a recovery group");
        let label = label.as_str();
        let emulator = Emulator::new();

        // The chain, deepest first, so each create names a cell that already exists.
        let names: Vec<String> = (0..depth).map(|level| format!("cell_{level}")).collect();
        for level in (0..depth).rev() {
            let nested: Vec<String> = match names.get(level + 1) {
                Some(child) => vec![arn("cell", child)],
                None => Vec::new(),
            };
            let created = create_cell(
                &emulator,
                json!({ "cellName": names[level], "cells": nested }),
            )
            .await;
            seed(created, label, "CreateCell(a chain cell)");
        }

        // The root group holds the top of the chain.
        seed(
            create_group(
                &emulator,
                json!({
                    "recoveryGroupName": "root_group",
                    "cells": [arn("cell", &names[0])],
                }),
            )
            .await,
            label,
            "CreateRecoveryGroup(root_group)",
        );

        for (level, name) in names.iter().enumerate() {
            let expected = match level {
                0 => arn("recovery-group", "root_group"),
                _ => arn("cell", &names[level - 1]),
            };
            assert_scope(&emulator, label, name, std::slice::from_ref(&expected)).await;

            // An update of the cell's own nested list reports the same one scope: the
            // member is derived from the records that hold it, not from this one.
            let nested: Vec<String> = match names.get(level + 1) {
                Some(child) => vec![arn("cell", child)],
                None => Vec::new(),
            };
            let updated = update_cell(&emulator, name, &nested).await;
            assert_eq!(
                updated.status, 200,
                "case {label:?}: expected UpdateCell({name}) to answer HTTP 200, got {} / {}",
                updated.status, updated.body
            );
            assert_eq!(
                strings(&updated, "parentReadinessScopes"),
                vec![expected.clone()],
                "case {label:?}: expected UpdateCell({name}) to report scope {expected:?}, got {:?}",
                strings(&updated, "parentReadinessScopes")
            );
        }
    }
}
