// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetImageFrame`.
//!
//! RULE: `GetImageFrame` serves the pixel data the image set holds under the image frame
//! identifier the request names, byte for byte as it was stored, and reports in
//! `contentType` what the response member's documentation gives for the transfer syntax
//! the pixel data is STORED in — the content type documented for that UID, `image/jph`
//! when the frame declares no transfer syntax, and the documented default
//! `application/octet-stream` for a UID the documentation does not list. Pixel data is
//! never invented: a data store, image set or frame the service holds nothing for is
//! 404 `ResourceNotFoundException`, worded by the resource KIND, and an image frame
//! identifier that fails the member's own constraint is 400 `ValidationException`.
//!
//! A copy is what moves pixel data between image sets here, and it does so when the
//! CLOCK completes it: the destination of a copy in flight holds nothing of the source
//! yet, and serves the source's frames once the copy has landed.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! stored transfer syntax (every documented UID, a missing one, an undocumented one),
//! the size of the stored pixel data, which resource the request gets wrong, and
//! whether the copy's destination is minted or an image set that already holds frames
//! of its own. Every case drives real HTTP through the production front door, so what
//! is asserted is the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS pixel data enters a data store through a DICOM import, and this emulator's
//! import job does not create image sets yet — that is
//! `issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`.
//! So the image set a case reads frames from is seeded through the crate's public
//! `state` module, with the same `ImageSet::new` / `ImageFrame` the handlers use, and
//! everything each case then asserts goes over HTTP.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageFrame, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    DEFAULT_IMAGE_FRAME_CONTENT_TYPE, IMAGE_FRAME_CONTENT_TYPES, IMAGE_SET_COPY_DELAY,
    MISSING_TRANSFER_SYNTAX_CONTENT_TYPE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};

/// A well-formed `[0-9a-z]{32}` resource id that no call ever mints (the ids `mint_id`
/// produces are SplitMix64 output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// The workflow status a seeded image set carries: `IMPORTED`, which is what a DICOM
/// import leaves behind.
const SEEDED_WORKFLOW_STATUS: ImageSetWorkflowStatus = ImageSetWorkflowStatus::Imported;

/// The service's constant text for an image frame no image set holds pixel data under.
const IMAGE_FRAME_NOT_FOUND: &str = "ImageFrame resource is not found.";

/// The service's constant text for an image set a data store does not hold.
const IMAGE_SET_NOT_FOUND: &str = "ImageSet resource is not found.";

/// The service's constant text for a data store it does not hold.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// One frame to seed: the transfer syntax its pixel data is stored in (`None` for an
/// instance that declared none) and the bytes themselves.
#[derive(Debug, Clone)]
struct SeedFrame {
    transfer_syntax_uid: Option<&'static str>,
    pixel_data: Vec<u8>,
}

