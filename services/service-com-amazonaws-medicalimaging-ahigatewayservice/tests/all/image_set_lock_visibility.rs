// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the state every version of an image set reports while a write
//! holds it, through `ListImageSetVersions` and the `GetImageSet` read of each version.
//!
//! RULE: `imageSetState` is "the image set state" — whether the image set can be read and
//! written — so it is a property of the IMAGE SET and every version of the image set
//! reports the state the image set is in at that instant: a write that locks it locks
//! every version, and the clock that releases the lock releases every version. The
//! workflow status is the other half of the pair and stays per version: it is what last
//! happened to that snapshot, so an earlier version keeps the status it was left on.
//! A reader therefore never reports `ACTIVE` for an image set every writer of it refuses
//! as not `ACTIVE` at the same instant.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! write takes the lock (a copy that lands a new version, a copy that reads the image set,
//! a metadata update that writes attributes, a metadata update that reverts to an earlier
//! version), and how many versions the image set holds going in. Every case drives real
//! HTTP through the production front door (`crate::datastore_lifecycle::Emulator`), so
//! what is asserted is the wire behaviour a `medical-imaging` SDK client sees.
//!
//! The in-flight window is observed by holding the instant the transition falls due
//! (`crate::image_set_deletion::move_due_instant`), not by racing the 400 ms span, and the
//! release is observed by making that instant fall due — nothing sleeps.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set comes into existence through a DICOM import, which this emulator
//! does not create image sets from yet, so each case seeds its image sets through the
//! crate's own `ImageSet::new` / `push_version`, exactly as `crate::image_set_copy` does,
//! and everything it then asserts goes over HTTP.

use std::time::{Duration, SystemTime};

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_NOT_ACTIVE_MESSAGE;

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::{
    copy_image_set, get_image_set, information, seed_image_set, SEEDED_WORKFLOW_STATUS,
};
use crate::image_set_deletion::move_due_instant;
use crate::image_set_metadata_updates::{dicom_updates, update_metadata};
use crate::image_set_version_listing::{list_versions, listed_versions, properties};

/// How far out the due instant is pushed so the image set stays mid-flight for the whole
/// of a case. Long enough that no scheduling hiccup can let the clock land it mid-case.
const HELD: Duration = Duration::from_secs(3600);

/// Which write takes the lock the case observes.
#[derive(Debug, Clone, Copy)]
enum Lock {
    /// `CopyImageSet` landing a new version ON this image set (the destination).
    CopyIntoDestination,
    /// `CopyImageSet` READING this image set (the source's read-only lock).
    CopySource,
    /// `UpdateImageSetMetadata` writing attributes.
    MetadataUpdate,
    /// `UpdateImageSetMetadata` reverting to an earlier version.
    MetadataRevert,
}

impl Lock {
    /// The `ImageSetWorkflowStatus` the LATEST version reports while this write is in
    /// flight (`smithy_query(query="shape",shape="ImageSetWorkflowStatus")`).
    fn in_flight_status(self) -> &'static str {
        match self {
            Lock::CopyIntoDestination => "COPYING",
            Lock::CopySource => "COPYING_WITH_READ_ONLY_ACCESS",
            Lock::MetadataUpdate | Lock::MetadataRevert => "UPDATING",
        }
    }

    /// The status the latest version reports once the clock has released the lock.
    ///
    /// `COPIED` is "the image set copy is complete", and a completed copy is what last
    /// happened to BOTH image sets it names — the destination it wrote and the source it
    /// read — so neither goes back to a status it carried going in.
    fn landed_status(self) -> &'static str {
        match self {
            Lock::CopyIntoDestination | Lock::CopySource => "COPIED",
            Lock::MetadataUpdate | Lock::MetadataRevert => "UPDATED",
        }
    }

    /// Whether this write lands a NEW version on the image set it locks.
    fn lands_a_version(self) -> bool {
        match self {
            Lock::CopyIntoDestination | Lock::MetadataUpdate | Lock::MetadataRevert => true,
            Lock::CopySource => false,
        }
    }
}

/// One row of the table.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// How many versions the image set holds going in.
    versions: u32,
    lock: Lock,
}

