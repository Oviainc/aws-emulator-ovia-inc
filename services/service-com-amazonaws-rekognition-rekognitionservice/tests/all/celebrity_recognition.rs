// SPDX-License-Identifier: CC0-1.0

//! `RecognizeCelebrities` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.RecognizeCelebrities`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `RecognizeCelebrities` PARTITIONS the faces measured in the
//! submitted image — "it lists the recognized celebrities in the CelebrityFaces array and
//! any unrecognized faces in the UnrecognizedFaces array" — so every face the detector
//! measures in the image is reported on one side or the other and none is left out of the
//! answer. The faces reported are the faces `DetectFaces` reports for the same pixels, with
//! the members that detector measured (a box, a confidence, landmarks, a pose and a
//! quality) and with every member a classifier would have had to judge absent; no face is
//! reported as a celebrity, because nothing in this process holds the identity of a face;
//! an image carrying no face is answered with both lists empty rather than refused; the
//! orientation correction the member documents as always null is absent; an image this
//! service cannot read at all is refused with the modelled error for why; and nothing is
//! persisted, so a later caller sees exactly what it would have seen without the call.
//!
//! Every portrait here is DRAWN from known tones and distances by the fixtures of the
//! detection suite, so each assertion compares the service's answer against the picture the
//! test drew rather than against a recorded answer. The tables span what the rule
//! generalises over: how many faces an image carries (none, one, two of different sizes,
//! three), where in the frame they sit, their size, tilt and tone, the frame's size, the
//! encoded format, how the image is submitted (inline, from an S3 object), and how an image
//! fails to be readable at all.

use image::ImageFormat;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper and the reduced reply come from the
// collection-lifecycle suite, and the drawn portraits and the object-store harness from the
// detection suite, rather than being built a second time here.
use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{
    encode, harness_with_object_store, number, s3, DrawnFace, Portrait, BUCKET,
};

/// Every member of a `ComparedFace` a classifier would have had to judge, which is nothing
/// this emulator runs (`smithy_query(query="shape",shape="ComparedFace")` lists the shape's
/// members; these two are the judged ones).
const UNJUDGED_MEMBERS: [&str; 2] = ["Emotions", "Smile"];

/// The members of a reported face the detector MEASURES, which are the members a
/// `ComparedFace` and a `FaceDetail` both carry
/// (`smithy_query(query="shape",shape="ComparedFace,FaceDetail")`).
const MEASURED_MEMBERS: [&str; 5] = ["BoundingBox", "Confidence", "Landmarks", "Pose", "Quality"];

/// One `RecognizeCelebrities` call on an image already in the `Image` form a request
/// carries.
async fn recognize(router: &MultiServiceRouter, image: Value) -> Reply {
    call(router, "RecognizeCelebrities", json!({ "Image": image })).await
}

/// One `DetectFaces` call: the faces the detector reports for the same pixels, through the
/// operation whose whole job is reporting them.
async fn detect(router: &MultiServiceRouter, image: Value) -> Reply {
    call(router, "DetectFaces", json!({ "Image": image })).await
}

/// The `UnrecognizedFaces` of a served answer, with what every answer must and must not
/// carry already checked: no celebrity named, no orientation correction, and on each
/// reported face the measured members and none of the judged ones.
fn served(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the recognition must be served, got {} with {:?}",
        reply.status, reply.body
    );
    // "Any returned values for this field included in an API response will always be NULL."
    assert!(
        reply.field("OrientationCorrection").is_none(),
        "case {case:?}: the orientation correction is always null, got {:?}",
        reply.body
    );
    // The celebrity side of the partition is EMPTY and present: nothing here holds the
    // identity of a face, so no celebrity is named — and the list is still answered.
    let celebrities = reply.field("CelebrityFaces");
    assert!(
        celebrities.is_some(),
        "case {case:?}: the answer carries the celebrity list, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.list("CelebrityFaces"),
        Vec::<Value>::new(),
        "case {case:?}: no face can be recognized as a celebrity here, got {:?}",
        reply.body
    );
    let faces = reply.list("UnrecognizedFaces");
    for face in &faces {
        for member in MEASURED_MEMBERS {
            assert!(
                face.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: a reported face carries the measured {member}, got {face:?}"
            );
        }
        for member in UNJUDGED_MEMBERS {
            assert!(
                face.get(member).is_none_or(Value::is_null),
                "case {case:?}: no classifier ran, so {member} must be absent, got {face:?}"
            );
        }
        let confidence = number(face, &["Confidence"]);
        assert!(
            (0.0..=100.0).contains(&confidence),
            "case {case:?}: a confidence is a percentage, got {confidence}"
        );
    }
    faces
}

