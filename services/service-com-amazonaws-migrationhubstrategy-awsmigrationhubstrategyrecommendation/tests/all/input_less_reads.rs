// SPDX-License-Identifier: CC0-1.0

//! What a read that takes NO INPUT answers when the account holds no resource for it,
//! end to end over HTTP.
//!
//! The rule under test is a property of the SERVICE, not of any one route: when a read
//! cannot produce its report because the per-account resource it would name does not
//! exist, the condition is a MISSING RESOURCE and the answer is the service's not-found
//! shape — `ResourceNotFoundException`, HTTP 404 — widened into the operation's error
//! channel where its own modelled `errors` list omits it. An operation whose input shape
//! has no members carries nothing a caller could have sent wrongly, so a validation
//! refusal (400 `ValidationException`) is never an available reading of this condition
//! however the account came to be empty.
//!
//! The rows span what the rule generalises over: which input-less read is asked, which
//! per-account record is the missing one, how the account came to be empty (nothing
//! called at all, an unrelated resource created, a create the service refused), and —
//! on the other side of the rule — the record existing, including after it reached a
//! terminal status, which still reports 200 rather than a not-found.
//!
//! Every message and status is spelled out here independently of `src/`, so a wrong
//! constant or a wrong shape in the crate fails here instead of agreeing with itself.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    call, emulator, minimal_import, start_assessment, start_import, stop_assessment, Answer,
};

/// Every read of this service whose input shape has no members and whose answer names a
/// per-account record that may be absent, with its route — from
/// `smithy_query(query="digest")`: `GetLatestAssessmentId` (`GET
/// /get-latest-assessment-id`, input `GetLatestAssessmentIdRequest` with no members) and
/// `GetPortfolioPreferences` (`GET /get-portfolio-preferences`, likewise).
///
/// `GetPortfolioSummary` takes no input either, but it reports a SUMMARY of the portfolio
/// rather than a record that can be absent, and it is still a scaffold stub — so it is
/// not a row here.
const INPUT_LESS_READS: &[(&str, &str)] = &[
    ("GetLatestAssessmentId", "/get-latest-assessment-id"),
    ("GetPortfolioPreferences", "/get-portfolio-preferences"),
];

/// The not-found this service answers for a resource that is not there, and the status
/// `ResourceNotFoundException` declares (`httpError` 404, from
/// `smithy_query(query="shape",shape="ResourceNotFoundException")`).
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// This service's sentence for a report it cannot produce because the resource it would
/// name does not exist yet.
const NOTHING_TO_REPORT: &str = "Something went wrong, please try again later.";

/// A member of each read's success payload, so a refusal can be checked to carry no part
/// of a report nothing could produce.
fn reported_member(operation: &str) -> &'static str {
    match operation {
        "GetLatestAssessmentId" => "/id",
        "GetPortfolioPreferences" => "/applicationMode",
        other => panic!("this suite has no reported member for {other:?}"),
    }
}

/// How the account under test was brought to the state the row describes.
#[derive(Debug, Clone, Copy)]
enum Account {
    /// Nothing has been called at all.
    Fresh,
    /// An accepted import brought servers in — neither an assessment nor preferences.
    AfterAnImport,
    /// A start the service refused, which must leave nothing behind.
    AfterARefusedStart,
    /// A preferences save the service refused, which must likewise leave nothing behind.
    AfterARefusedSave,
    /// An accepted start: the account now has a latest assessment id.
    AfterAStartedAssessment,
    /// An accepted save: the account now has a preferences record.
    AfterASavedPreference,
    /// A started assessment that was then stopped: the record still exists, in a
    /// terminal status.
    AfterAStoppedAssessment,
}