/// The rows span: each write that locks an image set; an image set at one, two, three and
/// five versions (so a lock is observed with nothing earlier to reach, with one earlier
/// version and with several); and the two writes that lock WITHOUT landing a version
/// against the three that land one.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a copy into a one-version destination",
            versions: 1,
            lock: Lock::CopyIntoDestination,
        },
        Case {
            case: "a copy into a two-version destination",
            versions: 2,
            lock: Lock::CopyIntoDestination,
        },
        Case {
            case: "a copy into a five-version destination",
            versions: 5,
            lock: Lock::CopyIntoDestination,
        },
        Case {
            case: "a copy reading a one-version source",
            versions: 1,
            lock: Lock::CopySource,
        },
        Case {
            case: "a copy reading a three-version source",
            versions: 3,
            lock: Lock::CopySource,
        },
        Case {
            case: "a metadata update on a one-version image set",
            versions: 1,
            lock: Lock::MetadataUpdate,
        },
        Case {
            case: "a metadata update on a three-version image set",
            versions: 3,
            lock: Lock::MetadataUpdate,
        },
        Case {
            case: "a revert on a two-version image set",
            versions: 2,
            lock: Lock::MetadataRevert,
        },
        Case {
            case: "a revert on a five-version image set",
            versions: 5,
            lock: Lock::MetadataRevert,
        },
    ]
}

/// The listing of every version `image_set_id` holds, oldest first — asserted to have
/// been served, so each caller reads the entries rather than the answer's status.
async fn listing(
    emulator: &Emulator,
    case: &str,
    datastore_id: &str,
    image_set_id: &str,
) -> Answer {
    let answer = list_versions(emulator, datastore_id, image_set_id, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListImageSetVersions, got {answer:?}"
    );
    answer
}

/// Send the write that locks `image_set_id` and answer the identifiers of every image set
/// it put a transition in flight on, the locked one first.
async fn take_the_lock(
    emulator: &Emulator,
    case: &Case,
    datastore_id: &str,
    image_set_id: &str,
) -> Vec<String> {
    let latest = case.versions.to_string();
    match case.lock {
        Lock::CopyIntoDestination => {
            // A separate image set supplies the content, so the locked one is the copy's
            // DESTINATION.
            let source_id = seed_image_set(emulator, datastore_id, 1);
            let answer = copy_image_set(
                emulator,
                datastore_id,
                &source_id,
                information("1", None, Some((image_set_id, &latest))),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from CopyImageSet, got {answer:?}",
                case.case
            );
            vec![image_set_id.to_string(), source_id]
        }
        Lock::CopySource => {
            // A copy naming no destination mints one, so the locked image set is the
            // copy's SOURCE and nothing else of the case's is touched.
            let answer = copy_image_set(
                emulator,
                datastore_id,
                image_set_id,
                information(&latest, None, None),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from CopyImageSet, got {answer:?}",
                case.case
            );
            let minted = answer.string(&["destinationImageSetProperties", "imageSetId"]);
            vec![image_set_id.to_string(), minted]
        }
        Lock::MetadataUpdate => {
            let answer = update_metadata(
                emulator,
                datastore_id,
                image_set_id,
                &latest,
                dicom_updates(
                    Some(&json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
                    })),
                    None,
                ),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from UpdateImageSetMetadata, got {answer:?}",
                case.case
            );
            vec![image_set_id.to_string()]
        }
        Lock::MetadataRevert => {
            let answer = update_metadata(
                emulator,
                datastore_id,
                image_set_id,
                &latest,
                json!({ "revertToVersionId": "1" }),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from UpdateImageSetMetadata, got {answer:?}",
                case.case
            );
            vec![image_set_id.to_string()]
        }
    }
}