/// Whether the reported box of `face` holds the point the test drew a face's eyes at, in
/// an image of `size`. The box carries fractions of the image's own width and height.
fn box_holds(face: &Value, size: (u32, u32), eyes: (f32, f32)) -> bool {
    let left = number(face, &["BoundingBox", "Left"]);
    let top = number(face, &["BoundingBox", "Top"]);
    let width = number(face, &["BoundingBox", "Width"]);
    let height = number(face, &["BoundingBox", "Height"]);
    let x = f64::from(eyes.0) / f64::from(size.0);
    let y = f64::from(eyes.1) / f64::from(size.1);
    (left..=left + width).contains(&x) && (top..=top + height).contains(&y)
}

/// One row of the portrait table: the picture, the eye centres the test drew on it, and how
/// many faces it carries.
struct PortraitCase {
    case: &'static str,
    portrait: Portrait,
    eyes: Vec<(f32, f32)>,
}

/// The portraits every table below draws from: none, one, two or three faces, at several
/// positions, sizes, tilts, tones and frame sizes. Each row's face count is what the
/// detection suite pins for the same picture.
fn portrait_cases() -> Vec<PortraitCase> {
    vec![
        PortraitCase {
            case: "a flat light image with no face at all",
            portrait: Portrait::blank(320, 320, 235),
            eyes: Vec::new(),
        },
        PortraitCase {
            case: "a flat dark image with no face at all",
            portrait: Portrait::blank(240, 240, 12),
            eyes: Vec::new(),
        },
        PortraitCase {
            case: "one centred face",
            portrait: Portrait::one_face(),
            eyes: vec![(160.0, 150.0)],
        },
        PortraitCase {
            case: "two faces of different sizes",
            portrait: Portrait::one_face().with_size(480, 320).with_faces(vec![
                DrawnFace::new(130.0, 150.0, 56.0),
                DrawnFace::new(350.0, 140.0, 36.0),
            ]),
            eyes: vec![(130.0, 150.0), (350.0, 140.0)],
        },
        PortraitCase {
            case: "three faces across a wide frame",
            portrait: Portrait::one_face().with_size(640, 480).with_faces(vec![
                DrawnFace::new(150.0, 140.0, 56.0),
                DrawnFace::new(470.0, 140.0, 56.0),
                DrawnFace::new(310.0, 360.0, 56.0),
            ]),
            eyes: vec![(150.0, 140.0), (470.0, 140.0), (310.0, 360.0)],
        },
        PortraitCase {
            case: "a face tilted clockwise",
            portrait: Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(10.0)]),
            eyes: vec![(160.0, 150.0)],
        },
        PortraitCase {
            case: "a face tilted the other way",
            portrait: Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(-10.0)]),
            eyes: vec![(160.0, 150.0)],
        },
        PortraitCase {
            case: "a small face in a large frame",
            portrait: Portrait::one_face()
                .with_size(640, 480)
                .with_faces(vec![DrawnFace::new(300.0, 220.0, 56.0)]),
            eyes: vec![(300.0, 220.0)],
        },
        PortraitCase {
            case: "a close-up face larger than the frame",
            portrait: Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 140.0, 110.0)]),
            eyes: vec![(160.0, 140.0)],
        },
        PortraitCase {
            case: "bright skin on a dark background",
            portrait: Portrait::one_face().with_tones(60, 220, 20),
            eyes: vec![(160.0, 150.0)],
        },
        PortraitCase {
            case: "a dim portrait",
            portrait: Portrait::one_face().with_tones(20, 90, 10),
            eyes: vec![(160.0, 150.0)],
        },
    ]
}

