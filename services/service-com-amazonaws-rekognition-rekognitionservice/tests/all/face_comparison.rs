// SPDX-License-Identifier: CC0-1.0

//! `CompareFaces` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.CompareFaces`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `CompareFaces` measures the faces of both submitted images and
//! reports, for EVERY face measured in the target image, how alike it and the largest face
//! of the source image look — as a `Similarity` on the 0..100 scale that is 100 for the
//! same pixels and that a change of exposure or of how large the face is in its image does
//! not move. A target face whose similarity reaches the `SimilarityThreshold` (80 by
//! default) is reported as a match and every other one as an unmatched face, so the two
//! lists together always hold every face the target image carries; matches come back in
//! descending order of similarity; the source face is reported with the box and confidence
//! measured for it; a member a classifier would have had to judge and the two orientation
//! corrections are absent; an image left with no comparable face — because it carries none
//! or because `QualityFilter` removed the faces it did carry — is refused with HTTP 400
//! `InvalidParameterException`; and nothing is persisted.
//!
//! Every portrait here is DRAWN from known tones and distances by the fixtures of the
//! detection suite, so each assertion compares the service's answer against the picture the
//! test drew rather than against a recorded answer. The tables span what the rule
//! generalises over: how many faces each image carries, where in the frame they sit, their
//! size, tilt and tone, the frame's size, the encoded format, how each image is submitted
//! (inline, from an S3 object), the threshold across its published range and past both of
//! its bounds, each `QualityFilter` level, and how an image fails to be readable at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper and the reduced reply come from the
// collection-lifecycle suite, and the drawn portraits and the object-store harness from the
// detection suite, rather than being built a second time here.
use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{
    blurred, encode, harness_with_object_store, number, s3, DrawnFace, Portrait, BUCKET,
};

/// The threshold `CompareFaces` applies when the request names none: "by default, only
/// faces with a similarity score of greater than or equal to 80% are returned in the
/// response" (`smithy_query(query="doc",shape="CompareFaces")`).
const DEFAULT_THRESHOLD: f64 = 80.0;

/// `SimilarityThreshold` is `Percent`, `range 0..100`
/// (`smithy_query(query="operation",operation="CompareFaces")`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// How near 100 the similarity of one face to ITSELF must be: the two patches are measured
/// off the same pixels, so their correlation is one but for the rounding of the arithmetic
/// that computes it.
const SAME_PIXELS_SIMILARITY: f64 = 99.5;

/// Every member of a `ComparedFace` a classifier would have had to judge, which is nothing
/// this emulator runs (`smithy_query(query="shape",shape="ComparedFace")` lists the shape's
/// members; these two are the judged ones).
const UNJUDGED_MEMBERS: [&str; 2] = ["Emotions", "Smile"];

/// The `QualityFilter` levels, ordered by how high a bar each sets: `NONE` filters nothing
/// and `HIGH` the most (`smithy_query(query="doc",shape="CompareFacesRequest",
/// member="QualityFilter")`: "if you specify LOW, MEDIUM, or HIGH, filtering removes all
/// faces that don't meet the chosen quality bar … if you specify NONE, no filtering is
/// performed"). `AUTO`, where "Amazon Rekognition chooses the quality bar", is not in the
/// order: nothing documents where its bar sits.
const QUALITY_LEVELS: [&str; 4] = ["NONE", "LOW", "MEDIUM", "HIGH"];

/// One comparison of two images, each already in the `Image` form the request carries.
async fn compare_images(
    router: &MultiServiceRouter,
    source: Value,
    target: Value,
    extra: Value,
) -> Reply {
    let mut body = extra;
    let members = body
        .as_object_mut()
        .expect("the extra members are an object");
    members.insert("SourceImage".to_string(), source);
    members.insert("TargetImage".to_string(), target);
    call(router, "CompareFaces", body).await
}

/// One comparison of two images submitted inline.
async fn compare(
    router: &MultiServiceRouter,
    source: &[u8],
    target: &[u8],
    extra: Value,
) -> Reply {
    compare_images(router, bytes_image(source), bytes_image(target), extra).await
}

