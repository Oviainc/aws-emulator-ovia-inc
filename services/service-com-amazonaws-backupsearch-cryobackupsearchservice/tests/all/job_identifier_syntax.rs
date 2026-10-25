// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SYNTAX GUARD every operation of this service that resolves a
//! job identifier applies, over all six of them: `GetSearchJob`, `ListSearchJobResults`,
//! `ListSearchJobBackups`, `StopSearchJob`, `GetSearchResultExportJob` and
//! `StartSearchResultExportJob`.
//!
//! The rule under test: an operation that resolves a job identifier to a record judges
//! that identifier's SHAPE before any store lookup — a string that cannot be an
//! identifier of this service is refused 400 `ValidationException`
//! (`state::INVALID_JOB_ID`) and resolves nothing, while a WELL-FORMED identifier that
//! names no record is refused 404 `ResourceNotFoundException`
//! (`state::RESOURCE_NOT_FOUND`) and a well-formed one that names a record is served.
//! The shape is the one this service mints (`state::job_identifier_well_formed`): the
//! `8-4-4-4-12` hex groups of a UUID. The model puts no `@pattern` on `GenericId`, so the
//! boundary validator cannot enforce that shape and the service does.
//!
//! So each assertion is made over a TABLE spanning the class rather than over the one
//! request the issue cited: the rows differ in WHY a string is not an identifier (its
//! length, a group of the wrong width, a missing or extra separator, a non-hex character,
//! another separator entirely, no separators at all), in the form a well-formed one takes
//! (the published nil UUID, an all-`f` one, an upper-case rendering, a shape whose
//! version nibble is not 4), and in which of the six operations carries it — in the URI
//! path for five of them and in the request body for `StartSearchResultExportJob`.
//!
//! The boundary of the rule is asserted too: `ListSearchResultExportJobs`'s
//! `SearchJobIdentifier` is a FILTER and never resolved, so it is not guarded and answers
//! an empty page for any value.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) and the transition-span
//! constants are the search-job suite's: every suite drives the same emulator the same
//! way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, Answer, PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT,
};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// Strings that are NOT identifiers of this service, one row each with why.
///
/// Source: the complement of the shape the service mints
/// (`state::job_identifier_well_formed`, which documents the `8-4-4-4-12` hex groups of
/// the UUID `state.rs` draws). Every way a string can miss that shape is a row: a word,
/// a single character, an identifier shaped like another service's, a group one digit
/// short and one digit long, a missing separator, an extra separator, a non-hex
/// character, another separator entirely, no separator at all, and a string far longer
/// than any identifier.
const MALFORMED: &[(&str, &str)] = &[
    ("a word that is not an identifier", "not-a-valid-uuid"),
    ("a plain word", "does-not-exist"),
    ("a single character", "x"),
    ("an identifier shaped like another service's", "searchjob-000000000042"),
    (
        "the last group one digit short",
        "48434c42-40a0-4057-b04d-3b484c610e2",
    ),
    (
        "the last group one digit too long",
        "48434c42-40a0-4057-b04d-3b484c610e2bb",
    ),
    (
        "a separator missing, so four groups",
        "48434c42-40a0-4057-b04d3b484c610e2b",
    ),
    (
        "a separator too many, so six groups",
        "48434c42-40a0-4057-b04d-3b48-4c610e2b",
    ),
    (
        "a non-hex character in the last group",
        "48434c42-40a0-4057-b04d-3b484c610e2g",
    ),
    (
        "underscores instead of the separator",
        "48434c42_40a0_4057_b04d_3b484c610e2b",
    ),
    (
        "the 32 hex digits with no separators",
        "48434c4240a04057b04d3b484c610e2b",
    ),
    (
        "far longer than any identifier",
        "a-very-long-identifier-that-no-call-ever-minted-0000000000000000",
    ),
];

/// Well-formed identifiers that name no record in a fresh store, one row each.
///
/// The rows differ in the form a well-formed identifier takes: the one the issue's
/// positive control uses, the nil UUID (RFC 4122 §4.1.7), the all-`f` one, an upper-case
/// rendering (a hex digit's case is not part of the shape, so this is well formed and
/// simply names nothing) and a shape whose version nibble is not 4 — the guard judges the
/// shape, not which record it names.
const WELL_FORMED_UNKNOWN: &[(&str, &str)] = &[
    ("a uuid no call minted", "48434c42-40a0-4057-b04d-3b484c610e2b"),
    ("the nil uuid", "00000000-0000-0000-0000-000000000000"),
    ("every digit f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    (
        "an upper-case rendering",
        "48434C42-40A0-4057-B04D-3B484C610E2B",
    ),
    (
        "a shape whose version nibble is not 4",
        "11111111-2222-3333-4444-555555555555",
    ),
];

