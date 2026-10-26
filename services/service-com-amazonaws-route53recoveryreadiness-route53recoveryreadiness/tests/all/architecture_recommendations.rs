// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetArchitectureRecommendations`.
//!
//! The rule under test: a report for a stored recovery group names one recommendation
//! for each resilience criterion the group's STORED architecture does not satisfy — its
//! member cells, the resources scoped to them and the readiness checks that watch those
//! resources — none at all for a group that satisfies every criterion, paged like every
//! other list of this service and stamped with the instant of the assessment it comes
//! from; and a name this account holds no recovery group under answers the modelled
//! `ResourceNotFoundException` (404) with this service's constant, resource-agnostic
//! sentence.
//!
//! The criteria are: REDUNDANCY (fewer than two member cells, so there is nothing to fail
//! over to), SYMMETRY (member cells holding different types of resource, so one cannot
//! take over another's traffic) and COVERAGE (a member cell no readiness check watches,
//! so nothing audits whether it is ready).
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! production emulator mounts (the harness is shared with `readiness_check_lifecycle.rs`),
//! so only the HTTP status, the `x-amzn-errortype` header and the JSON body are
//! inspected: an answer that is not computed from persisted state cannot pass.
//!
//! The tests are tables. Rows differ along what the rule generalises over — how many
//! cells a group holds, whether they hold resources at all and of how many types, how
//! deeply the resources sit (in the cell or in a cell nested inside it), whether a
//! readiness check watches them, whether a named cell exists at all, the page size asked
//! for and which token a page resumes from, and the form and length of a name that was
//! never created.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The instance URI this operation is bound to (model `@http`:
/// `GET /recoverygroups/{RecoveryGroupName}/architectureRecommendations`).
fn report_uri(group: &str, query: &str) -> String {
    let path = format!("/recoverygroups/{group}/architectureRecommendations");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// The constant, resource-agnostic sentence every not-found answer of this service's
/// resource-addressed operations carries. Written out here rather than imported, so the
/// test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The sentence a page answers for a `NextToken` this service did not mint.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// How far a reported instant may fall short of a wall-clock reading taken just before
/// the call that produced it: the `date-time` rendering keeps sub-second digits, so this
/// only absorbs the rounding of that rendering, not a whole assessment.
const CLOCK_TOLERANCE_SECONDS: f64 = 0.001;

/// The number of records a page carries when the caller names no `MaxResults`.
const DEFAULT_MAX_RESULTS: usize =
    service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::DEFAULT_MAX_RESULTS;

/// Two resource types the `ResourceSetType` member's own documentation enumerates
/// (model: `smithy_query(query="doc", shape="CreateResourceSetRequest",
/// member="ResourceSetType")`), used to make two cells hold the same type or different
/// ones.
const TABLE_TYPE: &str = "AWS::DynamoDB::Table";
const FUNCTION_TYPE: &str = "AWS::Lambda::Function";

/// ARNs of AWS resources a resource set can name, used as `ResourceIdentifier`s.
const TABLE_ARN: &str = "arn:aws:dynamodb:us-west-2:000000000000:table/t1";
const FUNCTION_ARN: &str = "arn:aws:lambda:us-west-2:000000000000:function:f1";

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out here rather
/// than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The recommendation a group with fewer than two member cells earns. Written out here
/// rather than imported, so the test pins the wording.
fn redundancy_text(cells: usize) -> String {
    format!(
        "A recovery group should contain at least 2 cells, so that an application can fail over \
         from one cell to another. This recovery group contains {cells}."
    )
}

/// The recommendation a group whose member cells hold different resource types earns.
const SYMMETRY_TEXT: &str = "The cells of a recovery group should be symmetric: a cell that holds \
     different types of resource than its sibling cells cannot take over their traffic. The cells \
     of this recovery group hold different resource types.";

/// The recommendation a member cell no readiness check watches earns.
fn coverage_text(cell_arn: &str) -> String {
    format!(
        "No readiness check watches a resource scoped to cell {cell_arn}. Create a resource set \
         whose resources name that cell in their readiness scopes, and a readiness check over \
         that resource set, so that the readiness of the cell is audited."
    )
}

/// POST `body` to `path` and assert the create succeeded, so a fixture that fails to
/// store fails where it was written rather than later as a wrong report.
async fn create(emulator: &Emulator, path: &str, body: Value, label: &str) {
    let reply = emulator.send("POST", path, Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "fixture {label:?}: expected the create to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

async fn cell(emulator: &Emulator, name: &str, nested: Vec<String>) {
    create(
        emulator,
        "/cells",
        json!({"cellName": name, "cells": nested}),
        name,
    )
    .await;
}

async fn recovery_group(emulator: &Emulator, name: &str, cells: Vec<String>) {
    create(
        emulator,
        "/recoverygroups",
        json!({"recoveryGroupName": name, "cells": cells}),
        name,
    )
    .await;
}

/// A resource set of `resource_set_type` holding one resource scoped to `scope`.
async fn resource_set(emulator: &Emulator, name: &str, resource_set_type: &str, scope: &str) {
    let resource_arn = if resource_set_type == FUNCTION_TYPE {
        FUNCTION_ARN
    } else {
        TABLE_ARN
    };
    create(
        emulator,
        "/resourcesets",
        json!({
            "resourceSetName": name,
            "resourceSetType": resource_set_type,
            "resources": [{"resourceArn": resource_arn, "readinessScopes": [scope]}],
        }),
        name,
    )
    .await;
}

async fn readiness_check(emulator: &Emulator, name: &str, set: &str) {
    create(
        emulator,
        "/readinesschecks",
        json!({"readinessCheckName": name, "resourceSetName": set}),
        name,
    )
    .await;
}

async fn report(emulator: &Emulator, group: &str, query: &str) -> Reply {
    emulator.send("GET", &report_uri(group, query), None).await
}

/// The `recommendationText`s the report named, in the order it named them.
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

/// The reported `lastAuditTimestamp`, as seconds since the epoch, parsed in the format
/// the member's `@timestampFormat date-time` names rather than compared as text (the
/// format omits a zero fractional part, so two equal instants can render differently).
fn audit_instant(reply: &Reply, case: &str) -> f64 {
    let text = reply
        .string("lastAuditTimestamp")
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastAuditTimestamp, got {}", reply.body));
    DateTime::from_str(&text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected an RFC-3339 lastAuditTimestamp, got {text:?} ({error})")
        })
        .as_secs_f64()
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A store holding one recovery group per architecture the criteria range over: an empty
/// group, a single bare cell, two symmetric cells, two asymmetric cells, two cells of
/// which one is watched, a group whose resources sit in a NESTED cell, a group naming a
/// cell that was never created, and a group of five bare cells (what the pagination test
/// pages over).
///
/// Every cell belongs to AT MOST ONE group, which is all this API's containment tree
/// admits: a second container for one cell is refused, so a fixture cannot be built that
/// way.
async fn architectures() -> Emulator {
    let emulator = Emulator::new();

    // A group with no cells at all, and one with a single cell holding nothing.
    recovery_group(&emulator, "group_empty_arch", vec![]).await;
    cell(&emulator, "single_cell", vec![]).await;
    recovery_group(
        &emulator,
        "group_single_bare",
        vec![arn("cell", "single_cell")],
    )
    .await;

    // Two cells holding one resource of the SAME type each, both watched.
    for suffix in ["a", "b"] {
        cell(&emulator, &format!("sym_{suffix}"), vec![]).await;
        resource_set(
            &emulator,
            &format!("set_sym_{suffix}"),
            TABLE_TYPE,
            &arn("cell", &format!("sym_{suffix}")),
        )
        .await;
        readiness_check(
            &emulator,
            &format!("check_sym_{suffix}"),
            &format!("set_sym_{suffix}"),
        )
        .await;
    }
    recovery_group(
        &emulator,
        "group_symmetric",
        vec![arn("cell", "sym_a"), arn("cell", "sym_b")],
    )
    .await;

    // Two cells holding DIFFERENT types of resource, both watched.
    for (suffix, resource_set_type) in [("a", TABLE_TYPE), ("b", FUNCTION_TYPE)] {
        cell(&emulator, &format!("asym_{suffix}"), vec![]).await;
        resource_set(
            &emulator,
            &format!("set_asym_{suffix}"),
            resource_set_type,
            &arn("cell", &format!("asym_{suffix}")),
        )
        .await;
        readiness_check(
            &emulator,
            &format!("check_asym_{suffix}"),
            &format!("set_asym_{suffix}"),
        )
        .await;
    }
    recovery_group(
        &emulator,
        "group_asymmetric",
        vec![arn("cell", "asym_a"), arn("cell", "asym_b")],
    )
    .await;

    // Two cells holding the same type of resource, of which only the first is watched.
    for suffix in ["a", "b"] {
        cell(&emulator, &format!("cov_{suffix}"), vec![]).await;
        resource_set(
            &emulator,
            &format!("set_cov_{suffix}"),
            TABLE_TYPE,
            &arn("cell", &format!("cov_{suffix}")),
        )
        .await;
    }
    readiness_check(&emulator, "check_cov_a", "set_cov_a").await;
    recovery_group(
        &emulator,
        "group_half_watched",
        vec![arn("cell", "cov_a"), arn("cell", "cov_b")],
    )
    .await;

    // A group whose first member cell holds nothing itself: its resource, and the check
    // watching it, sit on a cell NESTED inside it.
    cell(&emulator, "nest_child", vec![]).await;
    cell(
        &emulator,
        "nest_parent",
        vec![arn("cell", "nest_child")],
    )
    .await;
    resource_set(
        &emulator,
        "set_nest_child",
        TABLE_TYPE,
        &arn("cell", "nest_child"),
    )
    .await;
    readiness_check(&emulator, "check_nest_child", "set_nest_child").await;
    cell(&emulator, "nest_sibling", vec![]).await;
    resource_set(
        &emulator,
        "set_nest_sibling",
        TABLE_TYPE,
        &arn("cell", "nest_sibling"),
    )
    .await;
    readiness_check(&emulator, "check_nest_sibling", "set_nest_sibling").await;
    recovery_group(
        &emulator,
        "group_nested",
        vec![arn("cell", "nest_parent"), arn("cell", "nest_sibling")],
    )
    .await;

    // A group naming a cell that was never created, beside one that holds a watched
    // resource.
    cell(&emulator, "ghost_sibling", vec![]).await;
    resource_set(
        &emulator,
        "set_ghost_sibling",
        TABLE_TYPE,
        &arn("cell", "ghost_sibling"),
    )
    .await;
    readiness_check(&emulator, "check_ghost_sibling", "set_ghost_sibling").await;
    recovery_group(
        &emulator,
        "group_with_ghost_cell",
        vec![
            arn("cell", "ghost_sibling"),
            arn("cell", "never_created_cell"),
        ],
    )
    .await;

    // Five bare cells: five coverage recommendations to page over.
    let mut paged_cells = Vec::new();
    for index in 1..=5 {
        let name = format!("page_cell_{index}");
        cell(&emulator, &name, vec![]).await;
        paged_cells.push(arn("cell", &name));
    }
    recovery_group(&emulator, "group_paged", paged_cells).await;

    emulator
}

/// A report names exactly the recommendations the stored architecture earns, in criterion
/// order, and stamps them with an assessment instant in the modelled format.
#[tokio::test]
async fn a_report_names_one_recommendation_per_criterion_the_architecture_fails() {
    let emulator = architectures().await;
    let started = now_secs();

    for (case, group, expected) in [
        (
            "a group with no cells at all",
            "group_empty_arch",
            vec![redundancy_text(0)],
        ),
        (
            "a group with one cell, holding nothing and watched by nothing",
            "group_single_bare",
            vec![
                redundancy_text(1),
                coverage_text(&arn("cell", "single_cell")),
            ],
        ),
        (
            "two cells holding the same resource type, both watched",
            "group_symmetric",
            vec![],
        ),
        (
            "two cells holding different resource types, both watched",
            "group_asymmetric",
            vec![SYMMETRY_TEXT.to_string()],
        ),
        (
            "two cells holding the same resource type, only one watched",
            "group_half_watched",
            vec![coverage_text(&arn("cell", "cov_b"))],
        ),
        (
            "a group whose resources and checks sit on a nested cell",
            "group_nested",
            vec![],
        ),
        (
            "a group naming a cell that was never created",
            "group_with_ghost_cell",
            vec![
                SYMMETRY_TEXT.to_string(),
                coverage_text(&arn("cell", "never_created_cell")),
            ],
        ),
        (
            "five bare cells, none of them watched",
            "group_paged",
            (1..=5)
                .map(|index| coverage_text(&arn("cell", &format!("page_cell_{index}"))))
                .collect(),
        ),
    ] {
        let reply = report(&emulator, group, "").await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        let reported = reported_texts(&reply);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the report to name {expected:?}, got {reported:?}"
        );
        assert!(
            reply.field("recommendations").is_some(),
            "case {case:?}: expected a recommendations member even when it is empty, got {}",
            reply.body
        );
        let assessed = audit_instant(&reply, case);
        assert!(
            assessed >= started - 1.0 && assessed <= now_secs() + 1.0,
            "case {case:?}: expected the assessment instant to be the emulator's clock, got \
             {assessed} outside [{started}, {}]",
            now_secs()
        );
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

/// The report pages: a page carries at most `maxResults` recommendations, a full page
/// carries the cursor the next one resumes from, and walking the cursors reports every
/// recommendation exactly once in criterion order whatever page size was asked for.
#[tokio::test]
async fn the_report_pages_over_the_recommendations() {
    let emulator = architectures().await;
    let whole = reported_texts(&report(&emulator, "group_paged", "").await);
    assert_eq!(
        whole.len(),
        5,
        "expected the unpaged report to name one recommendation per unwatched cell, got {whole:?}"
    );

    for page_size in [1_usize, 2, 3, 5, 6, 1000] {
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
            let reply = report(&emulator, "group_paged", &query.join("&")).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected a page to answer HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            let reported = reported_texts(&reply);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: expected at most {page_size} recommendations on a page, got {}",
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
async fn the_report_refuses_a_token_it_did_not_mint() {
    let emulator = architectures().await;

    for token in ["not_a_token", "", "YWJj", "%7B%7D", "0123456789abcdef"] {
        let case = format!("nextToken={token:?}");
        let reply = report(&emulator, "group_paged", &format!("nextToken={token}")).await;
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
/// `ResourceNotFoundException` (404) with this service's constant sentence — whatever the
/// name's form and length, for a name stored as another kind of resource, and for one
/// whose record was deleted as much as for one that never existed.
#[tokio::test]
async fn a_report_for_a_group_that_does_not_exist_is_not_found() {
    let emulator = architectures().await;
    // Rows differ along name form and length: a name nothing was created under, a
    // one-character name, digits only, underscores only, and the longest name this
    // service can report (`__stringMax64PatternAAZAZ09Z` is `length 0..64`, model:
    // `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
    let longest = "g".repeat(64);
    let mut cases: Vec<String> = vec![
        "no_such_recovery_group".to_string(),
        "g".to_string(),
        "1234567890".to_string(),
        "___".to_string(),
        longest,
        // Names that ARE stored, but as another kind of resource.
        "single_cell".to_string(),
        "check_sym_a".to_string(),
        "set_sym_a".to_string(),
    ];

    // And a group that existed and was deleted: the report stops answering for it.
    recovery_group(&emulator, "doomed_group", vec![]).await;
    assert_eq!(
        report(&emulator, "doomed_group", "").await.status,
        200,
        "fixture \"doomed_group\": expected the stored group to report"
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
        let reply = report(&emulator, &name, "").await;
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
        // like "g" occurs inside the constant sentence by coincidence, which says nothing
        // about echoing.
        assert!(
            name.len() < 8 || !reply.body.to_string().contains(name.as_str()),
            "case {case:?}: expected the refusal not to echo the requested name, got {}",
            reply.body
        );
    }
}

/// The assessment instant belongs to the ASSESSMENT, not to the call: it holds while the
/// group's architecture is unchanged, including across a change elsewhere in the store,
/// and is re-stamped when something the criteria read about this group changes.
#[tokio::test]
async fn the_assessment_instant_holds_until_the_architecture_changes() {
    let emulator = architectures().await;

    // Two reports of an unchanged group are two reports of the same assessment.
    for group in ["group_symmetric", "group_half_watched", "group_empty_arch"] {
        let first = report(&emulator, group, "").await;
        let second = report(&emulator, group, "").await;
        assert_eq!(
            first.body, second.body,
            "case {group:?}: expected two reports of an unchanged group to be identical, got {} \
             then {}",
            first.body, second.body
        );
    }

    // A change that the criteria of THIS group do not read leaves its assessment alone.
    let before = audit_instant(
        &report(&emulator, "group_symmetric", "").await,
        "an unrelated resource set",
    );
    resource_set(
        &emulator,
        "set_elsewhere",
        FUNCTION_TYPE,
        &arn("cell", "page_cell_1"),
    )
    .await;
    let after = audit_instant(
        &report(&emulator, "group_symmetric", "").await,
        "an unrelated resource set",
    );
    assert_eq!(
        before, after,
        "case \"an unrelated resource set\": expected the assessment instant to hold, got {before} \
         then {after}"
    );

    // Changes the criteria DO read: a check that starts watching an unwatched cell, and a
    // resource of a new type that makes two cells asymmetric.
    for (case, group, before_texts, after_texts, change) in [
        (
            "a readiness check starts watching the unwatched cell",
            "group_half_watched",
            vec![coverage_text(&arn("cell", "cov_b"))],
            vec![],
            "check",
        ),
        (
            "a resource of another type makes the cells asymmetric",
            "group_symmetric",
            vec![],
            vec![SYMMETRY_TEXT.to_string()],
            "set",
        ),
    ] {
        let first = report(&emulator, group, "").await;
        let first_texts = reported_texts(&first);
        assert_eq!(
            first_texts, before_texts,
            "case {case:?}: expected the report before the change to name {before_texts:?}, got \
             {first_texts:?}"
        );
        let first_instant = audit_instant(&first, case);

        match change {
            "check" => readiness_check(&emulator, "check_cov_b", "set_cov_b").await,
            _ => {
                resource_set(
                    &emulator,
                    "set_sym_a_function",
                    FUNCTION_TYPE,
                    &arn("cell", "sym_a"),
                )
                .await
            }
        }

        let second = report(&emulator, group, "").await;
        let second_texts = reported_texts(&second);
        assert_eq!(
            second_texts, after_texts,
            "case {case:?}: expected the report after the change to name {after_texts:?}, got \
             {second_texts:?}"
        );
        let second_instant = audit_instant(&second, case);
        assert!(
            second_instant >= first_instant,
            "case {case:?}: expected the re-assessment not to predate the previous one, got \
             {second_instant} after {first_instant}"
        );
    }
}

/// A recovery group created again under a name that was deleted is a NEW resource: its
/// report is assessed afresh, so the instant it carries is this group's and not the
/// deleted one's — even when the recreated architecture is identical, which is the case
/// where a kept assessment would go unnoticed.
#[tokio::test]
async fn a_recreated_group_is_assessed_afresh() {
    let emulator = architectures().await;

    // Two rows: a group holding no cells, and one holding a bare cell — identical
    // configurations before and after the delete, so the recommendations are the same and
    // only the instant can tell the two assessments apart.
    cell(&emulator, "reborn_cell", vec![]).await;
    for (case, cells) in [
        ("a group holding no cells", Vec::new()),
        (
            "a group holding one bare cell",
            vec![arn("cell", "reborn_cell")],
        ),
    ] {
        recovery_group(&emulator, "reborn_group", cells.clone()).await;
        let first = report(&emulator, "reborn_group", "").await;
        let first_texts = reported_texts(&first);
        let first_instant = audit_instant(&first, case);

        let deleted = emulator
            .send("DELETE", "/recoverygroups/reborn_group", None)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected the delete to answer HTTP 204, got {} / {}",
            deleted.status, deleted.body
        );
        recovery_group(&emulator, "reborn_group", cells).await;

        // Sampled after the recreate and before the report, so an instant at or after it
        // can only have been taken by THIS group's assessment.
        let recreated_at = now_secs();
        let second = report(&emulator, "reborn_group", "").await;
        let second_texts = reported_texts(&second);
        assert_eq!(
            second_texts, first_texts,
            "case {case:?}: expected the recreated group to earn the same recommendations, got \
             {second_texts:?} after {first_texts:?}"
        );
        let second_instant = audit_instant(&second, case);
        assert!(
            second_instant + CLOCK_TOLERANCE_SECONDS >= recreated_at,
            "case {case:?}: expected the recreated group to be assessed afresh (at or after \
             {recreated_at}), got {second_instant}, the instant of the assessment before the \
             delete being {first_instant}"
        );

        // Left deleted, so the next row starts from the same state as this one.
        emulator
            .send("DELETE", "/recoverygroups/reborn_group", None)
            .await;
    }
}

/// The report is a REPORTING operation: reading it changes nothing a later caller sees —
/// not the report itself, not the group's record, and not the cells, resource sets or
/// checks the criteria read.
#[tokio::test]
async fn reading_the_report_changes_nothing() {
    let emulator = architectures().await;

    for group in ["group_paged", "group_symmetric", "group_empty_arch"] {
        let before_group = emulator
            .send("GET", &format!("/recoverygroups/{group}"), None)
            .await;
        let before_cells = emulator.send("GET", "/cells", None).await;
        let before_sets = emulator.send("GET", "/resourcesets", None).await;
        let before_checks = emulator.send("GET", "/readinesschecks", None).await;

        let first = report(&emulator, group, "").await;
        let second = report(&emulator, group, "").await;
        let paged = report(&emulator, group, "maxResults=1").await;
        let third = report(&emulator, group, "").await;

        let after_group = emulator
            .send("GET", &format!("/recoverygroups/{group}"), None)
            .await;

        assert_eq!(
            first.body, second.body,
            "case {group:?}: expected two reads to answer the same report, got {} then {}",
            first.body, second.body
        );
        assert_eq!(
            first.body, third.body,
            "case {group:?}: expected a paged read not to change the report, got {} then {}",
            first.body, third.body
        );
        let first_page = reported_texts(&paged);
        let whole = reported_texts(&first);
        assert_eq!(
            first_page,
            whole.iter().take(1).cloned().collect::<Vec<String>>(),
            "case {group:?}: expected the first page to be the report's first recommendation, got \
             {first_page:?}"
        );
        assert_eq!(
            before_group.body, after_group.body,
            "case {group:?}: expected the group's record to be unchanged by a read, got {} then {}",
            before_group.body, after_group.body
        );
        for (label, before) in [
            ("cells", &before_cells),
            ("resourcesets", &before_sets),
            ("readinesschecks", &before_checks),
        ] {
            let after = emulator.send("GET", &format!("/{label}"), None).await;
            assert_eq!(
                before.body, after.body,
                "case {group:?}: expected the {label} listing to be unchanged by a read, got {} \
                 then {}",
                before.body, after.body
            );
        }
    }
}
