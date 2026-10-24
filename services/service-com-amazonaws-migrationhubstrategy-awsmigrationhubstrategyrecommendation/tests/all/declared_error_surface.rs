// SPDX-License-Identifier: CC0-1.0

//! Which failure surface a REPORTING operation of Strategy Recommendations refuses on,
//! end to end over HTTP.
//!
//! The rule under test is a property of the service, not of any one route: a read that
//! cannot produce its report — because the account holds no such resource, or the id it
//! was given names none — is refused with an error shape the OPERATION ITSELF declares,
//! at that shape's modelled `httpError`. So the status a client keys on and the error
//! code it branches on both come from the operation's own declared failure modes; an
//! operation whose list carries no not-found reports the condition through the client
//! error it does carry (`ValidationException`, 400) instead of a 404 nothing in its list
//! admits.
//!
//! Each row names the condition and the pair it must answer, and the pair is then checked
//! against the operation's declared error list and that shape's modelled status, so a row
//! cannot agree with a wrong expectation. The rows differ along what the rule generalises
//! over: whether the operation takes an id at all, which declared error reports the
//! condition, how the account came to be empty, and which resource is missing.
//!
//! Two refusals of this crate are deliberately NOT rows here: `GetAssessment`'s 400 for
//! an unknown id and `StopAssessment`'s 404, each of which answers a shape its own list
//! omits because the recorded wire behaviour of that route shows the service doing so.
//! They are pinned, with that justification, in `assessment_error_paths.rs` and
//! `refusal_messages.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    call, emulator, minimal_import, start_assessment, start_import, Answer,
};

/// The `errors` list each reporting operation under test declares, from
/// `smithy_query(query="digest")` plus the service-common `InternalServerException` that
/// digest reports under `common_errors`.
const DECLARED_ERRORS: &[(&str, &[&str])] = &[
    (
        "GetLatestAssessmentId",
        &[
            "AccessDeniedException",
            "DependencyException",
            "InternalServerException",
            "ValidationException",
        ],
    ),
    (
        "GetPortfolioPreferences",
        &[
            "AccessDeniedException",
            "InternalServerException",
            "ResourceNotFoundException",
            "ThrottlingException",
        ],
    ),
    (
        "GetRecommendationReportDetails",
        &[
            "AccessDeniedException",
            "InternalServerException",
            "ResourceNotFoundException",
            "ThrottlingException",
            "ValidationException",
        ],
    ),
    (
        "GetImportFileTask",
        &[
            "AccessDeniedException",
            "InternalServerException",
            "ResourceNotFoundException",
            "ThrottlingException",
            "ValidationException",
        ],
    ),
    (
        "GetServerDetails",
        &[
            "AccessDeniedException",
            "InternalServerException",
            "ResourceNotFoundException",
            "ThrottlingException",
            "ValidationException",
        ],
    ),
];

/// The `httpError` each error shape of this service declares, from
/// `smithy_query(query="shape",shape="AccessDeniedException,DependencyException,InternalServerException,ResourceNotFoundException,ThrottlingException,ValidationException")`.
const MODELLED_STATUS: &[(&str, u16)] = &[
    ("AccessDeniedException", 403),
    ("DependencyException", 500),
    ("InternalServerException", 500),
    ("ResourceNotFoundException", 404),
    ("ThrottlingException", 429),
    ("ValidationException", 400),
];

/// An `AsyncTaskId` (`@length(0,52)`, `@pattern [0-9a-z-:]+`) this account was never
/// issued, legal everywhere it is sent so the refusal comes from the handler.
const UNKNOWN_TASK_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A `ServerId` (`@length(1,27)`, `@pattern .*\S.*`) naming no server of this account.
const UNKNOWN_SERVER_ID: &str = "o-never-imported-server";

/// What the account looks like before the read under test is made.
#[derive(Debug, Clone, Copy)]
enum Account {
    /// Nothing has been called at all.
    Fresh,
    /// An accepted import brought servers in, so the store is not empty — but no
    /// assessment was ever started.
    AfterAnImport,
    /// A start the service refused, which must leave nothing behind.
    AfterARefusedStart,
}

/// One row: the condition, how the account got there, the read, and the pair it answers.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    operation: &'static str,
    account: Account,
    status: u16,
    code: &'static str,
}

/// Every reporting read of this crate whose answer, for a resource the account does not
/// have, is a refusal — spanning both declared codes and both kinds of read (one that
/// takes an id, and one that takes no input at all).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no assessment has ever been started",
            operation: "GetLatestAssessmentId",
            account: Account::Fresh,
            status: 400,
            code: "ValidationException",
        },
        Row {
            case: "servers were imported but no assessment started",
            operation: "GetLatestAssessmentId",
            account: Account::AfterAnImport,
            status: 400,
            code: "ValidationException",
        },
        Row {
            case: "the only start call was refused",
            operation: "GetLatestAssessmentId",
            account: Account::AfterARefusedStart,
            status: 400,
            code: "ValidationException",
        },
        Row {
            case: "no portfolio preferences have ever been saved",
            operation: "GetPortfolioPreferences",
            account: Account::Fresh,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            case: "a recommendation report id nothing minted",
            operation: "GetRecommendationReportDetails",
            account: Account::Fresh,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            case: "an import file task id nothing minted, in an account that has imports",
            operation: "GetImportFileTask",
            account: Account::AfterAnImport,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Row {
            case: "a server id naming no server of this account",
            operation: "GetServerDetails",
            account: Account::AfterAnImport,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ]
}

