// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetImageSet` as a REPORTING operation.
//!
//! RULE: `GetImageSet` answers HTTP 200 with the properties PERSISTED for the image set
//! version the caller named through `version` — the latest version when it named none —
//! taking every reported member from state rather than from the request: the version's
//! own `imageSetState`, `imageSetWorkflowStatus`, `updatedAt`, `message` and
//! `Overrides.forced` (present only for a version created with the force flag), and the
//! image SET's own `imageSetArn`, `isPrimary`, `createdAt`, `deletedAt` and
//! `lastAccessedAt`. Because it reports rather than mutates, the call writes nothing of
//! its own: no access is recorded (`lastAccessedAt` is the stored instant, not this
//! call's), no status it finds is moved, and two reads of the same version answer
//! identically. A resource or version the data store does not hold is 404
//! `ResourceNotFoundException`, and an identifier outside its modelled pattern is 400
//! `ValidationException`.
//!
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees. The rows differ along what the
//! rule generalises over: how many versions the image set holds, which version is asked
//! for, which `ImageSetState` / `ImageSetWorkflowStatus` each version carries, whether it
//! was forced, whether it carries a `message`, whether the image set is primary,
//! whether its content has been accessed since it was created, and whether it has been
//! deleted.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set comes into existence through a DICOM import, which this emulator's
//! import job does not do yet, so each case seeds its image set through the crate's
//! public `state` module with `ImageSet::new` / `push_version` — the same constructors
//! the handlers mint image sets with — and then asserts only over the wire.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    IMAGE_SET_COPY_DELAY, IMAGE_SET_DELETION_DELAY,
};

use crate::datastore_lifecycle::{Emulator, ACCOUNT, SETTLE_MULTIPLE};
use crate::image_set_copy::get_image_set;

/// Every value `StorageTier` declares
/// (`smithy_query(query="shape",shape="StorageTier")`): FREQUENT_ACCESS,
/// ARCHIVE_INSTANT_ACCESS. A reported tier must be one of them.
const STORAGE_TIERS: [&str; 2] = ["FREQUENT_ACCESS", "ARCHIVE_INSTANT_ACCESS"];

/// The instant the seeded timestamps are measured from — whole seconds, so each
/// expected timestamp compares exactly against the number on the wire.
const BASE_EPOCH_SECS: u64 = 1_767_225_600;

/// A seeded instant `offset_secs` past [`BASE_EPOCH_SECS`].
fn at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS + offset_secs)
}

/// A well-formed `[0-9a-z]{32}` identifier no call ever mints (`mint_id` is SplitMix64
/// output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// One version to seed, in the order the versions are pushed.
#[derive(Debug, Clone, Copy)]
struct VersionSeed {
    state: ImageSetState,
    workflow: ImageSetWorkflowStatus,
    /// Whether this version was created with the force flag — what
    /// `Overrides.forced` reports.
    forced: bool,
    /// The version's `message`, absent for a version no action failed on.
    message: Option<&'static str>,
    /// Seconds past [`BASE_EPOCH_SECS`] this version was last updated.
    updated_offset_secs: u64,
}

/// One row of the read table: an image set to seed, and what every read of it must say.
struct ReadCase {
    case: &'static str,
    is_primary: bool,
    /// Seconds past [`BASE_EPOCH_SECS`] the image SET was created.
    created_offset_secs: u64,
    /// Seconds past [`BASE_EPOCH_SECS`] the image set was deleted, for a tombstone.
    deleted_offset_secs: Option<u64>,
    /// Seconds past [`BASE_EPOCH_SECS`] the content this image set holds was last
    /// accessed — the creation instant for one nothing has accessed since.
    last_accessed_offset_secs: u64,
    /// The versions, oldest first; version identifiers are 1..=len.
    versions: Vec<VersionSeed>,
}

/// Seed one image set, shaped exactly like one a handler mints, and answer its id.
fn seed(emulator: &Emulator, datastore_id: &str, case: &ReadCase) -> String {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let (first, later) = case
        .versions
        .split_first()
        .expect("an image set always holds at least one version");
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        first.state,
        first.workflow,
        case.is_primary,
        first.forced,
        at(first.updated_offset_secs),
    );
    image_set.latest_version.message = first.message.map(str::to_string);
    for seed in later {
        image_set.push_version(
            seed.state,
            seed.workflow,
            seed.forced,
            at(seed.updated_offset_secs),
        );
        image_set.latest_version.message = seed.message.map(str::to_string);
    }
    image_set.created_at = at(case.created_offset_secs);
    image_set.deleted_at = case.deleted_offset_secs.map(at);
    image_set.last_accessed_at = at(case.last_accessed_offset_secs);
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    image_set_id
}

