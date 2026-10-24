// SPDX-License-Identifier: CC0-1.0

//! What a data store serves while a status change the CLOCK owes it is still in flight.
//!
//! RULE: a data store answers work on itself or on its contents only once it has settled
//! into `ACTIVE`; while a transition the service has already promised is mid-flight
//! (`CREATING` on the way to `ACTIVE`, `DELETING` on the way to gone), such a call is
//! refused with the operation's modelled `ConflictException` (HTTP 409 — "Updating or
//! deleting a resource can cause an inconsistent state"), nothing in the store changes,
//! and the transition still completes on time. The one exception is the delete of a store
//! ALREADY `DELETING`, which is the idempotent repeat of the transition in flight.
//!
//! The rows span what the rule generalises over: which status is in flight (`CREATING`,
//! `DELETING`) crossed with which operation arrives (`DeleteDatastore`,
//! `StartDICOMImportJob`, `CopyImageSet`, `DeleteImageSet`), plus the settled `ACTIVE`
//! store every one of them is admitted on. The second suite drives the same rule through
//! the REAL clock, with no reach into the state struct: a delete issued inside the
//! activation span is refused and the creation it raced still completes.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::image_set_copy`): real requests through `MultiServiceRouter::dispatch`, with
//! the state struct touched only to SEED (an image set, which only a DICOM import mints)
//! and to HOLD a store in the status it already reached, never to assert.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::JOB_STATUS_VALUES;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, DATASTORE_ACTIVATION_DELAY, DATASTORE_NOT_ACTIVE_MESSAGE,
    DATASTORE_STILL_CREATING_MESSAGE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{copy_image_set, information, seed_image_set};
use crate::image_set_deletion::delete_image_set;
use crate::import_job_listing::{list_jobs, summaries};

/// How far out a held transition's due instant is pushed, so the data store stays in the
/// status it has already reached for the whole of a case. Long enough that no scheduling
/// hiccup can let the clock settle it mid-case.
const HELD: Duration = Duration::from_secs(3600);

/// The status a data store is in when the call under test arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `CREATING`: the clock owes the move to `ACTIVE`.
    Creating,
    /// `ACTIVE`: settled, nothing owed.
    Active,
    /// `DELETING`: the clock owes the deletion.
    Deleting,
}

impl Phase {
    /// The `DatastoreStatus` wire value a store in this phase reports
    /// (`smithy_query(query="shape",shape="DatastoreStatus")`).
    fn wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// The operation the case sends at the data store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    DeleteDatastore,
    StartImportJob,
    CopyImageSet,
    DeleteImageSet,
}

/// What the rule says the call is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// HTTP 409 `ConflictException`, carrying this exact sentence.
    Conflict(&'static str),
    /// Admitted: HTTP 200.
    Admitted,
}

/// One row of the status-guard table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    phase: Phase,
    call: Call,
    expectation: Expectation,
}

/// A `ClientToken` for a case: the case name with every character the member's pattern
/// forbids folded to `-` (`ClientToken` is `^[A-Za-z0-9._-]+$`, len 1..64 —
/// `smithy_query(query="shape",shape="ClientToken")`).
fn case_token(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{slug}").chars().take(64).collect()
}

