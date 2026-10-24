// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRecoveryGroupReadinessSummary`.
//!
//! The rule under test: a readiness summary of a named container reports the readiness
//! of exactly the readiness checks that watch a resource whose `ReadinessScopes` name
//! that container or something nested inside it — for a recovery group, its own ARN, the
//! ARN of one of its member cells, or the ARN of a cell nested inside one of those,
//! transitively — paged like every other list of this service, and answers the modelled
//! `ResourceNotFoundException` (404) with this service's constant, resource-agnostic
//! sentence when this account holds no record under the requested name. Because nothing
//! in this emulator audits the real AWS resources a resource set names, every readiness
//! it reports — each check's and the aggregate — is `UNKNOWN`.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected: an answer that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables. Rows differ along what the rule generalises over — how many
//! cells a group holds, how deeply they nest, which kind of ARN a resource is scoped by
//! (the group's own, a member cell's, a nested cell's, an unrelated cell's, none at
//! all), how many checks the summary covers, the page size asked for and which token a
//! page resumes from, and the form and length of a name that was never created. The
//! recorded unit's own call (a summary of a recovery group that was never created) is
//! one row of the not-found table.

use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The instance URI this operation is bound to (model `@http`:
/// `GET /recoverygroupreadiness/{RecoveryGroupName}`).
const PATH: &str = "/recoverygroupreadiness";

/// The readiness of anything nothing has audited, one of the four values the
/// `Readiness` enum declares (model: `smithy_query(query="shape", shape="Readiness")`:
/// READY, NOT_READY, UNKNOWN, NOT_AUTHORIZED).
const UNKNOWN: &str = "UNKNOWN";

/// The constant, resource-agnostic sentence every not-found answer of this service's
/// resource-addressed operations carries. Written out here rather than imported, so the
/// test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The sentence a page answers for a `NextToken` this service did not mint.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The number of records a page carries when the caller names no `MaxResults` — the
/// page size this service's shared cursor uses.
const DEFAULT_MAX_RESULTS: usize =
    service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::DEFAULT_MAX_RESULTS;

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

async fn summary(emulator: &Emulator, name: &str, query: &str) -> Reply {
    let uri = if query.is_empty() {
        format!("{PATH}/{name}")
    } else {
        format!("{PATH}/{name}?{query}")
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

/// A store holding the whole containment lattice the rule ranges over: a group whose
/// member cell nests another cell, a group holding that nested cell directly, a group
/// with no cells at all, and a cell belonging to no group — with one resource set, and
/// one check watching it, per kind of scope a resource can carry.
async fn lattice() -> Emulator {
    let emulator = Emulator::new();

    for (cell, nested) in [
        ("leaf_cell", vec![]),
        ("mid_cell", vec![arn("cell", "leaf_cell")]),
        ("lone_cell", vec![]),
    ] {
        create(
            &emulator,
            "/cells",
            json!({"cellName": cell, "cells": nested}),
            cell,
        )
        .await;
    }

    for (group, cells) in [
        ("group_deep", vec![arn("cell", "mid_cell")]),
        ("group_flat", vec![arn("cell", "leaf_cell")]),
        ("group_empty", vec![]),
        (
            "group_wide",
            vec![arn("cell", "leaf_cell"), arn("cell", "lone_cell")],
        ),
    ] {
        create(
            &emulator,
            "/recoverygroups",
            json!({"recoveryGroupName": group, "cells": cells}),
            group,
        )
        .await;
    }

    // One resource set per kind of readiness scope, and one check watching each: the
    // check is in a group's summary exactly when its set's scope is one of that group's.
    for (set, check, scopes) in [
        (
            "set_group_deep",
            "check_group_deep",
            vec![arn("recovery-group", "group_deep")],
        ),
        (
            "set_group_empty",
            "check_group_empty",
            vec![arn("recovery-group", "group_empty")],
        ),
        ("set_mid", "check_mid", vec![arn("cell", "mid_cell")]),
        ("set_leaf", "check_leaf", vec![arn("cell", "leaf_cell")]),
        ("set_lone", "check_lone", vec![arn("cell", "lone_cell")]),
        ("set_unscoped", "check_unscoped", vec![]),
    ] {
        create(
            &emulator,
            "/resourcesets",
            json!({
                "resourceSetName": set,
                "resourceSetType": "AWS::SQS::Queue",
                "resources": [{"resourceArn": QUEUE_ARN, "readinessScopes": scopes}],
            }),
            set,
        )
        .await;
        create(
            &emulator,
            "/readinesschecks",
            json!({"readinessCheckName": check, "resourceSetName": set}),
            check,
        )
        .await;
    }

    emulator
}

/// The summary names exactly the checks scoped to the group, whatever the shape of the
/// containment underneath it, and reports `UNKNOWN` for each of them and in aggregate.
#[tokio::test]
async fn a_recovery_group_summary_names_the_checks_scoped_to_that_group() {
    let emulator = lattice().await;

    for (case, group, expected) in [
        (
            "a group scoped directly, through its member cell and through a nested cell",
            "group_deep",
            vec!["check_group_deep", "check_leaf", "check_mid"],
        ),
        (
            "a group holding one cell, which nests nothing",
            "group_flat",
            vec!["check_leaf"],
        ),
        (
            "a group holding no cells, scoped only by its own ARN",
            "group_empty",
            vec!["check_group_empty"],
        ),
        (
            "a group holding two sibling cells",
            "group_wide",
            vec!["check_leaf", "check_lone"],
        ),
    ] {
        let reply = summary(&emulator, group, "").await;
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
        let reported = reported_names(&reply);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the summary to name {expected:?}, got {reported:?}"
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
        }
        assert!(
            reported.len() <= DEFAULT_MAX_RESULTS,
            "case {case:?}: expected a default page of at most {DEFAULT_MAX_RESULTS}, got {}",
            reported.len()
        );
        assert_eq!(
            reply.string("nextToken"),
            None,
            "case {case:?}: expected no cursor on a page that is not full, got {}",
            reply.body
        );
    }
}

/// The summary pages: a page carries at most `maxResults` entries, a full page carries
/// the cursor the next one resumes from, and walking the cursors reports every scoped
/// check exactly once in name order whatever page size was asked for.
#[tokio::test]
async fn the_summary_pages_over_the_scoped_checks() {
    let emulator = lattice().await;
    let whole = reported_names(&summary(&emulator, "group_deep", "").await);
    assert_eq!(
        whole,
        vec!["check_group_deep", "check_leaf", "check_mid"],
        "expected the unpaged summary to name every scoped check, got {whole:?}"
    );

    for page_size in [1_usize, 2, 3, 4, 1000] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // More rounds than there can be pages, so a non-terminating cursor fails loudly
        // instead of looping forever.
        for _ in 0..16 {
            let mut query = vec![format!("maxResults={page_size}")];
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let reply = summary(&emulator, "group_deep", &query.join("&")).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected a page to answer HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            let reported = reported_names(&reply);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries on a page, got {}",
                reported.len()
            );
            walked.extend(reported.clone());
            token = reply.string("nextToken");
            match &token {
                Some(_) => assert_eq!(
                    reported.len(),
                    page_size,
                    "case {case:?}: a page carrying a cursor must be full, got {}",
                    reported.len()
                ),
                None => break,
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: expected the walk to end without a cursor, got {token:?}"
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the paged walk to report {whole:?}, got {walked:?}"
        );
    }
}

/// A `NextToken` this service did not mint is refused with the modelled
/// `ValidationException` (400) and the constant sentence, which does not echo the token.
#[tokio::test]
async fn the_summary_refuses_a_token_it_did_not_mint() {
    let emulator = lattice().await;

    for token in ["not_a_token", "", "YWJj", "%7B%7D", "0123456789abcdef"] {
        let case = format!("nextToken={token:?}");
        let reply = summary(&emulator, "group_deep", &format!("nextToken={token}")).await;
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
        assert!(
            !reply.body.to_string().contains(token) || token.is_empty(),
            "case {case:?}: expected the refusal not to echo the token, got {}",
            reply.body
        );
    }
}

/// A recovery group this account holds no record of answers the modelled
/// `ResourceNotFoundException` (404) with this service's constant sentence — whatever
/// the name's form and length, and for a name whose record was deleted as much as for
/// one that never existed. The recorded unit's own call is the first row.
#[tokio::test]
async fn a_summary_of_a_group_that_does_not_exist_is_not_found() {
    let emulator = lattice().await;
    // Rows differ along name form and length: the recorded ghost name, a one-character
    // name, digits only, underscores only, and the longest name this service can report
    // (`__stringMax64PatternAAZAZ09Z` is `length 0..64`, model:
    // `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
    let longest = "g".repeat(64);
    let mut cases: Vec<String> = vec![
        "sim2real_ghost_rg_1".to_string(),
        "g".to_string(),
        "1234567890".to_string(),
        "___".to_string(),
        longest,
        // A name that IS stored, but as another kind of resource: a cell and a readiness
        // check of this name exist, and neither is a recovery group.
        "leaf_cell".to_string(),
        "check_leaf".to_string(),
    ];

    // And a group that existed and was deleted: the summary stops reporting it.
    create(
        &emulator,
        "/recoverygroups",
        json!({"recoveryGroupName": "doomed_group", "cells": []}),
        "doomed_group",
    )
    .await;
    assert_eq!(
        summary(&emulator, "doomed_group", "").await.status,
        200,
        "fixture \"doomed_group\": expected the stored group to summarise"
    );
    let deleted = emulator
        .send("DELETE", "/recoverygroups/doomed_group", None)
        .await;
    // `DeleteRecoveryGroup` answers 204 (model `@http code`), with an empty body.
    assert_eq!(
        deleted.status, 204,
        "fixture \"doomed_group\": expected the delete to answer HTTP 204, got {} / {}",
        deleted.status, deleted.body
    );
    cases.push("doomed_group".to_string());

    for name in cases {
        let case = format!("RecoveryGroupName={name:?}");
        let reply = summary(&emulator, &name, "").await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the error code ResourceNotFoundException, got {}",
            reply.error_code()
        );
        assert_eq!(
            reply.string("message").as_deref(),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?}: expected the constant not-found sentence, got {}",
            reply.body
        );
        // The non-echo property is asserted on the distinctive names only: a short name
        // like "g" occurs inside the constant sentence by coincidence, which says
        // nothing about echoing.
        assert!(
            name.len() < 8 || !reply.body.to_string().contains(name.as_str()),
            "case {case:?}: expected the refusal not to echo the requested name, got {}",
            reply.body
        );
    }
}

