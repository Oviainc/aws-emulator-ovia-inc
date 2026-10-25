// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE summary rule `GetCellReadinessSummary` and
//! `GetRecoveryGroupReadinessSummary` both answer with.
//!
//! The rule under test: a container's readiness summary reports one entry per readiness
//! check that watches a resource scoped to that container — each entry carrying the
//! check's name and its readiness, which is `UNKNOWN` because nothing in this emulator
//! audits the real AWS resources a resource set names — keyed and ordered by check name
//! so the page cursor is the same on every call, with the aggregate readiness of those
//! entries (`UNKNOWN`, the aggregate of parts that are all unknown) alongside the page.
//! The rule does not depend on WHICH kind of container was asked about: a cell and a
//! recovery group covering the same scopes report the same summary, page for page and
//! cursor for cursor.
//!
//! These tests drive BOTH summary operations over one table, so a change to the per-check
//! summary, to the cursor key or to the aggregate cannot land in one of them alone.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected.
//!
//! Rows differ along what the rule generalises over: how many checks a container covers
//! (none, one, a few), whether a check is scoped by the container's own ARN or by a cell
//! nested inside it, the page size asked for, the page a cursor resumes from, and the
//! form of a token that was never minted.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The instance URIs the two summaries are bound to (model `@http`:
/// `GET /cellreadiness/{CellName}` and
/// `GET /recoverygroupreadiness/{RecoveryGroupName}`).
const CELL_SUMMARY_PATH: &str = "/cellreadiness";
const GROUP_SUMMARY_PATH: &str = "/recoverygroupreadiness";

/// The readiness of anything nothing has audited, one of the four values the
/// `Readiness` enum declares (model: `smithy_query(query="shape", shape="Readiness")`:
/// READY, NOT_READY, UNKNOWN, NOT_AUTHORIZED).
const UNKNOWN: &str = "UNKNOWN";

/// The sentence a page answers for a `NextToken` this service did not mint.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The members a `ReadinessCheckSummary` carries (model:
/// `smithy_query(query="shape", shape="ReadinessCheckSummary")`).
const SUMMARY_MEMBERS: [&str; 2] = ["readiness", "readinessCheckName"];

/// An ARN of an AWS resource a resource set can name, used as a `ResourceIdentifier`.
const QUEUE_ARN: &str = "arn:aws:sqs:us-west-2:000000000000:q1";

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out here
/// rather than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// POST `body` to `path` and assert the create succeeded, so a fixture that fails to
/// store fails where it was written rather than later as a wrong summary.
async fn create(emulator: &Emulator, path: &str, body: Value, label: &str) {
    let reply = emulator.send("POST", path, Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "fixture {label:?}: expected the create to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

/// One container, named by the summary operation that reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Cell,
    RecoveryGroup,
}

impl Container {
    fn summary_path(self) -> &'static str {
        match self {
            Container::Cell => CELL_SUMMARY_PATH,
            Container::RecoveryGroup => GROUP_SUMMARY_PATH,
        }
    }
}

/// Request one page of a container's summary.
async fn summary(
    emulator: &Emulator,
    container: Container,
    name: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
) -> Reply {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        format!("{}/{name}", container.summary_path())
    } else {
        format!("{}/{name}?{}", container.summary_path(), query.join("&"))
    };
    emulator.send("GET", &uri, None).await
}

/// The `readinessCheckName`s the summary reported, in the order it reported them.
fn reported_names(reply: &Reply) -> Vec<String> {
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

/// Assert one page follows the summary rule: HTTP 200, the aggregate `UNKNOWN`, and one
/// entry per check carrying only the modelled members, each `UNKNOWN`.
fn assert_page_follows_the_rule(case: &str, reply: &Reply, expected: &[String]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("readiness").as_deref(),
        Some(UNKNOWN),
        "case {case:?}: expected the aggregate readiness {UNKNOWN}, got {}",
        reply.body
    );
    let reported = reported_names(reply);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the page to name {expected:?}, got {reported:?}"
    );
    for item in reply
        .field("readinessChecks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        assert_eq!(
            item.get("readiness").and_then(Value::as_str),
            Some(UNKNOWN),
            "case {case:?}: expected every summarised check to be {UNKNOWN}, got {item}"
        );
        let members: BTreeSet<String> = item
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let modelled: BTreeSet<String> = SUMMARY_MEMBERS.iter().map(|m| m.to_string()).collect();
        assert_eq!(
            members, modelled,
            "case {case:?}: expected an entry carrying exactly {modelled:?}, got {item}"
        );
    }
}

