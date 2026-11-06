// SPDX-License-Identifier: CC0-1.0

//! `SearchFacesByImage` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.SearchFacesByImage`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `SearchFacesByImage` measures the largest face of the submitted
//! image — the largest of the ones the `QualityFilter`'s bar leaves — and reports, out of
//! the collection the request named and no other, every stored face whose measured
//! resemblance to it reaches `FaceMatchThreshold` (80 by default), highest similarity
//! first, capped by `MaxFaces`, together with the box and confidence measured for the face
//! the search used and the collection's own face model version. An image carrying no such
//! face is refused with HTTP 400 `InvalidParameterException`, a collection the store does
//! not hold with HTTP 400 `ResourceNotFoundException`, and the search itself stores
//! nothing: the collection reads the same after it as before.
//!
//! Every image here is a PORTRAIT drawn by the test through the fixtures of the detection
//! suite, and every stored face got into its collection through `IndexFaces`, so each
//! assertion compares the service's answer against the pictures the test drew. The tables
//! span what the rule generalises over: how the searched image differs from the one that
//! was indexed (the same pixels, another scale, another exposure, another encoder, another
//! face), how it is submitted (inline, from an S3 object), how many faces the image and the
//! collection carry, the threshold across its published range and past both of its bounds,
//! the cap, each `QualityFilter` level, and which collection is named.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper and the reduced reply come from the
// collection-lifecycle suite, and the drawn portraits and the object-store harness from the
// detection suite, rather than being built a second time here.
use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{
    blurred, encode, harness_with_object_store, number, s3, DrawnFace, Portrait, BUCKET,
};

/// The threshold `SearchFacesByImage` applies when the request names none: "the default
/// value is 80%" (`smithy_query(query="doc",shape="SearchFacesByImageRequest",
/// member="FaceMatchThreshold")`).
const DEFAULT_THRESHOLD: f64 = 80.0;

/// `FaceMatchThreshold` is `Percent`, `range 0..100`, and `MaxFaces` is `range 1..4096`
/// (`smithy_query(query="operation",operation="SearchFacesByImage")`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;
const MAX_FACES_MIN: i64 = 1;
const MAX_FACES_MAX: i64 = 4096;

/// How near 100 the similarity of one face to ITSELF must be: the stored face and the
/// submitted one were measured off the same pixels, so their correlation is one but for the
/// rounding of the arithmetic that computes it.
const SAME_PIXELS_SIMILARITY: f64 = 99.5;

/// The face model version a collection created now is associated with, which every answer
/// about that collection reports.
const LATEST_FACE_MODEL_VERSION: &str = "7.0";

/// The `QualityFilter` levels, ordered by how high a bar each sets: `NONE` filters nothing
/// and `HIGH` the most (`smithy_query(query="doc",shape="SearchFacesByImageRequest",
/// member="QualityFilter")`: "if you specify LOW, MEDIUM, or HIGH, filtering removes all
/// faces that don't meet the chosen quality bar … if you specify NONE, no filtering is
/// performed"). `AUTO`, where "Amazon Rekognition chooses the quality bar", is not in the
/// order: nothing documents where its bar sits.
const QUALITY_LEVELS: [&str; 4] = ["NONE", "LOW", "MEDIUM", "HIGH"];

/// The upright portrait most cases index and search with.
fn upright() -> Portrait {
    Portrait::one_face()
}

/// The same face, tilted: a picture that differs from [`upright`] in what the resemblance
/// measurement reads and in nothing else.
fn tilted() -> Portrait {
    Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(12.0)])
}

/// Create `collection_id`, failing the test if the create does not answer.
async fn create_collection(router: &MultiServiceRouter, collection_id: &str) {
    let reply = call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "the collection {collection_id} must be created, got {:?}",
        reply.body
    );
}

