// SPDX-License-Identifier: CC0-1.0

//! The collection-and-face lifecycle a client walks — create, enrol, read back, delete —
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Operation>`),
//! dispatched through the same [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULES UNDER TEST, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! * an operation on a collection AS A RESOURCE (`DescribeCollection`, `DeleteCollection`)
//!   that names a collection the store does not hold is refused with HTTP 400,
//!   `ResourceNotFoundException` and the `Message` `Collection:<CollectionId> not found!` —
//!   two pieces of fixed text around the request's own id, whatever the id's form and
//!   however it came to be absent (never created, or created, used and deleted);
//! * `IndexFaces` enrols one face per face the DETECTOR found in the submitted image, so an
//!   image carrying none enrols none: an empty `FaceRecords` and an empty `UnindexedFaces`,
//!   both present, with the collection's `FaceCount` left where it was;
//! * each enrolled `Face` carries the two members the detector measured for it — its
//!   `BoundingBox`, a rectangle of fractions of the image, and its `Confidence`, a `Percent`
//!   (`range 0..100`) high enough for the face to have been accepted — and a later read of
//!   the collection reports the same values;
//! * `DeleteFaces` reports in `DeletedFaces` the ids it removed and reserves
//!   `UnsuccessfulFaceDeletions` for faces the collection still holds, so a `FaceId` the
//!   collection never held is reported in NEITHER list, wherever it sits in the request.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body, and every image is DRAWN by the test, so an answer that was not computed from
//! the submitted pixels and the persisted collection cannot pass.

use image::ImageFormat;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper, the inline-image encoding and the drawn
// portraits all come from the suites that already own them, so one picture is drawn one way
// and one router is built one way whichever operation is exercised.
use crate::face_collections::{bytes_image, call, create_collection, harness, one_face_png, Reply};
use crate::face_detection::{encode, DrawnFace, Portrait};

/// `Percent` is `range 0..100` (`smithy_query(query="shape",shape="Percent")`), which is
/// what `Face.Confidence` targets.
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// How far past the image's far edge a box's side may be reported, to absorb the rounding
/// of the fractions it is expressed in.
const BOX_TOLERANCE: f64 = 1e-3;

/// The floor a face ACCEPTED for indexing clears: a detection the service enrolled is one
/// it is more sure of than not, so its measured confidence is above the midpoint of
/// `Percent` rather than merely inside it.
const INDEXED_FACE_CONFIDENCE_FLOOR: f64 = 50.0;

/// `CollectionId` is `len 1..255, pattern ^[a-zA-Z0-9_.\-]+$`
/// (`smithy_query(query="shape",shape="CollectionId")`), so the ids below span its bounds
/// and every character class it admits.
fn collection_id_forms() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model allows", "a".to_string()),
        ("a hyphenated id", "lifecycle-collection".to_string()),
        (
            "every character class the pattern admits",
            "My_Collection.2026-v2".to_string(),
        ),
        ("the longest id the model allows", "z9".repeat(127) + "y"),
    ]
}

/// A `FaceId` of the modelled form
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no collection of
/// this service ever held: `which` picks one, so a row can name several.
fn never_indexed_face_id(which: u8) -> String {
    let nibble = char::from_digit(u32::from(which % 10), 16).expect("a hex digit");
    format!(
        "{0}{0}{0}{0}{0}{0}{0}{0}-{0}{0}{0}{0}-4{0}{0}{0}-8{0}{0}{0}-{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}",
        nibble
    )
}

/// The collection every row works in, created through the helper the lifecycle suite owns
/// and asserted with this row's own label.
async fn seed_collection(router: &MultiServiceRouter, case: &str, collection_id: &str) {
    let reply = create_collection(router, collection_id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: creating {collection_id:?} must succeed, got {} {:?}",
        reply.status, reply.body
    );
}

async fn face_count(router: &MultiServiceRouter, case: &str, collection_id: &str) -> i64 {
    let described = call(
        router,
        "DescribeCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: describing {collection_id:?} must succeed, got {} {:?}",
        described.status, described.body
    );
    described
        .field("FaceCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a described collection reports its FaceCount, got {:?}",
                described.body
            )
        })
}

/// The `(FaceId, Confidence, BoundingBox)` of every face the collection lists.
async fn listed_faces(
    router: &MultiServiceRouter,
    case: &str,
    collection_id: &str,
) -> Vec<(String, Option<f64>, Option<Value>)> {
    let listed = call(
        router,
        "ListFaces",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: listing {collection_id:?} must succeed, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .list("Faces")
        .iter()
        .map(|face| {
            (
                face.get("FaceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                face.get("Confidence").and_then(Value::as_f64),
                face.get("BoundingBox").cloned(),
            )
        })
        .collect()
}

/// Assert that `reply` is the refusal a request naming an absent collection as a RESOURCE
/// gets: the status, the class in both places it is named, and AWS's wording for it.
fn assert_collection_not_found(case: &str, reply: &Reply, collection_id: &str) {
    let expected_message = format!("Collection:{collection_id} not found!");
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected __type ResourceNotFoundException, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Code").as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected Code ResourceNotFoundException, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Message").as_deref(),
        Some(expected_message.as_str()),
        "case {case:?}: expected Message {expected_message:?}, got {:?}",
        reply.body
    );
}

/// The `Face` of a `FaceRecord`.
fn face_of(case: &str, record: &Value) -> Value {
    record
        .get("Face")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a FaceRecord carries its Face, got {record:?}"))
}

/// Assert that an enrolled face carries what the detector measured for it: a box of
/// fractions of the image with an area, and a confidence a face accepted for indexing
/// clears. Answers the `(FaceId, Confidence, BoundingBox)` it read.
fn assert_measured(case: &str, face: &Value) -> (String, f64, Value) {
    let face_id = face
        .get("FaceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: an enrolled face carries its id, got {face:?}"))
        .to_string();
    let confidence = face
        .get("Confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: an enrolled face carries its Confidence, got {face:?}")
        });
    assert!(
        (PERCENT_MIN..=PERCENT_MAX).contains(&confidence),
        "case {case:?}: Confidence is a Percent in {PERCENT_MIN}..={PERCENT_MAX}, got \
         {confidence}"
    );
    assert!(
        confidence > INDEXED_FACE_CONFIDENCE_FLOOR,
        "case {case:?}: a face accepted for indexing scores above \
         {INDEXED_FACE_CONFIDENCE_FLOOR}, got {confidence}"
    );
    let rect = face
        .get("BoundingBox")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: an enrolled face carries a box, got {face:?}"));
    let side = |member: &str| -> f64 {
        rect.get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: the box carries {member}, got {rect:?}"))
    };
    let (left, top, width, height) = (side("Left"), side("Top"), side("Width"), side("Height"));
    assert!(
        width > 0.0 && height > 0.0,
        "case {case:?}: the box covers a region of the image, got {width}x{height} at \
         ({left}, {top})"
    );
    // A `BoundingBox` is expressed in fractions of the image, so the whole rectangle lies
    // inside it: no corner outside, and no side running past the far edge.
    assert!(
        left >= 0.0
            && top >= 0.0
            && left + width <= 1.0 + BOX_TOLERANCE
            && top + height <= 1.0 + BOX_TOLERANCE,
        "case {case:?}: the box lies inside the image, got {width}x{height} at ({left}, {top})"
    );
    (face_id, confidence, rect)
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// A collection the store does not hold is refused, by both operations on the collection as
/// a resource, with AWS's own wording around the request's own id — however the id is formed
/// and however it came to be absent.
#[tokio::test]
async fn an_absent_collection_is_refused_with_the_documented_wording() {
    let router = harness();

    // The two ways a collection comes to be absent: it was never created, or it was created,
    // used and deleted. The wording is the same either way.
    for (form, collection_id) in collection_id_forms() {
        for lived in [false, true] {
            let case = format!(
                "{form}, {}",
                if lived {
                    "created, enrolled into and deleted"
                } else {
                    "never created"
                }
            );
            if lived {
                seed_collection(&router, &case, &collection_id).await;
                let indexed = call(
                    &router,
                    "IndexFaces",
                    json!({
                        "CollectionId": collection_id,
                        "Image": bytes_image(&one_face_png(7)),
                    }),
                )
                .await;
                assert_eq!(
                    indexed.list("FaceRecords").len(),
                    1,
                    "case {case:?}: the portrait's one face must be enrolled, got {:?}",
                    indexed.body
                );
                assert_eq!(
                    face_count(&router, &case, &collection_id).await,
                    1,
                    "case {case:?}: the enrolled face must be held"
                );
                let deleted = call(
                    &router,
                    "DeleteCollection",
                    json!({ "CollectionId": collection_id }),
                )
                .await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: deleting a collection that exists must succeed, got {:?}",
                    deleted.body
                );
            }

            for operation in ["DescribeCollection", "DeleteCollection"] {
                let case = format!("{operation} after {case}");
                let reply = call(
                    &router,
                    operation,
                    json!({ "CollectionId": collection_id }),
                )
                .await;
                assert_collection_not_found(&case, &reply, &collection_id);
            }

            // And the id names nothing in the list either, so the refusal agrees with the
            // store rather than being a wording applied to a live resource.
            let listed = call(&router, "ListCollections", json!({})).await;
            assert!(
                !listed
                    .list("CollectionIds")
                    .iter()
                    .any(|id| id.as_str() == Some(collection_id.as_str())),
                "case {case:?}: the absent id must not be listed, got {:?}",
                listed.body
            );
        }
    }
}

/// An image the detector finds no face in enrols nothing: both lists are present and empty
/// and the collection's `FaceCount` stays where it was.
#[tokio::test]
async fn an_image_carrying_no_face_enrols_nothing() {
    let router = harness();

    // Rows differ along the frame, the tone it is filled with and the format it is encoded
    // in: a flat frame carries no face structure whatever its size, brightness or codec.
    let cases: Vec<(&str, Portrait, ImageFormat)> = vec![
        (
            "a mid-grey square",
            Portrait::blank(240, 240, 128),
            ImageFormat::Png,
        ),
        (
            "a white square",
            Portrait::blank(240, 240, 255),
            ImageFormat::Png,
        ),
        (
            "a black square, as a JPEG",
            Portrait::blank(240, 240, 0),
            ImageFormat::Jpeg,
        ),
        (
            "a small light frame",
            Portrait::blank(64, 64, 235),
            ImageFormat::Png,
        ),
        (
            "a wide dim frame, as a JPEG",
            Portrait::blank(480, 120, 20),
            ImageFormat::Jpeg,
        ),
        (
            "a tall mid-tone frame",
            Portrait::blank(120, 480, 160),
            ImageFormat::Png,
        ),
        (
            // A head drawn in one tone: there is a shape in the frame, but no pair of
            // features on it for a face to be measured from.
            "a featureless head",
            Portrait::one_face().with_tones(110, 190, 190),
            ImageFormat::Png,
        ),
    ];

    for (row, (case, picture, format)) in cases.into_iter().enumerate() {
        let collection_id = format!("no-face-{row}");
        seed_collection(&router, case, &collection_id).await;
        let bytes = encode(&picture.render(), format);
        // Twice, so a second submission of the same contentless image cannot accumulate
        // anything either.
        for attempt in 0..2 {
            let case = format!("{case}, attempt {attempt}");
            let reply = call(
                &router,
                "IndexFaces",
                json!({
                    "CollectionId": collection_id,
                    "Image": bytes_image(&bytes),
                }),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a readable image is indexed, got {} {:?}",
                reply.status, reply.body
            );
            for member in ["FaceRecords", "UnindexedFaces"] {
                assert!(
                    reply.field(member).is_some(),
                    "case {case:?}: {member} is always reported, got {:?}",
                    reply.body
                );
                assert!(
                    reply.list(member).is_empty(),
                    "case {case:?}: no face was detected, so {member} is empty, got {:?}",
                    reply.list(member)
                );
            }
            assert_eq!(
                face_count(&router, &case, &collection_id).await,
                0,
                "case {case:?}: nothing was enrolled, so the collection holds nothing"
            );
            assert!(
                listed_faces(&router, &case, &collection_id).await.is_empty(),
                "case {case:?}: nothing was enrolled, so the collection lists nothing"
            );
        }
    }
}

/// Every enrolled face carries the box and the confidence it was detected with, and a later
/// read of the collection reports the same values.
#[tokio::test]
async fn an_enrolled_face_carries_its_box_and_confidence() {
    let router = harness();

    // Rows differ along how many faces the image holds and how large they are in the frame:
    // the members are measured per face, so each enrolled face carries its own.
    let cases: Vec<(&str, Portrait, usize)> = vec![
        ("one centred face", Portrait::one_face(), 1),
        (
            "a close-up face",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 140.0, 110.0)]),
            1,
        ),
        (
            "a small face in a large frame",
            Portrait::one_face()
                .with_size(640, 480)
                .with_faces(vec![DrawnFace::new(300.0, 220.0, 56.0)]),
            1,
        ),
        (
            "two faces of different sizes",
            Portrait::one_face().with_size(480, 320).with_faces(vec![
                DrawnFace::new(130.0, 150.0, 56.0),
                DrawnFace::new(350.0, 140.0, 36.0),
            ]),
            2,
        ),
        (
            "a dimly lit face",
            Portrait::one_face().with_tones(40, 120, 10),
            1,
        ),
    ];

    for (row, (case, picture, drawn)) in cases.into_iter().enumerate() {
        let collection_id = format!("measured-{row}");
        seed_collection(&router, case, &collection_id).await;
        let reply = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": collection_id,
                "Image": bytes_image(&picture.png()),
            }),
        )
        .await;
        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            drawn,
            "case {case:?}: the picture holds {drawn} face(s), so that many are enrolled, got \
             {:?}",
            reply.body
        );
        let mut measured = Vec::new();
        for record in &records {
            let face = face_of(case, record);
            measured.push(assert_measured(case, &face));
        }

        // What was measured is what the collection now reports for the same face.
        let listed = listed_faces(&router, case, &collection_id).await;
        assert_eq!(
            listed.len(),
            drawn,
            "case {case:?}: the collection lists the enrolled faces, got {listed:?}"
        );
        for (face_id, confidence, rect) in measured {
            let held = listed
                .iter()
                .find(|(id, _, _)| id == &face_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {face_id:?} must be listed, got {listed:?}")
                });
            assert_eq!(
                held.1,
                Some(confidence),
                "case {case:?}: {face_id:?} keeps the confidence it was measured with, got {:?}",
                held.1
            );
            assert_eq!(
                held.2.as_ref(),
                Some(&rect),
                "case {case:?}: {face_id:?} keeps the box it was measured with, got {:?}",
                held.2
            );
        }
    }
}

/// A `FaceId` the collection never held is nothing to delete: it is reported in neither
/// `DeletedFaces` nor `UnsuccessfulFaceDeletions`, wherever the request puts it.
#[tokio::test]
async fn delete_faces_ignores_ids_the_collection_never_held() {
    let router = harness();
    let collection_id = "mixed-deletes";
    seed_collection(&router, "the seeded collection", collection_id).await;

    let mut stored = Vec::new();
    for seed in 11..16u32 {
        let reply = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": collection_id,
                "Image": bytes_image(&one_face_png(seed)),
            }),
        )
        .await;
        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            1,
            "seed {seed}: the portrait's one face must be enrolled, got {:?}",
            reply.body
        );
        let case = format!("seed {seed}");
        let (face_id, _, _) = assert_measured(&case, &face_of(&case, &records[0]));
        stored.push(face_id);
    }
    let mut held = stored.len() as i64;

    // Rows differ along where the never-indexed ids sit in the request, how many of them
    // there are, and whether any stored id is named beside them — including the id of a face
    // an earlier row already removed, which is an id the collection no longer holds.
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "an unknown id before a stored one",
            vec![never_indexed_face_id(1), stored[0].clone()],
            vec![stored[0].clone()],
        ),
        (
            "an unknown id between two stored ones",
            vec![
                stored[1].clone(),
                never_indexed_face_id(2),
                stored[2].clone(),
            ],
            vec![stored[1].clone(), stored[2].clone()],
        ),
        (
            "unknown ids only",
            vec![never_indexed_face_id(3), never_indexed_face_id(4)],
            vec![],
        ),
        (
            "the same unknown id twice, beside a stored one",
            vec![
                never_indexed_face_id(5),
                stored[3].clone(),
                never_indexed_face_id(5),
            ],
            vec![stored[3].clone()],
        ),
        (
            "an id this collection held and already lost",
            vec![stored[0].clone(), stored[4].clone()],
            vec![stored[4].clone()],
        ),
    ];

    for (case, requested, expected_deleted) in cases {
        let reply = call(
            &router,
            "DeleteFaces",
            json!({ "CollectionId": collection_id, "FaceIds": requested }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the call is a success, got {} {:?}",
            reply.status, reply.body
        );
        let deleted: Vec<String> = reply
            .list("DeletedFaces")
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            deleted, expected_deleted,
            "case {case:?}: only the ids the collection held are reported deleted, got {:?}",
            reply.body
        );
        let unsuccessful: Vec<String> = reply
            .list("UnsuccessfulFaceDeletions")
            .iter()
            .map(|entry| {
                entry
                    .get("FaceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            reply.field("UnsuccessfulFaceDeletions").is_some(),
            "case {case:?}: the member is always reported, got {:?}",
            reply.body
        );
        assert!(
            unsuccessful.is_empty(),
            "case {case:?}: no face the collection still holds failed to delete, got \
             {unsuccessful:?}"
        );
        for id in &requested {
            if expected_deleted.contains(id) {
                continue;
            }
            assert!(
                !deleted.contains(id) && !unsuccessful.contains(id),
                "case {case:?}: {id:?} names no face the collection holds, so it belongs to \
                 neither list, got deleted {deleted:?} and unsuccessful {unsuccessful:?}"
            );
        }

        held -= expected_deleted.len() as i64;
        assert_eq!(
            face_count(&router, case, collection_id).await,
            held,
            "case {case:?}: FaceCount follows the removal"
        );
        let listed: Vec<String> = listed_faces(&router, case, collection_id)
            .await
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        for id in &expected_deleted {
            assert!(
                !listed.contains(id),
                "case {case:?}: {id:?} was deleted, so it is no longer listed, got {listed:?}"
            );
        }
        assert_eq!(
            listed.len() as i64,
            held,
            "case {case:?}: the collection lists exactly the surviving faces, got {listed:?}"
        );
    }
}