/// One request naming a job identifier, as one of the six operations that RESOLVES one
/// carries it: in the URI path, or in the body.
struct Resolving {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation of this service that resolves a job identifier, each naming
/// `identifier`. `StartSearchResultExportJob` carries it in the BODY and the other five
/// in an `@httpLabel`, so the guard is asserted on both bindings.
fn resolving_requests(identifier: &str) -> Vec<Resolving> {
    vec![
        Resolving {
            operation: "GetSearchJob",
            method: "GET",
            uri: format!("/search-jobs/{identifier}"),
            body: None,
        },
        Resolving {
            operation: "ListSearchJobResults",
            method: "GET",
            uri: format!("/search-jobs/{identifier}/search-results"),
            body: None,
        },
        Resolving {
            operation: "ListSearchJobBackups",
            method: "GET",
            uri: format!("/search-jobs/{identifier}/backups"),
            body: None,
        },
        Resolving {
            operation: "StopSearchJob",
            method: "PUT",
            uri: format!("/search-jobs/{identifier}/actions/cancel"),
            body: None,
        },
        Resolving {
            operation: "GetSearchResultExportJob",
            method: "GET",
            uri: format!("/export-search-jobs/{identifier}"),
            body: None,
        },
        Resolving {
            operation: "StartSearchResultExportJob",
            method: "PUT",
            uri: "/export-search-jobs".to_string(),
            body: Some(json!({
                "SearchJobIdentifier": identifier,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "search-exports"}
                }
            })),
        },
    ]
}

/// How many export jobs the store holds — what proves a refused call created nothing.
fn stored_export_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>()
        .lock()
        .export_jobs
        .len()
}

/// The status `GetSearchJob` reports for `identifier` — what proves a refused call moved
/// nothing.
async fn reported_status(emulator: &MultiServiceRouter, identifier: &str) -> String {
    let answer = call(
        emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    answer.string("Status")
}

/// Start a search job and answer its identifier.
async fn started_job(emulator: &MultiServiceRouter, case: &str) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed to set the case up, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobIdentifier")
}

/// Assert `answer` is the refusal a malformed identifier calls for: 400
/// `ValidationException` carrying this service's fixed invalid-identifier text, and never
/// the not-found refusal.
fn assert_invalid(answer: &Answer, case: &str, operation: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?} at {operation}: a malformed identifier must be refused 400, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?} at {operation}: the refusal must be a ValidationException, got {:?}",
        answer.error_code
    );
    let message = answer.string("message");
    // The recorded sentence, pinned literally: the constant the guard uses must be THIS
    // text, so a drift in it fails here rather than passing silently.
    assert_eq!(
        message, "Invalid Job Id",
        "case {case:?} at {operation}: expected this service's invalid-identifier text, got \
         {message:?}"
    );
    assert_eq!(
        message,
        state::INVALID_JOB_ID,
        "case {case:?} at {operation}: every guard must take that text from one constant, got \
         {message:?}"
    );
    assert_ne!(
        message,
        state::RESOURCE_NOT_FOUND,
        "case {case:?} at {operation}: a malformed identifier is not a missing resource, got \
         {message:?}"
    );
}

/// Assert `answer` is the refusal a well-formed identifier naming nothing calls for: 404
/// `ResourceNotFoundException` with this service's fixed not-found text.
fn assert_not_found(answer: &Answer, case: &str, operation: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?} at {operation}: a well-formed identifier naming nothing must be 404, got \
         {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?} at {operation}: the refusal must be a ResourceNotFoundException, got {:?}",
        answer.error_code
    );
    assert_eq!(
        answer.string("message"),
        state::RESOURCE_NOT_FOUND,
        "case {case:?} at {operation}: expected this service's fixed not-found text, got {:?}",
        answer.get("message")
    );
}

#[tokio::test]
async fn a_malformed_job_identifier_is_refused_as_invalid_input_and_resolves_nothing() {
    let (emulator, engine) = emulator_with_engine();
    // A real search job exists, so a refusal is the guard's and not an empty store
    // answering for everything, and its status is what proves nothing was moved.
    let held = started_job(&emulator, "the held job").await;
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    let status_before = reported_status(&emulator, &held).await;
    assert!(
        !status_before.is_empty(),
        "the held job must be reported before the table runs, got {status_before:?}"
    );

    for (case, identifier) in MALFORMED {
        for request in resolving_requests(identifier) {
            let answer = call(
                &emulator,
                REGION,
                request.method,
                &request.uri,
                request.body.clone(),
            )
            .await;
            assert_invalid(&answer, case, request.operation);
            assert!(
                !answer.string("message").contains(identifier),
                "case {case:?} at {}: the fixed refusal text must not echo the identifier, got \
                 {:?}",
                request.operation,
                answer.get("message")
            );
        }
    }

    assert_eq!(
        stored_export_jobs(&engine),
        0,
        "a refused start must create no export job, store holds {}",
        stored_export_jobs(&engine)
    );
    assert_eq!(
        reported_status(&emulator, &held).await,
        status_before,
        "a refused call must move no job the store holds"
    );
}