/// The partition covers every face the image carries: each drawn face is reported, and it
/// is reported as an UNRECOGNIZED face, since nothing here recognizes anyone.
#[tokio::test]
async fn every_face_the_image_carries_is_reported_as_an_unrecognized_face() {
    let router = harness();
    for PortraitCase {
        case,
        portrait,
        eyes,
    } in portrait_cases()
    {
        let faces = served(case, &recognize(&router, bytes_image(&portrait.png())).await);
        assert_eq!(
            faces.len(),
            eyes.len(),
            "case {case:?}: the {} drawn face(s) must each be reported once, got {faces:?}",
            eyes.len()
        );
        let size = (portrait.width, portrait.height);
        for drawn in &eyes {
            assert!(
                faces.iter().any(|face| box_holds(face, size, *drawn)),
                "case {case:?}: a reported box must hold the eyes drawn at {drawn:?}, \
                 got {faces:?}"
            );
        }
    }
}

/// A reported face is the face the DETECTOR measured, member for member: the same pixels
/// answered through `DetectFaces` report the same boxes, confidences, landmarks, poses and
/// qualities, in the same order. The table spans the frame's size and the encoded format,
/// which the rule generalises over.
#[tokio::test]
async fn a_reported_face_is_the_face_the_detector_measured() {
    let router = harness();
    // One drawn face per side and format, at the smallest eye span the detection suite pins
    // as found at that scale.
    let scales: [(&str, u32, f32); 3] = [
        ("160 px", 160, 28.0),
        ("320 px", 320, 56.0),
        ("640 px", 640, 112.0),
    ];
    let formats: [(&str, ImageFormat); 2] = [("PNG", ImageFormat::Png), ("JPEG", ImageFormat::Jpeg)];
    for (scale, side, eye_span) in scales {
        for (format_name, format) in formats {
            let case = format!("{scale}, {format_name}");
            let centre = f32::from(u16::try_from(side).expect("the test's sides are small"));
            let portrait = Portrait::one_face()
                .with_size(side, side)
                .with_faces(vec![DrawnFace::new(centre / 2.0, centre * 0.47, eye_span)]);
            let bytes = encode(&portrait.render(), format);

            let detected = detect(&router, bytes_image(&bytes)).await;
            assert_eq!(
                detected.status, 200,
                "case {case:?}: detecting must succeed, got {:?}",
                detected.body
            );
            let details = detected.list("FaceDetails");
            let faces = served(&case, &recognize(&router, bytes_image(&bytes)).await);
            assert_eq!(
                faces.len(),
                details.len(),
                "case {case:?}: the partition covers the {} detected face(s), got {faces:?}",
                details.len()
            );
            assert_eq!(
                faces.len(),
                1,
                "case {case:?}: one drawn face must be reported once, got {faces:?}"
            );
            for (index, (face, detail)) in faces.iter().zip(details.iter()).enumerate() {
                for member in MEASURED_MEMBERS {
                    assert_eq!(
                        face.get(member),
                        detail.get(member),
                        "case {case:?}: face {index}'s {member} is what the detector \
                         measured, got {face:?} against {detail:?}"
                    );
                }
            }
        }
    }
}

/// One image answers the same whichever way it was submitted: inline bytes and the object
/// this emulator's Amazon S3 holds at the named location are read alike.
#[tokio::test]
async fn an_s3_sourced_image_is_recognized_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    for PortraitCase {
        case,
        portrait,
        eyes,
    } in portrait_cases()
    {
        for key in ["portrait.png", "nested/prefix/portrait.png"] {
            let bytes = portrait.png();
            assert_eq!(
                s3(&router, &format!("/{BUCKET}/{key}"), &bytes).await,
                200,
                "case {case:?}: the image must be staged at {key}"
            );
            let staged = recognize(
                &router,
                json!({ "S3Object": { "Bucket": BUCKET, "Name": key } }),
            )
            .await;
            let inline = recognize(&router, bytes_image(&bytes)).await;
            let staged_faces = served(case, &staged);
            assert_eq!(
                staged_faces.len(),
                eyes.len(),
                "case {case:?}: the staged image carries {} face(s), got {staged_faces:?}",
                eyes.len()
            );
            assert_eq!(
                staged.body, inline.body,
                "case {case:?}: the object at {key} must answer exactly as the same bytes \
                 submitted inline, got {:?} against {:?}",
                staged.body, inline.body
            );
        }
    }
}

