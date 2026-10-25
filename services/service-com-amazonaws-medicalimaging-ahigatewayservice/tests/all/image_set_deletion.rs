// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteImageSet`, proved through the `GetImageSet` read and the
//! `GetImageFrame` read of the image set it retires.
//!
//! RULE: a delete of an image set the data store holds is ACCEPTED and starts an
//! asynchronous retirement — the call answers `LOCKED` / `DELETING` and the CLOCK, not a
//! later reader, lands the image set on `DELETED` / `DELETED`, which is when it gets its
//! `deletedAt` and loses the pixel data it held. The operation is `@idempotent`: a repeat
//! call reports the pair the image set is on without moving the instant the retirement
//! falls due, so no repeat can resurrect an image set or postpone its deletion. What the
//! rule does not admit is the modelled failure: a data store or image set the service
//! does not hold is 404 `ResourceNotFoundException` worded by the resource KIND, an image
//! set `LOCKED` by other work already in flight is 409 `ConflictException`, and an
//! identifier that fails its member's own constraint is 400 `ValidationException`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how many
//! versions the image set holds, how much pixel data it holds, how many times the delete
//! is repeated and at which phase of the retirement, and which resource or identifier a
//! refusal row gets wrong. Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle::Emulator`), so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! The rows about a data store that has not settled into `ACTIVE` live with the rest of
//! that rule, in `crate::datastore_status_guard`.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set and its pixel data enter a data store through a DICOM import, and
//! this emulator's import job does not create image sets yet — that is
//! `issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`.
//! So the image set a case deletes is seeded through the crate's public `state` module,
//! with the same `ImageSet` / `ImageFrame` the handlers hold, and everything each case
//! then asserts goes over HTTP.

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, ImageFrame, IMAGE_SET_COPY_DELAY, IMAGE_SET_DELETION_DELAY,
    IMAGE_SET_NOT_ACTIVE_MESSAGE, IMAGE_SET_NOT_DELETABLE_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_frames::get_image_frame;
use crate::image_set_copy::{copy_image_set, get_image_set, information, seed_image_set};

/// The service's constant text for a data store it does not hold.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// The service's constant text for an image set a data store does not hold.
const IMAGE_SET_NOT_FOUND: &str = "ImageSet resource is not found.";

/// How far out a retirement's due instant is pushed so an image set stays in the phase it
/// has already reached for the whole of a case. Long enough that no scheduling hiccup can
/// let the clock land it mid-case.
const HELD: Duration = Duration::from_secs(3600);

/// A well-formed `[0-9a-z]{32}` resource id that no call ever mints (the ids `mint_id`
/// produces are SplitMix64 output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/deleteImageSet` —
/// `DeleteImageSet`. Both members are `@httpLabel`, so the request carries no body.
///
/// `pub(crate)` because `crate::datastore_status_guard` sends this operation at a data
/// store that has not settled, which is that suite's rule rather than this one's.
pub(crate) async fn delete_image_set(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/deleteImageSet"),
            None,
        )
        .await
}

/// Add `count` image frames to an image set the data store already holds, and answer
/// their minted identifiers.
///
/// Each frame's bytes differ from every other's, so a frame that is served can be told
/// from one that is not. Built with the crate's own `ImageFrame` and `mint_id`, so a
/// seeded frame is shaped exactly like one a copy delivers.
fn seed_frames(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    count: usize,
) -> Vec<String> {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let frame_ids: Vec<String> = (0..count).map(|_| store.mint_id()).collect();
    let image_set = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store")
        .image_sets
        .get_mut(image_set_id)
        .expect("the seeded image set");
    for (index, frame_id) in frame_ids.iter().enumerate() {
        image_set.image_frames.insert(
            frame_id.clone(),
            ImageFrame {
                image_frame_id: frame_id.clone(),
                transfer_syntax_uid: None,
                // This suite is about the pixel data going with the deletion, which is
                // reached by its image frame id alone, so no case records the instance
                // it came from.
                sop_instance_uid: None,
                pixel_data: vec![(index as u8).wrapping_add(1); 8 * (index + 1)],
            },
        );
    }
    frame_ids
}

