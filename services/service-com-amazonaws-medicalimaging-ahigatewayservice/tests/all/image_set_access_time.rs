// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the instant an image set reports as its LAST ACCESS.
//!
//! RULE: an image set's `lastAccessedAt` ("When the image set was last accessed",
//! `smithy_query(query="operation",operation="GetImageSet")` and
//! `smithy_query(query="shape",shape="ImageSetsMetadataSummary")`) is the most recent
//! instant at which the DICOM content the image set holds was accessed — served to a
//! caller or delivered into it — and is the instant the image set came into existence
//! until something accesses that content. It is REPORTED for every image set, never
//! absent. Reporting an image set's properties is not an access of its content, so
//! `GetImageSet` and `SearchImageSets` report the stored instant without moving it, and a
//! call that serves no pixel data at all records no access.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how many
//! frames the image set holds, which of them is read and in which order, how often, the
//! size of the pixel data, which part of a frame request cannot be served, and whether a
//! copy's destination is minted or an image set that already holds content. Every case
//! drives real HTTP through the production front door, so what is asserted is the wire
//! behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS pixel data enters a data store through a DICOM import, which this emulator's
//! import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the
//! same `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the
//! wire.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    ImageSetState, IMAGE_SET_COPY_DELAY,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_frames::{get_image_frame, pixel_data, seed_image_set_with_frames, SeedFrame};
use crate::image_set_copy::{copy_image_set, get_image_set, information};
use crate::image_set_search::search;

/// How far apart two events a case orders in time are driven, so the instant each one
/// records is distinguishable on the wire from the one before it.
const APART: Duration = Duration::from_millis(25);

/// The slack allowed when a reported instant is bounded by instants the test read off the
/// same clock: the wire carries epoch seconds as a number, whose resolution near the
/// present is well under a millisecond.
const SLACK_SECS: f64 = 1e-3;

/// A well-formed `[0-9a-z]{32}` identifier no call ever mints (`mint_id` is SplitMix64
/// output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// The epoch-seconds number the wire carries for `instant`.
fn wire_instant(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the Unix epoch")
        .as_secs_f64()
}