#[tokio::test]
async fn a_well_formed_identifier_that_names_nothing_is_the_modelled_not_found() {
    let (emulator, engine) = emulator_with_engine();
    // Again with a record in the store, so the 404 is the lookup's answer.
    started_job(&emulator, "the held job").await;

    for (case, identifier) in WELL_FORMED_UNKNOWN {
        for request in resolving_requests(identifier) {
            let answer = call(
                &emulator,
                REGION,
                request.method,
                &request.uri,
                request.body.clone(),
            )
            .await;
            assert_not_found(&answer, case, request.operation);
        }
    }

    assert_eq!(
        stored_export_jobs(&engine),
        0,
        "a start refused as not found must create no export job, store holds {}",
        stored_export_jobs(&engine)
    );
}

#[tokio::test]
async fn an_identifier_this_service_minted_is_served_by_every_operation_that_resolves_one() {
    // The positive control of the rule, read back through the API: the guard refuses
    // nothing this service mints, on either binding.
    let (emulator, _engine) = emulator_with_engine();
    let identifier = started_job(&emulator, "the minted job").await;

    // Past the set-up phase, so the scan is the RUNNING one a stop acts from.
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    for request in resolving_requests(&identifier) {
        // The export start needs the job to have settled, so it is driven below.
        if request.operation == "StartSearchResultExportJob"
            || request.operation == "GetSearchResultExportJob"
        {
            continue;
        }
        let answer = call(
            &emulator,
            REGION,
            request.method,
            &request.uri,
            request.body.clone(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a minted identifier must be served, got {} ({:?})",
            request.operation, answer.status, answer.body
        );
    }

    // The stop above moved the job to STOPPING; once the clock has made STOPPED due its
    // results are final, so an export can be started from it.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": identifier,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "search-exports"}
            }
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchResultExportJob must accept the minted search job, got {} ({:?})",
        started.status, started.body
    );
    let export_identifier = started.string("ExportJobIdentifier");
    assert!(
        state::job_identifier_well_formed(&export_identifier),
        "the minted ExportJobIdentifier must satisfy the guard, got {export_identifier:?}"
    );
    let reported = call(
        &emulator,
        REGION,
        "GET",
        &format!("/export-search-jobs/{export_identifier}"),
        None,
    )
    .await;
    assert_eq!(
        reported.status, 200,
        "GetSearchResultExportJob must serve the export job just minted, got {} ({:?})",
        reported.status, reported.body
    );
    assert_eq!(
        reported.string("ExportJobIdentifier"),
        export_identifier,
        "the reported record must be the one the start minted, got {:?}",
        reported.body
    );
}

#[tokio::test]
async fn the_export_job_listing_filters_rather_than_resolves_so_its_member_is_not_guarded() {
    // The boundary of the rule: `ListSearchResultExportJobs`'s `SearchJobIdentifier`
    // narrows the listing and is never looked up, so no value of it is a refusal — the
    // answer is the matching records, which is the empty page when none match.
    let (emulator, _engine) = emulator_with_engine();
    let rows: Vec<(&str, &str)> = MALFORMED
        .iter()
        .chain(WELL_FORMED_UNKNOWN.iter())
        .copied()
        .collect();
    for (case, identifier) in rows {
        let listed = call(
            &emulator,
            REGION,
            "GET",
            &format!("/export-search-jobs?SearchJobIdentifier={identifier}"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a filtered listing must answer 200, got {} ({:?})",
            listed.status, listed.body
        );
        assert_eq!(
            listed.error_code, None,
            "case {case:?}: a filter value is never a refusal, got {:?}",
            listed.error_code
        );
        assert_eq!(
            listed.get("ExportJobs"),
            &json!([]),
            "case {case:?}: nothing matches the filter, so the page is empty, got {:?}",
            listed.get("ExportJobs")
        );
    }
}

#[tokio::test]
async fn the_guard_accepts_exactly_the_shape_this_service_mints() {
    // The shared helper every guarded operation goes through, over the same rows: it
    // admits the shape the mint draws and nothing else.
    for (case, identifier) in MALFORMED {
        assert!(
            !state::job_identifier_well_formed(identifier),
            "case {case:?}: {identifier:?} is not an identifier of this service, yet the guard \
             admitted it"
        );
        assert_eq!(
            state::check_job_identifier(identifier),
            Err(state::INVALID_JOB_ID.to_string()),
            "case {case:?}: the guard must refuse {identifier:?} with this service's \
             invalid-identifier text"
        );
    }
    for (case, identifier) in WELL_FORMED_UNKNOWN {
        assert!(
            state::job_identifier_well_formed(identifier),
            "case {case:?}: {identifier:?} has this service's identifier shape, yet the guard \
             refused it"
        );
        assert_eq!(
            state::check_job_identifier(identifier),
            Ok(()),
            "case {case:?}: a well-formed identifier is not an input refusal"
        );
    }

    // And whatever the mint draws is accepted: several draws, so the invariant is not
    // asserted on one id.
    let (emulator, _engine) = emulator_with_engine();
    for index in 0..5_usize {
        let case = format!("mint {index}");
        let identifier = started_job(&emulator, &case).await;
        assert!(
            state::job_identifier_well_formed(&identifier),
            "case {case:?}: every identifier this service mints must satisfy its own guard, got \
             {identifier:?}"
        );
    }
}