/// Move the instant the transition this image set has in flight falls due.
///
/// The only write a case makes into the state struct, and it changes no state or status:
/// it moves the DUE instant the handler itself recorded, so a case can observe the
/// retirement mid-flight ([`HELD`]) or make it fall due without sleeping, instead of
/// racing a 500 ms span. The suite below this one drives the same rule through the REAL
/// clock with no write at all.
fn move_due_instant(
    emulator: &Emulator,
    case: &str,
    datastore_id: &str,
    image_set_id: &str,
    due_at: SystemTime,
) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store")
        .image_sets
        .get_mut(image_set_id)
        .expect("the seeded image set");
    let pending = image_set
        .pending
        .as_mut()
        .unwrap_or_else(|| panic!("case {case:?}: expected a transition in flight to move"));
    pending.due_at = due_at;
}

// ---------------------------------------------------------------------------
// The retirement: LOCKED/DELETING now, DELETED/DELETED when the clock says so
// ---------------------------------------------------------------------------

/// One row of the deletion table.
#[derive(Debug)]
struct DeleteCase {
    case: &'static str,
    /// How many versions the image set holds going in.
    versions: u32,
    /// How many image frames it holds going in.
    frames: usize,
    /// How many times the delete is sent while the retirement is still in flight — one
    /// for the single call, more for the idempotent repeat.
    calls_in_flight: usize,
    /// How many times it is sent again once the retirement has landed.
    calls_after: usize,
}

/// The rows span: one, two and three versions; no pixel data, one frame and three
/// frames; a single delete and a repeated one; and a repeat after the retirement has
/// landed as well as during it.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "one version, no pixel data, one call",
            versions: 1,
            frames: 0,
            calls_in_flight: 1,
            calls_after: 0,
        },
        DeleteCase {
            case: "one version holding two frames, repeated twice in flight",
            versions: 1,
            frames: 2,
            calls_in_flight: 2,
            calls_after: 1,
        },
        DeleteCase {
            case: "two versions holding one frame",
            versions: 2,
            frames: 1,
            calls_in_flight: 1,
            calls_after: 2,
        },
        DeleteCase {
            case: "three versions, no pixel data, repeated three times in flight",
            versions: 3,
            frames: 0,
            calls_in_flight: 3,
            calls_after: 1,
        },
        DeleteCase {
            case: "one version holding three frames, repeated after it landed",
            versions: 1,
            frames: 3,
            calls_in_flight: 1,
            calls_after: 3,
        },
    ]
}