/// One row of the table: a cell nesting another cell, a recovery group holding that
/// cell, and `checks` readiness checks watching resources scoped to one of the two
/// cells.
///
/// Because nothing is scoped to the GROUP's own ARN, the group covers exactly the cell's
/// scopes, so both summaries must report the same checks — which is what makes the two
/// answers comparable page for page.
struct Row {
    label: &'static str,
    cell: String,
    group: String,
    expected: Vec<String>,
}

/// A store holding one row per check count the rule ranges over, all in the SAME engine,
/// so a summary that reported a check outside its own scopes would be caught.
async fn lattice() -> (Emulator, Vec<Row>) {
    let emulator = Emulator::new();
    let mut rows = Vec::new();

    for (label, checks) in [("none", 0_usize), ("one", 1), ("two", 2), ("several", 5)] {
        let nested = format!("nested_{label}");
        let cell = format!("cell_{label}");
        let group = format!("group_{label}");

        create(
            &emulator,
            "/cells",
            json!({"cellName": nested, "cells": []}),
            &nested,
        )
        .await;
        create(
            &emulator,
            "/cells",
            json!({"cellName": cell, "cells": [arn("cell", &nested)]}),
            &cell,
        )
        .await;
        create(
            &emulator,
            "/recoverygroups",
            json!({"recoveryGroupName": group, "cells": [arn("cell", &cell)]}),
            &group,
        )
        .await;

        let mut expected = Vec::new();
        for index in 0..checks {
            // Alternate the kind of scope a check is reached by: the container's own
            // cell ARN, and the ARN of the cell nested inside it.
            let scope = if index % 2 == 0 {
                arn("cell", &cell)
            } else {
                arn("cell", &nested)
            };
            let set = format!("set_{label}_{index}");
            let check = format!("chk_{label}_{index}");
            create(
                &emulator,
                "/resourcesets",
                json!({
                    "resourceSetName": set,
                    "resourceSetType": "AWS::SQS::Queue",
                    "resources": [{"resourceArn": QUEUE_ARN, "readinessScopes": [scope]}],
                }),
                &set,
            )
            .await;
            create(
                &emulator,
                "/readinesschecks",
                json!({"readinessCheckName": check, "resourceSetName": set}),
                &check,
            )
            .await;
            expected.push(check);
        }
        // The summary is keyed and ordered by check name.
        expected.sort();

        rows.push(Row {
            label,
            cell,
            group,
            expected,
        });
    }

    (emulator, rows)
}

/// A cell's summary and a recovery group's summary follow one rule: over every check
/// count and page size, the two answers are the same aggregate, the same entries and the
/// same cursor, and a cursor minted by either resumes both at the same page.
#[tokio::test]
async fn both_readiness_summaries_follow_one_summary_rule() {
    let (emulator, rows) = lattice().await;

    for row in &rows {
        for page_size in [None, Some(1_usize), Some(2), Some(1000)] {
            let case = format!(
                "{} checks ({}), maxResults={page_size:?}",
                row.expected.len(),
                row.label
            );
            let bound = page_size.unwrap_or(usize::MAX);

            let cell = summary(&emulator, Container::Cell, &row.cell, page_size, None).await;
            let group =
                summary(&emulator, Container::RecoveryGroup, &row.group, page_size, None).await;

            let first: Vec<String> = row.expected.iter().take(bound).cloned().collect();
            assert_page_follows_the_rule(&format!("cell: {case}"), &cell, &first);
            assert_page_follows_the_rule(&format!("recovery group: {case}"), &group, &first);
            assert_eq!(
                cell.body, group.body,
                "case {case:?}: expected a cell and a recovery group covering the same scopes \
                 to report the same summary, got {} then {}",
                cell.body, group.body
            );
            assert_eq!(
                cell.string("nextToken").is_some(),
                row.expected.len() > bound,
                "case {case:?}: expected a cursor exactly when checks remain past the page, got {}",
                cell.body
            );

            // A cursor one summary minted resumes the other at the same page: both key
            // the cursor by check name.
            if let Some(cursor) = cell.string("nextToken") {
                let rest: Vec<String> = row
                    .expected
                    .iter()
                    .skip(bound)
                    .take(bound)
                    .cloned()
                    .collect();
                let resumed_cell = summary(
                    &emulator,
                    Container::Cell,
                    &row.cell,
                    page_size,
                    Some(&cursor),
                )
                .await;
                let resumed_group = summary(
                    &emulator,
                    Container::RecoveryGroup,
                    &row.group,
                    page_size,
                    Some(&cursor),
                )
                .await;
                assert_page_follows_the_rule(
                    &format!("cell resumed: {case}"),
                    &resumed_cell,
                    &rest,
                );
                assert_page_follows_the_rule(
                    &format!("recovery group resumed: {case}"),
                    &resumed_group,
                    &rest,
                );
                assert_eq!(
                    resumed_cell.body, resumed_group.body,
                    "case {case:?}: expected a cursor minted by the cell summary to resume the \
                     recovery group summary identically, got {} then {}",
                    resumed_cell.body, resumed_group.body
                );
            }

            // And the whole cursor walk reports every scoped check exactly once, in name
            // order, through either operation.
            for container in [Container::Cell, Container::RecoveryGroup] {
                let name = match container {
                    Container::Cell => &row.cell,
                    Container::RecoveryGroup => &row.group,
                };
                let case = format!("{container:?} walk: {case}");
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                // More rounds than there can be pages, so a non-terminating cursor fails
                // loudly instead of looping forever.
                for _ in 0..16 {
                    let reply =
                        summary(&emulator, container, name, page_size, token.as_deref()).await;
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}: expected a page to answer HTTP 200, got {} / {}",
                        reply.status, reply.body
                    );
                    let reported = reported_names(&reply);
                    assert!(
                        reported.len() <= bound,
                        "case {case:?}: expected at most {bound} entries on a page, got {}",
                        reported.len()
                    );
                    walked.extend(reported);
                    token = reply.string("nextToken");
                    if token.is_none() {
                        break;
                    }
                }
                assert_eq!(
                    token, None,
                    "case {case:?}: expected the walk to end without a cursor, got {token:?}"
                );
                assert_eq!(
                    walked, row.expected,
                    "case {case:?}: expected the walk to report {:?}, got {walked:?}",
                    row.expected
                );
            }
        }
    }
}

