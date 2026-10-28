// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `CopyImageSet` answers when one of the two image sets it
//! names has been RETIRED.
//!
//! RULE: a call that carries an image set's CONTENT — its pixel data and its DICOM
//! attributes — finds a `DELETED` image set absent rather than busy, because the deletion
//! took that content with it: the answer is 404 `ResourceNotFoundException` worded by the
//! IMAGE SET kind, the same answer `GetImageFrame`, `GetImageSetMetadata` and
//! `UpdateImageSetMetadata` give for the same record. A copy reads its source's content and
//! writes it into its destination, so the rule holds for the image set in EITHER role, and
//! it is decided BEFORE the version and state checks: a record that is gone is not reported
//! as a stale view of one that is still there, nor as one that exists but is busy.
//! An image set that is merely not `ACTIVE` — `LOCKED` by work in flight — is still there
//! and still holds its content, so that one stays the modelled 409 `ConflictException`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which role
//! the retired image set is given, how it reached `DELETED` (seeded there, or retired by
//! `DeleteImageSet` and landed by the clock), how much pixel data it held, how many versions
//! it holds, whether the request claims its current latest version or a stale one, and
//! whether the request mints its other image set or names a live one. Every case drives real
//! HTTP through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set and its pixel data enter a data store through a DICOM import, which
//! this emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image sets through the crate's public `state` module with the
//! same `ImageSet` API the handlers use, and then asserts only over the wire.

use std::time::SystemTime;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, ImageSetState, ImageSetWorkflowStatus, IMAGE_SET_DELETION_DELAY,
    IMAGE_SET_NOT_ACTIVE_MESSAGE, IMAGE_SET_NOT_FOUND_MESSAGE,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_frames::{pixel_data, seed_image_set_with_frames, SeedFrame};
use crate::image_set_copy::{copy_image_set, get_image_set, information};
use crate::image_set_deletion::delete_image_set;

/// Which of the two image sets a copy names is the retired one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// `sourceImageSetId` — the image set the copy reads its content from.
    Source,
    /// `copyImageSetInformation.destinationImageSet` — the one it writes that content into.
    Destination,
}

impl Role {
    /// The label an assertion prints for this role.
    fn name(self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Destination => "destination",
        }
    }
}

/// How the image set under test reached the state the copy has to judge.
#[derive(Debug, Clone, Copy)]
enum Reached {
    /// Seeded on that state directly.
    Seeded,
    /// Accepted by `DeleteImageSet` and landed on `DELETED` by the clock.
    ByDelete,
}

/// Which version the request claims as the image set's latest.
#[derive(Debug, Clone, Copy)]
enum Claimed {
    /// The version the image set really is on.
    Current,
    /// One past it — a stale view, which on a LIVE image set is the modelled 409.
    Stale,
}

/// What the copy's OTHER image set is: either a live one the request names, or none at
/// all, which asks the copy to mint its destination.
#[derive(Debug, Clone, Copy)]
enum Other {
    Live,
    Minted,
}

/// One row of the retired-image-set table.
#[derive(Debug)]
struct Case {
    case: &'static str,
    role: Role,
    reached: Reached,
    /// The state every version of the image set under test is left on.
    state: ImageSetState,
    /// How many frames of pixel data it held going in.
    frames: usize,
    /// How many versions it holds.
    versions: u32,
    claimed: Claimed,
    other: Other,
    expected_status: u16,
    expected_code: &'static str,
    /// The service's constant text for this refusal.
    expected_message: &'static str,
}

/// `count` frames whose sizes and transfer syntaxes differ frame by frame, so a row is
/// driven over how much pixel data the retired image set held as well as over its state.
fn frames_of(count: usize) -> Vec<SeedFrame> {
    (0..count)
        .map(|index| SeedFrame {
            transfer_syntax_uid: (index % 2 == 0).then_some("1.2.840.10008.1.2.4.50"),
            pixel_data: pixel_data(index as u8 + 1, 8 * (index + 1)),
        })
        .collect()
}

