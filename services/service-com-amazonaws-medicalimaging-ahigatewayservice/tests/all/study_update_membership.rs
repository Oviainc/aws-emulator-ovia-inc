// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH image sets decide a study-wide `UpdateImageSetMetadata`
//! (`includeStudyImageSets`).
//!
//! RULE: a study-wide update is about the image sets the data store still HOLDS carrying
//! the named image set's Study Instance UID — an image set whose deletion has landed has
//! given up the DICOM content a study is made of, so it neither takes the update nor stands
//! in its way — and the request is refused with 409 `ConflictException` exactly when one of
//! the image sets the store still holds in that study cannot take an update right now (not
//! `ACTIVE`, or with a transition the clock still owes it). A refusal is therefore no more
//! lasting than the work that caused it: once the clock has settled that work the same
//! request is accepted, while a retired image set of the study never stands in the way at
//! all, however often the study is updated.
//!
//! This suite is a TABLE whose rows differ along the condition of the study's OTHER image
//! sets going in: no sibling at all, one still held and `ACTIVE`, one retired, two retired
//! and none held, a retired one beside a held one, a sibling with its own update in flight,
//! a sibling with its deletion in flight, and an in-flight sibling beside a retired or an
//! active one. Every row also holds an image set of ANOTHER study with an update in flight,
//! which the walk never reaches, and every row drives real HTTP through the production front
//! door.
//!
//! ## Why the rows that overlap a sibling suite are kept
//!
//! `crate::image_set_capacity` asserts the retired half of the rule beside the per-data-store
//! cap it owns, and `crate::image_set_metadata_updates` the plain held-sibling half beside the
//! flag itself. Both halves are rows HERE too, deliberately: what this suite is about is which
//! condition of a sibling decides the request, which can only be asserted by comparing the
//! conditions against each other in one table — a row that is refused is only meaningful
//! beside the rows that are accepted. No row of either sibling suite is changed or removed.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each row seeds its image sets through the crate's public `state` module with the same
//! `ImageSet::new` the handlers use, and asserts only over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    IMAGE_SET_DELETION_DELAY, IMAGE_SET_NOT_UPDATABLE_MESSAGE, IMAGE_SET_UPDATE_DELAY,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::get_image_set;
use crate::image_set_deletion::delete_image_set;
use crate::image_set_metadata::get_image_set_metadata;
use crate::image_set_metadata_updates::{
    at, dicom_updates, read_document, seed_image_set, seeded_tags, update_metadata, STUDY_UID,
};

/// The Study Instance UID of the image set no row's update is about, so each row can tell
/// the study the walk is scoped to from the rest of the data store.
const OTHER_STUDY_UID: &str = "1.2.840.113619.2.1.9.7";

/// The attribute every row's update writes, and what the image sets it reaches must report
/// afterwards.
const WRITTEN_NAME: &str = "ROE^RICHARD";

/// The condition one other image set of the study is in when the study-wide update arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sibling {
    /// Still held, `ACTIVE`, with nothing in flight: it takes the update.
    Active,
    /// Its deletion has LANDED: a tombstone, which the study is not made of.
    Retired,
    /// Its own metadata update is still in flight, so the store holds an image set of the
    /// study that cannot take another write yet.
    UpdateInFlight,
    /// Its deletion is still in flight: the record is held and locked, not yet a tombstone.
    DeletionInFlight,
}

impl Sibling {
    /// Whether an image set in this condition is one the study-wide update has to wait for.
    fn blocks(self) -> bool {
        matches!(self, Sibling::UpdateInFlight | Sibling::DeletionInFlight)
    }
}

/// One row: the other image sets of the study, each in the condition it is in going in.
struct StudyCase {
    case: &'static str,
    siblings: Vec<Sibling>,
}

/// The rows span: a study of one image set; one, two and three other image sets; each
/// condition an image set of the study can be in; and a blocking sibling beside a retired
/// one and beside an active one, so no row lets one condition stand in for another.
fn study_cases() -> Vec<StudyCase> {
    vec![
        StudyCase {
            case: "no other image set in the study",
            siblings: Vec::new(),
        },
        StudyCase {
            case: "one active sibling",
            siblings: vec![Sibling::Active],
        },
        StudyCase {
            case: "one retired sibling",
            siblings: vec![Sibling::Retired],
        },
        StudyCase {
            case: "two retired siblings and none held",
            siblings: vec![Sibling::Retired, Sibling::Retired],
        },
        StudyCase {
            case: "a retired sibling beside an active one",
            siblings: vec![Sibling::Retired, Sibling::Active],
        },
        StudyCase {
            case: "a sibling with its own update in flight",
            siblings: vec![Sibling::UpdateInFlight],
        },
        StudyCase {
            case: "a sibling with its deletion in flight",
            siblings: vec![Sibling::DeletionInFlight],
        },
        StudyCase {
            case: "a retired sibling beside one with its update in flight",
            siblings: vec![Sibling::Retired, Sibling::UpdateInFlight],
        },
        StudyCase {
            case: "an active sibling beside one with its deletion in flight",
            siblings: vec![Sibling::Active, Sibling::DeletionInFlight],
        },
    ]
}