/// Put a data store into `phase` and prove over HTTP that it reports that status.
///
/// A non-terminal phase is HELD (its due instant pushed out by [`HELD`]) so the case
/// cannot race the clock; the `ACTIVE` phase is reached by waiting the activation out,
/// exactly as the sibling suites do.
async fn datastore_in_phase(emulator: &Emulator, case: &str, phase: Phase) -> String {
    let datastore_id = match phase {
        Phase::Creating => {
            let created = emulator
                .create_datastore(json!({
                    "datastoreName": "guard.store",
                    "clientToken": case_token("guard", case),
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
            );
            let datastore_id = created.string(&["datastoreId"]);
            hold_transition(emulator, case, &datastore_id);
            datastore_id
        }
        Phase::Active => {
            emulator
                .active_datastore("guard.store", &case_token("guard", case))
                .await
        }
        Phase::Deleting => {
            let datastore_id = emulator
                .active_datastore("guard.store", &case_token("guard", case))
                .await;
            let deleted = emulator.delete_datastore(&datastore_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected HTTP 200 deleting a settled store, got {deleted:?}"
            );
            hold_transition(emulator, case, &datastore_id);
            datastore_id
        }
    };

    let read = emulator.get_datastore(&datastore_id).await;
    let reported = read.string(&["datastoreProperties", "datastoreStatus"]);
    assert_eq!(
        reported,
        phase.wire(),
        "case {case:?}: expected the store to report {} before the call under test, got {read:?}",
        phase.wire()
    );
    datastore_id
}

/// Push the instant the data store's next clock-owed transition falls due out by
/// [`HELD`], so the store stays in the status it has already reached.
///
/// The only write a case makes into the state struct, and it changes no status: it moves
/// the DUE instant the handler itself recorded.
fn hold_transition(emulator: &Emulator, case: &str, datastore_id: &str) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the created data store");
    assert!(
        datastore.status_due_at.is_some(),
        "case {case:?}: expected a transition in flight to hold, got {:?}",
        datastore.datastore_status
    );
    datastore.status_due_at = Some(SystemTime::now() + HELD);
}

/// The body `StartDICOMImportJob` takes, with a token unique to the case.
fn import_job_body(case: &str) -> Value {
    json!({
        "clientToken": case_token("guard-job", case),
        "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
        "inputS3Uri": "s3://imaging-in/",
        "outputS3Uri": "s3://imaging-out/",
    })
}

/// How many import jobs a data store reports over the wire
/// (`GET /listDICOMImportJobs/datastore/{datastoreId}`), read through the sibling
/// listing suite's own helpers so there is one `jobSummaries` walk in this crate.
async fn job_count(emulator: &Emulator, case: &str, datastore_id: &str) -> usize {
    let listing = list_jobs(emulator, datastore_id, "").await;
    assert_eq!(
        listing.status, 200,
        "case {case:?}: expected HTTP 200 from ListDICOMImportJobs, got {listing:?}"
    );
    summaries(&listing).len()
}

/// RULE (above): only a settled `ACTIVE` data store serves work on itself or its
/// contents; a call that arrives while a clock-owed transition is in flight is refused
/// with the modelled `ConflictException` and changes nothing — except the idempotent
/// repeat of a delete already in flight.
#[tokio::test]
async fn a_datastore_mid_transition_refuses_work_with_a_conflict() {
    let rows = vec![
        Row {
            case: "delete while creating",
            phase: Phase::Creating,
            call: Call::DeleteDatastore,
            expectation: Expectation::Conflict(DATASTORE_STILL_CREATING_MESSAGE),
        },
        Row {
            case: "import while creating",
            phase: Phase::Creating,
            call: Call::StartImportJob,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "copy while creating",
            phase: Phase::Creating,
            call: Call::CopyImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "import while deleting",
            phase: Phase::Deleting,
            call: Call::StartImportJob,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "copy while deleting",
            phase: Phase::Deleting,
            call: Call::CopyImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "delete while deleting is the idempotent repeat",
            phase: Phase::Deleting,
            call: Call::DeleteDatastore,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "delete once active",
            phase: Phase::Active,
            call: Call::DeleteDatastore,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "import once active",
            phase: Phase::Active,
            call: Call::StartImportJob,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "copy once active",
            phase: Phase::Active,
            call: Call::CopyImageSet,
            expectation: Expectation::Admitted,
        },
        Row {
            case: "delete an image set while creating",
            phase: Phase::Creating,
            call: Call::DeleteImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "delete an image set while deleting",
            phase: Phase::Deleting,
            call: Call::DeleteImageSet,
            expectation: Expectation::Conflict(DATASTORE_NOT_ACTIVE_MESSAGE),
        },
        Row {
            case: "delete an image set once active",
            phase: Phase::Active,
            call: Call::DeleteImageSet,
            expectation: Expectation::Admitted,
        },
    ];

    for row in &rows {
        let case = row.case;
        // One emulator per row, so no row inherits another's stores.
        let emulator = Emulator::new();
        let datastore_id = datastore_in_phase(&emulator, case, row.phase).await;

        // A copy needs a source image set, which only a DICOM import mints in AWS — so
        // it is seeded, AFTER the phase is set up: a store with image sets could not be
        // deleted at all, and the refusal under test must be about the store's status.
        let source_image_set_id = match row.call {
            Call::CopyImageSet | Call::DeleteImageSet => {
                Some(seed_image_set(&emulator, &datastore_id, 1))
            }
            _ => None,
        };

        let answer = match row.call {
            Call::DeleteDatastore => emulator.delete_datastore(&datastore_id).await,
            Call::StartImportJob => {
                emulator
                    .start_import_job(&datastore_id, import_job_body(case))
                    .await
            }
            Call::CopyImageSet => {
                let source = source_image_set_id
                    .as_deref()
                    .expect("a seeded source image set");
                copy_image_set(
                    &emulator,
                    &datastore_id,
                    source,
                    information("1", None, None),
                    &[],
                )
                .await
            }
            Call::DeleteImageSet => {
                let image_set = source_image_set_id
                    .as_deref()
                    .expect("a seeded image set to delete");
                delete_image_set(&emulator, &datastore_id, image_set).await
            }
        };

        match row.expectation {
            Expectation::Conflict(message) => {
                assert_eq!(
                    answer.status, 409,
                    "case {case:?}: expected HTTP 409, got {answer:?}"
                );
                assert_eq!(
                    answer.error_code, "ConflictException",
                    "case {case:?}: expected ConflictException, got {answer:?}"
                );
                let reported = answer.string(&["message"]);
                assert_eq!(
                    reported, message,
                    "case {case:?}: expected message {message:?}, got {reported:?}"
                );
                assert!(
                    !reported.contains(datastore_id.as_str()),
                    "case {case:?}: the requested id must not be echoed, got {reported:?}"
                );

                // Nothing moved: the status the store was in is the status it is still
                // in, so a refused call cannot cancel or advance the transition.
                let after = emulator.get_datastore(&datastore_id).await;
                let still = after.string(&["datastoreProperties", "datastoreStatus"]);
                assert_eq!(
                    still,
                    row.phase.wire(),
                    "case {case:?}: expected the store still {} after the refusal, got {after:?}",
                    row.phase.wire()
                );
            }
            Expectation::Admitted => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {answer:?}"
                );
                match row.call {
                    Call::DeleteDatastore => assert_eq!(
                        answer.string(&["datastoreStatus"]),
                        "DELETING",
                        "case {case:?}: expected DELETING from DeleteDatastore, got {answer:?}"
                    ),
                    Call::StartImportJob => assert_eq!(
                        answer.string(&["jobStatus"]),
                        "SUBMITTED",
                        "case {case:?}: expected SUBMITTED from StartDICOMImportJob, got {answer:?}"
                    ),
                    Call::CopyImageSet => {
                        let destination =
                            answer.string(&["destinationImageSetProperties", "imageSetId"]);
                        assert!(
                            !destination.is_empty(),
                            "case {case:?}: expected a destination image set from CopyImageSet, got {answer:?}"
                        );
                    }
                    Call::DeleteImageSet => assert_eq!(
                        answer.string(&["imageSetState"]),
                        "LOCKED",
                        "case {case:?}: expected LOCKED from DeleteImageSet, got {answer:?}"
                    ),
                }
            }
        }
    }
}