/// Put the account into the state the row describes.
async fn prepare(router: &MultiServiceRouter, account: Account) {
    match account {
        Account::Fresh => {}
        Account::AfterAnImport => {
            let imported = start_import(router, minimal_import("declared-errors")).await;
            assert_eq!(
                imported.status, 200,
                "the import that sets this row up must succeed, got {} with {:?}",
                imported.status, imported.body
            );
        }
        Account::AfterARefusedStart => {
            // No `s3bucketForAnalysisData`: the refusal is tabled in
            // `assessment_error_paths.rs`; here it only has to create nothing.
            let refused = start_assessment(router, json!({})).await;
            assert_eq!(
                refused.status, 400,
                "the start that sets this row up must be refused, got {} with {:?}",
                refused.status, refused.body
            );
        }
    }
}

/// Make the row's read.
async fn read(router: &MultiServiceRouter, operation: &str) -> Answer {
    match operation {
        "GetLatestAssessmentId" => call(router, "GET", "/get-latest-assessment-id", None).await,
        "GetPortfolioPreferences" => call(router, "GET", "/get-portfolio-preferences", None).await,
        "GetRecommendationReportDetails" => {
            call(
                router,
                "GET",
                &format!("/get-recommendation-report-details/{UNKNOWN_TASK_ID}"),
                None,
            )
            .await
        }
        "GetImportFileTask" => {
            call(
                router,
                "GET",
                &format!("/get-import-file-task/{UNKNOWN_TASK_ID}"),
                None,
            )
            .await
        }
        "GetServerDetails" => {
            call(
                router,
                "GET",
                &format!("/get-server-details/{UNKNOWN_SERVER_ID}"),
                None,
            )
            .await
        }
        other => panic!("this suite has no case for operation {other:?}"),
    }
}

/// The error shapes `operation` declares, from the catalog above.
fn declared(operation: &str) -> &'static [&'static str] {
    DECLARED_ERRORS
        .iter()
        .find(|(name, _)| *name == operation)
        .map(|(_, errors)| *errors)
        .unwrap_or_else(|| panic!("this suite has no declared error list for {operation:?}"))
}

/// The `httpError` the named error shape declares, from the catalog above.
fn modelled_status(code: &str) -> u16 {
    MODELLED_STATUS
        .iter()
        .find(|(name, _)| *name == code)
        .map(|(_, status)| *status)
        .unwrap_or_else(|| panic!("this suite has no modelled status for {code:?}"))
}

/// A read that cannot produce its report is refused with an error the operation declares,
/// at that error's modelled status — and asking again answers exactly the same thing.
#[tokio::test]
async fn an_unproducible_report_is_refused_with_an_error_the_operation_declares() {
    for row in rows() {
        let Row {
            case,
            operation,
            account,
            status,
            code,
        } = row;
        // A fresh emulator per row, so the account state is only what the row set up.
        let router = emulator();
        prepare(&router, account).await;

        let answer = read(&router, operation).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: {operation} must answer HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: {operation} must answer {code}, got {:?}",
            answer.body
        );
        assert!(
            declared(operation).contains(&answer.error_code()),
            "case {case:?}: {operation} answered {:?}, which its modelled errors \
             {:?} do not declare",
            answer.error_code(),
            declared(operation)
        );
        assert_eq!(
            answer.status,
            modelled_status(answer.error_code()),
            "case {case:?}: {operation} answered {:?} with HTTP {}, not the status that \
             shape declares",
            answer.error_code(),
            answer.status
        );
        assert!(
            !answer.string("/message").is_empty(),
            "case {case:?}: {operation} must say why it refused, got {:?}",
            answer.body
        );

        // A refusal reports; it must not change what the next caller sees.
        let again = read(&router, operation).await;
        assert_eq!(
            (again.status, again.error_code().to_string()),
            (status, code.to_string()),
            "case {case:?}: asking {operation} twice must answer the same refusal, got {} \
             with {:?}",
            again.status,
            again.body
        );
    }
}

/// The `GetLatestAssessmentId` refusal is computed from state, not fixed: the moment a
/// start the service accepts records an assessment the read answers 200 with that id, and
/// the answer follows every later start.
#[tokio::test]
async fn the_latest_assessment_id_refusal_tracks_the_accounts_assessments() {
    let router = emulator();

    let empty = read(&router, "GetLatestAssessmentId").await;
    assert_eq!(
        (empty.status, empty.error_code()),
        (400, "ValidationException"),
        "an account with no assessment is refused 400 ValidationException, got {} with {:?}",
        empty.status,
        empty.body
    );
    assert!(
        empty.string("/id").is_empty(),
        "a refusal must carry no id, got {:?}",
        empty.body
    );

    // Two accepted starts, differing in which optional members they carry, so the read
    // has to follow the LAST one rather than the only one.
    let starts: &[Value] = &[
        json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"}),
        json!({
            "s3bucketForAnalysisData": "migrationhub-strategy-analysis.two",
            "s3bucketForReportData": "migrationhub-strategy-reports",
            "assessmentDataSourceType": "ManualImport",
        }),
    ];
    let mut seen: Vec<String> = Vec::new();
    for (index, request) in starts.iter().enumerate() {
        let started = start_assessment(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "start {index}: the start must be accepted, got {} with {:?}",
            started.status, started.body
        );
        let id = started.string("/assessmentId");
        assert!(
            !seen.contains(&id),
            "start {index}: every start mints a fresh id, but {id:?} was already minted \
             ({seen:?})"
        );
        seen.push(id.clone());

        let latest = read(&router, "GetLatestAssessmentId").await;
        assert_eq!(
            latest.status, 200,
            "start {index}: once an assessment exists its id is reported, got {} with {:?}",
            latest.status, latest.body
        );
        assert_eq!(
            latest.string("/id"),
            id,
            "start {index}: the latest id must be the one just started, got {:?}",
            latest.body
        );
    }
}