/// The two face lists of a served comparison, with what every answer must and must not
/// carry already checked.
fn served(case: &str, reply: &Reply) -> (Vec<Value>, Vec<Value>) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the comparison must be served, got {} with {:?}",
        reply.status, reply.body
    );
    for member in [
        "SourceImageOrientationCorrection",
        "TargetImageOrientationCorrection",
    ] {
        assert!(
            reply.field(member).is_none(),
            "case {case:?}: {member} is always null, got {:?}",
            reply.body
        );
    }
    let matches = reply.list("FaceMatches");
    let unmatched = reply.list("UnmatchedFaces");
    let source_face = reply
        .field("SourceImageFace")
        .unwrap_or_else(|| panic!("case {case:?}: SourceImageFace must be reported, got {:?}", reply.body))
        .clone();
    for member in ["BoundingBox", "Confidence"] {
        assert!(
            source_face.get(member).is_some(),
            "case {case:?}: the source face carries the {member} measured for it, got \
             {source_face:?}"
        );
    }
    for face in matches
        .iter()
        .map(|entry| {
            entry.get("Face").unwrap_or_else(|| {
                panic!("case {case:?}: a match reports the face it matched, got {entry:?}")
            })
        })
        .chain(unmatched.iter())
    {
        for member in UNJUDGED_MEMBERS {
            assert!(
                face.get(member).is_none_or(Value::is_null),
                "case {case:?}: no classifier ran, so {member} must be absent, got {face:?}"
            );
        }
        let pose = face.get("Pose").unwrap_or(&Value::Null);
        for member in ["Yaw", "Pitch"] {
            assert!(
                pose.get(member).is_none_or(Value::is_null),
                "case {case:?}: two eye positions establish no {member}, got {pose:?}"
            );
        }
        for member in ["BoundingBox", "Confidence", "Landmarks", "Quality"] {
            assert!(
                face.get(member).is_some(),
                "case {case:?}: a compared face carries the {member} measured for it, got \
                 {face:?}"
            );
        }
    }
    (matches, unmatched)
}

/// The similarities of a served answer's matches, in the order they were reported.
fn similarities(matches: &[Value]) -> Vec<f64> {
    matches
        .iter()
        .map(|entry| number(entry, &["Similarity"]))
        .collect()
}

/// The reported box of a face or of the source face, as `(left, top, width, height)`.
fn box_of(case: &str, face: &Value) -> (f64, f64, f64, f64) {
    let found = face
        .get("BoundingBox")
        .unwrap_or_else(|| panic!("case {case:?}: BoundingBox must be present, got {face:?}"));
    (
        number(found, &["Left"]),
        number(found, &["Top"]),
        number(found, &["Width"]),
        number(found, &["Height"]),
    )
}

/// How many faces `DetectFaces` measures in an image: what grounds a case that is about
/// WHICH of an image's faces the comparison used.
async fn drawn_faces(router: &MultiServiceRouter, bytes: &[u8]) -> usize {
    let reply = call(router, "DetectFaces", json!({ "Image": bytes_image(bytes) })).await;
    assert_eq!(
        reply.status, 200,
        "the portrait must be readable, got {:?}",
        reply.body
    );
    reply.list("FaceDetails").len()
}

