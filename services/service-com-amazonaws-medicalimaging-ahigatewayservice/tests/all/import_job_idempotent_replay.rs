// SPDX-License-Identifier: CC0-1.0

//! What a `StartDICOMImportJob` request bearing a `clientToken` this service has ALREADY
//! recorded is answered with.
//!
//! RULE: `clientToken` is an `@idempotencyToken` on this `@idempotent` operation
//! (`smithy_query(query="operation",operation="StartDICOMImportJob")`), so a request
//! repeating a token the service already recorded is a REPLAY of the call that recorded
//! it: it submits nothing and reports the job that token already named, with that job's
//! CURRENT status. Because it submits nothing, no precondition on SUBMITTING — the data
//! store having settled into `ACTIVE`, the per-store job cap — governs it, so a
//! condition that changed after the first call was answered cannot turn the retry into a
//! failure. The one thing that does change the answer is the job's own absence: once the
//! data store the job lived in has been reaped, there is no job to report and the retry
//! is the modelled `ResourceNotFoundException` (HTTP 404).
//!
//! The rows span what the rule generalises over: what has CHANGED between the first
//! submit and the retry (nothing, the store's deletion now in flight, the clock having
//! carried the job to its terminal status, the per-store job cap now full, the store
//! reaped), crossed with whether the retry bears the recorded token or a fresh one, and
//! with how many times it is repeated. The FRESH-token rows are the control half: a
//! fresh token is a submission, so every one of those preconditions still governs it,
//! which is what keeps this rule from being "the guard is gone".
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::datastore_status_guard`): real requests through
//! `MultiServiceRouter::dispatch`, with the state struct touched only to HOLD a store in
//! the status it has already reached, never to assert.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::JOB_STATUS_VALUES;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    DATASTORE_DELETED_RETENTION, DATASTORE_DELETION_DELAY, DATASTORE_NOT_ACTIVE_MESSAGE,
    DATASTORE_NOT_FOUND_MESSAGE, IMPORT_JOB_COMPLETION_DELAY, MAX_IMPORT_JOBS_PER_DATASTORE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::datastore_status_guard::hold_transition;
use crate::import_job_listing::{list_jobs, summaries};

/// What happened to the data store, or to the job, between the first submit and the
/// retry the case sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Nothing: the store is still `ACTIVE` and the job still young.
    Nothing,
    /// The clock has carried the job to its terminal status and the store's deletion is in
    /// flight (`DELETING`) on top of that, held so the case cannot race it.
    ///
    /// The job settling FIRST is not an extra the row chose: a data store with a DICOM
    /// import job still running is not deletable at all, so a deletion in flight is only
    /// reachable once the job has reached a terminal status ([`settle_the_job`]).
    DeletionInFlight,
    /// The per-store import-job cap is full.
    JobCapFull,
    /// The deletion finished and the store's record was reaped, taking its jobs with it.
    DatastoreReaped,
}

/// Which `clientToken` the retry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    /// The token the first submit used — the retry a client doing at-least-once
    /// delivery sends.
    Recorded,
    /// A token this service has never seen, i.e. a genuine new submission.
    Fresh,
}

/// What the rule says the retry is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// HTTP 200 reporting the job the recorded token already named.
    SameJob,
    /// HTTP 200 submitting a second job, which the first one's identity differs from.
    NewJob,
    /// HTTP 409 `ConflictException` carrying this exact sentence.
    Conflict(&'static str),
    /// HTTP 402 `ServiceQuotaExceededException`.
    QuotaExceeded,
    /// HTTP 404 `ResourceNotFoundException` carrying this exact sentence.
    NotFound(&'static str),
}

/// One row of the replay table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    change: Change,
    token: Token,
    /// How many times the retry is sent: a replay is answered the same every time.
    retries: usize,
    expectation: Expectation,
    /// The `JobStatus` the job has settled into before the retry, for a row that waits
    /// the job's clock out — `None` where the job is still moving and only the modelled
    /// set and the progression below are assertable.
    settled_status: Option<&'static str>,
}

/// The `IAM` role a submit names (`RoleArn` is
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$` —
/// `smithy_query(query="operation",operation="StartDICOMImportJob")`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// The body `StartDICOMImportJob` takes, with the token the case is driving.
fn import_body(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "dataAccessRoleArn": ROLE_ARN,
        "inputS3Uri": "s3://imaging-in/studies/",
        "outputS3Uri": "s3://imaging-out/results/",
    })
}