/// Index `image` into `collection_id` and answer the `FaceId` of every face stored from it,
/// in the order the answer reports them.
async fn index(router: &MultiServiceRouter, collection_id: &str, image: &[u8]) -> Vec<String> {
    let reply = call(
        router,
        "IndexFaces",
        json!({ "CollectionId": collection_id, "Image": bytes_image(image) }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "indexing into {collection_id} must succeed, got {:?}",
        reply.body
    );
    reply
        .list("FaceRecords")
        .iter()
        .map(|record| {
            record
                .get("Face")
                .and_then(|face| face.get("FaceId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A collection holding exactly the faces of `image`, and the ids it stored for them.
async fn collection_of(
    router: &MultiServiceRouter,
    collection_id: &str,
    image: &[u8],
) -> Vec<String> {
    create_collection(router, collection_id).await;
    let ids = index(router, collection_id, image).await;
    assert!(
        !ids.is_empty(),
        "the portrait indexed into {collection_id} must carry a face"
    );
    ids
}

/// One search of `collection_id` by an image already in the `Image` form the request carries.
async fn search_image(
    router: &MultiServiceRouter,
    collection_id: &str,
    image: Value,
    extra: Value,
) -> Reply {
    let mut body = extra;
    let members = body
        .as_object_mut()
        .expect("the extra members are an object");
    members.insert("CollectionId".to_string(), json!(collection_id));
    members.insert("Image".to_string(), image);
    call(router, "SearchFacesByImage", body).await
}

/// One search by an image submitted inline.
async fn search(
    router: &MultiServiceRouter,
    collection_id: &str,
    image: &[u8],
    extra: Value,
) -> Reply {
    search_image(router, collection_id, bytes_image(image), extra).await
}

/// The matches of a served search, with what every answer must carry already checked.
fn served(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the search must be served, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("FaceModelVersion").as_deref(),
        Some(LATEST_FACE_MODEL_VERSION),
        "case {case:?}: the answer carries the collection's own face model version, got {:?}",
        reply.body
    );
    assert!(
        reply.field("SearchedFaceBoundingBox").is_some(),
        "case {case:?}: the box of the face the search used must be reported, got {:?}",
        reply.body
    );
    let confidence = number(&reply.body, &["SearchedFaceConfidence"]);
    assert!(
        (PERCENT_MIN..=PERCENT_MAX).contains(&confidence),
        "case {case:?}: the searched face's confidence is a Percent, got {confidence}"
    );
    let matches = reply.list("FaceMatches");
    for (index, entry) in matches.iter().enumerate() {
        let similarity = number(entry, &["Similarity"]);
        assert!(
            (PERCENT_MIN..=PERCENT_MAX).contains(&similarity),
            "case {case:?}: match {index} reports a Percent similarity, got {similarity}"
        );
        assert!(
            entry
                .get("Face")
                .and_then(|face| face.get("FaceId"))
                .and_then(Value::as_str)
                .is_some(),
            "case {case:?}: match {index} must report the stored face, got {entry:?}"
        );
    }
    matches
}

/// The `(FaceId, Similarity)` pairs of a served answer, in the order they were reported.
fn reported(matches: &[Value]) -> Vec<(String, f64)> {
    matches
        .iter()
        .map(|entry| {
            (
                entry
                    .get("Face")
                    .and_then(|face| face.get("FaceId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                number(entry, &["Similarity"]),
            )
        })
        .collect()
}

/// A refusal, by status, wire code and message.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Code").as_deref(),
        Some(code),
        "case {case:?}: expected Code {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Message").as_deref(),
        Some(message),
        "case {case:?}: expected Message {message:?}, got {:?}",
        reply.body
    );
}

/// The face stored from one picture is matched by a later picture of that same face,
/// whatever changed between the two that the resemblance measurement divides out — the
/// scale of the face in its frame, the exposure, the encoder — and the same pixels match it
/// at the top of the scale.
#[tokio::test]
async fn the_submitted_face_matches_the_face_indexed_from_it() {
    let router = harness();
    let indexed = upright().png();
    let stored = collection_of(&router, "enrolled-upright", &indexed).await;

    let cases: Vec<(&str, Vec<u8>, f64)> = vec![
        ("the same pixels again", indexed.clone(), SAME_PIXELS_SIMILARITY),
        (
            // Twice the eye span in a frame twice as wide: the same face, twice as
            // large in the image.
            "the same face at twice the scale",
            Portrait::one_face()
                .with_size(640, 640)
                .with_faces(vec![DrawnFace::new(320.0, 300.0, 112.0)])
                .png(),
            DEFAULT_THRESHOLD,
        ),
        (
            "the same face at another exposure",
            Portrait::one_face().with_tones(95, 135, 55).png(),
            DEFAULT_THRESHOLD,
        ),
        (
            "the same face through a lossy encoder",
            encode(&upright().render(), image::ImageFormat::Jpeg),
            DEFAULT_THRESHOLD,
        ),
    ];
    for (case, submitted, floor) in cases {
        let matches = served(case, &search(&router, "enrolled-upright", &submitted, json!({})).await);
        let found = reported(&matches);
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: the collection holds one face, which this picture of it must \
             match at the default threshold, got {found:?}"
        );
        assert_eq!(
            found[0].0, stored[0],
            "case {case:?}: the match must name the stored face, got {found:?}"
        );
        assert!(
            found[0].1 >= floor,
            "case {case:?}: expected a similarity of at least {floor}, got {:?}",
            found[0].1
        );
    }
}

/// A picture that is LESS alike the stored face than the stored face's own picture scores
/// below it, and a threshold set between the two scores reports the one and not the other:
/// the measurement ranks resemblance, so it does not answer every search with every stored
/// face.
#[tokio::test]
async fn a_less_alike_face_ranks_below_and_a_threshold_between_them_excludes_it() {
    let router = harness();
    let own = upright().png();
    let stored = collection_of(&router, "enrolled-contrast", &own).await;

    let similarity_of = |submitted: Vec<u8>, case: &'static str| {
        let router = &router;
        let stored = stored.clone();
        async move {
            let matches = served(
                case,
                &search(
                    router,
                    "enrolled-contrast",
                    &submitted,
                    json!({ "FaceMatchThreshold": PERCENT_MIN }),
                )
                .await,
            );
            let found = reported(&matches);
            assert_eq!(
                found.len(),
                1,
                "case {case:?}: at the bottom of the scale the one stored face is reported, \
                 got {found:?}"
            );
            assert_eq!(
                found[0].0, stored[0],
                "case {case:?}: the match must name the stored face, got {found:?}"
            );
            found[0].1
        }
    };

    let own_score = similarity_of(own.clone(), "the stored face's own picture").await;
    let cases: Vec<(&'static str, Vec<u8>)> = vec![
        ("the same face tilted", tilted().png()),
        (
            "the same face tilted the other way",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(-12.0)])
                .png(),
        ),
        (
            "a face drawn with a wider mouth and closer eyes",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 34.0).rolled(16.0)])
                .with_tones(60, 150, 20)
                .png(),
        ),
    ];
    for (case, submitted) in cases {
        let score = similarity_of(submitted.clone(), case).await;
        assert!(
            score < own_score,
            "case {case:?}: a picture of another face must rank below the stored face's own \
             picture ({own_score}), got {score}"
        );
        // A threshold strictly between the two scores: the less alike face is no longer a
        // match, while the stored face's own picture still is.
        let between = (score + own_score) / 2.0;
        let excluded = served(
            case,
            &search(
                &router,
                "enrolled-contrast",
                &submitted,
                json!({ "FaceMatchThreshold": between }),
            )
            .await,
        );
        assert!(
            excluded.is_empty(),
            "case {case:?}: at a threshold of {between} the stored face is not a match, got \
             {:?}",
            reported(&excluded)
        );
        let kept = served(
            case,
            &search(
                &router,
                "enrolled-contrast",
                &own,
                json!({ "FaceMatchThreshold": between }),
            )
            .await,
        );
        assert_eq!(
            reported(&kept)
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            stored,
            "case {case:?}: at a threshold of {between} the stored face's own picture still \
             matches it"
        );
    }
}