/// Seed one image set holding `frames` frames of pixel data at `versions` versions, with
/// `state` on every version, and answer its identifier.
///
/// Built on the crate's own `seed_image_set_with_frames` plus `ImageSet::push_version` and
/// `ImageSet::set_state_on_every_version` — the same calls a retirement goes through — so a
/// seeded record is shaped exactly like one the handlers hold. A `DELETED` record also
/// carries the `deletedAt` the clock stamps on it.
fn seed(
    emulator: &Emulator,
    datastore_id: &str,
    state: ImageSetState,
    versions: u32,
    frames: usize,
) -> String {
    assert!(
        versions >= 1,
        "an image set always holds at least one version, asked for {versions}"
    );
    let (image_set_id, _) = seed_image_set_with_frames(
        emulator,
        datastore_id,
        ImageSetState::Active,
        &frames_of(frames),
    );
    let store_handle = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = store_handle.lock();
    let image_set = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store")
        .image_sets
        .get_mut(&image_set_id)
        .expect("the seeded image set");
    for _ in 1..versions {
        image_set.push_version(
            ImageSetState::Active,
            ImageSetWorkflowStatus::Updated,
            false,
            SystemTime::now(),
        );
    }
    image_set.set_state_on_every_version(state);
    if state == ImageSetState::Deleted {
        image_set.deleted_at = Some(SystemTime::now());
        // A deleted image set holds no pixel data: the deletion took it, exactly as the
        // clock-owed retirement does.
        image_set.image_frames.clear();
    }
    image_set_id
}

/// The rows span: the retired image set in each of the two roles; `DELETED` reached by
/// seeding and by `DeleteImageSet` landing on the clock; no pixel data, one frame and
/// several; one, two and three versions; the claimed version current and stale; the other
/// image set named and minted; and `LOCKED` as the contrast, which is still a 409.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a DELETED source holding no pixel data, destination minted",
            role: Role::Source,
            reached: Reached::Seeded,
            state: ImageSetState::Deleted,
            frames: 0,
            versions: 1,
            claimed: Claimed::Current,
            other: Other::Minted,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a DELETED source at three versions, claimed stale, into a live destination",
            role: Role::Source,
            reached: Reached::Seeded,
            state: ImageSetState::Deleted,
            frames: 3,
            versions: 3,
            claimed: Claimed::Stale,
            other: Other::Live,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a source retired by DeleteImageSet, one frame, destination minted",
            role: Role::Source,
            reached: Reached::ByDelete,
            state: ImageSetState::Deleted,
            frames: 1,
            versions: 1,
            claimed: Claimed::Current,
            other: Other::Minted,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a source retired by DeleteImageSet at two versions, claimed stale",
            role: Role::Source,
            reached: Reached::ByDelete,
            state: ImageSetState::Deleted,
            frames: 2,
            versions: 2,
            claimed: Claimed::Stale,
            other: Other::Live,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a LOCKED source is still there and still a conflict",
            role: Role::Source,
            reached: Reached::Seeded,
            state: ImageSetState::Locked,
            frames: 1,
            versions: 1,
            claimed: Claimed::Current,
            other: Other::Minted,
            expected_status: 409,
            expected_code: "ConflictException",
            expected_message: IMAGE_SET_NOT_ACTIVE_MESSAGE,
        },
        Case {
            case: "a DELETED destination holding no pixel data",
            role: Role::Destination,
            reached: Reached::Seeded,
            state: ImageSetState::Deleted,
            frames: 0,
            versions: 1,
            claimed: Claimed::Current,
            other: Other::Live,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a DELETED destination at three versions, claimed stale",
            role: Role::Destination,
            reached: Reached::Seeded,
            state: ImageSetState::Deleted,
            frames: 2,
            versions: 3,
            claimed: Claimed::Stale,
            other: Other::Live,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a destination retired by DeleteImageSet, four frames",
            role: Role::Destination,
            reached: Reached::ByDelete,
            state: ImageSetState::Deleted,
            frames: 4,
            versions: 1,
            claimed: Claimed::Current,
            other: Other::Live,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: IMAGE_SET_NOT_FOUND_MESSAGE,
        },
        Case {
            case: "a LOCKED destination is still there and still a conflict",
            role: Role::Destination,
            reached: Reached::Seeded,
            state: ImageSetState::Locked,
            frames: 0,
            versions: 2,
            claimed: Claimed::Current,
            other: Other::Live,
            expected_status: 409,
            expected_code: "ConflictException",
            expected_message: IMAGE_SET_NOT_ACTIVE_MESSAGE,
        },
    ]
}

/// The version the request claims for the image set under test: the one it is really on,
/// or one past it.
fn claimed_version(case: &Case) -> String {
    match case.claimed {
        Claimed::Current => case.versions.to_string(),
        Claimed::Stale => (case.versions + 1).to_string(),
    }
}