/// A check scoped to a recovery group's OWN ARN is in that group's summary and in no
/// cell's: the shared rule is applied to each container's own scope set, not to one set
/// for both.
#[tokio::test]
async fn each_summary_applies_the_rule_to_its_own_scopes() {
    let (emulator, rows) = lattice().await;
    let row = rows
        .iter()
        .find(|row| row.label == "one")
        .expect("the one-check row");

    create(
        &emulator,
        "/resourcesets",
        json!({
            "resourceSetName": "set_group_scoped",
            "resourceSetType": "AWS::SQS::Queue",
            "resources": [{
                "resourceArn": QUEUE_ARN,
                "readinessScopes": [arn("recovery-group", &row.group)],
            }],
        }),
        "set_group_scoped",
    )
    .await;
    create(
        &emulator,
        "/readinesschecks",
        json!({"readinessCheckName": "chk_group_scoped", "resourceSetName": "set_group_scoped"}),
        "chk_group_scoped",
    )
    .await;

    let mut with_group_scope = row.expected.clone();
    with_group_scope.push("chk_group_scoped".to_string());
    with_group_scope.sort();

    for (case, container, name, expected) in [
        (
            "a group scoped check belongs to that group's summary",
            Container::RecoveryGroup,
            &row.group,
            with_group_scope,
        ),
        (
            "and to no cell's summary, the group ARN being no scope of a cell",
            Container::Cell,
            &row.cell,
            row.expected.clone(),
        ),
    ] {
        let reply = summary(&emulator, container, name, None, None).await;
        assert_page_follows_the_rule(case, &reply, &expected);
    }
}

/// Both summaries refuse a `NextToken` neither of them minted, with the modelled
/// `ValidationException` (400) and the one constant sentence of the shared cursor codec.
#[tokio::test]
async fn both_readiness_summaries_refuse_a_token_neither_minted() {
    let (emulator, rows) = lattice().await;
    let row = rows
        .iter()
        .find(|row| row.label == "several")
        .expect("the several-check row");

    for token in ["not_a_token", "YWJj", "%7B%7D", "0123456789abcdef"] {
        for container in [Container::Cell, Container::RecoveryGroup] {
            let name = match container {
                Container::Cell => &row.cell,
                Container::RecoveryGroup => &row.group,
            };
            let case = format!("{container:?} nextToken={token:?}");
            let reply = summary(&emulator, container, name, None, Some(token)).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}: expected the error code ValidationException, got {}",
                reply.error_code()
            );
            assert_eq!(
                reply.string("message").as_deref(),
                Some(BAD_TOKEN_MESSAGE),
                "case {case:?}: expected the constant invalid-token sentence, got {}",
                reply.body
            );
        }
    }
}