/// The same pixels on both sides correlate perfectly, whatever the picture is: one match,
/// at the top of the scale, for the one face each portrait carries — and the face reported
/// as the source face is the same face, measured the same way.
#[tokio::test]
async fn an_image_matches_itself_perfectly() {
    let router = harness();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("a centred face", Portrait::one_face().png()),
        (
            "a face near the frame's edge",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(70.0, 95.0, 40.0)])
                .png(),
        ),
        (
            "a small face",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 26.0)])
                .png(),
        ),
        (
            "a tilted face",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(12.0)])
                .png(),
        ),
        (
            "a face in dark tones",
            Portrait::one_face().with_tones(40, 120, 10).png(),
        ),
        (
            "a face on a wide frame",
            Portrait::one_face().with_size(480, 320).png(),
        ),
        (
            "a face encoded as JPEG",
            encode(&Portrait::one_face().render(), image::ImageFormat::Jpeg),
        ),
    ];
    for (case, bytes) in cases {
        let reply = compare(&router, &bytes, &bytes, json!({})).await;
        let (matches, unmatched) = served(case, &reply);
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: the one drawn face must match itself, got {matches:?}"
        );
        assert!(
            unmatched.is_empty(),
            "case {case:?}: the image carries no other face, got {unmatched:?}"
        );
        let similarity = similarities(&matches)[0];
        assert!(
            (SAME_PIXELS_SIMILARITY..=PERCENT_MAX).contains(&similarity),
            "case {case:?}: the same pixels must score at least {SAME_PIXELS_SIMILARITY} and \
             no more than {PERCENT_MAX}, got {similarity}"
        );
        let source_face = reply.field("SourceImageFace").expect("checked above").clone();
        assert_eq!(
            box_of(case, &matches[0]["Face"]),
            box_of(case, &source_face),
            "case {case:?}: one image on both sides reports one face, measured once"
        );
    }
}

/// The measurement is of the face and not of the file: the same drawn face photographed
/// larger, at another exposure or through another encoder still matches at the default
/// threshold.
#[tokio::test]
async fn the_same_face_matches_across_scale_exposure_and_format() {
    let router = harness();
    let source = Portrait::one_face();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            // Every distance of the 320x320 original doubled: the same face, twice as
            // large in a frame twice as wide.
            "the same face at twice the scale",
            Portrait::one_face()
                .with_size(640, 640)
                .with_faces(vec![DrawnFace::new(320.0, 300.0, 112.0)])
                .png(),
        ),
        (
            // An affine change of every tone (halved, then lifted by 40), which is what a
            // change of exposure is: a normalised appearance divides it out.
            "the same face at another exposure",
            Portrait::one_face().with_tones(95, 135, 55).png(),
        ),
        (
            "the same face through a lossy encoder",
            encode(&Portrait::one_face().render(), image::ImageFormat::Jpeg),
        ),
    ];
    for (case, target) in cases {
        let (matches, unmatched) =
            served(case, &compare(&router, &source.png(), &target, json!({})).await);
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: the same face must still match, got {matches:?} and \
             unmatched {unmatched:?}"
        );
        let similarity = similarities(&matches)[0];
        assert!(
            similarity >= DEFAULT_THRESHOLD,
            "case {case:?}: the default threshold is {DEFAULT_THRESHOLD}, got {similarity}"
        );
    }
}

/// The comparison RANKS the target's faces: a face laid out like the source's scores above
/// one that is not, and the matches come back in that order.
#[tokio::test]
async fn resemblance_ranks_the_faces_of_the_target_image() {
    let router = harness();
    let source = Portrait::one_face().png();
    // Two faces of the same size in one frame: one drawn upright like the source's, one
    // tilted, so the two differ in what the comparison measures and in nothing else.
    let target = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(130.0, 150.0, 56.0),
            DrawnFace::new(350.0, 150.0, 56.0).rolled(12.0),
        ])
        .png();
    let case = "an upright and a tilted face in one target image";
    assert_eq!(
        drawn_faces(&router, &target).await,
        2,
        "case {case:?}: the target image must carry both drawn faces"
    );
    // Every face compared, so the ranking is over both of them.
    let (matches, unmatched) = served(
        case,
        &compare(
            &router,
            &source,
            &target,
            json!({ "SimilarityThreshold": PERCENT_MIN }),
        )
        .await,
    );
    assert_eq!(
        matches.len(),
        2,
        "case {case:?}: at the bottom of the scale every face is a match, got {matches:?} \
         and unmatched {unmatched:?}"
    );
    let scores = similarities(&matches);
    assert!(
        scores[0] > scores[1],
        "case {case:?}: matches are ordered by similarity in descending order, got {scores:?}"
    );
    let (left, _, _, _) = box_of(case, &matches[0]["Face"]);
    assert!(
        left < 0.5,
        "case {case:?}: the upright face, drawn in the left half of the frame, must rank \
         above the tilted one, got the box left {left} first with {scores:?}"
    );
}