/// The rows span: one, two and four versions; each version's own state, workflow status,
/// force flag and message; a primary and a secondary image set; a locked image set; a
/// deleted tombstone; and an image set whose content was last accessed when it was
/// created against ones accessed later than that.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "one imported version, secondary, nothing forced",
            is_primary: false,
            created_offset_secs: 0,
            deleted_offset_secs: None,
            // Nothing has accessed its content, so the last access is its creation.
            last_accessed_offset_secs: 0,
            versions: vec![VersionSeed {
                state: ImageSetState::Active,
                workflow: ImageSetWorkflowStatus::Imported,
                forced: false,
                message: None,
                updated_offset_secs: 0,
            }],
        },
        ReadCase {
            case: "two versions, only the later one forced, primary",
            is_primary: true,
            created_offset_secs: 10,
            deleted_offset_secs: None,
            // Accessed after its latest version was written.
            last_accessed_offset_secs: 30,
            versions: vec![
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Created,
                    forced: false,
                    message: None,
                    updated_offset_secs: 10,
                },
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Copied,
                    forced: true,
                    message: None,
                    updated_offset_secs: 25,
                },
            ],
        },
        ReadCase {
            case: "four versions, each with its own status, the first forced",
            is_primary: false,
            created_offset_secs: 100,
            deleted_offset_secs: None,
            last_accessed_offset_secs: 100,
            versions: vec![
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Imported,
                    forced: true,
                    message: None,
                    updated_offset_secs: 100,
                },
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Updated,
                    forced: false,
                    message: None,
                    updated_offset_secs: 140,
                },
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::UpdatingForStudyConsistency,
                    forced: false,
                    message: None,
                    updated_offset_secs: 180,
                },
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Copied,
                    forced: true,
                    message: None,
                    updated_offset_secs: 260,
                },
            ],
        },
        ReadCase {
            case: "a version an update failed on, carrying its message",
            is_primary: true,
            created_offset_secs: 7,
            deleted_offset_secs: None,
            // Accessed between the two versions, which no version reports: the last
            // access belongs to the image set, not to a snapshot of it.
            last_accessed_offset_secs: 12,
            versions: vec![
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::Imported,
                    forced: false,
                    message: None,
                    updated_offset_secs: 7,
                },
                VersionSeed {
                    state: ImageSetState::Active,
                    workflow: ImageSetWorkflowStatus::UpdateFailed,
                    forced: false,
                    // `Message` is `^[\w -:`]+$`, len 1..2048.
                    message: Some("Update failed - inconsistent metadata at Series level"),
                    updated_offset_secs: 19,
                },
            ],
        },
        ReadCase {
            case: "a locked image set with a copy recorded against it",
            is_primary: false,
            created_offset_secs: 3,
            deleted_offset_secs: None,
            last_accessed_offset_secs: 3,
            versions: vec![
                VersionSeed {
                    state: ImageSetState::Locked,
                    workflow: ImageSetWorkflowStatus::CopyingWithReadOnlyAccess,
                    forced: false,
                    message: None,
                    updated_offset_secs: 3,
                },
                VersionSeed {
                    state: ImageSetState::Locked,
                    workflow: ImageSetWorkflowStatus::Copying,
                    forced: false,
                    message: None,
                    updated_offset_secs: 5,
                },
            ],
        },
        ReadCase {
            case: "a deleted tombstone, which still reports its properties",
            is_primary: true,
            created_offset_secs: 40,
            deleted_offset_secs: Some(90),
            // Accessed while it was still readable; the tombstone keeps reporting it.
            last_accessed_offset_secs: 45,
            versions: vec![
                VersionSeed {
                    state: ImageSetState::Deleted,
                    workflow: ImageSetWorkflowStatus::Deleted,
                    forced: false,
                    message: None,
                    updated_offset_secs: 40,
                },
                VersionSeed {
                    state: ImageSetState::Deleted,
                    workflow: ImageSetWorkflowStatus::Deleted,
                    forced: false,
                    message: None,
                    updated_offset_secs: 90,
                },
            ],
        },
    ]
}

/// The expected number on the wire for a seeded offset.
fn expected_instant(offset_secs: u64) -> f64 {
    (BASE_EPOCH_SECS + offset_secs) as f64
}