/// RULE: a copy that names a `DELETED` image set as its source or as its destination
/// answers 404 `ResourceNotFoundException` worded by the image-set kind — the content the
/// copy carries went with the deletion, so the image set itself is what is absent — while
/// one that is merely `LOCKED` is still there and stays a 409 `ConflictException`. A live
/// pair in the same data store still copies, which is what shows the refusal comes from the
/// state of the image set and not from the shape of the request.
#[tokio::test]
async fn a_copy_naming_a_deleted_image_set_answers_the_image_set_absent() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.tombstone", "copy-tombstone")
        .await;

    for case in cases() {
        let retired = seed(
            &emulator,
            &datastore_id,
            match case.reached {
                // `DeleteImageSet` is only admitted on a live image set, so a row that
                // retires one through the API seeds it ACTIVE first.
                Reached::ByDelete => ImageSetState::Active,
                Reached::Seeded => case.state,
            },
            case.versions,
            case.frames,
        );
        // The live counterpart: the same content, in the same data store, never retired.
        let live = seed(
            &emulator,
            &datastore_id,
            ImageSetState::Active,
            case.versions,
            case.frames,
        );

        if matches!(case.reached, Reached::ByDelete) {
            let accepted = delete_image_set(&emulator, &datastore_id, &retired).await;
            assert_eq!(
                accepted.status, 200,
                "case {:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}",
                case.case
            );
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;
        }

        let claimed = claimed_version(&case);
        let live_version = case.versions.to_string();
        let (source_id, destination) = match case.role {
            Role::Source => (
                retired.clone(),
                match case.other {
                    Other::Live => Some((live.clone(), live_version.clone())),
                    Other::Minted => None,
                },
            ),
            // The destination is the retired one, so the SOURCE is the live image set and
            // the version the request claims for it is its own current latest.
            Role::Destination => (live.clone(), Some((retired.clone(), claimed.clone()))),
        };
        let source_version = match case.role {
            Role::Source => claimed.clone(),
            Role::Destination => live_version.clone(),
        };

        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(
                &source_version,
                None,
                destination
                    .as_ref()
                    .map(|(id, version)| (id.as_str(), version.as_str())),
            ),
            &[],
        )
        .await;

        assert_eq!(
            refused.status,
            case.expected_status,
            "case {:?} ({} retired): expected HTTP {}, got {refused:?}",
            case.case,
            case.role.name(),
            case.expected_status
        );
        assert_eq!(
            refused.error_code,
            case.expected_code,
            "case {:?} ({} retired): expected {}, got {refused:?}",
            case.case,
            case.role.name(),
            case.expected_code
        );
        assert_eq!(
            refused.string(&["message"]),
            case.expected_message,
            "case {:?} ({} retired): expected {:?}, got {refused:?}",
            case.case,
            case.role.name(),
            case.expected_message
        );

        // A refusal writes nothing: the retired image set keeps its state and its version,
        // and the live counterpart is untouched.
        let read = get_image_set(&emulator, &datastore_id, &retired, None).await;
        assert_eq!(
            read.string(&["imageSetState"]),
            case.state.as_wire(),
            "case {:?}: the refused copy must leave the image set on its own state, got \
             {read:?}",
            case.case
        );
        assert_eq!(
            read.string(&["versionId"]),
            case.versions.to_string(),
            "case {:?}: the refused copy must add no version, got {read:?}",
            case.case
        );
        let live_read = get_image_set(&emulator, &datastore_id, &live, None).await;
        assert_eq!(
            live_read.string(&["imageSetState"]),
            "ACTIVE",
            "case {:?}: the live counterpart must be untouched by the refusal, got \
             {live_read:?}",
            case.case
        );
        assert_eq!(
            live_read.string(&["versionId"]),
            case.versions.to_string(),
            "case {:?}: the live counterpart must gain no version from the refusal, got \
             {live_read:?}",
            case.case
        );

        // The control: with both image sets live, the very same request is admitted — so
        // the refusal above came from the retired record, not from the request.
        let control_source = seed(
            &emulator,
            &datastore_id,
            ImageSetState::Active,
            case.versions,
            case.frames,
        );
        let control_destination = match case.other {
            Other::Minted if case.role == Role::Source => None,
            _ => Some(seed(
                &emulator,
                &datastore_id,
                ImageSetState::Active,
                case.versions,
                case.frames,
            )),
        };
        let admitted = copy_image_set(
            &emulator,
            &datastore_id,
            &control_source,
            information(
                &live_version,
                None,
                control_destination
                    .as_ref()
                    .map(|id| (id.as_str(), live_version.as_str())),
            ),
            &[],
        )
        .await;
        assert_eq!(
            admitted.status, 200,
            "case {:?}: a copy between two live image sets must be admitted, got {admitted:?}",
            case.case
        );
    }
}