/// RULE (above): the delete answers `LOCKED` / `DELETING`, the clock lands
/// `DELETED` / `DELETED` with `deletedAt`, the pixel data goes with the landing, and
/// every call is idempotent.
#[tokio::test]
async fn a_delete_locks_the_image_set_and_the_clock_retires_it() {
    for case in delete_cases() {
        let name = case.case;
        // One emulator per row, so no row inherits another's resources.
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("delete.store", "token-delete-store")
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, case.versions);
        let frame_ids = seed_frames(&emulator, &datastore_id, &image_set_id, case.frames);

        // The call itself: accepted, and reporting the pair a retirement in flight is on.
        for call in 0..case.calls_in_flight {
            let deleted = delete_image_set(&emulator, &datastore_id, &image_set_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {name:?} call {call}: expected HTTP 200 from DeleteImageSet, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["datastoreId"]),
                datastore_id,
                "case {name:?} call {call}: expected the data store identifier back, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["imageSetId"]),
                image_set_id,
                "case {name:?} call {call}: expected the image set identifier back, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["imageSetState"]),
                "LOCKED",
                "case {name:?} call {call}: expected imageSetState LOCKED, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["imageSetWorkflowStatus"]),
                "DELETING",
                "case {name:?} call {call}: expected imageSetWorkflowStatus DELETING, got {deleted:?}"
            );
            // Held after the first call so the repeats cannot race the retirement; the
            // hold moves only the due instant the handler recorded.
            move_due_instant(
                &emulator,
                name,
                &datastore_id,
                &image_set_id,
                SystemTime::now() + HELD,
            );
        }

        // Mid-flight the image set is still there, on the pair the delete put it on, and
        // it carries no `deletedAt` yet: nothing has been deleted.
        let in_flight = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            in_flight.status, 200,
            "case {name:?}: a retirement in flight is still readable, got {in_flight:?}"
        );
        assert_eq!(
            in_flight.string(&["imageSetState"]),
            "LOCKED",
            "case {name:?}: expected GetImageSet to report LOCKED in flight, got {in_flight:?}"
        );
        assert_eq!(
            in_flight.string(&["imageSetWorkflowStatus"]),
            "DELETING",
            "case {name:?}: expected GetImageSet to report DELETING in flight, got {in_flight:?}"
        );
        assert!(
            in_flight.field(&["deletedAt"]).is_null(),
            "case {name:?}: deletedAt must be absent before the retirement lands, got {in_flight:?}"
        );
        // Every version reports the state of the image SET, which the delete locked.
        for version in 1..=case.versions {
            let read = get_image_set(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version.to_string()),
            )
            .await;
            assert_eq!(
                read.string(&["imageSetState"]),
                "LOCKED",
                "case {name:?} version {version}: expected LOCKED in flight, got {read:?}"
            );
        }
        // The pixel data goes when the retirement LANDS, not when the call is made.
        for (index, frame_id) in frame_ids.iter().enumerate() {
            let served = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
            assert_eq!(
                served.status, 200,
                "case {name:?} frame {index}: pixel data is still served while the retirement \
                 is in flight, got {served:?}"
            );
        }

        // Make the retirement fall due, without sleeping out the span.
        let due_at = SystemTime::now();
        move_due_instant(&emulator, name, &datastore_id, &image_set_id, due_at);

        let landed = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            landed.status, 200,
            "case {name:?}: a retired image set is still readable as a tombstone, got {landed:?}"
        );
        assert_eq!(
            landed.string(&["imageSetState"]),
            "DELETED",
            "case {name:?}: expected imageSetState DELETED once the retirement landed, got {landed:?}"
        );
        assert_eq!(
            landed.string(&["imageSetWorkflowStatus"]),
            "DELETED",
            "case {name:?}: expected imageSetWorkflowStatus DELETED once it landed, got {landed:?}"
        );
        let deleted_at = landed.field(&["deletedAt"]).as_f64();
        assert!(
            deleted_at.is_some_and(|instant| instant > 0.0),
            "case {name:?}: expected a deletedAt timestamp once the retirement landed, got {landed:?}"
        );
        let created_at = landed.field(&["createdAt"]).as_f64();
        assert!(
            created_at.is_some_and(|created| deleted_at.is_some_and(|gone| gone >= created)),
            "case {name:?}: deletedAt must not precede createdAt, got {landed:?}"
        );
        // Stable across reads: a reporting operation does not re-date the deletion.
        let reread = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            reread.field(&["deletedAt"]),
            landed.field(&["deletedAt"]),
            "case {name:?}: deletedAt must not move between reads, got {reread:?} after {landed:?}"
        );
        for version in 1..=case.versions {
            let read = get_image_set(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version.to_string()),
            )
            .await;
            assert_eq!(
                read.string(&["imageSetState"]),
                "DELETED",
                "case {name:?} version {version}: no version of a retired image set reports \
                 another state, got {read:?}"
            );
        }
        // The pixel data went with the image set.
        for (index, frame_id) in frame_ids.iter().enumerate() {
            let gone = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
            assert_eq!(
                gone.status, 404,
                "case {name:?} frame {index}: a retired image set serves no pixel data, got {gone:?}"
            );
            assert_eq!(
                gone.string(&["message"]),
                IMAGE_SET_NOT_FOUND,
                "case {name:?} frame {index}: expected the image-set-not-found text, got {gone:?}"
            );
        }

        // `@idempotent` past the landing too: the terminal pair is reported, and the
        // instant the image set was deleted at does not move.
        for call in 0..case.calls_after {
            let again = delete_image_set(&emulator, &datastore_id, &image_set_id).await;
            assert_eq!(
                again.status, 200,
                "case {name:?} repeat {call}: expected HTTP 200 deleting a retired image set, got {again:?}"
            );
            assert_eq!(
                again.string(&["imageSetState"]),
                "DELETED",
                "case {name:?} repeat {call}: expected DELETED, got {again:?}"
            );
            assert_eq!(
                again.string(&["imageSetWorkflowStatus"]),
                "DELETED",
                "case {name:?} repeat {call}: expected DELETED, got {again:?}"
            );
            let after = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            assert_eq!(
                after.field(&["deletedAt"]),
                landed.field(&["deletedAt"]),
                "case {name:?} repeat {call}: a repeat delete must not re-date the deletion, \
                 got {after:?} after {landed:?}"
            );
        }
    }
}