/// How far along its progression a reported `JobStatus` is, so a read taken after an
/// answer can be asserted to be at or past what that answer reported.
///
/// The order is the import job's own: `SUBMITTED` is where it starts, `IN_PROGRESS` is
/// the move the clock owes it, and `COMPLETED` / `FAILED` are terminal
/// (`smithy_query(query="shape",shape="JobStatus")`).
fn progress(case: &str, status: &str) -> usize {
    match status {
        "SUBMITTED" => 0,
        "IN_PROGRESS" => 1,
        "COMPLETED" | "FAILED" => 2,
        other => panic!("case {case:?}: expected a modelled JobStatus, got {other:?}"),
    }
}

/// How many import jobs the data store HOLDS, read over the wire and paged to the end so
/// the count is exact past one page (`maxResults` tops out at 50).
async fn jobs_held(emulator: &Emulator, case: &str, datastore_id: &str) -> usize {
    let mut held = 0_usize;
    let mut next: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: ListDICOMImportJobs paging did not terminate after {pages} pages"
        );
        let mut query = "maxResults=50".to_string();
        if let Some(token) = &next {
            query.push_str(&format!("&nextToken={token}"));
        }
        let page = list_jobs(emulator, datastore_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected HTTP 200 from ListDICOMImportJobs, got {page:?}"
        );
        held += summaries(&page).len();
        match page.field(&["nextToken"]).as_str() {
            Some(token) => next = Some(token.to_string()),
            None => return held,
        }
    }
}

/// Wait out the clock the submitted job owes, so the data store holds no RUNNING import
/// job.
///
/// Every row that DELETES the store goes through here first: a data store with a DICOM
/// import job still in flight is refused the delete outright, so a deletion in flight is
/// reachable only once the job has settled.
async fn settle_the_job() {
    tokio::time::sleep(SETTLE_MULTIPLE * IMPORT_JOB_COMPLETION_DELAY).await;
}

/// Bring about the row's change, so the retry under test arrives into it.
async fn apply(emulator: &Emulator, case: &str, change: Change, datastore_id: &str, index: usize) {
    match change {
        Change::Nothing => {}
        Change::DeletionInFlight => {
            // The job's own clock first, while the store is still ACTIVE: the deletion is
            // what the row stacks ON TOP of a job that has already finished.
            settle_the_job().await;
            start_deleting(emulator, case, datastore_id, Hold::Yes).await;
        }
        Change::JobCapFull => {
            // The first submit already occupies one place, so the cap is reached by
            // filling the rest with tokens of their own.
            for filler in 1..MAX_IMPORT_JOBS_PER_DATASTORE {
                let body = import_body(&format!("cap-filler-{index}-{filler}"));
                let answer = emulator.start_import_job(datastore_id, body).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200 filling job {filler} of {MAX_IMPORT_JOBS_PER_DATASTORE}, got {answer:?}"
                );
            }
        }
        Change::DatastoreReaped => {
            // NOT held: this row is the one that waits the deletion, and the retention
            // after it, all the way out.
            settle_the_job().await;
            start_deleting(emulator, case, datastore_id, Hold::No).await;
            tokio::time::sleep(SETTLE_MULTIPLE * (DATASTORE_DELETION_DELAY + DATASTORE_DELETED_RETENTION))
                .await;
            let gone = emulator.get_datastore(datastore_id).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: expected the reaped store to be gone, got {gone:?}"
            );
        }
    }
}

/// Whether the deletion the case starts is HELD in flight, i.e. whether its due instant
/// is pushed out so the case cannot race the clock settling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Yes,
    No,
}

/// Delete the data store and prove over HTTP that its deletion is in flight.
async fn start_deleting(emulator: &Emulator, case: &str, datastore_id: &str, hold: Hold) {
    let deleted = emulator.delete_datastore(datastore_id).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 deleting the settled store, got {deleted:?}"
    );
    assert_eq!(
        deleted.string(&["datastoreStatus"]),
        "DELETING",
        "case {case:?}: expected DELETING from DeleteDatastore, got {deleted:?}"
    );
    if hold == Hold::Yes {
        hold_transition(emulator, case, datastore_id);
    }
}