/// The summary is a REPORTING operation: reading it changes nothing a later caller
/// sees — not the summary itself, not the group's record, and not the checks it covers.
#[tokio::test]
async fn reading_the_summary_changes_nothing() {
    let emulator = lattice().await;

    for group in ["group_deep", "group_empty"] {
        let before_group = emulator
            .send("GET", &format!("/recoverygroups/{group}"), None)
            .await;
        let first = summary(&emulator, group, "").await;
        let second = summary(&emulator, group, "").await;
        let third = summary(&emulator, group, "maxResults=1").await;
        let after_group = emulator
            .send("GET", &format!("/recoverygroups/{group}"), None)
            .await;

        assert_eq!(
            first.body, second.body,
            "case {group:?}: expected two reads to report the same summary, got {} then {}",
            first.body, second.body
        );
        assert_eq!(
            before_group.body, after_group.body,
            "case {group:?}: expected the group's record to be unchanged by a read, got {} then {}",
            before_group.body, after_group.body
        );
        let paged = reported_names(&third);
        let whole = reported_names(&first);
        assert_eq!(
            paged,
            whole.iter().take(1).cloned().collect::<Vec<String>>(),
            "case {group:?}: expected the first page to be the summary's first entry, got {paged:?}"
        );
        let checks = emulator.send("GET", "/readinesschecks", None).await;
        assert_eq!(
            checks.status, 200,
            "case {group:?}: expected the checks to still list, got {} / {}",
            checks.status, checks.body
        );
    }
}