#[tokio::test]
async fn get_image_set_reports_the_persisted_properties_of_the_version_asked_for() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("properties.store", "properties")
        .await;

    for case in read_cases() {
        let image_set_id = seed(&emulator, &datastore_id, &case);
        let expected_arn = format!(
            "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}/imageset/{image_set_id}"
        );

        for (index, version) in case.versions.iter().enumerate() {
            let version_id = (index + 1).to_string();
            let read = get_image_set(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version_id),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {:?} version={version_id}: expected HTTP 200, got {read:?}",
                case.case
            );
            assert_eq!(
                read.string(&["datastoreId"]),
                datastore_id,
                "case {:?} version={version_id}: wrong datastoreId, got {read:?}",
                case.case
            );
            assert_eq!(
                read.string(&["imageSetId"]),
                image_set_id,
                "case {:?} version={version_id}: wrong imageSetId, got {read:?}",
                case.case
            );
            assert_eq!(
                read.string(&["versionId"]),
                version_id,
                "case {:?} version={version_id}: wrong versionId, got {read:?}",
                case.case
            );
            assert_eq!(
                read.string(&["imageSetState"]),
                version.state.as_wire(),
                "case {:?} version={version_id}: expected the version's own state {:?}, got {read:?}",
                case.case,
                version.state.as_wire()
            );
            assert_eq!(
                read.string(&["imageSetWorkflowStatus"]),
                version.workflow.as_wire(),
                "case {:?} version={version_id}: expected the version's own workflow status {:?}, got {read:?}",
                case.case,
                version.workflow.as_wire()
            );
            assert_eq!(
                read.string(&["imageSetArn"]),
                expected_arn,
                "case {:?} version={version_id}: expected imageSetArn {expected_arn:?}, got {read:?}",
                case.case
            );
            assert_eq!(
                read.field(&["isPrimary"]),
                &Value::Bool(case.is_primary),
                "case {:?} version={version_id}: expected isPrimary {}, got {read:?}",
                case.case,
                case.is_primary
            );
            // `createdAt` is the image SET's creation, so every version reports it.
            assert_eq!(
                read.field(&["createdAt"]).as_f64(),
                Some(expected_instant(case.created_offset_secs)),
                "case {:?} version={version_id}: expected the image set's createdAt, got {read:?}",
                case.case
            );
            // `updatedAt` is the reported VERSION's own.
            assert_eq!(
                read.field(&["updatedAt"]).as_f64(),
                Some(expected_instant(version.updated_offset_secs)),
                "case {:?} version={version_id}: expected the version's own updatedAt, got {read:?}",
                case.case
            );
            assert_eq!(
                read.field(&["deletedAt"]).as_f64(),
                case.deleted_offset_secs.map(expected_instant),
                "case {:?} version={version_id}: wrong deletedAt, got {read:?}",
                case.case
            );
            match version.message {
                Some(message) => assert_eq!(
                    read.string(&["message"]),
                    message,
                    "case {:?} version={version_id}: expected the version's message, got {read:?}",
                    case.case
                ),
                None => assert!(
                    read.field(&["message"]).is_null(),
                    "case {:?} version={version_id}: a version no action failed on carries no message, got {read:?}",
                    case.case
                ),
            }
            if version.forced {
                assert_eq!(
                    read.field(&["overrides", "forced"]),
                    &Value::Bool(true),
                    "case {:?} version={version_id}: a forced version reports overrides.forced, got {read:?}",
                    case.case
                );
            } else {
                assert!(
                    read.field(&["overrides"]).is_null(),
                    "case {:?} version={version_id}: overrides is present only for a forced version, got {read:?}",
                    case.case
                );
            }
            assert!(
                STORAGE_TIERS.contains(&read.string(&["storageTier"]).as_str()),
                "case {:?} version={version_id}: expected a modelled storageTier, got {read:?}",
                case.case
            );
            assert_eq!(
                read.field(&["lastAccessedAt"]).as_f64(),
                Some(expected_instant(case.last_accessed_offset_secs)),
                "case {:?} version={version_id}: expected the image set's stored last-access \
                 instant, got {read:?}",
                case.case
            );
        }

        // No `version` reports the LATEST version, member for member.
        let latest = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        let named_latest = get_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&case.versions.len().to_string()),
        )
        .await;
        assert_eq!(
            latest.body, named_latest.body,
            "case {:?}: omitting `version` must report the latest version, got {latest:?} against {named_latest:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn reading_an_image_set_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("reads.store", "reads").await;

    // The rows differ along the state a read could be tempted to move on: an active
    // image set, one held locked with nothing owed on it, and a deleted tombstone.
    for case in read_cases() {
        let image_set_id = seed(&emulator, &datastore_id, &case);
        let first = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected HTTP 200, got {first:?}",
            case.case
        );

        // Read it again, and again past the longest span any image-set transition is
        // configured with: a reader materialises only what the CLOCK already owed, and
        // nothing is owed on a seeded image set, so every answer is the same one.
        let again = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            again.body, first.body,
            "case {:?}: a second read must answer identically, got {again:?} after {first:?}",
            case.case
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY.max(IMAGE_SET_DELETION_DELAY))
            .await;
        let settled = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            settled.body, first.body,
            "case {:?}: no span is owed on a read-only image set, got {settled:?} after {first:?}",
            case.case
        );

        // And the versions it held are still exactly the versions it holds: the read
        // minted none.
        assert_eq!(
            settled.string(&["versionId"]),
            case.versions.len().to_string(),
            "case {:?}: a read must not add a version, got {settled:?}",
            case.case
        );
        let past_latest = get_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&(case.versions.len() + 1).to_string()),
        )
        .await;
        assert_eq!(
            past_latest.status, 404,
            "case {:?}: one past the latest version is still absent, got {past_latest:?}",
            case.case
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The data store identifier the request carries, or `None` for the held one.
    datastore_id: Option<String>,
    /// The image set identifier the request carries, or `None` for the held one.
    image_set_id: Option<String>,
    version_id: Option<&'static str>,
    expected_status: u16,
    expected_code: &'static str,
}