/// Seed one image set in `state`, holding one frame per entry of `frames`, and answer
/// its identifier beside the minted identifier of each frame, in the order given.
///
/// The record is built with the crate's own `ImageSet::new`, and each frame id is minted
/// by the same `mint_id` every handler mints identifiers with, so a seeded image set is
/// shaped exactly like one a handler would hold.
fn seed_image_set_with_frames(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_state: ImageSetState,
    frames: &[SeedFrame],
) -> (String, Vec<String>) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let frame_ids: Vec<String> = frames.iter().map(|_| store.mint_id()).collect();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        image_set_state,
        SEEDED_WORKFLOW_STATUS,
        false,
        false,
        SystemTime::now(),
    );
    for (frame_id, frame) in frame_ids.iter().zip(frames) {
        image_set.image_frames.insert(
            frame_id.clone(),
            ImageFrame {
                image_frame_id: frame_id.clone(),
                transfer_syntax_uid: frame.transfer_syntax_uid.map(str::to_string),
                pixel_data: frame.pixel_data.clone(),
            },
        );
    }
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    (image_set_id, frame_ids)
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageFrame` —
/// `GetImageFrame`. The body IS the `imageFrameInformation` payload.
async fn get_image_frame(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    image_frame_id: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageFrame"),
            Some(json!({ "imageFrameId": image_frame_id })),
        )
        .await
}

/// `POST /datastore/{datastoreId}/imageSet/{sourceImageSetId}/copyImageSet` —
/// `CopyImageSet`, with the source's claimed latest version and an optional destination.
async fn copy_image_set(
    emulator: &Emulator,
    datastore_id: &str,
    source_image_set_id: &str,
    source_latest_version: &str,
    destination: Option<(&str, &str)>,
) -> Answer {
    let mut payload: Value = json!({
        "sourceImageSet": { "latestVersionId": source_latest_version },
    });
    if let Some((image_set_id, latest_version_id)) = destination {
        payload["destinationImageSet"] = json!({
            "imageSetId": image_set_id,
            "latestVersionId": latest_version_id,
        });
    }
    emulator
        .call(
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{source_image_set_id}/copyImageSet"),
            Some(payload),
        )
        .await
}

/// Pixel data of `len` bytes that no two frames share: a frame's blob must come back
/// byte for byte, so each case's payload has to be distinguishable from every other's.
fn pixel_data(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| seed.wrapping_add((index % 251) as u8))
        .collect()
}

// ---------------------------------------------------------------------------
// The documented transfer syntax -> contentType catalog
// ---------------------------------------------------------------------------

/// One row of the content-type table: what the frame is stored as, and what
/// `GetImageFrame` must report for it.
#[derive(Debug)]
struct ContentTypeCase {
    case: String,
    transfer_syntax_uid: Option<&'static str>,
    expected_content_type: &'static str,
    pixel_data: Vec<u8>,
}

/// Every row of the documented catalog, plus the two rows the catalog does not list by
/// UID: a frame with no stored transfer syntax, and one stored in a transfer syntax the
/// documentation says nothing about.
///
/// The UID -> content type pairs come from the crate's `IMAGE_FRAME_CONTENT_TYPES`
/// table, which is transcribed from the member's own documentation
/// (`smithy_query(query="doc",shape="GetImageFrameResponse",member="contentType")`), so
/// every documented entry is a case here rather than only the few an example cites. The
/// pixel-data length varies row by row, including an empty frame, so the blob is proved
/// over size as well as over content type.
fn content_type_cases() -> Vec<ContentTypeCase> {
    let mut cases: Vec<ContentTypeCase> = IMAGE_FRAME_CONTENT_TYPES
        .iter()
        .enumerate()
        .map(|(index, (uid, content_type))| ContentTypeCase {
            case: format!("stored transfer syntax {uid}"),
            transfer_syntax_uid: Some(uid),
            expected_content_type: content_type,
            // 0, 1, 2, ... bytes: an empty frame is a row of this class too.
            pixel_data: pixel_data(index as u8, index),
        })
        .collect();
    cases.push(ContentTypeCase {
        case: "no stored transfer syntax".to_string(),
        transfer_syntax_uid: None,
        expected_content_type: MISSING_TRANSFER_SYNTAX_CONTENT_TYPE,
        pixel_data: pixel_data(200, 64),
    });
    cases.push(ContentTypeCase {
        // Implicit VR Little Endian: a real DICOM transfer syntax the response
        // documentation lists no content type for, so it takes the documented default.
        case: "undocumented transfer syntax 1.2.840.10008.1.2".to_string(),
        transfer_syntax_uid: Some("1.2.840.10008.1.2"),
        expected_content_type: DEFAULT_IMAGE_FRAME_CONTENT_TYPE,
        pixel_data: pixel_data(37, 4096),
    });
    cases
}

/// RULE: the blob comes back byte for byte as it was stored, and `contentType` is what
/// the response member's documentation gives for the transfer syntax the pixel data is
/// stored in.
#[tokio::test]
async fn a_frame_serves_its_stored_bytes_and_the_documented_content_type() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("frame.catalog", "token-frame-catalog").await;

    let cases = content_type_cases();
    let seeds: Vec<SeedFrame> = cases
        .iter()
        .map(|case| SeedFrame {
            transfer_syntax_uid: case.transfer_syntax_uid,
            pixel_data: case.pixel_data.clone(),
        })
        .collect();
    let (image_set_id, frame_ids) = seed_image_set_with_frames(
        &emulator,
        &datastore_id,
        ImageSetState::Active,
        &seeds,
    );

    for (case, frame_id) in cases.iter().zip(&frame_ids) {
        let read = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetImageFrame, got {read:?}",
            case.case
        );
        assert_eq!(
            read.content_type, case.expected_content_type,
            "case {:?}: expected contentType {:?}, got {:?}",
            case.case, case.expected_content_type, read.content_type
        );
        assert_eq!(
            read.body_bytes, case.pixel_data,
            "case {:?}: expected the {} stored bytes back verbatim, got {} bytes",
            case.case,
            case.pixel_data.len(),
            read.body_bytes.len()
        );
    }
}

/// RULE: pixel data is read from state, so a frame is served to every caller the same
/// way and a read changes nothing a later caller sees.
#[tokio::test]
async fn reading_a_frame_leaves_it_exactly_as_it_was() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("frame.reread", "token-frame-reread").await;
    let seeds = [
        SeedFrame {
            transfer_syntax_uid: Some("1.2.840.10008.1.2.4.50"),
            pixel_data: pixel_data(11, 128),
        },
        SeedFrame {
            transfer_syntax_uid: None,
            pixel_data: pixel_data(12, 1),
        },
    ];
    let (image_set_id, frame_ids) =
        seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &seeds);

    for (index, frame_id) in frame_ids.iter().enumerate() {
        let mut answers = Vec::new();
        for _ in 0..3 {
            answers.push(get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await);
        }
        for (read_number, read) in answers.iter().enumerate() {
            assert_eq!(
                read.status, 200,
                "frame {index} read {read_number}: expected HTTP 200, got {read:?}"
            );
            assert_eq!(
                read.body_bytes, seeds[index].pixel_data,
                "frame {index} read {read_number}: expected the stored bytes, got {} bytes",
                read.body_bytes.len()
            );
            assert_eq!(
                read.content_type, answers[0].content_type,
                "frame {index} read {read_number}: contentType must not vary between reads, \
                 got {:?} after {:?}",
                read.content_type, answers[0].content_type
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the rule does not admit
// ---------------------------------------------------------------------------

/// Which resource one refusal row gets wrong.
#[derive(Debug)]
enum Wrong {
    Datastore,
    ImageSet,
    /// A well-formed frame id the image set holds no pixel data under.
    Frame,
    /// A frame id that fails `ImageFrameId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedFrameId(&'static str),
    /// An image set whose state is `DELETED`: its pixel data went with it.
    DeletedImageSet,
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    wrong: Wrong,
    expected_status: u16,
    expected_error_code: &'static str,
    /// The text the refusal must state, or `None` where the row asserts only the
    /// status and the code (a constraint envelope the framework words).
    expected_message: Option<&'static str>,
}