/// The threshold is the only thing that decides which side of the answer a measured face
/// is reported on: every face is reported either way, no match is below the threshold, and
/// raising the threshold never adds one.
#[tokio::test]
async fn the_threshold_decides_which_measured_faces_are_matches() {
    let router = harness();
    let source = Portrait::one_face().png();
    let target = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(130.0, 150.0, 56.0),
            DrawnFace::new(350.0, 150.0, 56.0).rolled(12.0),
        ])
        .png();
    let faces = drawn_faces(&router, &target).await;
    assert_eq!(faces, 2, "the target image must carry both drawn faces");

    // The published range, both its bounds, and the default the request leaves out.
    let thresholds: Vec<(String, Option<f64>)> = vec![
        ("the default".to_string(), None),
        ("the bottom of the range".to_string(), Some(PERCENT_MIN)),
        ("a quarter".to_string(), Some(25.0)),
        ("a half".to_string(), Some(50.0)),
        ("the default, named".to_string(), Some(DEFAULT_THRESHOLD)),
        ("almost the top".to_string(), Some(99.0)),
        ("the top of the range".to_string(), Some(PERCENT_MAX)),
    ];
    let mut previous: Option<(String, usize)> = None;
    for (case, threshold) in thresholds {
        let extra = match threshold {
            Some(value) => json!({ "SimilarityThreshold": value }),
            None => json!({}),
        };
        let (matches, unmatched) = served(&case, &compare(&router, &source, &target, extra).await);
        assert_eq!(
            matches.len() + unmatched.len(),
            faces,
            "case {case:?}: every measured target face is reported on one side or the other, \
             got {} matches and {} unmatched",
            matches.len(),
            unmatched.len()
        );
        let floor = threshold.unwrap_or(DEFAULT_THRESHOLD);
        for (index, similarity) in similarities(&matches).iter().enumerate() {
            assert!(
                *similarity >= floor,
                "case {case:?}: match {index} is below the threshold {floor}, got {similarity}"
            );
        }
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
        ("below the bottom of the range", PERCENT_MIN - 0.5),
        ("above the top of the range", PERCENT_MAX + 0.5),
    ] {
        let reply = compare(
            &router,
            &source,
            &target,
            json!({ "SimilarityThreshold": value }),
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

/// "If the source image contains multiple faces, the service detects the largest face and
/// compares it with each face detected in the target image."
#[tokio::test]
async fn the_largest_face_of_the_source_image_is_the_one_compared() {
    let router = harness();
    // One face drawn at more than twice the eye span of the other, so which of them was
    // measured is readable off the box reported for the source face.
    let large_span = 88.0_f64;
    let small_span = 36.0_f64;
    let width = 480.0_f64;
    let source = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(150.0, 170.0, large_span as f32),
            DrawnFace::new(390.0, 120.0, small_span as f32),
        ])
        .png();
    let case = "a large and a small face in the source image";
    assert_eq!(
        drawn_faces(&router, &source).await,
        2,
        "case {case:?}: the source image must carry both drawn faces"
    );
    let target = Portrait::one_face().png();
    let reply = compare(&router, &source, &target, json!({})).await;
    served(case, &reply);
    let source_face = reply.field("SourceImageFace").expect("checked above").clone();
    let (_, _, box_width, _) = box_of(case, &source_face);
    // A box derived from an eye span is proportional to it, so the halfway point between
    // the two drawn spans separates the two readings whatever the proportion is.
    let reported_span = box_width * width;
    assert!(
        reported_span > large_span.midpoint(small_span),
        "case {case:?}: the face compared must be the one drawn {large_span} wide, not the \
         one drawn {small_span} wide, got a box {reported_span} pixels across"
    );
}

