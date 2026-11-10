// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAssociatedStacks` over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * the names reported for a fleet are exactly the stacks an `AssociateFleet` wrote for
//!   it and a `DisassociateFleet` has not cleared — never a stack associated with another
//!   fleet;
//! * the operation models no error, so a fleet with no association — including a name the
//!   account does not hold — reports an empty list rather than a refusal;
//! * following `NextToken` walks every associated stack exactly once, in a stable order,
//!   and a token this service never issued is refused;
//! * the two halves of one stored edge set agree: whenever `ListAssociatedStacks` names a
//!   stack for a fleet, `ListAssociatedFleets` names that fleet for that stack;
//! * the operation reports, it does not mutate: the same read repeated answers the same
//!   names.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The fleet under test, and a second fleet whose associations must never leak into it.
const SUBJECT_FLEET: &str = "subject-fleet";
const OTHER_FLEET: &str = "other-fleet";

/// The stack associated with [`OTHER_FLEET`] only.
const OTHER_STACK: &str = "other-fleets-stack";

/// The instance type every fleet in these tests is created with; `CreateFleet` requires
/// one and nothing here depends on which.
const INSTANCE_TYPE: &str = "stream.standard.small";

/// Associate `count` stacks with [`SUBJECT_FLEET`], plus one stack on another fleet, and
/// report the names the subject fleet must list.
async fn emulator_with_associations(case: &str, count: usize) -> (Emulator, BTreeSet<String>) {
    let emulator = Emulator::new();
    for fleet in [SUBJECT_FLEET, OTHER_FLEET] {
        emulator
            .ok(
                case,
                "CreateFleet",
                json!({ "Name": fleet, "InstanceType": INSTANCE_TYPE }),
            )
            .await;
    }
    emulator
        .ok(case, "CreateStack", json!({ "Name": OTHER_STACK }))
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
        let stack = format!("member-{index:03}");
        emulator
            .ok(case, "CreateStack", json!({ "Name": stack }))
            .await;
        emulator
            .ok(
                case,
                "AssociateFleet",
                json!({ "FleetName": SUBJECT_FLEET, "StackName": stack }),
            )
            .await;
        expected.insert(stack);
    }
    (emulator, expected)
}

