// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAssociatedFleets` over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * the names reported for a stack are exactly the fleets an `AssociateFleet` wrote for
//!   it and a `DisassociateFleet` has not cleared — never a fleet associated with another
//!   stack;
//! * the operation models no error, so a stack with no association — including a name
//!   the account does not hold — reports an empty list rather than a refusal;
//! * following `NextToken` walks every associated fleet exactly once, in a stable order,
//!   and a token this service never issued is refused;
//! * the names come from the stack half of each stored edge, so a fleet associated with
//!   several stacks is reported by each of them, a repeated association writes no second
//!   edge, and a disassociation clears only the pair it names;
//! * the operation reports, it does not mutate: the same read repeated answers the same
//!   names.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The stack under test, and a second stack whose associations must never leak into it.
const SUBJECT_STACK: &str = "subject-stack";
const OTHER_STACK: &str = "other-stack";

/// The fleet associated with [`OTHER_STACK`] only.
const OTHER_FLEET: &str = "other-stacks-fleet";

/// Associate `count` fleets with [`SUBJECT_STACK`], plus one fleet on another stack, and
/// report the names the subject stack must list.
async fn emulator_with_associations(case: &str, count: usize) -> (Emulator, BTreeSet<String>) {
    let emulator = Emulator::new();
    for stack in [SUBJECT_STACK, OTHER_STACK] {
        emulator
            .ok(case, "CreateStack", json!({ "Name": stack }))
            .await;
    }
    emulator
        .ok(
            case,
            "CreateFleet",
            json!({ "Name": OTHER_FLEET, "InstanceType": "stream.standard.small" }),
        )
        .await;
    emulator
        .ok(
            case,
            "AssociateFleet",
            json!({ "FleetName": OTHER_FLEET, "StackName": OTHER_STACK }),
        )
        .await;

    let mut expected = BTreeSet::new();
    // Created in descending order, so the stable order the list reports is the store's
    // and not the order the edges were written in.
    for index in (0..count).rev() {
        let fleet = format!("member-{index:03}");
        emulator
            .ok(
                case,
                "CreateFleet",
                json!({ "Name": fleet, "InstanceType": "stream.standard.small" }),
            )
            .await;
        emulator
            .ok(
                case,
                "AssociateFleet",
                json!({ "FleetName": fleet, "StackName": SUBJECT_STACK }),
            )
            .await;
        expected.insert(fleet);
    }
    (emulator, expected)
}

/// Every name `ListAssociatedFleets` reports for `stack`, following `NextToken` to the
/// end. Panics if a name is reported twice, so the walk is proved to visit each exactly
/// once.
async fn list_every_page(emulator: &Emulator, case: &str, stack: &str) -> Vec<String> {
    let mut reported: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "StackName": stack });
        if let Some(token) = &next_token {
            request["NextToken"] = Value::String(token.clone());
        }
        let payload = emulator.ok(case, "ListAssociatedFleets", request).await;
        let names = payload
            .get("Names")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a Names list, got {payload}"));
        for name in names {
            let name = name
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a fleet name, got {payload}"))
                .to_string();
            assert!(
                !reported.contains(&name),
                "case {case:?}: {name:?} was reported twice while paging, got {reported:?}"
            );
            reported.push(name);
        }
        next_token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    reported
}

#[tokio::test]
async fn list_associated_fleets_reports_the_edges_associate_wrote() {
    // Rows: how many fleets the stack has associated — none, one, a few, and more than
    // one page holds (the page size is `DESCRIBE_PAGE_SIZE` in `src/state.rs`).
    for count in [0_usize, 1, 7, 51] {
        let case = format!("{count} associated fleet(s)");
        let (emulator, expected) = emulator_with_associations(&case, count).await;

        let reported = list_every_page(&emulator, &case, SUBJECT_STACK).await;
        assert_eq!(
            reported,
            expected.iter().cloned().collect::<Vec<String>>(),
            "case {case:?}: the list must report every associated fleet exactly once, in \
             a stable order"
        );
        assert!(
            !reported.iter().any(|name| name == OTHER_FLEET),
            "case {case:?}: a fleet associated with another stack must not be listed, \
             got {reported:?}"
        );

        // The other stack still reports only its own fleet, so the list is derived per
        // stack rather than from the whole edge set.
        let other = list_every_page(&emulator, &case, OTHER_STACK).await;
        assert_eq!(
            other,
            vec![OTHER_FLEET.to_string()],
            "case {case:?}: the other stack must list only the fleet associated with it"
        );

        // Clearing one edge removes exactly that name from the list.
        if let Some(cleared) = expected.iter().next().cloned() {
            emulator
                .ok(
                    &case,
                    "DisassociateFleet",
                    json!({ "FleetName": cleared, "StackName": SUBJECT_STACK }),
                )
                .await;
            let after = list_every_page(&emulator, &case, SUBJECT_STACK).await;
            let mut still_expected = expected.clone();
            still_expected.remove(&cleared);
            assert_eq!(
                after,
                still_expected.iter().cloned().collect::<Vec<String>>(),
                "case {case:?}: disassociating {cleared:?} must drop it from the list"
            );
        }
    }
}

/// The stacks every edge-script case creates, and the fleets it associates with them.
///
/// `StackName` and `FleetName` carry no enumerated values — they are plain names, only
/// length-constrained on the wire
/// (`smithy_query(query="operation", operation="ListAssociatedFleets")`) — so what the
/// rows below vary is the SHAPE of the edge set, not the spelling of a name.
const SCRIPT_STACKS: [&str; 3] = ["alpha-stack", "beta-stack", "gamma-stack"];
const SCRIPT_FLEETS: [&str; 2] = ["shared-fleet", "solo-fleet"];