/// The document every row's update carries.
fn update_document() -> Value {
    json!({
        "SchemaVersion": "1.1",
        "Patient": { "DICOM": { "PatientName": WRITTEN_NAME } },
    })
}

/// Send one study-wide update of `image_set_id` at `latest_version`.
async fn update_study(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    latest_version: &str,
) -> crate::datastore_lifecycle::Answer {
    update_metadata(
        emulator,
        datastore_id,
        image_set_id,
        latest_version,
        dicom_updates(Some(&update_document()), None),
        &[("includeStudyImageSets", true)],
    )
    .await
}

/// How long the clock needs to settle everything a row can have in flight.
fn settle() -> std::time::Duration {
    SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY.max(IMAGE_SET_DELETION_DELAY)
}

/// RULE (above): the study a study-wide update is about is made of the image sets the data
/// store still HOLDS, and only one of those can hold the update up.
#[tokio::test]
async fn a_study_wide_update_is_decided_by_the_image_sets_the_store_still_holds() {
    for (row, case) in study_cases().into_iter().enumerate() {
        let name = case.case;
        // One emulator per row, so no row inherits another's image sets.
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(&format!("study.membership.{row}"), &format!("token-study-{row}"))
            .await;

        let named = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let siblings: Vec<(Sibling, String)> = case
            .siblings
            .iter()
            .map(|condition| {
                (
                    *condition,
                    seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID)),
                )
            })
            .collect();
        // An image set of another study, whose own update is in flight for the whole row:
        // the walk is scoped to ONE study, so a locked outsider decides nothing.
        let outsider = seed_image_set(&emulator, &datastore_id, seeded_tags(OTHER_STUDY_UID));

        // The retirements land first, so a row that holds a tombstone really holds one by
        // the time the study-wide update arrives.
        for (condition, image_set_id) in &siblings {
            if *condition == Sibling::Retired {
                let accepted = delete_image_set(&emulator, &datastore_id, image_set_id).await;
                assert_eq!(
                    accepted.status, 200,
                    "case {name:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}"
                );
            }
        }
        if siblings.iter().any(|(condition, _)| *condition == Sibling::Retired) {
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;
            for (condition, image_set_id) in &siblings {
                if *condition == Sibling::Retired {
                    let read = get_image_set(&emulator, &datastore_id, image_set_id, None).await;
                    assert_eq!(
                        read.string(&["imageSetState"]),
                        "DELETED",
                        "case {name:?}: the row needs a landed retirement going in, got {read:?}"
                    );
                }
            }
        }

        // Then the work that is still in flight when the request arrives.
        for (condition, image_set_id) in &siblings {
            match condition {
                Sibling::UpdateInFlight => {
                    let accepted = update_metadata(
                        &emulator,
                        &datastore_id,
                        image_set_id,
                        "1",
                        dicom_updates(Some(&update_document()), None),
                        &[],
                    )
                    .await;
                    assert_eq!(
                        accepted.status, 200,
                        "case {name:?}: expected HTTP 200 from the sibling's own update, got \
                         {accepted:?}"
                    );
                }
                Sibling::DeletionInFlight => {
                    let accepted = delete_image_set(&emulator, &datastore_id, image_set_id).await;
                    assert_eq!(
                        accepted.status, 200,
                        "case {name:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}"
                    );
                }
                Sibling::Active | Sibling::Retired => {}
            }
        }
        let outsider_update = update_metadata(
            &emulator,
            &datastore_id,
            &outsider,
            "1",
            dicom_updates(Some(&update_document()), None),
            &[],
        )
        .await;
        assert_eq!(
            outsider_update.status, 200,
            "case {name:?}: expected HTTP 200 from the outsider's own update, got \
             {outsider_update:?}"
        );

        let blocked = case.siblings.iter().any(|condition| condition.blocks());
        let answer = update_study(&emulator, &datastore_id, &named, "1").await;
        if blocked {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (409, "ConflictException"),
                "case {name:?}: expected 409 ConflictException while an image set of the study \
                 has work in flight, got {answer:?}"
            );
            assert_eq!(
                answer.string(&["message"]),
                IMAGE_SET_NOT_UPDATABLE_MESSAGE,
                "case {name:?}: expected the not-updatable sentence, got {answer:?}"
            );
            // A refusal writes nothing, in the study or in the image set the request named.
            let read = get_image_set(&emulator, &datastore_id, &named, None).await;
            assert_eq!(
                (read.string(&["versionId"]), read.string(&["imageSetState"])),
                ("1".to_string(), "ACTIVE".to_string()),
                "case {name:?}: expected the refused update to leave version 1 ACTIVE, got \
                 {read:?}"
            );
            for (condition, image_set_id) in &siblings {
                if *condition == Sibling::Active {
                    let read = get_image_set(&emulator, &datastore_id, image_set_id, None).await;
                    assert_eq!(
                        read.string(&["versionId"]),
                        "1",
                        "case {name:?}: expected the refused update to leave the active sibling \
                         at version 1, got {read:?}"
                    );
                }
            }

            // The refusal lasts exactly as long as the work it was about: once the clock has
            // settled that work, the very same request is accepted.
            tokio::time::sleep(settle()).await;
            let retried = update_study(&emulator, &datastore_id, &named, "1").await;
            assert_eq!(
                retried.status, 200,
                "case {name:?}: expected HTTP 200 once the work in flight settled, got \
                 {retried:?}"
            );
            assert_eq!(
                retried.string(&["latestVersionId"]),
                "2",
                "case {name:?}: expected the named image set at version 2, got {retried:?}"
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected HTTP 200 with no held image set of the study in flight, \
             got {answer:?}"
        );
        assert_eq!(
            (
                answer.string(&["imageSetId"]),
                answer.string(&["latestVersionId"]),
            ),
            (named.clone(), "2".to_string()),
            "case {name:?}: expected the named image set at version 2, got {answer:?}"
        );

        // Every image set the store still holds in the study took the update; a tombstone
        // took no part in it.
        for (condition, image_set_id) in &siblings {
            let read = get_image_set(&emulator, &datastore_id, image_set_id, None).await;
            match condition {
                Sibling::Active => assert_eq!(
                    (
                        read.string(&["versionId"]),
                        read.string(&["imageSetState"]),
                        read.string(&["imageSetWorkflowStatus"]),
                    ),
                    (
                        "2".to_string(),
                        "LOCKED".to_string(),
                        "UPDATING_FOR_STUDY_CONSISTENCY".to_string(),
                    ),
                    "case {name:?}: expected the held sibling brought into line, got {read:?}"
                ),
                Sibling::Retired => assert_eq!(
                    (
                        read.string(&["versionId"]),
                        read.string(&["imageSetState"]),
                    ),
                    ("1".to_string(), "DELETED".to_string()),
                    "case {name:?}: expected the retired sibling untouched and still DELETED, \
                     got {read:?}"
                ),
                Sibling::UpdateInFlight | Sibling::DeletionInFlight => unreachable!(
                    "case {name:?}: a row with work in flight is refused, not accepted"
                ),
            }
        }

        tokio::time::sleep(settle()).await;
        for (condition, image_set_id) in &siblings {
            match condition {
                Sibling::Active => {
                    let document =
                        read_document(&emulator, &datastore_id, image_set_id, name).await;
                    assert_eq!(
                        at(&document, &["Patient", "DICOM", "PatientName"]),
                        json!(WRITTEN_NAME),
                        "case {name:?}: expected the held sibling to report {WRITTEN_NAME:?}, \
                         got {document}"
                    );
                }
                Sibling::Retired => {
                    // A tombstone serves no metadata document at all, so what it did not
                    // take is asserted where the answer for it lives.
                    let served =
                        get_image_set_metadata(&emulator, &datastore_id, image_set_id, None).await;
                    assert_eq!(
                        served.status, 404,
                        "case {name:?}: expected a retired sibling to serve no document, got \
                         {served:?}"
                    );
                }
                Sibling::UpdateInFlight | Sibling::DeletionInFlight => {}
            }
        }
        // The outsider reports what its OWN update wrote and nothing of the study's, and it
        // is still at the one version that update minted.
        let outsider_read = get_image_set(&emulator, &datastore_id, &outsider, None).await;
        assert_eq!(
            outsider_read.string(&["versionId"]),
            "2",
            "case {name:?}: expected the image set of another study at the version its own \
             update minted, got {outsider_read:?}"
        );

        // The image set the request named reports the attribute the update wrote, and keeps
        // the ones no update named.
        let named_document = read_document(&emulator, &datastore_id, &named, name).await;
        assert_eq!(
            at(&named_document, &["Patient", "DICOM", "PatientName"]),
            json!(WRITTEN_NAME),
            "case {name:?}: expected the named image set to report {WRITTEN_NAME:?}, got \
             {named_document}"
        );
        assert_eq!(
            at(&named_document, &["Patient", "DICOM", "PatientID"]),
            json!("PID-1"),
            "case {name:?}: expected the named image set to keep the attributes no update \
             named, got {named_document}"
        );

        // And the study takes the NEXT update too: a tombstone in it is not a conflict that
        // outlives the image set it belongs to.
        let again = update_study(&emulator, &datastore_id, &named, "2").await;
        assert_eq!(
            again.status, 200,
            "case {name:?}: expected a second study-wide update to be accepted, got {again:?}"
        );
        assert_eq!(
            again.string(&["latestVersionId"]),
            "3",
            "case {name:?}: expected the named image set at version 3, got {again:?}"
        );
        // Nothing of the second update reached a tombstone either.
        for (condition, image_set_id) in &siblings {
            if *condition == Sibling::Retired {
                let read = get_image_set(&emulator, &datastore_id, image_set_id, None).await;
                assert_eq!(
                    (
                        read.string(&["versionId"]),
                        read.string(&["imageSetState"]),
                    ),
                    ("1".to_string(), "DELETED".to_string()),
                    "case {name:?}: expected the retired sibling still untouched, got {read:?}"
                );
            }
        }
    }
}