/// Every name `ListAssociatedStacks` reports for `fleet`, following `NextToken` to the
/// end. Panics if a name is reported twice, so the walk is proved to visit each exactly
/// once.
async fn list_every_page(emulator: &Emulator, case: &str, fleet: &str) -> Vec<String> {
    let mut reported: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "FleetName": fleet });
        if let Some(token) = &next_token {
            request["NextToken"] = Value::String(token.clone());
        }
        let payload = emulator.ok(case, "ListAssociatedStacks", request).await;
        let names = payload
            .get("Names")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a Names list, got {payload}"));
        for name in names {
            let name = name
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a stack name, got {payload}"))
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

/// Every fleet `ListAssociatedFleets` reports for `stack`, so the mirror read can be
/// checked against this one.
async fn fleets_for_stack(emulator: &Emulator, case: &str, stack: &str) -> Vec<String> {
    let payload = emulator
        .ok(case, "ListAssociatedFleets", json!({ "StackName": stack }))
        .await;
    payload
        .get("Names")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Names list, got {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a fleet name, got {payload}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn list_associated_stacks_reports_the_edges_associate_wrote() {
    // Rows: how many stacks the fleet has associated — none, one, a few, and more than
    // one page holds (the page size is `DESCRIBE_PAGE_SIZE` in `src/state.rs`).
    for count in [0_usize, 1, 7, 51] {
        let case = format!("{count} associated stack(s)");
        let (emulator, expected) = emulator_with_associations(&case, count).await;

        let reported = list_every_page(&emulator, &case, SUBJECT_FLEET).await;
        assert_eq!(
            reported,
            expected.iter().cloned().collect::<Vec<String>>(),
            "case {case:?}: the list must report every associated stack exactly once, in \
             a stable order"
        );
        assert!(
            !reported.iter().any(|name| name == OTHER_STACK),
            "case {case:?}: a stack associated with another fleet must not be listed, \
             got {reported:?}"
        );

        // The other fleet still reports only its own stack, so the list is derived per
        // fleet rather than from the whole edge set.
        let other = list_every_page(&emulator, &case, OTHER_FLEET).await;
        assert_eq!(
            other,
            vec![OTHER_STACK.to_string()],
            "case {case:?}: the other fleet must list only the stack associated with it"
        );

        // Clearing one edge removes exactly that name from the list.
        if let Some(cleared) = expected.iter().next().cloned() {
            emulator
                .ok(
                    &case,
                    "DisassociateFleet",
                    json!({ "FleetName": SUBJECT_FLEET, "StackName": cleared }),
                )
                .await;
            let after = list_every_page(&emulator, &case, SUBJECT_FLEET).await;
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

/// The fleets every edge-script case creates, and the stacks it associates with them.
///
/// `FleetName` and `StackName` carry no enumerated values — they are plain names, only
/// length-constrained on the wire
/// (`smithy_query(query="operation", operation="ListAssociatedStacks")`) — so what the
/// rows below vary is the SHAPE of the edge set, not the spelling of a name.
const SCRIPT_FLEETS: [&str; 3] = ["alpha-fleet", "beta-fleet", "gamma-fleet"];
const SCRIPT_STACKS: [&str; 2] = ["shared-stack", "solo-stack"];

/// One write in an edge script: the pair it associates, or the pair it clears.
#[derive(Clone, Copy, Debug)]
enum Edge {
    Associate(&'static str, &'static str),
    Disassociate(&'static str, &'static str),
}

/// Write scripts spanning the class: a stack shared by several fleets, a repeated
/// association, a cleared pair, a pair re-associated after being cleared, and several
/// stacks on one fleet.
fn edge_scripts() -> Vec<(&'static str, Vec<Edge>)> {
    let (shared, solo) = (SCRIPT_STACKS[0], SCRIPT_STACKS[1]);
    let (alpha, beta, gamma) = (SCRIPT_FLEETS[0], SCRIPT_FLEETS[1], SCRIPT_FLEETS[2]);
    vec![
        (
            "one stack, two fleets",
            vec![Edge::Associate(alpha, shared), Edge::Associate(beta, shared)],
        ),
        (
            "the same pair associated twice",
            vec![Edge::Associate(alpha, solo), Edge::Associate(alpha, solo)],
        ),
        (
            "one of a shared stack's two fleets cleared",
            vec![
                Edge::Associate(alpha, shared),
                Edge::Associate(beta, shared),
                Edge::Disassociate(alpha, shared),
            ],
        ),
        (
            "a pair re-associated after being cleared",
            vec![
                Edge::Associate(gamma, solo),
                Edge::Disassociate(gamma, solo),
                Edge::Associate(gamma, solo),
            ],
        ),
        (
            "several stacks on one fleet, one of them shared",
            vec![
                Edge::Associate(alpha, shared),
                Edge::Associate(alpha, solo),
                Edge::Associate(beta, shared),
                Edge::Disassociate(alpha, solo),
            ],
        ),
    ]
}

/// The names each fleet must report are exactly the stacks the stored edge set names for
/// it, after EVERY write of the script — never a stack a sibling fleet's edge names.
#[tokio::test]
async fn the_listed_names_track_every_edge_write() {
    for (script_case, script) in edge_scripts() {
        let emulator = Emulator::new();
        for fleet in SCRIPT_FLEETS {
            emulator
                .ok(
                    script_case,
                    "CreateFleet",
                    json!({ "Name": fleet, "InstanceType": INSTANCE_TYPE }),
                )
                .await;
        }
        for stack in SCRIPT_STACKS {
            emulator
                .ok(script_case, "CreateStack", json!({ "Name": stack }))
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

            for fleet in SCRIPT_FLEETS {
                let case = format!("{script_case}: after step {step} ({edge:?}), {fleet}");
                let expected: Vec<String> = edges
                    .iter()
                    .filter(|(associated, _)| associated == fleet)
                    .map(|(_, stack)| stack.clone())
                    .collect();
                let reported = list_every_page(&emulator, &case, fleet).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the stacks the stored edges name for this \
                     fleet, {expected:?}, got {reported:?}"
                );
                // A report, not a mutation: reading the list leaves the edge set alone,
                // so the same read repeated answers the same names.
                let again = list_every_page(&emulator, &case, fleet).await;
                assert_eq!(
                    again, expected,
                    "case {case:?}: a repeated read must answer {expected:?}, got {again:?}"
                );
                // Both halves of one stored edge set: every stack this fleet names must
                // name this fleet back.
                for stack in &expected {
                    let mirror = fleets_for_stack(&emulator, &case, stack).await;
                    assert!(
                        mirror.contains(&fleet.to_string()),
                        "case {case:?}: {stack:?} lists {mirror:?}, which must contain \
                         {fleet:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_fleet_with_no_association_lists_nothing() {
    let case = "unassociated fleets";
    let (emulator, _) = emulator_with_associations(case, 2).await;
    emulator
        .ok(
            case,
            "CreateFleet",
            json!({ "Name": "lonely-fleet", "InstanceType": INSTANCE_TYPE }),
        )
        .await;

    // Rows: a stored fleet nothing is associated with, and a name the account does not
    // hold at all. The operation models no error, so both are an empty list.
    for fleet in ["lonely-fleet", "never-created"] {
        let payload = emulator
            .ok(case, "ListAssociatedStacks", json!({ "FleetName": fleet }))
            .await;
        assert_eq!(
            payload["Names"],
            json!([]),
            "case {case:?}: {fleet:?} has no associated stack, got {payload}"
        );
        assert!(
            payload.get("NextToken").is_none(),
            "case {case:?}: an empty list leaves nothing to resume from, got {payload}"
        );
    }
}

#[tokio::test]
async fn list_associated_stacks_refuses_a_next_token_it_did_not_issue() {
    let case = "hand-crafted tokens";
    let (emulator, _) = emulator_with_associations(case, 3).await;

    // Rows: tokens a client could hand-craft. The cursor is an opaque encoding this
    // service mints, so none of these is a token it issued.
    for token in ["member-000", "abc", "!!", "0"] {
        assert_error(
            &emulator,
            case,
            "ListAssociatedStacks",
            json!({ "FleetName": SUBJECT_FLEET, "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }
}