/// RULE (above): while a write holds the image set EVERY version reports `LOCKED`, which
/// is the state every writer of the image set refuses it on at that instant, and the
/// clock puts every version back to `ACTIVE`. The workflow status stays per version.
#[tokio::test]
async fn every_version_reports_the_state_the_image_set_is_in_while_a_write_holds_it() {
    for (index, case) in cases().into_iter().enumerate() {
        let name = case.case;
        // One emulator per row, so no row inherits another's resources.
        let emulator = Emulator::new();
        // `clientToken` is `^[A-Za-z0-9._-]+$`
        // (`smithy_query(query="operation",operation="CreateDatastore")`), so the row is
        // named by its index rather than by its prose.
        let datastore_id = emulator
            .active_datastore("lock.visibility.store", &format!("lock-visibility-{index}"))
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, case.versions);

        let before = properties(&listing(&emulator, name, &datastore_id, &image_set_id).await);
        for entry in &before {
            assert_eq!(
                entry["imageSetState"].as_str(),
                Some("ACTIVE"),
                "case {name:?}: expected every version ACTIVE before the write, got {entry:?}"
            );
        }

        let in_flight_on = take_the_lock(&emulator, &case, &datastore_id, &image_set_id).await;
        // Hold every transition the write started, so the window it exists for lasts the
        // whole case instead of being raced.
        for held in &in_flight_on {
            move_due_instant(
                &emulator,
                name,
                &datastore_id,
                held,
                SystemTime::now() + HELD,
            );
        }

        // The versions the image set holds mid-flight: one more than it went in with where
        // the write lands a version.
        let expected_versions = if case.lock.lands_a_version() {
            case.versions + 1
        } else {
            case.versions
        };
        let during_answer = listing(&emulator, name, &datastore_id, &image_set_id).await;
        let during = properties(&during_answer);
        assert_eq!(
            listed_versions(&during_answer),
            (1..=expected_versions)
                .map(|version| version.to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: expected every version oldest first mid-flight, got {during_answer:?}"
        );
        for entry in &during {
            let version = entry["versionId"].as_str().unwrap_or_default();
            assert_eq!(
                entry["imageSetState"].as_str(),
                Some("LOCKED"),
                "case {name:?} version {version}: expected LOCKED while the write holds the \
                 image set, got {entry:?}"
            );
            // The version's own reader agrees with the listing, member for member.
            let read = get_image_set(&emulator, &datastore_id, &image_set_id, Some(version)).await;
            assert_eq!(
                read.status, 200,
                "case {name:?} version {version}: expected HTTP 200 from GetImageSet, got {read:?}"
            );
            assert_eq!(
                read.string(&["imageSetState"]),
                "LOCKED",
                "case {name:?} version {version}: expected GetImageSet to report LOCKED \
                 mid-flight, got {read:?}"
            );
        }
        // The workflow status is the per-version half of the pair: the write records what
        // it is doing on the version it is doing it to, and the first version keeps the
        // status it was left on.
        assert_eq!(
            during
                .last()
                .map(|entry| entry["ImageSetWorkflowStatus"].as_str())
                .unwrap_or_default(),
            Some(case.lock.in_flight_status()),
            "case {name:?}: expected the latest version to report {}, got {during:?}",
            case.lock.in_flight_status()
        );
        assert_eq!(
            during[0]["ImageSetWorkflowStatus"].as_str(),
            Some(if expected_versions == 1 {
                case.lock.in_flight_status()
            } else {
                SEEDED_WORKFLOW_STATUS.as_wire()
            }),
            "case {name:?}: expected version 1 to keep its own workflow status, got {during:?}"
        );

        // The service's own answer at the same instant: a copy of this image set is
        // refused because it is not ACTIVE — which is what every version now reports.
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            information(&expected_versions.to_string(), None, None),
            &[],
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {name:?}: expected HTTP 409 for a copy of a locked image set, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case {name:?}: expected ConflictException, got {refused:?}"
        );
        assert_eq!(
            refused.string(&["message"]),
            IMAGE_SET_NOT_ACTIVE_MESSAGE,
            "case {name:?}: expected the service's constant not-ACTIVE text, got {refused:?}"
        );

        // Make the write fall due: the clock, not a reader, releases the lock.
        for held in &in_flight_on {
            move_due_instant(&emulator, name, &datastore_id, held, SystemTime::now());
        }

        let after_answer = listing(&emulator, name, &datastore_id, &image_set_id).await;
        let after = properties(&after_answer);
        assert_eq!(
            listed_versions(&after_answer),
            listed_versions(&during_answer),
            "case {name:?}: expected the same versions once the write landed, got {after_answer:?}"
        );
        for entry in &after {
            let version = entry["versionId"].as_str().unwrap_or_default();
            assert_eq!(
                entry["imageSetState"].as_str(),
                Some("ACTIVE"),
                "case {name:?} version {version}: expected ACTIVE once the write landed, got \
                 {entry:?}"
            );
            let read = get_image_set(&emulator, &datastore_id, &image_set_id, Some(version)).await;
            assert_eq!(
                read.string(&["imageSetState"]),
                "ACTIVE",
                "case {name:?} version {version}: expected GetImageSet to report ACTIVE once \
                 the write landed, got {read:?}"
            );
        }
        assert_eq!(
            after
                .last()
                .map(|entry| entry["ImageSetWorkflowStatus"].as_str())
                .unwrap_or_default(),
            Some(case.lock.landed_status()),
            "case {name:?}: expected the latest version to report {} once the write landed, got \
             {after:?}",
            case.lock.landed_status()
        );
    }
}