/// The rows span: each identifier naming nothing held; a version below, at and far past
/// the versions the image set holds; and each identifier outside its modelled pattern
/// (wrong length, wrong character class) including the `version` query parameter.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a data store nothing holds",
            datastore_id: Some(unheld_id()),
            image_set_id: None,
            version_id: None,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "an image set nothing holds",
            datastore_id: None,
            image_set_id: Some(unheld_id()),
            version_id: None,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "version 0, below the first version identifier",
            datastore_id: None,
            image_set_id: None,
            version_id: Some("0"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a version one past the latest",
            datastore_id: None,
            image_set_id: None,
            version_id: Some("3"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a version far past the latest",
            datastore_id: None,
            image_set_id: None,
            version_id: Some("4096"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a datastoreId one character short of the pattern",
            datastore_id: Some("a".repeat(31)),
            image_set_id: None,
            version_id: None,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a datastoreId carrying an upper-case character",
            datastore_id: Some(format!("A{}", "0".repeat(31))),
            image_set_id: None,
            version_id: None,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "an imageSetId one character past the pattern",
            datastore_id: None,
            image_set_id: Some("b".repeat(33)),
            version_id: None,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a version that is not a decimal number",
            datastore_id: None,
            image_set_id: None,
            version_id: Some("latest"),
            expected_status: 400,
            expected_code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn get_image_set_refuses_what_it_cannot_report() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("refusal.store", "refusal").await;
    // Two versions, so "3" is one past the latest and "2" is the latest.
    let held_case = ReadCase {
        case: "held",
        is_primary: false,
        created_offset_secs: 0,
        deleted_offset_secs: None,
        last_accessed_offset_secs: 0,
        versions: vec![
            VersionSeed {
                state: ImageSetState::Active,
                workflow: ImageSetWorkflowStatus::Imported,
                forced: false,
                message: None,
                updated_offset_secs: 0,
            },
            VersionSeed {
                state: ImageSetState::Active,
                workflow: ImageSetWorkflowStatus::Updated,
                forced: false,
                message: None,
                updated_offset_secs: 1,
            },
        ],
    };
    let image_set_id = seed(&emulator, &datastore_id, &held_case);

    for row in refusal_cases() {
        let requested_datastore = row.datastore_id.clone().unwrap_or_else(|| datastore_id.clone());
        let requested_image_set = row.image_set_id.clone().unwrap_or_else(|| image_set_id.clone());
        let answer = get_image_set(
            &emulator,
            &requested_datastore,
            &requested_image_set,
            row.version_id,
        )
        .await;
        assert_eq!(
            answer.status, row.expected_status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            row.case, row.expected_status
        );
        assert_eq!(
            answer.error_code, row.expected_code,
            "case {:?}: expected {}, got {answer:?}",
            row.case, row.expected_code
        );
    }

    // Every refusal above left the image set exactly as it was.
    let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
    assert_eq!(
        read.string(&["versionId"]),
        "2",
        "case \"after the refusals\": the image set must be untouched, got {read:?}"
    );
    assert_eq!(
        read.string(&["imageSetState"]),
        ImageSetState::Active.as_wire(),
        "case \"after the refusals\": the image set must be untouched, got {read:?}"
    );
}