/// The `lastAccessedAt` one answer reports. The member is reported for EVERY image set,
/// so an absent one fails the case rather than being skipped.
fn last_access(answer: &Answer, case: &str) -> f64 {
    answer
        .field(&["lastAccessedAt"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastAccessedAt instant, got {answer:?}"))
}

/// The `createdAt` one answer reports.
fn created_at(answer: &Answer, case: &str) -> f64 {
    answer
        .field(&["createdAt"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a createdAt instant, got {answer:?}"))
}

/// The summary `SearchImageSets` reports for `image_set_id`.
fn summary_for(answer: &Answer, image_set_id: &str, case: &str) -> Value {
    answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary["imageSetId"].as_str() == Some(image_set_id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for the image set, got {answer:?}"))
}

/// `frames` frames whose sizes and transfer syntaxes differ row by row, so the rule is
/// proved over the pixel data's size as well as over its identifier.
fn frames_of(count: usize) -> Vec<SeedFrame> {
    (0..count)
        .map(|index| SeedFrame {
            transfer_syntax_uid: (index % 2 == 0).then_some("1.2.840.10008.1.2.4.50"),
            pixel_data: pixel_data(index as u8 + 1, 8 * (index + 1)),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// An image set nothing has accessed reports when it came into existence
// ---------------------------------------------------------------------------

/// One row of the never-accessed table.
#[derive(Debug)]
struct UnaccessedCase {
    case: &'static str,
    /// How many frames of pixel data the image set holds.
    frames: usize,
    state: ImageSetState,
}

/// RULE: `lastAccessedAt` is reported for every image set, and until something accesses
/// the content it holds the instant it reports is the one the image set came into
/// existence at — which no reporting call moves, however long the caller waits.
#[tokio::test]
async fn an_unaccessed_image_set_reports_the_instant_it_came_into_existence() {
    let cases = [
        UnaccessedCase {
            case: "holds no pixel data at all",
            frames: 0,
            state: ImageSetState::Active,
        },
        UnaccessedCase {
            case: "holds one frame nobody has read",
            frames: 1,
            state: ImageSetState::Active,
        },
        UnaccessedCase {
            case: "holds three frames nobody has read",
            frames: 3,
            state: ImageSetState::Active,
        },
        UnaccessedCase {
            case: "a deleted image set, which still reports its properties",
            frames: 1,
            state: ImageSetState::Deleted,
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("access.unread", "token-access-unread")
        .await;

    for case in &cases {
        let (image_set_id, _) = seed_image_set_with_frames(
            &emulator,
            &datastore_id,
            case.state,
            &frames_of(case.frames),
        );

        let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetImageSet, got {read:?}",
            case.case
        );
        let reported = last_access(&read, case.case);
        assert_eq!(
            reported,
            created_at(&read, case.case),
            "case {:?}: an image set nothing has accessed reports its creation instant, got \
             {read:?}",
            case.case
        );

        // Reporting its properties is not an access, so the instant does not move —
        // neither on the next read nor past the longest span an image-set transition is
        // configured with.
        let again = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            last_access(&again, case.case),
            reported,
            "case {:?}: a second read must report the same last-access instant, got {again:?}",
            case.case
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
        let settled = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            last_access(&settled, case.case),
            reported,
            "case {:?}: no clock span moves the last-access instant of an image set nothing \
             accesses, got {settled:?}",
            case.case
        );

        // The search summary reports the same instant for the same image set. A DELETED
        // image set is not among the image sets a search reports at all, so that row has
        // no summary to compare.
        if case.state != ImageSetState::Deleted {
            let searched = search(&emulator, &datastore_id, None, "").await;
            let summary = summary_for(&searched, &image_set_id, case.case);
            assert_eq!(
                summary["lastAccessedAt"].as_f64(),
                Some(reported),
                "case {:?}: a summary must report the image set's own last-access instant, got \
                 {summary:?}",
                case.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Serving the content IS the access
// ---------------------------------------------------------------------------

/// One row of the access table: an image set holding `frames` frames, whose frames are
/// read in the order `read_order` names them (by index into the seeded frames).
#[derive(Debug)]
struct AccessCase {
    case: &'static str,
    frames: usize,
    read_order: &'static [usize],
}

/// RULE: serving an image set's pixel data accesses the image set, so `lastAccessedAt`
/// reports the instant of the LATEST such call — and reporting the image set afterwards
/// records nothing of its own.
#[tokio::test]
async fn serving_pixel_data_records_the_access_on_the_image_set() {
    let cases = [
        AccessCase {
            case: "one frame, read once",
            frames: 1,
            read_order: &[0],
        },
        AccessCase {
            case: "one frame, read three times",
            frames: 1,
            read_order: &[0, 0, 0],
        },
        AccessCase {
            case: "three frames, read last then first then middle",
            frames: 3,
            read_order: &[2, 0, 1],
        },
        AccessCase {
            case: "four frames, every one of them read in turn",
            frames: 4,
            read_order: &[0, 1, 2, 3],
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("access.serve", "token-access-serve")
        .await;

    for case in &cases {
        let seeds = frames_of(case.frames);
        let (image_set_id, frame_ids) =
            seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &seeds);

        let before_any = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        let mut previous = last_access(&before_any, case.case);
        assert_eq!(
            previous,
            created_at(&before_any, case.case),
            "case {:?}: nothing has been served yet, got {before_any:?}",
            case.case
        );

        for (step, frame_index) in case.read_order.iter().enumerate() {
            tokio::time::sleep(APART).await;

            let before = wire_instant(SystemTime::now());
            let served =
                get_image_frame(&emulator, &datastore_id, &image_set_id, &frame_ids[*frame_index])
                    .await;
            let after = wire_instant(SystemTime::now());
            assert_eq!(
                served.status, 200,
                "case {:?} step {step}: expected HTTP 200 from GetImageFrame, got {served:?}",
                case.case
            );
            assert_eq!(
                served.body_bytes, seeds[*frame_index].pixel_data,
                "case {:?} step {step}: expected the stored bytes back, got {} bytes",
                case.case,
                served.body_bytes.len()
            );

            let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            let reported = last_access(&read, case.case);
            assert!(
                reported >= before - SLACK_SECS && reported <= after + SLACK_SECS,
                "case {:?} step {step}: expected the last-access instant inside \
                 [{before}, {after}], got {reported} from {read:?}",
                case.case
            );
            assert!(
                reported > previous,
                "case {:?} step {step}: serving pixel data must move the last-access instant \
                 past the previous one ({previous}), got {reported} from {read:?}",
                case.case
            );

            // Reading the properties records nothing: the next read answers the same
            // instant, and so does the search summary.
            let again = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            assert_eq!(
                last_access(&again, case.case),
                reported,
                "case {:?} step {step}: reporting the image set must not record an access, got \
                 {again:?}",
                case.case
            );
            let searched = search(&emulator, &datastore_id, None, "").await;
            let summary = summary_for(&searched, &image_set_id, case.case);
            assert_eq!(
                summary["lastAccessedAt"].as_f64(),
                Some(reported),
                "case {:?} step {step}: the summary must report the same instant GetImageSet \
                 does, got {summary:?}",
                case.case
            );

            previous = reported;
        }
    }
}

// ---------------------------------------------------------------------------
// A call that serves no content is no access
// ---------------------------------------------------------------------------

/// What one refusal row gets wrong about the frame request.
#[derive(Debug)]
enum Wrong {
    /// A data store the service does not hold.
    Datastore,
    /// A well-formed frame id the image set holds no pixel data under.
    Frame,
    /// A frame id outside `ImageFrameId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedFrameId(&'static str),
    /// An image set whose state is `DELETED`: its pixel data went with it.
    DeletedImageSet,
}

/// One row of the no-access table.
#[derive(Debug)]
struct NoAccessCase {
    case: &'static str,
    wrong: Wrong,
    expected_status: u16,
}

/// RULE: `lastAccessedAt` records an access, and a call that served no pixel data was
/// not one — so a refused frame request leaves the instant exactly where it was.
#[tokio::test]
async fn a_call_that_serves_no_pixel_data_records_no_access() {
    let cases = [
        NoAccessCase {
            case: "a data store nothing holds",
            wrong: Wrong::Datastore,
            expected_status: 404,
        },
        NoAccessCase {
            case: "a frame the image set holds nothing under",
            wrong: Wrong::Frame,
            expected_status: 404,
        },
        NoAccessCase {
            case: "a frame id one character short of the pattern",
            wrong: Wrong::MalformedFrameId("0123456789abcdef0123456789abcde"),
            expected_status: 400,
        },
        NoAccessCase {
            case: "a frame id carrying an upper-case character",
            wrong: Wrong::MalformedFrameId("0123456789abcdef0123456789abcdeF"),
            expected_status: 400,
        },
        NoAccessCase {
            case: "a deleted image set, whose pixel data went with it",
            wrong: Wrong::DeletedImageSet,
            expected_status: 404,
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("access.refused", "token-access-refused")
        .await;
    let seeds = frames_of(2);
    let (held_id, held_frames) =
        seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &seeds);
    let (deleted_id, deleted_frames) =
        seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Deleted, &seeds);

    // One real access first, so each row proves the instant it finds is KEPT rather than
    // merely still equal to the creation instant.
    let warmed = get_image_frame(&emulator, &datastore_id, &held_id, &held_frames[0]).await;
    assert_eq!(
        warmed.status, 200,
        "case \"warm-up\": expected HTTP 200 from GetImageFrame, got {warmed:?}"
    );

    for case in &cases {
        let read_back_id = match case.wrong {
            Wrong::DeletedImageSet => deleted_id.clone(),
            _ => held_id.clone(),
        };
        let before = get_image_set(&emulator, &datastore_id, &read_back_id, None).await;
        let expected = last_access(&before, case.case);

        tokio::time::sleep(APART).await;
        let (datastore, image_set, frame) = match &case.wrong {
            Wrong::Datastore => (unheld_id(), held_id.clone(), held_frames[0].clone()),
            Wrong::Frame => (datastore_id.clone(), held_id.clone(), unheld_id()),
            Wrong::MalformedFrameId(frame_id) => (
                datastore_id.clone(),
                held_id.clone(),
                (*frame_id).to_string(),
            ),
            Wrong::DeletedImageSet => (
                datastore_id.clone(),
                deleted_id.clone(),
                deleted_frames[0].clone(),
            ),
        };
        let refused = get_image_frame(&emulator, &datastore, &image_set, &frame).await;
        assert_eq!(
            refused.status, case.expected_status,
            "case {:?}: expected HTTP {}, got {refused:?}",
            case.case, case.expected_status
        );

        let after = get_image_set(&emulator, &datastore_id, &read_back_id, None).await;
        assert_eq!(
            last_access(&after, case.case),
            expected,
            "case {:?}: a request that served no pixel data must leave the last-access instant \
             at {expected}, got {after:?}",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// A copy reads one image set's content and delivers content into another
// ---------------------------------------------------------------------------

/// One row of the copy table.
#[derive(Debug)]
struct CopyCase {
    case: &'static str,
    /// How many frames the SOURCE holds.
    source_frames: usize,
    /// Whether the destination is an image set that already holds content of its own,
    /// rather than one the copy mints.
    destination_held: bool,
}

/// RULE: a copy READS its source's content when it is admitted and DELIVERS content into
/// its destination when the clock completes it, so each image set's `lastAccessedAt`
/// moves to the instant its own content was touched — and the release of the source's
/// read-only lock, which delivers nothing, moves nothing.
#[tokio::test]
async fn a_copy_records_the_access_on_the_content_it_reads_and_the_content_it_delivers() {
    let cases = [
        CopyCase {
            case: "minted destination, one frame copied",
            source_frames: 1,
            destination_held: false,
        },
        CopyCase {
            case: "minted destination, three frames copied",
            source_frames: 3,
            destination_held: false,
        },
        CopyCase {
            case: "destination that already holds content of its own",
            source_frames: 2,
            destination_held: true,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(
                &format!("access.copy.{index}"),
                &format!("token-access-copy-{index}"),
            )
            .await;

        let (source_id, _) = seed_image_set_with_frames(
            &emulator,
            &datastore_id,
            ImageSetState::Active,
            &frames_of(case.source_frames),
        );
        let destination_seeded = case.destination_held.then(|| {
            seed_image_set_with_frames(
                &emulator,
                &datastore_id,
                ImageSetState::Active,
                &frames_of(1),
            )
        });

        let source_before = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        let source_unaccessed = last_access(&source_before, case.case);
        let destination_unaccessed = match &destination_seeded {
            Some((destination_id, _)) => {
                let read = get_image_set(&emulator, &datastore_id, destination_id, None).await;
                Some(last_access(&read, case.case))
            }
            None => None,
        };

        tokio::time::sleep(APART).await;
        let admitted_from = wire_instant(SystemTime::now());
        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(
                "1",
                None,
                destination_seeded
                    .as_ref()
                    .map(|(image_set_id, _)| (image_set_id.as_str(), "1")),
            ),
            &[],
        )
        .await;
        let admitted_to = wire_instant(SystemTime::now());
        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);

        // The copy read the SOURCE's content to carry it, so the source was accessed at
        // the instant the copy was admitted.
        let source_read = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        let source_accessed = last_access(&source_read, case.case);
        assert!(
            source_accessed >= admitted_from - SLACK_SECS
                && source_accessed <= admitted_to + SLACK_SECS,
            "case {:?}: expected the source's last access inside [{admitted_from}, \
             {admitted_to}], got {source_accessed} from {source_read:?}",
            case.case
        );
        assert!(
            source_accessed > source_unaccessed,
            "case {:?}: the copy must move the source's last access past {source_unaccessed}, \
             got {source_accessed}",
            case.case
        );

        // The destination has had nothing delivered into it yet, so an image set that
        // already existed still reports the access it had going in.
        if let Some(unaccessed) = destination_unaccessed {
            let in_flight = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
            assert_eq!(
                last_access(&in_flight, case.case),
                unaccessed,
                "case {:?}: a copy in flight has delivered nothing, so the destination's last \
                 access is still {unaccessed}, got {in_flight:?}",
                case.case
            );
        }

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        // The content landed, which accessed the destination — at the instant the copy
        // fell due, so the reported instant is past the admission and does not move
        // between reads.
        let landed = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        let destination_accessed = last_access(&landed, case.case);
        assert!(
            destination_accessed >= admitted_from + IMAGE_SET_COPY_DELAY.as_secs_f64()
                - SLACK_SECS,
            "case {:?}: expected the destination's last access at the instant the copy fell \
             due (at least {}), got {destination_accessed} from {landed:?}",
            case.case,
            admitted_from + IMAGE_SET_COPY_DELAY.as_secs_f64()
        );
        assert!(
            destination_accessed > created_at(&landed, case.case),
            "case {:?}: the delivery accessed the destination after it came into existence \
             ({}), got {destination_accessed} from {landed:?}",
            case.case,
            created_at(&landed, case.case)
        );
        let landed_again = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
        assert_eq!(
            last_access(&landed_again, case.case),
            destination_accessed,
            "case {:?}: an instant the clock recorded must not move between reads, got \
             {landed_again:?}",
            case.case
        );

        // Releasing the source's read-only lock delivers no content, so it is no access:
        // the source still reports the instant the copy read it.
        let source_settled = get_image_set(&emulator, &datastore_id, &source_id, None).await;
        assert_eq!(
            last_access(&source_settled, case.case),
            source_accessed,
            "case {:?}: the release of the source's lock delivers nothing, so its last access \
             is still {source_accessed}, got {source_settled:?}",
            case.case
        );
    }
}