/// One row: how the account got there, and which of the input-less reads has a resource
/// to report once it did.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    account: Account,
    /// The operations whose record EXISTS after the setup; every other row operation has
    /// none and must be refused.
    present: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "nothing has ever been called",
            account: Account::Fresh,
            present: &[],
        },
        Row {
            case: "servers were imported, but nothing else",
            account: Account::AfterAnImport,
            present: &[],
        },
        Row {
            case: "the only start call was refused",
            account: Account::AfterARefusedStart,
            present: &[],
        },
        Row {
            case: "the only preferences save was refused",
            account: Account::AfterARefusedSave,
            present: &[],
        },
        Row {
            case: "an assessment was started",
            account: Account::AfterAStartedAssessment,
            present: &["GetLatestAssessmentId"],
        },
        Row {
            case: "preferences were saved",
            account: Account::AfterASavedPreference,
            present: &["GetPortfolioPreferences"],
        },
        Row {
            case: "an assessment was started and then stopped",
            account: Account::AfterAStoppedAssessment,
            present: &["GetLatestAssessmentId"],
        },
    ]
}

/// A `StartAssessment` request the service accepts: the analysis bucket carries the
/// prefix every bucket member of this service documents.
fn accepted_start() -> serde_json::Value {
    json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"})
}

/// Save one preferences member, which is enough to create the account's record.
async fn save_preferences(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "POST",
        "/put-portfolio-preferences",
        Some(json!({"applicationMode": "ALL"})),
    )
    .await
}

/// Put the account into the state the row describes.
async fn prepare(router: &MultiServiceRouter, case: &str, account: Account) {
    match account {
        Account::Fresh => {}
        Account::AfterAnImport => {
            let imported = start_import(router, minimal_import("input-less-reads")).await;
            assert_eq!(
                imported.status, 200,
                "case {case:?}: the import that sets this row up must succeed, got {} with {:?}",
                imported.status, imported.body
            );
        }
        Account::AfterARefusedStart => {
            // No `s3bucketForAnalysisData`, which `StartAssessment` cannot run without;
            // the refusal itself is tabled in `assessment_error_paths.rs`.
            let refused = start_assessment(router, json!({})).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: the start that sets this row up must be refused, got {} with {:?}",
                refused.status, refused.body
            );
        }
        Account::AfterARefusedSave => {
            // A `speedOfMigration` of 99, past `BusinessGoalsInteger`'s `@range(1, 5)`.
            let refused = call(
                router,
                "POST",
                "/put-portfolio-preferences",
                Some(json!({"prioritizeBusinessGoals": {"businessGoals": {"speedOfMigration": 99}}})),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: the save that sets this row up must be refused, got {} with {:?}",
                refused.status, refused.body
            );
        }
        Account::AfterAStartedAssessment => {
            let started = start_assessment(router, accepted_start()).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: the start that sets this row up must succeed, got {} with {:?}",
                started.status, started.body
            );
        }
        Account::AfterASavedPreference => {
            let saved = save_preferences(router).await;
            assert_eq!(
                saved.status, 200,
                "case {case:?}: the save that sets this row up must succeed, got {} with {:?}",
                saved.status, saved.body
            );
        }
        Account::AfterAStoppedAssessment => {
            let started = start_assessment(router, accepted_start()).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: the start that sets this row up must succeed, got {} with {:?}",
                started.status, started.body
            );
            let id = started.string("/assessmentId");
            let stopped = stop_assessment(router, &id).await;
            assert_eq!(
                stopped.status, 200,
                "case {case:?}: the stop that sets this row up must succeed, got {} with {:?}",
                stopped.status, stopped.body
            );
        }
    }
}

