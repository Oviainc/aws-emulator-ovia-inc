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

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, DATASTORE_ACTIVATION_DELAY, DATASTORE_NOT_ACTIVE_MESSAGE,
    DATASTORE_STILL_CREATING_MESSAGE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{copy_image_set, information, seed_image_set};
use crate::image_set_deletion::delete_image_set;

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
fn import_job_body(case: &str) -> serde_json::Value {
    json!({
        "clientToken": case_token("guard-job", case),
        "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
        "inputS3Uri": "s3://imaging-in/",
        "outputS3Uri": "s3://imaging-out/",
    })
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