/// "The response returns an array of faces that match, ordered by similarity score with the
/// highest similarity first", and `MaxFaces` "returns the maximum number of faces with the
/// highest confidence in the match" — so the cap keeps the strongest matches of that order.
#[tokio::test]
async fn matches_are_ranked_and_capped() {
    let router = harness();
    let searched = upright().png();
    let stored_upright = collection_of(&router, "ranked", &searched).await;
    let stored_tilted = index(&router, "ranked", &tilted().png()).await;
    let stored_small = index(
        &router,
        "ranked",
        &Portrait::one_face()
            .with_faces(vec![DrawnFace::new(160.0, 150.0, 30.0).rolled(-9.0)])
            .png(),
    )
    .await;
    let held = stored_upright.len() + stored_tilted.len() + stored_small.len();
    assert_eq!(held, 3, "the collection must hold the three drawn faces");

    let case = "three stored faces, searched by the first one's own picture";
    let matches = served(
        case,
        &search(
            &router,
            "ranked",
            &searched,
            json!({ "FaceMatchThreshold": PERCENT_MIN }),
        )
        .await,
    );
    let found = reported(&matches);
    assert_eq!(
        found.len(),
        held,
        "case {case:?}: at the bottom of the scale every stored face is a match, got {found:?}"
    );
    for pair in found.windows(2) {
        assert!(
            pair[0].1 >= pair[1].1,
            "case {case:?}: matches come back highest similarity first, got {found:?}"
        );
    }
    assert_eq!(
        found[0].0, stored_upright[0],
        "case {case:?}: the face indexed from these same pixels must rank first, got {found:?}"
    );
    assert!(
        found[0].1 >= SAME_PIXELS_SIMILARITY,
        "case {case:?}: the same pixels correlate at the top of the scale, got {:?}",
        found[0].1
    );

    // The cap, across its published range: it keeps the head of that same order.
    for cap in [MAX_FACES_MIN, 2, 3, 4, MAX_FACES_MAX] {
        let case = format!("MaxFaces {cap}");
        let capped = served(
            &case,
            &search(
                &router,
                "ranked",
                &searched,
                json!({ "FaceMatchThreshold": PERCENT_MIN, "MaxFaces": cap }),
            )
            .await,
        );
        let wanted = usize::try_from(cap).expect("the cap is positive").min(held);
        assert_eq!(
            reported(&capped),
            found[..wanted].to_vec(),
            "case {case:?}: the cap keeps the first {wanted} of the ranked matches"
        );
    }

    // One past each published bound: the modelled range refuses it before the handler runs.
    for (case, cap) in [
        ("below the bottom of the MaxFaces range", MAX_FACES_MIN - 1),
        ("above the top of the MaxFaces range", MAX_FACES_MAX + 1),
    ] {
        let reply = search(&router, "ranked", &searched, json!({ "MaxFaces": cap })).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a cap outside MaxFaces is refused, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: the modelled range refuses it at the boundary, got {:?}",
            reply.body
        );
    }
}

