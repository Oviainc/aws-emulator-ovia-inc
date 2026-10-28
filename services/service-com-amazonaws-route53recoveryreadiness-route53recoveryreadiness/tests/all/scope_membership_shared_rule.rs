// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE "does this resource set hold a resource in scope"
//! predicate that both of this service's scope-reading criteria apply.
//!
//! The rule under test: a stored resource set is held by a container exactly when one of
//! the set's resources names one of that container's readiness scopes — its own ARN or
//! the ARN of a cell nested inside it — matched as a whole ARN, so an entry carrying no
//! scopes, an empty scope list, a scope of some other container, a prefix of a scope or a
//! scope differing in case is NOT held.
//!
//! The same predicate decides two different answers, reached through two different
//! operations:
//!
//! * `GetCellReadinessSummary` names the readiness checks that watch a held set, and
//! * `GetArchitectureRecommendations` reads the resource TYPES the held sets carry, which
//!   is what its symmetry criterion compares between sibling cells (and, through the same
//!   walk, what its coverage criterion reports for a cell no check watches).
//!
//! So each row below is driven through BOTH operations over one store, and the two answers
//! are required to agree about the same membership: a change to what "in scope" means
//! cannot land in one criterion and leave the other reporting the old rule.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the production
//! emulator mounts (the harness is shared with `readiness_check_lifecycle.rs`), so only
//! the HTTP status and the JSON body are inspected.
//!
//! Rows differ along what the rule generalises over: how many scopes an entry carries
//! (none at all, zero, one, several) and where the matching one sits in the list, which
//! container the scope names (the cell itself, a cell nested inside it, a sibling cell,
//! the enclosing recovery group, a cell that was never created), and the FORM of a scope
//! that is nearly the cell's ARN (a prefix of it, it with a trailing segment, it in
//! another case).