/// The call is a READ of the submitted pixels: it persists nothing, so a second identical
/// call answers identically and every stored resource is exactly as it was.
#[tokio::test]
async fn recognizing_celebrities_changes_nothing_a_later_caller_sees() {
    let router = harness();
    let collection = "celebrity-recognition";
    let created = call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": collection }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the collection must be created, got {:?}",
        created.body
    );
    let portrait = Portrait::one_face();
    let bytes = portrait.png();
    let indexed = call(
        &router,
        "IndexFaces",
        json!({ "CollectionId": collection, "Image": bytes_image(&bytes) }),
    )
    .await;
    assert_eq!(
        indexed.status, 200,
        "the face must be indexed, got {:?}",
        indexed.body
    );
    let before = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": collection }),
    )
    .await;

    let first = recognize(&router, bytes_image(&bytes)).await;
    let second = recognize(&router, bytes_image(&bytes)).await;
    assert_eq!(served("first call", &first).len(), 1, "the face is reported");
    assert_eq!(
        first.body, second.body,
        "case {:?}: two identical calls answer identically, got {:?} against {:?}",
        "repeated call", first.body, second.body
    );

    let after = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": collection }),
    )
    .await;
    assert_eq!(
        before.body, after.body,
        "case {:?}: the stored collection is untouched by the call, got {:?} against {:?}",
        "described collection", after.body, before.body
    );
    let listed = call(&router, "ListCollections", json!({})).await;
    assert_eq!(
        listed.list("CollectionIds"),
        vec![Value::from(collection)],
        "case {:?}: no resource is created by the call, got {:?}",
        "listed collections",
        listed.body
    );
}

/// An image this service cannot read at all is refused with the modelled error for WHY,
/// never with an empty answer: a 200 carrying no face is reserved for an image that was
/// read and carried none.
#[tokio::test]
async fn an_unreadable_image_is_refused_with_the_modelled_error_for_why() {
    let router = harness();
    // Each row: what the request identifies as its image, and the modelled answer for it.
    // The messages are the service's own fixed wording for each class.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "bytes that are not an image at all",
            bytes_image(b"not-an-image-at-all"),
            "InvalidImageFormatException",
            "Request has invalid image format",
        ),
        (
            // The service reads PNG and JPEG — "the image must be either a PNG or JPEG
            // formatted file" — and the format comes from the bytes' own magic numbers, so
            // a GIF signature is content in a format it does not read.
            "bytes in an image format this service does not read",
            bytes_image(b"GIF89a\x01\x00\x01\x00\x80\x00\x00"),
            "InvalidImageFormatException",
            "Request has invalid image format",
        ),
        (
            "an image identifying no content at all",
            json!({}),
            "InvalidParameterException",
            "Requested image should either contain bytes or s3 object.",
        ),
        (
            "an S3 object the store does not serve",
            json!({ "S3Object": { "Bucket": "no-such-bucket", "Name": "no-such-key" } }),
            "InvalidS3ObjectException",
            "Unable to get object metadata from S3. Check object key, region and/or access \
             permissions.",
        ),
        (
            "an S3 object with no key",
            json!({ "S3Object": { "Bucket": "no-such-bucket" } }),
            "InvalidS3ObjectException",
            "Unable to get object metadata from S3. Check object key, region and/or access \
             permissions.",
        ),
    ];

    for (case, image, expected_code, expected_message) in cases {
        let reply = recognize(&router, image).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            expected_code,
            "case {case:?}: expected __type {expected_code}, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Code").as_deref(),
            Some(expected_code),
            "case {case:?}: expected Code {expected_code}, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Message").as_deref(),
            Some(expected_message),
            "case {case:?}: expected Message {expected_message:?}, got {:?}",
            reply.body
        );
        assert!(
            reply.field("UnrecognizedFaces").is_none() && reply.field("CelebrityFaces").is_none(),
            "case {case:?}: a refusal carries no face list, got {:?}",
            reply.body
        );
    }
}