/// RULE (above): a `clientToken` this service already recorded makes the repeat call a
/// REPLAY — it reports the job that token named and submits nothing, so no precondition
/// on submitting governs it; a token it has never seen is a submission, and every one of
/// those preconditions still does.
#[tokio::test]
async fn a_recorded_client_token_replays_the_job_it_named() {
    let rows = [
        Row {
            case: "nothing changed, recorded token",
            change: Change::Nothing,
            token: Token::Recorded,
            retries: 3,
            expectation: Expectation::SameJob,
            settled_status: None,
        },
        Row {
            case: "nothing changed, fresh token",
            change: Change::Nothing,
            token: Token::Fresh,
            retries: 1,
            expectation: Expectation::NewJob,
            settled_status: None,
        },
        Row {
            case: "the store's deletion is in flight, recorded token",
            change: Change::DeletionInFlight,
            token: Token::Recorded,
            retries: 3,
            expectation: Expectation::SameJob,
            settled_status: Some("COMPLETED"),
        },
        Row {
            case: "the store's deletion is in flight, fresh token",
            change: Change::DeletionInFlight,
            token: Token::Fresh,
            retries: 2,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
            settled_status: Some("COMPLETED"),
        },
        Row {
            case: "the job cap is full, recorded token",
            change: Change::JobCapFull,
            token: Token::Recorded,
            retries: 2,
            expectation: Expectation::SameJob,
            settled_status: None,
        },
        Row {
            case: "the job cap is full, fresh token",
            change: Change::JobCapFull,
            token: Token::Fresh,
            retries: 1,
            expectation: Expectation::QuotaExceeded,
            settled_status: None,
        },
        Row {
            case: "the store was reaped, recorded token",
            change: Change::DatastoreReaped,
            token: Token::Recorded,
            retries: 2,
            expectation: Expectation::NotFound(DATASTORE_NOT_FOUND_MESSAGE),
            settled_status: None,
        },
        Row {
            case: "the store was reaped, fresh token",
            change: Change::DatastoreReaped,
            token: Token::Fresh,
            retries: 1,
            expectation: Expectation::NotFound(DATASTORE_NOT_FOUND_MESSAGE),
            settled_status: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One emulator per row, so no row inherits another's stores or tokens.
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("replay.store", &format!("replay-store-{index}"))
            .await;

        let recorded_token = format!("replay-token-{index}");
        let first = emulator
            .start_import_job(&datastore_id, import_body(&recorded_token))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200 from the first submit, got {first:?}"
        );
        assert_eq!(
            first.string(&["jobStatus"]),
            "SUBMITTED",
            "case {case:?}: expected SUBMITTED from the first submit, got {first:?}"
        );
        let job_id = first.string(&["jobId"]);

        apply(&emulator, case, row.change, &datastore_id, index).await;

        // What the row changed is observable BEFORE the retry: the job the token named
        // has reached the status the row waits for.
        if let Some(expected) = row.settled_status {
            let read = emulator.get_import_job(&datastore_id, &job_id).await;
            assert_eq!(
                read.string(&["jobProperties", "jobStatus"]),
                expected,
                "case {case:?}: expected the job to report {expected} before the retry, got {read:?}"
            );
        }

        // How many jobs the store holds before the retry, so what the retry did or did
        // not persist is read off the wire rather than assumed. A reaped store holds
        // nothing and answers no listing.
        let held_before = match row.change {
            Change::DatastoreReaped => None,
            _ => Some(jobs_held(&emulator, case, &datastore_id).await),
        };

        let token = match row.token {
            Token::Recorded => recorded_token.clone(),
            Token::Fresh => format!("fresh-token-{index}"),
        };

        let mut minted: Vec<String> = Vec::new();
        for retry in 0..row.retries {
            let answer = emulator
                .start_import_job(&datastore_id, import_body(&token))
                .await;
            match row.expectation {
                Expectation::SameJob => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}, retry {retry}: expected HTTP 200 for the replay, got {answer:?}"
                    );
                    assert_eq!(
                        answer.string(&["jobId"]),
                        job_id,
                        "case {case:?}, retry {retry}: expected the replay to name {job_id:?}, got {answer:?}"
                    );
                    assert_eq!(
                        answer.string(&["datastoreId"]),
                        datastore_id,
                        "case {case:?}, retry {retry}: expected the replay to name the same data store, got {answer:?}"
                    );
                    assert_eq!(
                        answer.field(&["submittedAt"]),
                        first.field(&["submittedAt"]),
                        "case {case:?}, retry {retry}: expected the replay to report the first submit's submittedAt {:?}, got {answer:?}",
                        first.field(&["submittedAt"])
                    );
                    let reported = answer.string(&["jobStatus"]);
                    assert!(
                        JOB_STATUS_VALUES.contains(&reported.as_str()),
                        "case {case:?}, retry {retry}: expected a modelled JobStatus, got {answer:?}"
                    );
                    if let Some(expected) = row.settled_status {
                        assert_eq!(
                            reported, expected,
                            "case {case:?}, retry {retry}: expected the settled {expected}, got {answer:?}"
                        );
                    }
                    // It is the SAME job the reader reports, carried no further back
                    // than the replay found it.
                    let read = emulator.get_import_job(&datastore_id, &job_id).await;
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}, retry {retry}: expected the replayed job to read back, got {read:?}"
                    );
                    let read_status = read.string(&["jobProperties", "jobStatus"]);
                    assert!(
                        progress(case, &read_status) >= progress(case, &reported),
                        "case {case:?}, retry {retry}: expected GetDICOMImportJob to report {reported} or later, got {read:?}"
                    );
                }
                Expectation::NewJob => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}, retry {retry}: expected HTTP 200 for a new submission, got {answer:?}"
                    );
                    let new_id = answer.string(&["jobId"]);
                    assert_ne!(
                        new_id, job_id,
                        "case {case:?}, retry {retry}: expected a fresh token to mint a job of its own, got {answer:?}"
                    );
                    assert_eq!(
                        answer.string(&["jobStatus"]),
                        "SUBMITTED",
                        "case {case:?}, retry {retry}: expected SUBMITTED for a new submission, got {answer:?}"
                    );
                    minted.push(new_id);
                }
                Expectation::Conflict(message) => {
                    assert_eq!(
                        answer.status, 409,
                        "case {case:?}, retry {retry}: expected HTTP 409, got {answer:?}"
                    );
                    assert_eq!(
                        answer.error_code, "ConflictException",
                        "case {case:?}, retry {retry}: expected ConflictException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.string(&["message"]),
                        message,
                        "case {case:?}, retry {retry}: expected the refusal's own sentence, got {answer:?}"
                    );
                    assert!(
                        answer.field(&["jobId"]).is_null(),
                        "case {case:?}, retry {retry}: a refusal must mint no jobId, got {answer:?}"
                    );
                }
                Expectation::QuotaExceeded => {
                    assert_eq!(
                        answer.status, 402,
                        "case {case:?}, retry {retry}: expected HTTP 402, got {answer:?}"
                    );
                    assert_eq!(
                        answer.error_code, "ServiceQuotaExceededException",
                        "case {case:?}, retry {retry}: expected ServiceQuotaExceededException, got {answer:?}"
                    );
                    assert!(
                        answer.field(&["jobId"]).is_null(),
                        "case {case:?}, retry {retry}: a refusal must mint no jobId, got {answer:?}"
                    );
                }
                Expectation::NotFound(message) => {
                    assert_eq!(
                        answer.status, 404,
                        "case {case:?}, retry {retry}: expected HTTP 404, got {answer:?}"
                    );
                    assert_eq!(
                        answer.error_code, "ResourceNotFoundException",
                        "case {case:?}, retry {retry}: expected ResourceNotFoundException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.string(&["message"]),
                        message,
                        "case {case:?}, retry {retry}: expected the not-found sentence, got {answer:?}"
                    );
                    assert!(
                        answer.field(&["jobId"]).is_null(),
                        "case {case:?}, retry {retry}: a refusal must mint no jobId, got {answer:?}"
                    );
                }
            }
        }

        // And what the retries left behind: a replay and a refusal persist nothing, a
        // new submission persists exactly the jobs it reported.
        if let Some(before) = held_before {
            let after = jobs_held(&emulator, case, &datastore_id).await;
            let expected = before + minted.len();
            assert_eq!(
                after, expected,
                "case {case:?}: expected the store to hold {expected} jobs after {} retries (it held {before} before), got {after}",
                row.retries
            );
        }
    }
}