/// RULE: a resource the service holds nothing for is 404 `ResourceNotFoundException`
/// worded by the resource KIND, and an image frame identifier that fails the member's
/// constraint is 400 `ValidationException` — never an empty 200 and never invented
/// pixel data.
#[tokio::test]
async fn a_frame_the_service_does_not_hold_is_refused() {
    // `ImageFrameId` is `^[0-9a-z]{32}$`
    // (`smithy_query(query="shape",shape="ImageFrameInformation")`), so a row past each
    // end of that constraint is a validation failure, not a 404.
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
            case: "image set holds no such frame",
            wrong: Wrong::Frame,
            expected_status: 404,
            expected_error_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_FRAME_NOT_FOUND),
        },
        RefusalCase {
            case: "DELETED image set",
            wrong: Wrong::DeletedImageSet,
            expected_status: 404,
            expected_error_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_NOT_FOUND),
        },
        RefusalCase {
            case: "frame id one character short",
            wrong: Wrong::MalformedFrameId("0123456789abcdef0123456789abcde"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "frame id one character long",
            wrong: Wrong::MalformedFrameId("0123456789abcdef0123456789abcdef0"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "frame id with an uppercase character",
            wrong: Wrong::MalformedFrameId("0123456789abcdef0123456789abcdeF"),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "empty frame id",
            wrong: Wrong::MalformedFrameId(""),
            expected_status: 400,
            expected_error_code: "ValidationException",
            expected_message: None,
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("frame.refusal", "token-frame-refusal").await;
    let held_seed = [SeedFrame {
        transfer_syntax_uid: Some("1.2.840.10008.1.2.1"),
        pixel_data: pixel_data(5, 32),
    }];
    let (held_image_set, held_frames) =
        seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &held_seed);
    let (deleted_image_set, deleted_frames) = seed_image_set_with_frames(
        &emulator,
        &datastore_id,
        ImageSetState::Deleted,
        &held_seed,
    );

    for case in &cases {
        let (datastore, image_set, frame) = match &case.wrong {
            Wrong::Datastore => (
                unheld_id(),
                held_image_set.clone(),
                held_frames[0].clone(),
            ),
            Wrong::ImageSet => (datastore_id.clone(), unheld_id(), held_frames[0].clone()),
            Wrong::Frame => (
                datastore_id.clone(),
                held_image_set.clone(),
                unheld_id(),
            ),
            Wrong::DeletedImageSet => (
                datastore_id.clone(),
                deleted_image_set.clone(),
                deleted_frames[0].clone(),
            ),
            Wrong::MalformedFrameId(frame_id) => (
                datastore_id.clone(),
                held_image_set.clone(),
                (*frame_id).to_string(),
            ),
        };
        let refused = get_image_frame(&emulator, &datastore, &image_set, &frame).await;
        assert_eq!(
            refused.status, case.expected_status,
            "case {:?}: expected HTTP {}, got {refused:?}",
            case.case, case.expected_status
        );
        assert_eq!(
            refused.error_code, case.expected_error_code,
            "case {:?}: expected {}, got {refused:?}",
            case.case, case.expected_error_code
        );
        assert!(
            !refused.body.is_null(),
            "case {:?}: a refusal must carry the modelled error body, got {refused:?}",
            case.case
        );
        if let Some(expected) = case.expected_message {
            assert_eq!(
                refused.string(&["message"]),
                expected,
                "case {:?}: expected the message {expected:?}, got {refused:?}",
                case.case
            );
        }
    }

    // The frame the held image set DOES hold is still served, so the rows above refused
    // what they named rather than everything.
    let served = get_image_frame(&emulator, &datastore_id, &held_image_set, &held_frames[0]).await;
    assert_eq!(
        served.status, 200,
        "control: the held frame must still be served, got {served:?}"
    );
}

// ---------------------------------------------------------------------------
// A copy delivers pixel data, and the clock is what lands it
// ---------------------------------------------------------------------------

/// One row of the copy-delivery table.
#[derive(Debug)]
struct DeliveryCase {
    case: &'static str,
    /// How many frames the SOURCE holds.
    source_frames: usize,
    /// Whether the destination is an image set that already holds a frame of its own,
    /// rather than one the copy mints.
    destination_holds_frames: bool,
}

/// RULE: a copy carries the source's pixel data to its destination, and the CLOCK lands
/// it: before the copy completes the destination serves nothing the copy has not
/// delivered, and afterwards it serves every frame of the source byte for byte, beside
/// any frame it already held.
#[tokio::test]
async fn a_copy_delivers_the_sources_frames_when_the_clock_completes_it() {
    let cases = [
        DeliveryCase {
            case: "minted destination, one frame",
            source_frames: 1,
            destination_holds_frames: false,
        },
        DeliveryCase {
            case: "minted destination, three frames",
            source_frames: 3,
            destination_holds_frames: false,
        },
        DeliveryCase {
            case: "destination that already holds a frame",
            source_frames: 2,
            destination_holds_frames: true,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(
                &format!("frame.copy.{index}"),
                &format!("token-frame-copy-{index}"),
            )
            .await;

        let source_seeds: Vec<SeedFrame> = (0..case.source_frames)
            .map(|frame| SeedFrame {
                transfer_syntax_uid: Some("1.2.840.10008.1.2.4.91"),
                pixel_data: pixel_data(frame as u8 + 1, 16 * (frame + 1)),
            })
            .collect();
        let (source_id, source_frames) = seed_image_set_with_frames(
            &emulator,
            &datastore_id,
            ImageSetState::Active,
            &source_seeds,
        );

        let destination_seed = [SeedFrame {
            transfer_syntax_uid: None,
            pixel_data: pixel_data(99, 24),
        }];
        let destination_seeded = case.destination_holds_frames.then(|| {
            seed_image_set_with_frames(
                &emulator,
                &datastore_id,
                ImageSetState::Active,
                &destination_seed,
            )
        });

        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            "1",
            destination_seeded
                .as_ref()
                .map(|(image_set_id, _)| (image_set_id.as_str(), "1")),
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);

        // While the copy is in flight the destination holds only what it held before:
        // nothing has delivered the source's pixel data to it yet.
        let in_flight =
            get_image_frame(&emulator, &datastore_id, &destination_id, &source_frames[0]).await;
        assert_eq!(
            in_flight.status, 404,
            "case {:?}: a copy in flight has delivered no pixel data yet, got {in_flight:?}",
            case.case
        );
        assert_eq!(
            in_flight.string(&["message"]),
            IMAGE_FRAME_NOT_FOUND,
            "case {:?}: expected the frame-not-found text, got {in_flight:?}",
            case.case
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        for (frame_index, (frame_id, seed)) in
            source_frames.iter().zip(&source_seeds).enumerate()
        {
            let delivered =
                get_image_frame(&emulator, &datastore_id, &destination_id, frame_id).await;
            assert_eq!(
                delivered.status, 200,
                "case {:?}: frame {frame_index} must be served from the destination once the \
                 copy has landed, got {delivered:?}",
                case.case
            );
            assert_eq!(
                delivered.body_bytes, seed.pixel_data,
                "case {:?}: frame {frame_index} must come back byte for byte, got {} bytes",
                case.case,
                delivered.body_bytes.len()
            );

            // The source still serves its own frames: a copy reads it.
            let from_source =
                get_image_frame(&emulator, &datastore_id, &source_id, frame_id).await;
            assert_eq!(
                from_source.body_bytes, seed.pixel_data,
                "case {:?}: the source must still serve frame {frame_index}, got {from_source:?}",
                case.case
            );
        }

        // A destination that already held pixel data keeps it: a copy adds frames, it
        // does not replace the ones the destination had.
        if let Some((_, held)) = &destination_seeded {
            let kept = get_image_frame(&emulator, &datastore_id, &destination_id, &held[0]).await;
            assert_eq!(
                kept.status, 200,
                "case {:?}: the destination's own frame must survive the copy, got {kept:?}",
                case.case
            );
            assert_eq!(
                kept.body_bytes, destination_seed[0].pixel_data,
                "case {:?}: the destination's own frame must come back unchanged, got {} bytes",
                case.case,
                kept.body_bytes.len()
            );
            assert_eq!(
                kept.content_type, MISSING_TRANSFER_SYNTAX_CONTENT_TYPE,
                "case {:?}: the destination's own frame declares no transfer syntax, so it \
                 reports {MISSING_TRANSFER_SYNTAX_CONTENT_TYPE}, got {:?}",
                case.case, kept.content_type
            );
        }
    }
}