/// The threshold is the only thing that decides which measured faces are reported: no
/// reported match is below it, and raising it never adds one.
#[tokio::test]
async fn the_threshold_decides_which_stored_faces_are_matches() {
    let router = harness();
    let searched = upright().png();
    create_collection(&router, "thresholds").await;
    let mut held = index(&router, "thresholds", &searched).await.len();
    held += index(&router, "thresholds", &tilted().png()).await.len();
    assert_eq!(held, 2, "the collection must hold both drawn faces");

    let thresholds: Vec<(String, Option<f64>)> = vec![
        ("the bottom of the range".to_string(), Some(PERCENT_MIN)),
        ("a quarter".to_string(), Some(25.0)),
        ("a half".to_string(), Some(50.0)),
        ("the default, named".to_string(), Some(DEFAULT_THRESHOLD)),
        ("the default".to_string(), None),
        ("almost the top".to_string(), Some(99.0)),
        ("the top of the range".to_string(), Some(PERCENT_MAX)),
    ];
    let mut previous: Option<(String, usize)> = None;
    for (case, threshold) in thresholds {
        let extra = match threshold {
            Some(value) => json!({ "FaceMatchThreshold": value }),
            None => json!({}),
        };
        let matches = served(&case, &search(&router, "thresholds", &searched, extra).await);
        let floor = threshold.unwrap_or(DEFAULT_THRESHOLD);
        for (index, (_, similarity)) in reported(&matches).iter().enumerate() {
            assert!(
                *similarity >= floor,
                "case {case:?}: match {index} is below the threshold {floor}, got {similarity}"
            );
        }
        assert!(
            matches.len() <= held,
            "case {case:?}: no more matches than the collection holds, got {}",
            matches.len()
        );
        if let Some((lower_case, lower_count)) = previous.replace((case.clone(), matches.len())) {
            assert!(
                matches.len() <= lower_count,
                "case {case:?}: raising the threshold from {lower_case:?} cannot add a match, \
                 got {} after {lower_count}",
                matches.len()
            );
        }
    }

    // One past each published bound: the modelled range refuses it before the handler runs.
    for (case, value) in [
        ("below the bottom of the Percent range", PERCENT_MIN - 0.5),
        ("above the top of the Percent range", PERCENT_MAX + 0.5),
    ] {
        let reply = search(
            &router,
            "thresholds",
            &searched,
            json!({ "FaceMatchThreshold": value }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a threshold outside Percent is refused, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: the modelled range refuses it at the boundary, got {:?}",
            reply.body
        );
    }
}

/// "First detects the largest face in the image": which face of a crowded image the search
/// used is reported, as the box and confidence measured for it, and it is the largest one
/// `DetectFaces` measures in that same image.
#[tokio::test]
async fn the_largest_face_of_the_submitted_image_is_the_one_searched() {
    let router = harness();
    let large = upright().png();
    let stored_large = collection_of(&router, "crowded", &large).await;
    let stored_tilted = index(&router, "crowded", &tilted().png()).await;
    assert_eq!(
        stored_large.len() + stored_tilted.len(),
        2,
        "the collection must hold both drawn faces"
    );

    // One face drawn at nearly twice the eye span of the other, and the smaller one tilted,
    // so which of them the search used shows in the answer.
    let crowd = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(150.0, 170.0, 72.0),
            DrawnFace::new(390.0, 120.0, 38.0).rolled(12.0),
        ])
        .png();
    let detected = call(
        &router,
        "DetectFaces",
        json!({ "Image": bytes_image(&crowd) }),
    )
    .await;
    let faces = detected.list("FaceDetails");
    assert_eq!(
        faces.len(),
        2,
        "the crowded image must carry both drawn faces, got {:?}",
        detected.body
    );
    let widest = faces
        .iter()
        .max_by(|left, right| {
            number(left, &["BoundingBox", "Width"]).total_cmp(&number(
                right,
                &["BoundingBox", "Width"],
            ))
        })
        .expect("the image carries a face")
        .clone();

    let case = "a large and a small face in the submitted image";
    let reply = search(&router, "crowded", &crowd, json!({})).await;
    served(case, &reply);
    for member in ["Left", "Top", "Width", "Height"] {
        let searched = number(&reply.body, &["SearchedFaceBoundingBox", member]);
        let measured = number(&widest, &["BoundingBox", member]);
        assert!(
            (searched - measured).abs() < 1e-6,
            "case {case:?}: the searched face's box {member} must be the largest detected \
             face's, expected {measured}, got {searched}"
        );
    }
    let searched_confidence = number(&reply.body, &["SearchedFaceConfidence"]);
    let measured_confidence = number(&widest, &["Confidence"]);
    assert!(
        (searched_confidence - measured_confidence).abs() < 1e-6,
        "case {case:?}: the searched face's confidence must be the one measured for that \
         face, expected {measured_confidence}, got {searched_confidence}"
    );
    let found = reported(&served(case, &reply));
    assert_eq!(
        found.first().map(|(id, _)| id.clone()),
        Some(stored_large[0].clone()),
        "case {case:?}: the larger face is the one searched, so the face indexed from that \
         same picture ranks first, got {found:?}"
    );
}

