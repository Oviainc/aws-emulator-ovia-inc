// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two state rules BOTH content reads of an image set decide.
//!
//! RULE: `GetImageFrame` (pixel data) and `GetImageSetMetadata` (the metadata document)
//! are the two calls that serve the content an image set holds, and they decide the same
//! two questions by the same rule:
//!
//! * an image set whose latest version is `DELETED` holds no content at all, so either
//!   read answers 404 `ResourceNotFoundException` worded by the IMAGE SET kind — the
//!   whole resource is what is absent, not the frame or the attribute asked for;
//! * the access a successful read records is recorded on the image set of the data store
//!   the request NAMES, resolved in the REGION the request was addressed to, so a read
//!   moves that one image set's `lastAccessedAt` and nothing else's — and a read a region
//!   cannot reach the data store from serves nothing and records nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the two readers is called, how the image set reached `DELETED` (seeded there, or
//! retired by `DeleteImageSet` and landed by the clock), how much pixel data it holds, and
//! which region the data store lives in and the request is addressed to. Every case drives
//! real HTTP through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set and its pixel data enter a data store through a DICOM import, which
//! this emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the
//! same `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the
//! wire.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    ImageSetState, DATASTORE_ACTIVATION_DELAY, DATASTORE_NOT_FOUND_MESSAGE,
    IMAGE_FRAME_NOT_FOUND_MESSAGE, IMAGE_SET_DELETION_DELAY, IMAGE_SET_NOT_FOUND_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, DEFAULT_TEST_REGION, SETTLE_MULTIPLE};
use crate::image_frames::{pixel_data, seed_image_set_with_frames, SeedFrame};
use crate::image_set_deletion::delete_image_set;

/// The regions the access rows are addressed to. Real AWS region codes; the suite reads
/// an image set in each one of them and from each of the others.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// How many image sets each region's data store holds, so a read can be shown to move the
/// instant of ONE of them.
const SETS_PER_REGION: usize = 2;

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

/// Which of the two content reads a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reader {
    /// `GetImageFrame` — the pixel data.
    Frame,
    /// `GetImageSetMetadata` — the metadata document.
    Metadata,
}

impl Reader {
    /// Every reader, so a row can be driven through both without naming them twice.
    const ALL: [Reader; 2] = [Reader::Frame, Reader::Metadata];