/// An image with no face in it is refused, whichever of the two it is: "if no faces are
/// detected in the source or target images, CompareFaces returns an
/// InvalidParameterException error".
#[tokio::test]
async fn an_image_with_no_face_is_refused() {
    let router = harness();
    let face = Portrait::one_face().png();
    let flat = Portrait::blank(240, 240, 200).png();
    let dark = Portrait::blank(320, 200, 20).png();
    let featureless = Portrait::one_face()
        .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0)])
        .with_tones(110, 190, 190)
        .png();
    let cases: Vec<(&str, &Vec<u8>, &Vec<u8>)> = vec![
        ("a flat source image", &flat, &face),
        ("a flat target image", &face, &flat),
        ("neither image carrying a face", &flat, &dark),
        ("a source image whose head has no features", &featureless, &face),
        ("a target image whose head has no features", &face, &featureless),
    ];
    for (case, source, target) in cases {
        let reply = compare(&router, source, target, json!({})).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an image with no face to compare is a client error, got {} with \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected __type InvalidParameterException, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Code").as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: the body names the class in its own Code member, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Message").as_deref(),
            Some("Request has invalid parameters"),
            "case {case:?}: the refusal carries the wording for the class, got {:?}",
            reply.body
        );
    }
}

/// An image this service cannot read at all is refused with the modelled error for WHY,
/// never with an empty answer that would read as "no face here" — and the same whichever of
/// the two images it is.
#[tokio::test]
async fn an_unreadable_image_is_refused_with_the_reason() {
    let router = harness();
    let png = Portrait::one_face().png();
    let readable = bytes_image(&png);
    let unreadable: Vec<(&str, Value, &str)> = vec![
        (
            "bytes that are no image",
            bytes_image(b"this is not an image at all"),
            "InvalidImageFormatException",
        ),
        (
            "a truncated PNG",
            bytes_image(&png[..png.len() / 3]),
            "InvalidImageFormatException",
        ),
        (
            "an image identifying no content",
            json!({}),
            "InvalidParameterException",
        ),
        (
            "an S3 object the store does not serve",
            json!({ "S3Object": { "Bucket": "no-such-bucket", "Name": "no-such-key" } }),
            "InvalidS3ObjectException",
        ),
    ];
    for (reason, image, expected) in unreadable {
        for (side, source, target) in [
            ("as the source image", image.clone(), readable.clone()),
            ("as the target image", readable.clone(), image.clone()),
        ] {
            let case = format!("{reason} {side}");
            let reply = compare_images(&router, source, target, json!({})).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: an unreadable image is a client error, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                expected,
                "case {case:?}: expected {expected}, got {:?}",
                reply.body
            );
            assert!(
                reply
                    .string("Message")
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the refusal must say why, got {:?}",
                reply.body
            );
        }
    }
}

/// `QualityFilter` only ever NARROWS what is compared: a higher bar leaves no more faces
/// compared than a lower one, and a bar that removes every face of an image leaves nothing
/// to compare, which is the same refusal as an image with no face in it.
#[tokio::test]
async fn the_quality_filter_only_narrows_what_is_compared() {
    let router = harness();
    let sharp = Portrait::one_face();
    let source = sharp.png();
    // The same picture, softened more and more: blur takes the contrast out of the edges
    // the sharpness measurement reads without moving the face.
    let targets: Vec<(usize, Vec<u8>)> = [0, 2, 5, 9]
        .into_iter()
        .map(|passes| {
            (
                passes,
                encode(&blurred(&sharp.render(), passes), image::ImageFormat::Png),
            )
        })
        .collect();
    let mut filtered_somewhere = false;
    for (passes, target) in &targets {
        let mut compared: Vec<(&str, usize)> = Vec::new();
        for level in QUALITY_LEVELS {
            let case = format!("{passes} blur passes at the {level} bar");
            let reply = compare(
                &router,
                &source,
                target,
                json!({ "QualityFilter": level, "SimilarityThreshold": PERCENT_MIN }),
            )
            .await;
            let count = if reply.status == 200 {
                let (matches, unmatched) = served(&case, &reply);
                matches.len() + unmatched.len()
            } else {
                // The bar removed every face of one of the images, so there was nothing
                // left to compare.
                assert_eq!(
                    reply.error_code(),
                    "InvalidParameterException",
                    "case {case:?}: a filtered-out image leaves no face to compare, got {:?}",
                    reply.body
                );
                0
            };
            if let Some((lower_level, lower_count)) = compared.last() {
                assert!(
                    count <= *lower_count,
                    "case {case:?}: a higher bar cannot compare more faces than the \
                     {lower_level} bar did, got {count} after {lower_count}"
                );
                filtered_somewhere |= count < *lower_count;
            }
            compared.push((level, count));
        }
        assert!(
            compared[0].1 >= 1,
            "case {passes} blur passes: NONE filters nothing, so every face the picture \
             carries is compared, got {compared:?}"
        );
    }
    assert!(
        filtered_somewhere,
        "a bar above NONE must filter a face out of at least one of the softened pictures, \
         or the filter decides nothing"
    );

    // "If you specify AUTO, Amazon Rekognition chooses the quality bar": whatever it
    // chooses, it is one of the bars, so it compares no more than NONE does.
    let soft = &targets[targets.len() - 1].1;
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for level in ["NONE", "AUTO"] {
        let case = format!("the softest picture at the {level} bar");
        let reply = compare(
            &router,
            &source,
            soft,
            json!({ "QualityFilter": level, "SimilarityThreshold": PERCENT_MIN }),
        )
        .await;
        let count = if reply.status == 200 {
            let (matches, unmatched) = served(&case, &reply);
            matches.len() + unmatched.len()
        } else {
            0
        };
        counts.push((level, count));
    }
    assert!(
        counts[1].1 <= counts[0].1,
        "AUTO chooses a quality bar, so it compares no more faces than NONE, got {counts:?}"
    );
}