/// "Filtered faces aren't searched for in the collection": raising the bar can only take
/// faces out of the search, and an image whose every face it removes is left with none,
/// which is refused like an image that carried none.
#[tokio::test]
async fn the_quality_filter_only_narrows_what_is_searched() {
    let router = harness();
    let sharp = upright();
    collection_of(&router, "quality", &sharp.png()).await;
    // The same picture, softened more and more: blur takes the contrast out of the edges
    // the sharpness measurement reads without moving the face.
    let softened: Vec<(usize, Vec<u8>)> = [0, 2, 5]
        .into_iter()
        .map(|passes| {
            (
                passes,
                encode(&blurred(&sharp.render(), passes), image::ImageFormat::Png),
            )
        })
        .collect();
    for (passes, submitted) in &softened {
        let mut previous: Option<(&str, usize)> = None;
        for level in QUALITY_LEVELS {
            let case = format!("{passes} blur passes at QualityFilter {level}");
            let reply = search(
                &router,
                "quality",
                submitted,
                json!({ "QualityFilter": level, "FaceMatchThreshold": PERCENT_MIN }),
            )
            .await;
            let count = if reply.status == 200 {
                served(&case, &reply).len()
            } else {
                // The only other answer is the refusal for an image left with no face to
                // search with.
                assert_refused(
                    &case,
                    &reply,
                    400,
                    "InvalidParameterException",
                    "Request has invalid parameters",
                );
                0
            };
            if let Some((lower, lower_count)) = previous.replace((level, count)) {
                assert!(
                    count <= lower_count,
                    "case {case:?}: raising the bar from {lower} cannot add a match, got \
                     {count} after {lower_count}"
                );
            }
        }
    }
}