/// RULE, through the REAL clock and nothing else: the retirement the delete promised
/// lands on its own, and nothing but the clock lands it.
///
/// Rows span how many versions the image set holds, which the retirement reaches all of.
#[tokio::test]
async fn the_clock_alone_lands_the_retirement_the_delete_promised() {
    for versions in [1_u32, 2, 3] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("clock.store", "token-clock-store")
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);

        let started = Instant::now();
        let deleted = delete_image_set(&emulator, &datastore_id, &image_set_id).await;
        assert_eq!(
            deleted.status, 200,
            "case versions={versions}: expected HTTP 200 from DeleteImageSet, got {deleted:?}"
        );

        // Asserted as a disjunction so no row is silently skipped: inside the retirement
        // span the image set MUST still report the pair the delete put it on; past it the
        // clock has legitimately landed the retirement.
        let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        let elapsed = started.elapsed();
        assert!(
            (read.string(&["imageSetState"]) == "LOCKED"
                && read.string(&["imageSetWorkflowStatus"]) == "DELETING")
                || elapsed >= IMAGE_SET_DELETION_DELAY,
            "case versions={versions}: expected LOCKED/DELETING {elapsed:?} into the retirement \
             span (the span is {IMAGE_SET_DELETION_DELAY:?}), got {read:?}"
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;

        let landed = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            landed.string(&["imageSetState"]),
            "DELETED",
            "case versions={versions}: expected DELETED after the retirement span, got {landed:?}"
        );
        assert_eq!(
            landed.string(&["imageSetWorkflowStatus"]),
            "DELETED",
            "case versions={versions}: expected DELETED after the retirement span, got {landed:?}"
        );
        assert!(
            landed.field(&["deletedAt"]).as_f64().is_some_and(|instant| instant > 0.0),
            "case versions={versions}: expected a deletedAt timestamp, got {landed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the rule does not admit
// ---------------------------------------------------------------------------

/// Which resource or identifier one refusal row gets wrong.
#[derive(Debug)]
enum Wrong {
    /// A well-formed `DatastoreId` the emulator holds no data store under.
    Datastore,
    /// A well-formed `ImageSetId` the data store holds no image set under.
    ImageSet,
    /// The SOURCE of a copy in flight, which that copy holds read-only.
    CopySource,
    /// The DESTINATION of a copy in flight, which that copy holds locked.
    CopyDestination,
    /// A `datastoreId` that fails `DatastoreId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedDatastoreId(&'static str),
    /// An `imageSetId` that fails `ImageSetId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedImageSetId(&'static str),
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    wrong: Wrong,
    expected_status: u16,
    expected_error_code: &'static str,
    /// The sentence the refusal must state, or `None` where the row asserts only the
    /// status and the code (a constraint envelope the framework words).
    expected_message: Option<&'static str>,
}

/// RULE: a resource the service does not hold is 404 `ResourceNotFoundException` worded
/// by the resource KIND, an image set locked by other work in flight is 409
/// `ConflictException`, and an identifier that fails its member's constraint is 400
/// `ValidationException` — never a 200 that deletes nothing.
#[tokio::test]
async fn a_delete_the_rule_does_not_admit_is_refused() {
    // `DatastoreId` and `ImageSetId` are both `^[0-9a-z]{32}$`
    // (`smithy_query(query="operation",operation="DeleteImageSet")`), so a row past each
    // end of that constraint, and one outside its alphabet, is a validation failure.
    let cases = [
        RefusalCase {
            case: "unheld data store",
            wrong: Wrong::Datastore,
            expected_status: 404,
            expected_error_code: "ResourceNotFoundException",
            expected_message: Some(DATASTORE_NOT_FOUND),
        },
        RefusalCase {
            case: "unheld image set",
            wrong: Wrong::ImageSet,
            expected_status: 404,
            expected_error_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_NOT_FOUND),
        },
        RefusalCase {
            case: "image set held read-only as a copy's source",
            wrong: Wrong::CopySource,
            expected_status: 409,
            expected_error_code: "ConflictException",
            expected_message: Some(IMAGE_SET_NOT_DELETABLE_MESSAGE),
        },
        RefusalCase {
            case: "image set locked as a copy's destination",
            wrong: Wrong::CopyDestination,
            expected_status: 409,
            expected_error_code: "ConflictException",
            expected_message: Some(IMAGE_SET_NOT_DELETABLE_MESSAGE),
        },
        RefusalCase {
            case: "data store id one character short",
            wrong: Wrong::MalformedDatastoreId("0123456789abcdef0123456789abcde"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "image set id one character long",
            wrong: Wrong::MalformedImageSetId("0123456789abcdef0123456789abcdef0"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "image set id with an uppercase character",
            wrong: Wrong::MalformedImageSetId("0123456789abcdef0123456789abcdeF"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "image set id with a character outside the alphabet",
            wrong: Wrong::MalformedImageSetId("0123456789abcdef0123456789abcde-"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
    ];

    for case in &cases {
        let name = case.case;
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("refuse.store", "token-refuse-store")
            .await;
        let held_image_set = seed_image_set(&emulator, &datastore_id, 1);

        // A copy in flight is what locks an image set other than by a delete: it holds
        // its source read-only and its destination locked until the clock completes it.
        let locked = matches!(case.wrong, Wrong::CopySource | Wrong::CopyDestination);
        let copy_destination = if locked {
            let destination = seed_image_set(&emulator, &datastore_id, 1);
            let copied = copy_image_set(
                &emulator,
                &datastore_id,
                &held_image_set,
                information("1", None, Some((destination.as_str(), "1"))),
                &[],
            )
            .await;
            assert_eq!(
                copied.status, 200,
                "case {name:?}: expected HTTP 200 from the copy that locks the image set, got {copied:?}"
            );
            // Held so the refusal under test cannot race the copy's completion.
            move_due_instant(
                &emulator,
                name,
                &datastore_id,
                &held_image_set,
                SystemTime::now() + HELD,
            );
            move_due_instant(
                &emulator,
                name,
                &datastore_id,
                &destination,
                SystemTime::now() + HELD,
            );
            Some(destination)
        } else {
            None
        };

        let (datastore, image_set) = match &case.wrong {
            Wrong::Datastore => (unheld_id(), held_image_set.clone()),
            Wrong::ImageSet => (datastore_id.clone(), unheld_id()),
            Wrong::CopySource => (datastore_id.clone(), held_image_set.clone()),
            Wrong::CopyDestination => (
                datastore_id.clone(),
                copy_destination.clone().expect("the copy's destination"),
            ),
            Wrong::MalformedDatastoreId(id) => ((*id).to_string(), held_image_set.clone()),
            Wrong::MalformedImageSetId(id) => (datastore_id.clone(), (*id).to_string()),
        };

        let refused = delete_image_set(&emulator, &datastore, &image_set).await;
        assert_eq!(
            refused.status, case.expected_status,
            "case {name:?}: expected HTTP {}, got {refused:?}",
            case.expected_status
        );
        assert_eq!(
            refused.error_code, case.expected_error_code,
            "case {name:?}: expected {}, got {refused:?}",
            case.expected_error_code
        );
        assert!(
            !refused.body.is_null(),
            "case {name:?}: a refusal must carry the modelled error body, got {refused:?}"
        );
        if let Some(expected) = case.expected_message {
            assert_eq!(
                refused.string(&["message"]),
                expected,
                "case {name:?}: expected the message {expected:?}, got {refused:?}"
            );
        }

        // Nothing moved: the image set the row named is still readable and has no
        // `deletedAt`, so a refused delete deletes nothing.
        let after = get_image_set(&emulator, &datastore_id, &held_image_set, None).await;
        assert_eq!(
            after.status, 200,
            "case {name:?}: the seeded image set must survive a refused delete, got {after:?}"
        );
        assert!(
            after.field(&["deletedAt"]).is_null(),
            "case {name:?}: a refused delete must not date the image set deleted, got {after:?}"
        );
    }
}

/// RULE: the lock a copy holds is released by the CLOCK, and the delete that was refused
/// while it held is admitted once it is — so the refusal above is about the lock, not
/// about the image set.
#[tokio::test]
async fn a_delete_refused_under_a_copys_lock_is_admitted_once_the_clock_releases_it() {
    for versions in [1_u32, 2] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("release.store", "token-release-store")
            .await;
        let source = seed_image_set(&emulator, &datastore_id, versions);

        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source,
            information(&versions.to_string(), None, None),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case versions={versions}: expected HTTP 200 from CopyImageSet, got {copied:?}"
        );
        let destination = copied.string(&["destinationImageSetProperties", "imageSetId"]);

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        for (label, image_set_id) in [("source", &source), ("destination", &destination)] {
            let deleted = delete_image_set(&emulator, &datastore_id, image_set_id).await;
            assert_eq!(
                deleted.status, 200,
                "case versions={versions} {label}: expected HTTP 200 once the copy's lock was \
                 released, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["imageSetState"]),
                "LOCKED",
                "case versions={versions} {label}: expected imageSetState LOCKED, got {deleted:?}"
            );
            assert_eq!(
                deleted.string(&["imageSetWorkflowStatus"]),
                "DELETING",
                "case versions={versions} {label}: expected DELETING, got {deleted:?}"
            );
        }
    }
}

/// RULE: a retired image set takes part in no further work, and a data store holds none
/// of them — so the copy of a retired image set is the modelled conflict, and the data
/// store whose image sets have all been retired is deletable.
///
/// Rows span how many image sets the data store holds.
#[tokio::test]
async fn retiring_every_image_set_frees_the_data_store() {
    for held in [1_usize, 2, 3] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("free.store", "token-free-store")
            .await;
        let image_sets: Vec<String> = (0..held)
            .map(|_| seed_image_set(&emulator, &datastore_id, 1))
            .collect();

        // While the data store holds image sets, its own delete is refused.
        let refused = emulator.delete_datastore(&datastore_id).await;
        assert_eq!(
            refused.status, 409,
            "case held={held}: a data store holding image sets refuses its own delete, got {refused:?}"
        );

        for (index, image_set_id) in image_sets.iter().enumerate() {
            let deleted = delete_image_set(&emulator, &datastore_id, image_set_id).await;
            assert_eq!(
                deleted.status, 200,
                "case held={held} image set {index}: expected HTTP 200, got {deleted:?}"
            );
        }

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;

        // A retired image set is not copyable: it is not in the one state a copy admits.
        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &image_sets[0],
            information("1", None, None),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 409,
            "case held={held}: a retired image set cannot be copied, got {copied:?}"
        );
        assert_eq!(
            copied.string(&["message"]),
            IMAGE_SET_NOT_ACTIVE_MESSAGE,
            "case held={held}: expected the not-active text, got {copied:?}"
        );

        let deleted = emulator.delete_datastore(&datastore_id).await;
        assert_eq!(
            deleted.status, 200,
            "case held={held}: a data store whose image sets are all retired is deletable, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["datastoreStatus"]),
            "DELETING",
            "case held={held}: expected DELETING from DeleteDatastore, got {deleted:?}"
        );
    }
}