use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The instance URIs the two operations are bound to (model `@http`:
/// `GET /cellreadiness/{CellName}` and
/// `GET /recoverygroups/{RecoveryGroupName}/architectureRecommendations`).
const CELL_SUMMARY_PATH: &str = "/cellreadiness";
fn report_uri(group: &str) -> String {
    format!("/recoverygroups/{group}/architectureRecommendations")
}

/// One resource type the `ResourceSetType` member's own documentation enumerates (model:
/// `smithy_query(query="doc", shape="CreateResourceSetRequest", member="ResourceSetType")`),
/// and an ARN of such a resource used as a `ResourceIdentifier`.
const TABLE_TYPE: &str = "AWS::DynamoDB::Table";
const TABLE_ARN: &str = "arn:aws:dynamodb:us-west-2:000000000000:table/t1";

/// The recommendation a group whose member cells hold different resource types earns, and
/// the one a member cell no readiness check watches earns. Written out here rather than
/// imported, so the test pins the wording.
const SYMMETRY_TEXT: &str = "The cells of a recovery group should be symmetric: a cell that holds \
     different types of resource than its sibling cells cannot take over their traffic. The cells \
     of this recovery group hold different resource types.";

fn coverage_text(cell_arn: &str) -> String {
    format!(
        "No readiness check watches a resource scoped to cell {cell_arn}. Create a resource set \
         whose resources name that cell in their readiness scopes, and a readiness check over \
         that resource set, so that the readiness of the cell is audited."
    )
}

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out here rather
/// than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// POST `body` to `path` and assert the create succeeded, so a fixture that fails to store
/// fails where it was written rather than later as a wrong answer.
async fn create(emulator: &Emulator, path: &str, body: Value, label: &str) {
    let reply = emulator.send("POST", path, Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "fixture {label:?}: expected the create to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

/// A resource set holding one resource whose `readinessScopes` member is exactly `scopes`
/// (absent when `scopes` is `None`).
async fn resource_set(emulator: &Emulator, name: &str, scopes: Option<&[String]>) {
    let mut resource = json!({"resourceArn": TABLE_ARN});
    if let Some(scopes) = scopes {
        resource["readinessScopes"] = json!(scopes);
    }
    create(
        emulator,
        "/resourcesets",
        json!({
            "resourceSetName": name,
            "resourceSetType": TABLE_TYPE,
            "resources": [resource],
        }),
        name,
    )
    .await;
}

/// The `readinessCheckName`s a cell's readiness summary reported, in the order it
/// reported them.
fn summarised_checks(reply: &Reply) -> Vec<String> {
    reply
        .field("readinessChecks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("readinessCheckName"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `recommendationText`s an architecture report named, in the order it named them.
fn reported_texts(reply: &Reply) -> Vec<String> {
    reply
        .field("recommendations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("recommendationText"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One form a probe resource set's `readinessScopes` can take, and which of the two
/// sibling cells the predicate must therefore say holds it.
struct Case {
    label: &'static str,
    /// The scope list, built from this row's cell ARNs: `None` omits the member entirely.
    scopes: fn(&Fixture) -> Option<Vec<String>>,
    /// Whether the probe set is held by the cell under test.
    held_by_probed_cell: bool,
    /// Whether it is held by that cell's sibling.
    held_by_sibling: bool,
}

/// The records one row is driven over: two sibling cells in one recovery group, a cell
/// nested inside the probed one, and the names of the probe check and the sibling's check.
struct Fixture {
    probed_cell: String,
    nested_cell: String,
    sibling_cell: String,
    group: String,
    probe_check: String,
    sibling_check: String,
}

/// Every form of scope list the rule ranges over.
///
/// `held_by_probed_cell` is the membership BOTH operations must report: the checks the
/// probed cell's summary names, and the resource types the architecture report compares
/// between the two siblings.
const CASES: &[Case] = &[
    Case {
        label: "the probed cell's own ARN",
        scopes: |f| Some(vec![arn("cell", &f.probed_cell)]),
        held_by_probed_cell: true,
        held_by_sibling: false,
    },
    Case {
        label: "the ARN of a cell nested inside the probed cell",
        scopes: |f| Some(vec![arn("cell", &f.nested_cell)]),
        held_by_probed_cell: true,
        held_by_sibling: false,
    },
    Case {
        label: "several scopes, the matching one last",
        scopes: |f| {
            Some(vec![
                arn("cell", "a_cell_that_was_never_created"),
                arn("cell", &f.probed_cell),
            ])
        },
        held_by_probed_cell: true,
        held_by_sibling: false,
    },
    Case {
        label: "several scopes, the matching one first",
        scopes: |f| {
            Some(vec![
                arn("cell", &f.nested_cell),
                arn("cell", "a_cell_that_was_never_created"),
            ])
        },
        held_by_probed_cell: true,
        held_by_sibling: false,
    },
    Case {
        label: "no readinessScopes member at all",
        scopes: |_| None,
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "an empty readinessScopes list",
        scopes: |_| Some(Vec::new()),
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "the ARN of a cell that was never created",
        scopes: |_| Some(vec![arn("cell", "a_cell_that_was_never_created")]),
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "several scopes, none of them matching",
        scopes: |_| {
            Some(vec![
                arn("cell", "a_cell_that_was_never_created"),
                arn("recovery-group", "a_group_that_was_never_created"),
            ])
        },
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "the sibling cell's ARN",
        scopes: |f| Some(vec![arn("cell", &f.sibling_cell)]),
        held_by_probed_cell: false,
        held_by_sibling: true,
    },
    Case {
        label: "the enclosing recovery group's own ARN, which is no scope of a cell",
        scopes: |f| Some(vec![arn("recovery-group", &f.group)]),
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "a prefix of the probed cell's ARN",
        scopes: |f| {
            let full = arn("cell", &f.probed_cell);
            Some(vec![full[..full.len() - 1].to_string()])
        },
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "the probed cell's ARN with a trailing segment",
        scopes: |f| Some(vec![format!("{}/nested", arn("cell", &f.probed_cell))]),
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
    Case {
        label: "the probed cell's ARN in upper case",
        scopes: |f| Some(vec![arn("cell", &f.probed_cell).to_uppercase()]),
        held_by_probed_cell: false,
        held_by_sibling: false,
    },
];

/// The name of the probe resource set of the row at `index`.
fn probe_set_name(index: usize) -> String {
    format!("probe_set_{index}_r")
}

/// A store holding one independent fixture per case, all in the SAME engine, so an answer
/// that counted a resource set belonging to another row would be caught.
async fn lattice() -> (Emulator, Vec<Fixture>) {
    let emulator = Emulator::new();
    let mut fixtures = Vec::new();

    for (index, case) in CASES.iter().enumerate() {
        let fixture = Fixture {
            // Every name ends in a NON-digit, so the "prefix of the probed cell's ARN"
            // case below (which drops the last character) cannot land on the name of
            // another row's cell.
            probed_cell: format!("probed_cell_{index}_p"),
            nested_cell: format!("nested_cell_{index}_n"),
            sibling_cell: format!("sibling_cell_{index}_s"),
            group: format!("group_{index}_g"),
            probe_check: format!("probe_check_{index}_c"),
            sibling_check: format!("sibling_check_{index}_c"),
        };

        create(
            &emulator,
            "/cells",
            json!({"cellName": fixture.nested_cell, "cells": []}),
            &fixture.nested_cell,
        )
        .await;
        create(
            &emulator,
            "/cells",
            json!({
                "cellName": fixture.probed_cell,
                "cells": [arn("cell", &fixture.nested_cell)],
            }),
            &fixture.probed_cell,
        )
        .await;
        create(
            &emulator,
            "/cells",
            json!({"cellName": fixture.sibling_cell, "cells": []}),
            &fixture.sibling_cell,
        )
        .await;
        create(
            &emulator,
            "/recoverygroups",
            json!({
                "recoveryGroupName": fixture.group,
                "cells": [
                    arn("cell", &fixture.sibling_cell),
                    arn("cell", &fixture.probed_cell),
                ],
            }),
            &fixture.group,
        )
        .await;

        // The sibling cell always holds one resource set of `TABLE_TYPE`, watched by one
        // check: it is the baseline the symmetry criterion compares the probed cell's
        // types against, and the reason the sibling never earns a coverage finding.
        let sibling_set = format!("sibling_set_{index}_r");
        resource_set(
            &emulator,
            &sibling_set,
            Some(&[arn("cell", &fixture.sibling_cell)]),
        )
        .await;
        create(
            &emulator,
            "/readinesschecks",
            json!({
                "readinessCheckName": fixture.sibling_check,
                "resourceSetName": sibling_set,
            }),
            &fixture.sibling_check,
        )
        .await;

        // The probe: the same type, reachable (or not) through this case's scope list.
        let probe_set = probe_set_name(index);
        let scopes = (case.scopes)(&fixture);
        resource_set(&emulator, &probe_set, scopes.as_deref()).await;
        create(
            &emulator,
            "/readinesschecks",
            json!({
                "readinessCheckName": fixture.probe_check,
                "resourceSetName": probe_set,
            }),
            &fixture.probe_check,
        )
        .await;

        fixtures.push(fixture);
    }

    (emulator, fixtures)
}

async fn cell_summary(emulator: &Emulator, cell: &str) -> Reply {
    emulator
        .send("GET", &format!("{CELL_SUMMARY_PATH}/{cell}"), None)
        .await
}

async fn report(emulator: &Emulator, group: &str) -> Reply {
    emulator.send("GET", &report_uri(group), None).await
}

/// A cell's readiness summary and a recovery group's architecture report decide "is this
/// resource set in scope" the same way: over every form of scope list, the checks the
/// summary names and the symmetry and coverage findings the report names agree about which
/// cell holds the probe set.
#[tokio::test]
async fn both_scope_readers_agree_on_which_sets_a_container_holds() {
    let (emulator, fixtures) = lattice().await;

    for (case, fixture) in CASES.iter().zip(&fixtures) {
        let label = case.label;

        // The summary path (the readiness-check reader of the predicate).
        let mut expected_probed: Vec<String> = Vec::new();
        if case.held_by_probed_cell {
            expected_probed.push(fixture.probe_check.clone());
        }
        let mut expected_sibling: Vec<String> = vec![fixture.sibling_check.clone()];
        if case.held_by_sibling {
            expected_sibling.push(fixture.probe_check.clone());
        }
        // The summary is keyed and ordered by check name.
        expected_probed.sort();
        expected_sibling.sort();

        for (cell, expected) in [
            (&fixture.probed_cell, &expected_probed),
            (&fixture.sibling_cell, &expected_sibling),
        ] {
            let reply = cell_summary(&emulator, cell).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected the summary of {cell:?} to answer HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            let reported = summarised_checks(&reply);
            assert_eq!(
                &reported, expected,
                "case {label:?}: expected the summary of {cell:?} to name {expected:?}, got \
                 {reported:?}"
            );
        }

        // The architecture path (the resource-type reader of the SAME predicate). The two
        // siblings hold the same single type exactly when the probed cell holds the probe
        // set, so the symmetry finding appears exactly when it does not — and the probed
        // cell is then also watched by no check, which is the coverage finding.
        let expected_texts: Vec<String> = if case.held_by_probed_cell {
            Vec::new()
        } else {
            vec![
                SYMMETRY_TEXT.to_string(),
                coverage_text(&arn("cell", &fixture.probed_cell)),
            ]
        };
        let reply = report(&emulator, &fixture.group).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected the architecture report to answer HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        let reported = reported_texts(&reply);
        assert_eq!(
            reported, expected_texts,
            "case {label:?}: expected the report of {:?} to name {expected_texts:?}, got \
             {reported:?}",
            fixture.group
        );
    }
}

/// One edit to what "in scope" means moves both readers together: deleting the resource
/// set the probe check watches takes that check out of the cell's summary AND takes its
/// type out of the cell's contribution to the architecture, in the same store.
///
/// A second criterion reading its own copy of the predicate is what this would catch: the
/// two answers are re-read after the SAME store change and must both have moved.
#[tokio::test]
async fn a_store_change_moves_both_scope_readers_together() {
    let (emulator, fixtures) = lattice().await;
    let index = CASES
        .iter()
        .position(|case| case.held_by_probed_cell)
        .expect("a case whose probe set is held by the probed cell");
    let fixture = &fixtures[index];
    let probe_set = probe_set_name(index);

    let before_summary = cell_summary(&emulator, &fixture.probed_cell).await;
    let before_report = report(&emulator, &fixture.group).await;
    assert_eq!(
        summarised_checks(&before_summary),
        vec![fixture.probe_check.clone()],
        "before the delete: expected the probed cell's summary to name its check, got {}",
        before_summary.body
    );
    assert_eq!(
        reported_texts(&before_report),
        Vec::<String>::new(),
        "before the delete: expected a symmetric, covered group to earn no recommendation, got {}",
        before_report.body
    );

    let deleted = emulator
        .send(
            "DELETE",
            &format!("/readinesschecks/{}", fixture.probe_check),
            None,
        )
        .await;
    // `DeleteReadinessCheck` and `DeleteResourceSet` answer 204 (model `@http code`).
    assert_eq!(
        deleted.status, 204,
        "expected the check delete to answer HTTP 204, got {} / {}",
        deleted.status, deleted.body
    );
    let deleted = emulator
        .send("DELETE", &format!("/resourcesets/{probe_set}"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "expected the resource-set delete to answer HTTP 204, got {} / {}",
        deleted.status, deleted.body
    );

    let after_summary = cell_summary(&emulator, &fixture.probed_cell).await;
    let after_report = report(&emulator, &fixture.group).await;
    assert_eq!(
        summarised_checks(&after_summary),
        Vec::<String>::new(),
        "after the delete: expected the probed cell's summary to name no check, got {}",
        after_summary.body
    );
    assert_eq!(
        reported_texts(&after_report),
        vec![
            SYMMETRY_TEXT.to_string(),
            coverage_text(&arn("cell", &fixture.probed_cell)),
        ],
        "after the delete: expected the report to name the symmetry and coverage findings the \
         same store change implies, got {}",
        after_report.body
    );
}