    /// The operation name, for the case label an assertion prints.
    fn name(self) -> &'static str {
        match self {
            Reader::Frame => "GetImageFrame",
            Reader::Metadata => "GetImageSetMetadata",
        }
    }

    /// Send this read to `region`. `image_frame_id` is what the pixel-data read names;
    /// the metadata read carries no body at all.
    async fn call_in(
        self,
        emulator: &Emulator,
        region: &str,
        datastore_id: &str,
        image_set_id: &str,
        image_frame_id: &str,
    ) -> Answer {
        match self {
            Reader::Frame => {
                emulator
                    .call_in(
                        region,
                        "POST",
                        &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageFrame"),
                        Some(json!({ "imageFrameId": image_frame_id })),
                    )
                    .await
            }
            Reader::Metadata => {
                emulator
                    .call_in(
                        region,
                        "POST",
                        &format!(
                            "/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSetMetadata"
                        ),
                        None,
                    )
                    .await
            }
        }
    }
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSet` in `region`.
async fn get_image_set_in(
    emulator: &Emulator,
    region: &str,
    datastore_id: &str,
    image_set_id: &str,
) -> Answer {
    emulator
        .call_in(
            region,
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSet"),
            None,
        )
        .await
}

/// The `lastAccessedAt` instant `GetImageSet` reports for one image set in `region`. The
/// member is reported for every image set, so an absent one fails the case.
async fn last_access_in(
    emulator: &Emulator,
    region: &str,
    datastore_id: &str,
    image_set_id: &str,
    case: &str,
) -> f64 {
    let read = get_image_set_in(emulator, region, datastore_id, image_set_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected HTTP 200 from GetImageSet in {region:?}, got {read:?}"
    );
    read.field(&["lastAccessedAt"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastAccessedAt instant, got {read:?}"))
}

/// `POST /datastore` in `region`, answering the minted data store's id. The caller waits
/// out the activation the clock owes every new store once, for all of them.
async fn create_datastore_in(
    emulator: &Emulator,
    region: &str,
    datastore_name: &str,
    client_token: &str,
) -> String {
    let created = emulator
        .call_in(
            region,
            "POST",
            "/datastore",
            Some(json!({
                "datastoreName": datastore_name,
                "clientToken": client_token,
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {region:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
    );
    created.string(&["datastoreId"])
}

/// `count` frames whose sizes and transfer syntaxes differ frame by frame, so a row is
/// driven over the pixel data's size as well as over its identifier.
fn frames_of(count: usize) -> Vec<SeedFrame> {
    (0..count)
        .map(|index| SeedFrame {
            transfer_syntax_uid: (index % 2 == 0).then_some("1.2.840.10008.1.2.4.50"),
            pixel_data: pixel_data(index as u8 + 1, 8 * (index + 1)),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// A DELETED image set holds no content for either reader
// ---------------------------------------------------------------------------

/// How the image set a row reads reached `DELETED`.
#[derive(Debug, Clone, Copy)]
enum Retired {
    /// Seeded on the terminal state directly.
    Seeded,
    /// Accepted by `DeleteImageSet` and landed on `DELETED` by the clock.
    ByDelete,
}

/// One row of the deleted-image-set table.
#[derive(Debug)]
struct DeletedCase {
    case: &'static str,
    /// How many frames of pixel data the image set held.
    frames: usize,
    retired: Retired,
}

/// RULE: an image set whose latest version is `DELETED` holds no content, so both content
/// reads answer 404 `ResourceNotFoundException` worded by the image-set kind — while a
/// live image set beside it still serves what it holds, which is what shows the refusal
/// comes from the deletion and not from the shape of the request.
#[tokio::test]
async fn a_deleted_image_set_serves_no_content_to_either_reader() {
    let cases = [
        DeletedCase {
            case: "seeded DELETED, holding no pixel data",
            frames: 0,
            retired: Retired::Seeded,
        },
        DeletedCase {
            case: "seeded DELETED, holding one frame",
            frames: 1,
            retired: Retired::Seeded,
        },
        DeletedCase {
            case: "seeded DELETED, holding three frames",
            frames: 3,
            retired: Retired::Seeded,
        },
        DeletedCase {
            case: "retired by DeleteImageSet, holding one frame",
            frames: 1,
            retired: Retired::ByDelete,
        },
        DeletedCase {
            case: "retired by DeleteImageSet, holding four frames",
            frames: 4,
            retired: Retired::ByDelete,
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("content.deleted", "token-content-deleted")
        .await;

    for case in &cases {
        let seeds = frames_of(case.frames);
        let seeded_state = match case.retired {
            Retired::Seeded => ImageSetState::Deleted,
            Retired::ByDelete => ImageSetState::Active,
        };
        let (image_set_id, frame_ids) =
            seed_image_set_with_frames(&emulator, &datastore_id, seeded_state, &seeds);
        // The live sibling: the same content, in the same data store, never deleted.
        let (live_id, live_frames) =
            seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &seeds);

        if matches!(case.retired, Retired::ByDelete) {
            let accepted = delete_image_set(&emulator, &datastore_id, &image_set_id).await;
            assert_eq!(
                accepted.status, 200,
                "case {:?}: expected HTTP 200 from DeleteImageSet, got {accepted:?}",
                case.case
            );
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;
        }

        let frame_id = frame_ids.first().cloned().unwrap_or_else(unheld_id);
        for reader in Reader::ALL {
            let refused = reader
                .call_in(
                    &emulator,
                    DEFAULT_TEST_REGION,
                    &datastore_id,
                    &image_set_id,
                    &frame_id,
                )
                .await;
            assert_eq!(
                refused.status,
                404,
                "case {:?} through {}: expected HTTP 404, got {refused:?}",
                case.case,
                reader.name()
            );
            assert_eq!(
                refused.error_code,
                "ResourceNotFoundException",
                "case {:?} through {}: expected ResourceNotFoundException, got {refused:?}",
                case.case,
                reader.name()
            );
            assert_eq!(
                refused.string(&["message"]),
                IMAGE_SET_NOT_FOUND_MESSAGE,
                "case {:?} through {}: expected the IMAGE SET to be the missing resource, got \
                 {refused:?}",
                case.case,
                reader.name()
            );

            // The control, on the live sibling: the metadata document is served, and so is
            // the pixel data when the image set holds any — an image set holding none
            // refuses by the FRAME kind, which is a different absence from the one above.
            let live_frame = live_frames.first().cloned().unwrap_or_else(unheld_id);
            let served = reader
                .call_in(
                    &emulator,
                    DEFAULT_TEST_REGION,
                    &datastore_id,
                    &live_id,
                    &live_frame,
                )
                .await;
            if reader == Reader::Frame && case.frames == 0 {
                assert_eq!(
                    served.status, 404,
                    "case {:?} through GetImageFrame on the live sibling: an image set holding \
                     no pixel data refuses the frame, got {served:?}",
                    case.case
                );
                assert_eq!(
                    served.string(&["message"]),
                    IMAGE_FRAME_NOT_FOUND_MESSAGE,
                    "case {:?} through GetImageFrame on the live sibling: expected the FRAME to \
                     be the missing resource, got {served:?}",
                    case.case
                );
            } else {
                assert_eq!(
                    served.status,
                    200,
                    "case {:?} through {} on the live sibling: expected the content to be served, \
                     got {served:?}",
                    case.case,
                    reader.name()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The access a read records is recorded on the record it read
// ---------------------------------------------------------------------------

/// Every image set this suite seeds, keyed by the region holding it, each entry carrying
/// its data store's id, the image set's id and the id of its first frame.
#[derive(Debug, Clone)]
struct Seeded {
    datastore_id: String,
    image_set_id: String,
    frame_id: String,
}

/// Snapshot the `lastAccessedAt` of every seeded image set, keyed by region and image set
/// id, so a row can assert exactly which one of them moved.
async fn snapshot(
    emulator: &Emulator,
    seeded: &BTreeMap<&'static str, Vec<Seeded>>,
    case: &str,
) -> BTreeMap<(&'static str, String), f64> {
    let mut instants = BTreeMap::new();
    for (region, sets) in seeded {
        for set in sets {
            let instant = last_access_in(
                emulator,
                region,
                &set.datastore_id,
                &set.image_set_id,
                case,
            )
            .await;
            instants.insert((*region, set.image_set_id.clone()), instant);
        }
    }
    instants
}

/// RULE: a content read that serves what an image set holds records the access on THAT
/// image set — the one held by the data store the request names, in the region the request
/// was addressed to — so one instant moves and every other image set's stays where it was;
/// and a read addressed to a region the data store is not in reaches nothing, so it serves
/// nothing and records nothing anywhere.
#[tokio::test]
async fn a_content_read_records_its_access_on_the_image_set_it_served_in_that_region() {
    let emulator = Emulator::new();

    let mut seeded: BTreeMap<&'static str, Vec<Seeded>> = BTreeMap::new();
    for region in REGIONS {
        let datastore_id = create_datastore_in(
            &emulator,
            region,
            &format!("content.access.{region}"),
            &format!("token-content-access-{region}"),
        )
        .await;
        seeded.insert(
            region,
            vec![Seeded {
                datastore_id,
                image_set_id: String::new(),
                frame_id: String::new(),
            }],
        );
    }
    // One settle for every store created above: the clock owes each the move to ACTIVE.
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;

    for region in REGIONS {
        let datastore_id = seeded[region][0].datastore_id.clone();
        let sets = (0..SETS_PER_REGION)
            .map(|index| {
                let (image_set_id, frame_ids) = seed_image_set_with_frames(
                    &emulator,
                    &datastore_id,
                    ImageSetState::Active,
                    &frames_of(index + 1),
                );
                Seeded {
                    datastore_id: datastore_id.clone(),
                    frame_id: frame_ids
                        .first()
                        .cloned()
                        .expect("a seeded image set holding at least one frame"),
                    image_set_id,
                }
            })
            .collect();
        seeded.insert(region, sets);
    }

    for reader in Reader::ALL {
        for region in REGIONS {
            for (index, target) in seeded[region].clone().iter().enumerate() {
                let case = format!("{} on image set {index} in {region}", reader.name());
                let before_instants = snapshot(&emulator, &seeded, &case).await;
                let before = wire_instant(SystemTime::now());

                let served = reader
                    .call_in(
                        &emulator,
                        region,
                        &target.datastore_id,
                        &target.image_set_id,
                        &target.frame_id,
                    )
                    .await;
                let after = wire_instant(SystemTime::now());
                assert_eq!(
                    served.status, 200,
                    "case {case:?}: expected the content to be served, got {served:?}"
                );

                let after_instants = snapshot(&emulator, &seeded, &case).await;
                for (key, recorded) in &after_instants {
                    let previous = before_instants[key];
                    if key == &(region, target.image_set_id.clone()) {
                        assert!(
                            *recorded >= before - SLACK_SECS && *recorded <= after + SLACK_SECS,
                            "case {case:?}: expected the served image set's last access inside \
                             [{before}, {after}], got {recorded}"
                        );
                        assert!(
                            *recorded > previous,
                            "case {case:?}: serving the content must move the last access past \
                             {previous}, got {recorded}"
                        );
                    } else {
                        assert_eq!(
                            *recorded, previous,
                            "case {case:?}: the read must not touch {key:?}, whose last access \
                             was {previous}, got {recorded}"
                        );
                    }
                }
            }
        }
    }

    // A read addressed to any other region reaches no data store at all, so it serves
    // nothing and records nothing — not on the image set it names, nor anywhere else.
    for reader in Reader::ALL {
        for region in REGIONS {
            for elsewhere in REGIONS.iter().filter(|other| **other != region) {
                let target = seeded[region][0].clone();
                let case = format!("{} addressed to {elsewhere} for {region}", reader.name());
                let before_instants = snapshot(&emulator, &seeded, &case).await;

                let refused = reader
                    .call_in(
                        &emulator,
                        elsewhere,
                        &target.datastore_id,
                        &target.image_set_id,
                        &target.frame_id,
                    )
                    .await;
                assert_eq!(
                    refused.status, 404,
                    "case {case:?}: expected HTTP 404, got {refused:?}"
                );
                assert_eq!(
                    refused.string(&["message"]),
                    DATASTORE_NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the DATA STORE to be the missing resource, got \
                     {refused:?}"
                );

                let after_instants = snapshot(&emulator, &seeded, &case).await;
                assert_eq!(
                    after_instants, before_instants,
                    "case {case:?}: a read that served nothing must record no access anywhere"
                );
            }
        }
    }
}