/// One write in an edge script: the pair it associates, or the pair it clears.
#[derive(Clone, Copy, Debug)]
enum Edge {
    Associate(&'static str, &'static str),
    Disassociate(&'static str, &'static str),
}

/// Write scripts spanning the class: a fleet shared by several stacks, a repeated
/// association, a cleared pair, a pair re-associated after being cleared, and several
/// fleets on one stack.
fn edge_scripts() -> Vec<(&'static str, Vec<Edge>)> {
    let (shared, solo) = (SCRIPT_FLEETS[0], SCRIPT_FLEETS[1]);
    let (alpha, beta, gamma) = (SCRIPT_STACKS[0], SCRIPT_STACKS[1], SCRIPT_STACKS[2]);
    vec![
        (
            "one fleet, two stacks",
            vec![Edge::Associate(shared, alpha), Edge::Associate(shared, beta)],
        ),
        (
            "the same pair associated twice",
            vec![Edge::Associate(solo, alpha), Edge::Associate(solo, alpha)],
        ),
        (
            "one of a shared fleet's two stacks cleared",
            vec![
                Edge::Associate(shared, alpha),
                Edge::Associate(shared, beta),
                Edge::Disassociate(shared, alpha),
            ],
        ),
        (
            "a pair re-associated after being cleared",
            vec![
                Edge::Associate(solo, gamma),
                Edge::Disassociate(solo, gamma),
                Edge::Associate(solo, gamma),
            ],
        ),
        (
            "several fleets on one stack, one of them shared",
            vec![
                Edge::Associate(shared, alpha),
                Edge::Associate(solo, alpha),
                Edge::Associate(shared, beta),
                Edge::Disassociate(solo, alpha),
            ],
        ),
    ]
}

/// The names each stack must report are exactly the fleets the stored edge set names for
/// it, after EVERY write of the script — never a fleet a sibling stack's edge names.
#[tokio::test]
async fn the_listed_names_track_every_edge_write() {
    for (script_case, script) in edge_scripts() {
        let emulator = Emulator::new();
        for stack in SCRIPT_STACKS {
            emulator
                .ok(script_case, "CreateStack", json!({ "Name": stack }))
                .await;
        }
        for fleet in SCRIPT_FLEETS {
            emulator
                .ok(
                    script_case,
                    "CreateFleet",
                    json!({ "Name": fleet, "InstanceType": "stream.standard.small" }),
                )
                .await;
        }

        // The edge set the script has written so far, kept independently of the service.
        let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
        for (step, edge) in script.iter().enumerate() {
            match *edge {
                Edge::Associate(fleet, stack) => {
                    emulator
                        .ok(
                            script_case,
                            "AssociateFleet",
                            json!({ "FleetName": fleet, "StackName": stack }),
                        )
                        .await;
                    edges.insert((fleet.to_string(), stack.to_string()));
                }
                Edge::Disassociate(fleet, stack) => {
                    emulator
                        .ok(
                            script_case,
                            "DisassociateFleet",
                            json!({ "FleetName": fleet, "StackName": stack }),
                        )
                        .await;
                    edges.remove(&(fleet.to_string(), stack.to_string()));
                }
            }

            for stack in SCRIPT_STACKS {
                let case = format!("{script_case}: after step {step} ({edge:?}), {stack}");
                let expected: Vec<String> = edges
                    .iter()
                    .filter(|(_, associated)| associated == stack)
                    .map(|(fleet, _)| fleet.clone())
                    .collect();
                let reported = list_every_page(&emulator, &case, stack).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the fleets the stored edges name for this \
                     stack, {expected:?}, got {reported:?}"
                );
                // A report, not a mutation: reading the list leaves the edge set alone,
                // so the same read repeated answers the same names.
                let again = list_every_page(&emulator, &case, stack).await;
                assert_eq!(
                    again, expected,
                    "case {case:?}: a repeated read must answer {expected:?}, got {again:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stack_with_no_association_lists_nothing() {
    let case = "unassociated stacks";
    let (emulator, _) = emulator_with_associations(case, 2).await;
    emulator
        .ok(case, "CreateStack", json!({ "Name": "lonely-stack" }))
        .await;

    // Rows: a stored stack nothing is associated with, and a name the account does not
    // hold at all. The operation models no error, so both are an empty list.
    for stack in ["lonely-stack", "never-created"] {
        let payload = emulator
            .ok(case, "ListAssociatedFleets", json!({ "StackName": stack }))
            .await;
        assert_eq!(
            payload["Names"],
            json!([]),
            "case {case:?}: {stack:?} has no associated fleet, got {payload}"
        );
        assert!(
            payload.get("NextToken").is_none(),
            "case {case:?}: an empty list leaves nothing to resume from, got {payload}"
        );
    }
}

#[tokio::test]
async fn list_associated_fleets_refuses_a_next_token_it_did_not_issue() {
    let case = "hand-crafted tokens";
    let (emulator, _) = emulator_with_associations(case, 3).await;

    // Rows: tokens a client could hand-craft. The cursor is an opaque encoding this
    // service mints, so none of these is a token it issued.
    for token in ["member-000", "abc", "!!", "0"] {
        assert_error(
            &emulator,
            case,
            "ListAssociatedFleets",
            json!({ "StackName": SUBJECT_STACK, "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }
}