/// Only the collection the request NAMED is searched, and a collection the store does not
/// hold is answered as not found.
#[tokio::test]
async fn only_the_named_collection_is_searched() {
    let router = harness();
    let searched = upright().png();
    let mine = collection_of(&router, "mine", &searched).await;
    let theirs = collection_of(&router, "theirs", &searched).await;
    assert_ne!(
        mine, theirs,
        "two collections mint their own ids for the same picture"
    );
    for (case, collection, expected) in [
        ("the first collection", "mine", &mine),
        ("the second collection", "theirs", &theirs),
    ] {
        let matches = served(
            case,
            &search(
                &router,
                collection,
                &searched,
                json!({ "FaceMatchThreshold": PERCENT_MIN }),
            )
            .await,
        );
        assert_eq!(
            reported(&matches)
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            *expected,
            "case {case:?}: a search reports the named collection's faces and no other's"
        );
    }

    for case in ["never-created", "mine-2", "MINE"] {
        let reply = search(&router, case, &searched, json!({})).await;
        assert_refused(
            case,
            &reply,
            400,
            "ResourceNotFoundException",
            &format!("The collection id: {case} does not exist"),
        );
    }

    // And once the collection is deleted, its id names nothing either.
    let deleted = call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "theirs" }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the collection must be deleted, got {:?}",
        deleted.body
    );
    assert_refused(
        "a deleted collection",
        &search(&router, "theirs", &searched, json!({})).await,
        400,
        "ResourceNotFoundException",
        "The collection id: theirs does not exist",
    );
}