/// RULE, through the REAL clock and nothing else: a delete that races the activation the
/// create promised is refused, and the creation it raced still completes — so the store
/// is `ACTIVE` afterwards and only then deletable.
///
/// Rows span how many deletes race the one creation, which is the loop a client retrying
/// a delete would run.
#[tokio::test]
async fn a_delete_racing_the_creation_is_refused_and_the_creation_still_lands() {
    for attempts in [1_usize, 2, 3] {
        let emulator = Emulator::new();
        let started = Instant::now();
        let created = emulator
            .create_datastore(json!({
                "datastoreName": "race.store",
                "clientToken": format!("race-token-{attempts}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case attempts={attempts}: expected HTTP 200 from CreateDatastore, got {created:?}"
        );
        assert_eq!(
            created.string(&["datastoreStatus"]),
            "CREATING",
            "case attempts={attempts}: expected CREATING from CreateDatastore, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);

        for attempt in 0..attempts {
            let refused = emulator.delete_datastore(&datastore_id).await;
            let elapsed = started.elapsed();
            // Asserted as a disjunction so no row is silently skipped: inside the
            // activation span the delete MUST be the conflict; past it the clock has
            // settled the store and the delete is legitimately admitted.
            assert!(
                (refused.status == 409 && refused.error_code == "ConflictException")
                    || elapsed >= DATASTORE_ACTIVATION_DELAY,
                "case attempts={attempts}, attempt {attempt}: expected 409 ConflictException {elapsed:?} into the activation span (the span is {DATASTORE_ACTIVATION_DELAY:?}), got {refused:?}"
            );
        }

        // The creation the deletes raced still falls due: the store is ACTIVE, not gone.
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
        let settled = emulator.get_datastore(&datastore_id).await;
        assert_eq!(
            settled.status, 200,
            "case attempts={attempts}: expected the raced creation to survive, got {settled:?}"
        );
        assert_eq!(
            settled.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case attempts={attempts}: expected ACTIVE after the activation span, got {settled:?}"
        );

        // And only now is the delete admitted.
        let deleted = emulator.delete_datastore(&datastore_id).await;
        assert_eq!(
            deleted.status, 200,
            "case attempts={attempts}: expected HTTP 200 deleting the settled store, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["datastoreStatus"]),
            "DELETING",
            "case attempts={attempts}: expected DELETING, got {deleted:?}"
        );
    }
}

/// One row of the "a refused import creates nothing" table: which status is in flight,
/// crossed with which optional members the refused request carries.
#[derive(Debug)]
struct RefusedImportRow {
    case: &'static str,
    phase: Phase,
    job_name: Option<&'static str>,
    input_owner_account_id: Option<&'static str>,
    import_configuration: bool,
}

/// The request one [`RefusedImportRow`] sends, built from the operation's required
/// members plus the optional ones the row names
/// (`smithy_query(query="operation",operation="StartDICOMImportJob")`).
fn refused_import_request(row: &RefusedImportRow) -> Value {
    let mut request = import_job_body(row.case);
    if let Some(name) = row.job_name {
        request["jobName"] = json!(name);
    }
    if let Some(account) = row.input_owner_account_id {
        request["inputOwnerAccountId"] = json!(account);
    }
    if row.import_configuration {
        request["importConfiguration"] = json!({
            "dicomJsonMetadataImportConfiguration": {
                "dicomMetadataMappings": [{
                    "studyInstanceUID": "1.2.840.113619.2.1.1",
                    "metadataFilePath": "metadata/study.json"
                }]
            }
        });
    }
    request
}

/// RULE (above), on what the refusal LEAVES BEHIND: an import refused for the data
/// store's status creates no job at all — the answer carries no `jobId` and the store's
/// job listing is as empty afterwards as before — and it is refused for the STORE, so no
/// member of the request can buy admission.
///
/// Rows span both axes the rule generalises over: which status is in flight (`CREATING`,
/// `DELETING`) crossed with which optional members the request carries (none, a
/// `jobName`, a source account id, the `importConfiguration` union).
#[tokio::test]
async fn an_import_refused_for_the_datastore_status_creates_no_job() {
    let rows = vec![
        RefusedImportRow {
            case: "creating, required members only",
            phase: Phase::Creating,
            job_name: None,
            input_owner_account_id: None,
            import_configuration: false,
        },
        RefusedImportRow {
            case: "creating, named job with a source account",
            phase: Phase::Creating,
            job_name: Some("nightly.import#1"),
            input_owner_account_id: Some("000000000001"),
            import_configuration: false,
        },
        RefusedImportRow {
            case: "creating, with an import configuration",
            phase: Phase::Creating,
            job_name: Some("metadata-enhance"),
            input_owner_account_id: None,
            import_configuration: true,
        },
        RefusedImportRow {
            case: "deleting, required members only",
            phase: Phase::Deleting,
            job_name: None,
            input_owner_account_id: None,
            import_configuration: false,
        },
        RefusedImportRow {
            case: "deleting, named job with an import configuration",
            phase: Phase::Deleting,
            job_name: Some("late.import"),
            input_owner_account_id: Some("000000000001"),
            import_configuration: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        // One emulator per row, so no row inherits another's stores or jobs.
        let emulator = Emulator::new();
        let datastore_id = datastore_in_phase(&emulator, case, row.phase).await;
        let before = job_count(&emulator, case, &datastore_id).await;
        assert_eq!(
            before, 0,
            "case {case:?}: expected a store holding no import jobs before the call, got {before}"
        );

        let refused = emulator
            .start_import_job(&datastore_id, refused_import_request(row))
            .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {refused:?}"
        );
        assert_eq!(
            refused.string(&["message"]),
            DATASTORE_NOT_ACTIVE_MESSAGE,
            "case {case:?}: expected the not-ACTIVE sentence, got {refused:?}"
        );
        assert!(
            refused.field(&["jobId"]).is_null(),
            "case {case:?}: a refusal must mint no jobId, got {refused:?}"
        );

        // Nothing was persisted: the store holds exactly as many jobs as it did before,
        // so no later read can report a job against a store that never served it.
        let after = job_count(&emulator, case, &datastore_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to persist no job (held {before} before), got {after}"
        );
    }
}

/// RULE (above), through the REAL clock and the issue's own sequence: an import that
/// races the activation the create promised is refused, and the refusal costs the caller
/// nothing — the creation still lands, and the SAME `clientToken` then submits exactly
/// one job, which reads back through `GetDICOMImportJob`.
///
/// Rows span how many imports race the one creation, which is the loop a client retrying
/// a submit would run; the idempotency token is the same across a row's attempts, so a
/// refusal that had consumed it would show up as a second job or a missing one.
#[tokio::test]
async fn an_import_racing_the_activation_is_refused_and_its_token_survives() {
    for attempts in [1_usize, 2, 3] {
        let case = format!("attempts={attempts}");
        let emulator = Emulator::new();
        let started = Instant::now();
        let created = emulator
            .create_datastore(json!({
                "datastoreName": "race.import.store",
                "clientToken": format!("race-import-store-{attempts}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
        );
        assert_eq!(
            created.string(&["datastoreStatus"]),
            "CREATING",
            "case {case:?}: expected CREATING from CreateDatastore, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);
        let request = json!({
            "clientToken": format!("race-import-token-{attempts}"),
            "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
            "inputS3Uri": "s3://imaging-in/studies/",
            "outputS3Uri": "s3://imaging-out/results/",
        });

        // Whether any racing attempt was already admitted, i.e. the clock settled the
        // store before the attempt landed. The post-settle assertions below then read
        // the idempotent repeat of a job that has been running for a while, rather than
        // a fresh submit, so they must not insist on `SUBMITTED`.
        let mut admitted_in_the_race = false;
        for attempt in 0..attempts {
            let answer = emulator
                .start_import_job(&datastore_id, request.clone())
                .await;
            let elapsed = started.elapsed();
            // A disjunction so no row is silently skipped, and BOTH arms pin an answer:
            // inside the activation span the import must be the conflict; past it the
            // clock has settled the store, so the only other admissible answer is the
            // submitted job itself — never, say, a 500.
            let refused = answer.status == 409 && answer.error_code == "ConflictException";
            let admitted = elapsed >= DATASTORE_ACTIVATION_DELAY
                && answer.status == 200
                && answer.string(&["jobStatus"]) == "SUBMITTED";
            assert!(
                refused || admitted,
                "case {case:?}, attempt {attempt}: expected 409 ConflictException {elapsed:?} into the activation span (the span is {DATASTORE_ACTIVATION_DELAY:?}), or SUBMITTED past it, got {answer:?}"
            );
            admitted_in_the_race |= admitted;
        }

        // The creation the imports raced still falls due.
        tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
        let settled = emulator.get_datastore(&datastore_id).await;
        assert_eq!(
            settled.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case {case:?}: expected ACTIVE after the activation span, got {settled:?}"
        );

        // And now the SAME token is admitted: the refusal consumed neither the token nor
        // a job slot.
        let submitted = emulator
            .start_import_job(&datastore_id, request.clone())
            .await;
        assert_eq!(
            submitted.status, 200,
            "case {case:?}: expected HTTP 200 once the store is ACTIVE, got {submitted:?}"
        );
        let reported_status = submitted.string(&["jobStatus"]);
        if admitted_in_the_race {
            // The token already named a job, so this is the idempotent repeat reporting
            // that job's CURRENT status — whichever modelled value the clock has carried
            // it to (`smithy_query(query="shape",shape="JobStatus")`).
            assert!(
                JOB_STATUS_VALUES.contains(&reported_status.as_str()),
                "case {case:?}: expected a modelled JobStatus from the idempotent repeat, got {submitted:?}"
            );
        } else {
            assert_eq!(
                reported_status, "SUBMITTED",
                "case {case:?}: expected SUBMITTED, got {submitted:?}"
            );
        }
        let job_id = submitted.string(&["jobId"]);

        // The ledger entry the admitted submit wrote is LIVE, not dangling: repeating the
        // same token names the same job rather than minting a second one.
        let repeated = emulator.start_import_job(&datastore_id, request).await;
        assert_eq!(
            repeated.status, 200,
            "case {case:?}: expected HTTP 200 repeating the token, got {repeated:?}"
        );
        assert_eq!(
            repeated.string(&["jobId"]),
            job_id,
            "case {case:?}: expected the repeat to name {job_id:?}, got {repeated:?}"
        );

        // Exactly one job exists: the refused attempts left nothing behind, and the one
        // that was admitted reads back.
        let held = job_count(&emulator, &case, &datastore_id).await;
        assert_eq!(
            held, 1,
            "case {case:?}: expected the store to hold exactly the one admitted job, got {held}"
        );
        let read = emulator.get_import_job(&datastore_id, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the admitted job to read back, got {read:?}"
        );
        assert_eq!(
            read.string(&["jobProperties", "jobId"]),
            job_id,
            "case {case:?}: expected GetDICOMImportJob to report {job_id:?}, got {read:?}"
        );
        assert_eq!(
            read.string(&["jobProperties", "datastoreId"]),
            datastore_id,
            "case {case:?}: expected the job to name its data store, got {read:?}"
        );
    }
}