/// An image staged in the emulator's own Amazon S3 compares exactly like the same bytes
/// submitted inline, on either side of the comparison.
#[tokio::test]
async fn an_s3_sourced_image_compares_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    let source = Portrait::one_face().png();
    let target = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(130.0, 150.0, 56.0),
            DrawnFace::new(350.0, 150.0, 56.0).rolled(12.0),
        ])
        .png();
    for (key, bytes) in [("source.png", &source), ("nested/target.png", &target)] {
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), bytes).await,
            200,
            "the {key} object must be stored"
        );
    }
    let object = |key: &str| json!({ "S3Object": { "Bucket": BUCKET, "Name": key } });
    let inline = compare_images(
        &router,
        bytes_image(&source),
        bytes_image(&target),
        json!({ "SimilarityThreshold": PERCENT_MIN }),
    )
    .await;
    served("both images inline", &inline);
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the source image from S3",
            object("source.png"),
            bytes_image(&target),
        ),
        (
            "the target image from S3",
            bytes_image(&source),
            object("nested/target.png"),
        ),
        (
            "both images from S3",
            object("source.png"),
            object("nested/target.png"),
        ),
    ];
    for (case, source_image, target_image) in cases {
        let reply = compare_images(
            &router,
            source_image,
            target_image,
            json!({ "SimilarityThreshold": PERCENT_MIN }),
        )
        .await;
        served(case, &reply);
        assert_eq!(
            reply.body, inline.body,
            "case {case:?}: an S3-sourced image must compare like the same bytes inline"
        );
    }
}

/// `CompareFaces` persists nothing: "this is a stateless API operation. That is, data
/// returned by this operation doesn't persist." The same call answers identically a second
/// time, and the collections this service does store are untouched by it.
#[tokio::test]
async fn compare_faces_persists_nothing() {
    let router = harness();
    let before = call(&router, "ListCollections", json!({})).await;
    let source = Portrait::one_face().png();
    let target = Portrait::one_face()
        .with_size(480, 320)
        .with_faces(vec![
            DrawnFace::new(130.0, 150.0, 56.0),
            DrawnFace::new(350.0, 150.0, 56.0).rolled(12.0),
        ])
        .png();
    let first = compare(
        &router,
        &source,
        &target,
        json!({ "SimilarityThreshold": PERCENT_MIN }),
    )
    .await;
    served("first call", &first);
    let second = compare(
        &router,
        &source,
        &target,
        json!({ "SimilarityThreshold": PERCENT_MIN }),
    )
    .await;
    served("second call", &second);
    assert_eq!(
        first.body, second.body,
        "a comparison of the same two images answers identically every time"
    );
    let after = call(&router, "ListCollections", json!({})).await;
    assert_eq!(
        after.body, before.body,
        "comparing two faces changes nothing a later caller can read"
    );
}