/// "If no faces are detected in the input image, SearchFacesByImage returns an
/// InvalidParameterException error", and an image that cannot be read at all is refused with
/// the reason it could not be.
#[tokio::test]
async fn an_image_the_search_cannot_use_is_refused_with_the_reason() {
    let router = harness();
    collection_of(&router, "refusals", &upright().png()).await;

    let faceless: Vec<(&str, Vec<u8>)> = vec![
        ("a flat image", Portrait::blank(240, 240, 200).png()),
        ("a dark image", Portrait::blank(320, 200, 20).png()),
        (
            "a featureless head",
            Portrait::one_face().with_tones(110, 190, 190).png(),
        ),
    ];
    for (case, submitted) in faceless {
        assert_refused(
            case,
            &search(&router, "refusals", &submitted, json!({})).await,
            400,
            "InvalidParameterException",
            "Request has invalid parameters",
        );
    }

    let unreadable: Vec<(&str, Value, &str, &str)> = vec![
        (
            "bytes that are not an image",
            bytes_image(b"not an image at all"),
            "InvalidImageFormatException",
            "Request has invalid image format",
        ),
        (
            "an image identifying no content",
            json!({}),
            "InvalidParameterException",
            "Requested image should either contain bytes or s3 object.",
        ),
        (
            "an S3 object the store does not serve",
            json!({ "S3Object": { "Bucket": "absent-bucket", "Name": "absent.png" } }),
            "InvalidS3ObjectException",
            "Unable to get object metadata from S3. Check object key, region and/or access \
             permissions.",
        ),
    ];
    for (case, image, code, message) in unreadable {
        assert_refused(
            case,
            &search_image(&router, "refusals", image, json!({})).await,
            400,
            code,
            message,
        );
    }
}

/// An image read out of the emulator's own Amazon S3 is searched exactly like the same bytes
/// submitted inline.
#[tokio::test]
async fn an_s3_sourced_image_searches_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    let searched = upright().png();
    collection_of(&router, "s3-sourced", &searched).await;
    for key in ["searched.png", "nested/searched.png"] {
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), &searched).await,
            200,
            "the {key} object must be stored"
        );
    }
    let inline = search(&router, "s3-sourced", &searched, json!({})).await;
    served("the image inline", &inline);
    for key in ["searched.png", "nested/searched.png"] {
        let case = format!("the image from S3 at {key}");
        let reply = search_image(
            &router,
            "s3-sourced",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": key } }),
            json!({}),
        )
        .await;
        served(&case, &reply);
        assert_eq!(
            reply.body, inline.body,
            "case {case:?}: an S3-sourced image must search like the same bytes inline"
        );
    }
}

/// A search REPORTS the collection: it stores nothing, so every later read of the
/// collection, and the search itself, answers exactly as it did before.
#[tokio::test]
async fn searching_by_image_changes_nothing() {
    let router = harness();
    let searched = upright().png();
    collection_of(&router, "unchanged", &searched).await;

    let before = read_collection(&router).await;
    let first = search(&router, "unchanged", &searched, json!({})).await;
    served("the first search", &first);
    // Every form of the call: one that matches, one refused for its image, one refused for
    // its collection. None of them may leave a trace.
    served(
        "a search of another picture",
        &search(&router, "unchanged", &tilted().png(), json!({})).await,
    );
    let _ = search(&router, "unchanged", &Portrait::blank(64, 64, 120).png(), json!({})).await;
    let _ = search(&router, "never-created", &searched, json!({})).await;
    let second = search(&router, "unchanged", &searched, json!({})).await;

    assert_eq!(
        first.body, second.body,
        "the same search answers identically while nothing else changed the collection"
    );
    assert_eq!(
        read_collection(&router).await,
        before,
        "a search must leave the collection exactly as it found it"
    );
}

/// Everything a later caller can read about the collection the search-persists-nothing case
/// uses: what it reports about itself and the faces it holds.
async fn read_collection(router: &MultiServiceRouter) -> (Value, Value) {
    let described = call(
        router,
        "DescribeCollection",
        json!({ "CollectionId": "unchanged" }),
    )
    .await;
    let listed = call(router, "ListFaces", json!({ "CollectionId": "unchanged" })).await;
    (described.body, listed.body)
}