/// An input-less read whose per-account record does not exist answers the service's
/// not-found (404 `ResourceNotFoundException`) with the one sentence for that condition,
/// never a validation refusal and never a report; and where the record does exist the
/// same read answers 200.
#[tokio::test]
async fn an_input_less_read_with_no_resource_answers_the_services_not_found() {
    for Row {
        case,
        account,
        present,
    } in rows()
    {
        for (operation, uri) in INPUT_LESS_READS {
            // A fresh emulator per (row, operation), so the account holds only what the
            // row set up and no earlier read can have changed it.
            let router = emulator();
            prepare(&router, case, account).await;

            let answer = call(&router, "GET", uri, None).await;
            let case = format!("{case} / {operation}");

            if present.contains(operation) {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the record exists, so the read must report it, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                assert!(
                    !answer.string(reported_member(operation)).is_empty(),
                    "case {case:?}: a report must carry {}, got {:?}",
                    reported_member(operation),
                    answer.body
                );
                continue;
            }

            assert_eq!(
                (answer.status, answer.error_code()),
                NOT_FOUND,
                "case {case:?}: a missing resource is reported as {NOT_FOUND:?}, got {} \
                 with {:?}",
                answer.status,
                answer.body
            );
            assert_ne!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: this read takes no input, so nothing can be refused as \
                 invalid, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                NOTHING_TO_REPORT,
                "case {case:?}: expected the service's {NOTHING_TO_REPORT:?}, got {:?}",
                answer.body
            );
            assert!(
                answer.string(reported_member(operation)).is_empty(),
                "case {case:?}: a refusal must carry no part of a report, got {:?}",
                answer.body
            );

            // The refusal reports; asking again answers exactly the same thing.
            let again = call(&router, "GET", uri, None).await;
            assert_eq!(
                (again.status, again.error_code().to_string()),
                (NOT_FOUND.0, NOT_FOUND.1.to_string()),
                "case {case:?}: asking twice must answer the same refusal, got {} with {:?}",
                again.status,
                again.body
            );
        }
    }
}

/// Both reads report an absent resource on a fresh account.
async fn assert_both_absent(router: &MultiServiceRouter, case: &str) {
    for (operation, uri) in INPUT_LESS_READS {
        let before = call(router, "GET", uri, None).await;
        assert_eq!(
            (before.status, before.error_code()),
            NOT_FOUND,
            "case {case:?}: {operation} has no resource to report yet, got {} with {:?}",
            before.status,
            before.body
        );
    }
}

/// The not-found is computed from state: the moment the account holds the record an
/// input-less read reports, that route answers 200 — while the OTHER input-less read goes
/// on reporting its own, still absent, resource as a not-found.
///
/// Each half runs on its own account, so the record under test is the only one there.
#[tokio::test]
async fn creating_the_resource_turns_the_not_found_into_a_report() {
    // Half one: start an assessment, which is the record `GetLatestAssessmentId` names.
    let router = emulator();
    assert_both_absent(&router, "before a start").await;

    let started = start_assessment(&router, accepted_start()).await;
    assert_eq!(
        started.status, 200,
        "the start must be accepted, got {} with {:?}",
        started.status, started.body
    );
    let latest = call(&router, "GET", "/get-latest-assessment-id", None).await;
    assert_eq!(
        (latest.status, latest.string("/id")),
        (200, started.string("/assessmentId")),
        "the latest id must be the one the accepted start minted, got {:?}",
        latest.body
    );
    let preferences = call(&router, "GET", "/get-portfolio-preferences", None).await;
    assert_eq!(
        (preferences.status, preferences.error_code()),
        NOT_FOUND,
        "starting an assessment saves no preferences, got {} with {:?}",
        preferences.status,
        preferences.body
    );

    // Half two: save preferences, which is the record `GetPortfolioPreferences` names.
    let router = emulator();
    assert_both_absent(&router, "before a save").await;

    let saved = save_preferences(&router).await;
    assert_eq!(
        saved.status, 200,
        "the save must be accepted, got {} with {:?}",
        saved.status, saved.body
    );
    let preferences = call(&router, "GET", "/get-portfolio-preferences", None).await;
    assert_eq!(
        (preferences.status, preferences.string("/applicationMode")),
        (200, "ALL".to_string()),
        "the preferences record must be the one just saved, got {:?}",
        preferences.body
    );
    let latest = call(&router, "GET", "/get-latest-assessment-id", None).await;
    assert_eq!(
        (latest.status, latest.error_code()),
        NOT_FOUND,
        "saving preferences starts no assessment, got {} with {:?}",
        latest.status,
        latest.body
    );
}
