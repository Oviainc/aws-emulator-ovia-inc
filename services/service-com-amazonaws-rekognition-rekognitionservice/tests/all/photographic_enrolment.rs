// SPDX-License-Identifier: CC0-1.0

//! `IndexFaces` over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.IndexFaces`) on PHOTOGRAPH-like images, dispatched
//! through the same [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `IndexFaces` enrols and reports exactly ONE `FaceRecord` per face
//! the submitted image carries — "an array of faces detected and added to the collection" —
//! each with a `BoundingBox` over that face's own region of the image, so a picture of one
//! person enrols one face however much else the picture holds, a picture of two enrols two,
//! a picture of nobody enrols none, and the collection then reports exactly those faces
//! through `DescribeCollection.FaceCount` and `ListFaces`. One reading of a face is one
//! face: neither a second reading of the same person at another scale nor a region of the
//! picture that is not a face may be enrolled beside it.
//!
//! The suite the detection rule is pinned by (`face_detection`) draws its portraits as flat
//! tones — a head, two eyes, a mouth on an even background — which is the clean case. The
//! images here are the awkward one, and the one a client actually submits: a photograph,
//! whose every square of background carries grain and a gradient, and whose subject carries
//! hair, eyebrows, nostrils, lips, shading and clothing, i.e. many more dark features than
//! there are eyes. Each is DRAWN by the test from known distances and tones and encoded as a
//! real JPEG or PNG, so every assertion compares the service's answer against where the test
//! put the person rather than against a recorded answer.
//!
//! The table spans what the rule generalises over: how many people the picture holds (none,
//! one, two of different sizes), what else it holds (grain, a tonal gradient, hair with no
//! face turned to the camera, clothing), how large the subject is in the frame (a
//! head-and-shoulders portrait, a close crop, a small figure in a wide frame), the tones it
//! is lit in, and the format it is encoded in.

use std::sync::Arc;

use image::{DynamicImage, GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, Reply};

/// How far a reported box's side may sit from the drawn head's own, as a ratio: the box the
/// detector derives from the eye line is not the ellipse the test drew, so it is compared
/// loosely — but a box over something other than the head cannot be this close.
const BOX_SIDE_TOLERANCE: (f64, f64) = (0.55, 1.45);

/// The seed the grain of every picture below is drawn from, so a failure reproduces exactly.
const GRAIN_SEED: u64 = 0x5245_4b5f_5048_4f54;

/// The emulator, with this one service mounted on a shared engine.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

// ---------------------------------------------------------------------------
// The photographs the tests draw
// ---------------------------------------------------------------------------

/// One person in a drawn photograph: where the eyes are, how far apart, and whether the
/// face is turned to the camera at all (a subject photographed from behind shows hair and
/// no features).
#[derive(Debug, Clone, Copy)]
struct Subject {
    eyes_x: f32,
    eyes_y: f32,
    /// The distance between the two eyes, which every other distance below is a multiple
    /// of — the way a face's proportions are stated.
    eye_span: f32,
    facing: bool,
}

impl Subject {
    fn new(eyes_x: f32, eyes_y: f32, eye_span: f32) -> Self {
        Self {
            eyes_x,
            eyes_y,
            eye_span,
            facing: true,
        }
    }

    /// The same person photographed from behind: the hair and the shoulders are there, the
    /// face is not.
    fn turned_away(mut self) -> Self {
        self.facing = false;
        self
    }

    fn left_eye(&self) -> (f32, f32) {
        (self.eyes_x - self.eye_span / 2.0, self.eyes_y)
    }

    fn right_eye(&self) -> (f32, f32) {
        (self.eyes_x + self.eye_span / 2.0, self.eyes_y)
    }

    /// Where the mouth sits: about one interocular distance below the eye line.
    fn mouth_centre(&self) -> (f32, f32) {
        (self.eyes_x, self.eyes_y + 1.05 * self.eye_span)
    }

    /// The head: an ellipse a little wider than the eyes are apart, half again as tall,
    /// with the eye line above its middle.
    fn head(&self) -> (f32, f32, f32, f32) {
        (
            self.eyes_x,
            self.eyes_y + 0.25 * self.eye_span,
            1.0 * self.eye_span,
            1.4 * self.eye_span,
        )
    }
}

/// One drawn photograph: the frame, the light it was taken in and the people in it.
#[derive(Debug, Clone)]
struct Photograph {
    width: u32,
    height: u32,
    /// The background at the top and at the bottom of the frame: a photograph is never one
    /// flat tone, so the background runs between the two.
    sky: u8,
    floor: u8,
    skin: u8,
    /// How much the grain of the film perturbs every pixel, in grey levels.
    grain: u8,
    subjects: Vec<Subject>,
}

impl Photograph {
    /// A head-and-shoulders portrait: one person, centred, in an evenly lit frame. The base
    /// every case below varies from.
    fn portrait() -> Self {
        Self {
            width: 320,
            height: 320,
            sky: 96,
            floor: 132,
            skin: 186,
            grain: 7,
            subjects: vec![Subject::new(160.0, 128.0, 60.0)],
        }
    }

    fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    fn with_subjects(mut self, subjects: Vec<Subject>) -> Self {
        self.subjects = subjects;
        self
    }

    fn with_light(mut self, sky: u8, floor: u8, skin: u8) -> Self {
        self.sky = sky;
        self.floor = floor;
        self.skin = skin;
        self
    }

    fn with_grain(mut self, grain: u8) -> Self {
        self.grain = grain;
        self
    }

    /// The pixels of this photograph: the lit background, then each person from the back
    /// forwards — clothing, hair, head, and the features of a face turned to the camera —
    /// and the grain of the film over all of it.
    fn render(&self) -> GrayImage {
        let mut image = GrayImage::from_pixel(self.width, self.height, Luma([self.sky]));
        let height = self.height.max(1) as f32;
        for y in 0..self.height {
            let across = y as f32 / height;
            let tone = f32::from(self.sky) + (f32::from(self.floor) - f32::from(self.sky)) * across;
            for x in 0..self.width {
                image.put_pixel(x, y, Luma([tone.round().clamp(0.0, 255.0) as u8]));
            }
        }
        for subject in &self.subjects {
            self.draw_subject(&mut image, subject);
        }
        self.grain(&mut image);
        image
    }

    /// One person: the clothes they are wearing, their hair, their head, and — when the face
    /// is turned to the camera — the eyebrows, eyes, nose and mouth on it.
    fn draw_subject(&self, image: &mut GrayImage, subject: &Subject) {
        let span = subject.eye_span;
        let skin = f32::from(self.skin);
        let clothing = (skin * 0.35).round().clamp(0.0, 255.0) as u8;
        let hair = (skin * 0.28).round().clamp(0.0, 255.0) as u8;
        let feature = (skin * 0.22).round().clamp(0.0, 255.0) as u8;
        let lips = (skin * 0.62).round().clamp(0.0, 255.0) as u8;
        let shadow = (skin * 0.86).round().clamp(0.0, 255.0) as u8;
        let highlight = (skin * 1.06).round().clamp(0.0, 255.0) as u8;

        // The shoulders, then the neck: a torso wider than the head, the way a portrait
        // frames one.
        fill_ellipse(
            image,
            subject.eyes_x,
            subject.eyes_y + 3.6 * span,
            2.2 * span,
            1.5 * span,
            clothing,
        );
        fill_ellipse(
            image,
            subject.eyes_x,
            subject.eyes_y + 2.1 * span,
            0.45 * span,
            0.8 * span,
            shadow,
        );

        // The hair behind the head, then the head itself, so the hair shows as a crown and
        // a pair of sides rather than as a block.
        fill_ellipse(
            image,
            subject.eyes_x,
            subject.eyes_y - 0.15 * span,
            1.22 * span,
            1.6 * span,
            hair,
        );
        let (head_x, head_y, head_rx, head_ry) = subject.head();
        fill_ellipse(image, head_x, head_y, head_rx, head_ry, self.skin);
        // The light falls from above: the forehead and the cheekbones take it, the jaw and
        // the sides of the face fall away from it.
        fill_ellipse(
            image,
            head_x,
            head_y - 0.5 * span,
            0.55 * head_rx,
            0.5 * head_ry,
            highlight,
        );
        fill_ellipse(
            image,
            head_x,
            head_y + 1.05 * span,
            0.7 * head_rx,
            0.45 * span,
            shadow,
        );
        if !subject.facing {
            return;
        }

        // The eyebrows: a dark bar above each eye, which is the pair of features a detector
        // that only looked for "two dark things, level with each other" would take for eyes.
        for side in [-1.0_f32, 1.0] {
            fill_ellipse(
                image,
                subject.eyes_x + side * span / 2.0,
                subject.eyes_y - 0.38 * span,
                0.26 * span,
                0.07 * span,
                hair,
            );
        }
        // The eyes themselves: an iris in the white of the eye.
        for (x, y) in [subject.left_eye(), subject.right_eye()] {
            fill_ellipse(image, x, y, 0.26 * span, 0.12 * span, highlight);
            fill_ellipse(image, x, y, 0.1 * span, 0.1 * span, feature);
        }
        // The nose: a shaded ridge down the middle of the face and two nostrils under it.
        fill_ellipse(
            image,
            subject.eyes_x,
            subject.eyes_y + 0.45 * span,
            0.12 * span,
            0.4 * span,
            shadow,
        );
        for side in [-1.0_f32, 1.0] {
            fill_ellipse(
                image,
                subject.eyes_x + side * 0.17 * span,
                subject.eyes_y + 0.72 * span,
                0.06 * span,
                0.045 * span,
                feature,
            );
        }
        // The mouth: lips with the line between them darker than either.
        let (mouth_x, mouth_y) = subject.mouth_centre();
        fill_ellipse(image, mouth_x, mouth_y, 0.42 * span, 0.17 * span, lips);
        fill_ellipse(image, mouth_x, mouth_y, 0.38 * span, 0.05 * span, feature);
    }

    /// The grain of the film: every pixel perturbed by up to [`Photograph::grain`] grey
    /// levels, enumerated from [`GRAIN_SEED`] so the same photograph is the same bytes.
    fn grain(&self, image: &mut GrayImage) {
        if self.grain == 0 {
            return;
        }
        let swing = i32::from(self.grain);
        let mut state = GRAIN_SEED;
        for pixel in image.pixels_mut() {
            let draw = (next(&mut state) % (2 * swing as u64 + 1)) as i32 - swing;
            pixel.0[0] = (i32::from(pixel.0[0]) + draw).clamp(0, 255) as u8;
        }
    }

    fn bytes(&self, format: ImageFormat) -> Vec<u8> {
        let mut encoded = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(self.render())
            .write_to(&mut encoded, format)
            .expect("encode the drawn photograph");
        encoded.into_inner()
    }
}

/// Fill the ellipse of radii `rx`/`ry` around `(cx, cy)` with `tone`.
fn fill_ellipse(image: &mut GrayImage, cx: f32, cy: f32, rx: f32, ry: f32, tone: u8) {
    let (width, height) = (image.width() as i64, image.height() as i64);
    let x_from = ((cx - rx).floor() as i64).max(0);
    let x_to = ((cx + rx).ceil() as i64).min(width - 1);
    let y_from = ((cy - ry).floor() as i64).max(0);
    let y_to = ((cy + ry).ceil() as i64).min(height - 1);
    for y in y_from..=y_to {
        for x in x_from..=x_to {
            let dx = (x as f32 - cx) / rx.max(0.5);
            let dy = (y as f32 - cy) / ry.max(0.5);
            if dx * dx + dy * dy <= 1.0 {
                image.put_pixel(x as u32, y as u32, Luma([tone]));
            }
        }
    }
}

/// The grain's generator: xorshift64, so every picture is enumerated from the pinned seed.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

// ---------------------------------------------------------------------------
// Reading the answers
// ---------------------------------------------------------------------------

/// A reported `BoundingBox`, as the `(left, top, width, height)` fractions it carries.
type ReportedBox = (f64, f64, f64, f64);

async fn create_collection(router: &MultiServiceRouter, collection_id: &str) {
    let reply = call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "creating {collection_id:?} must succeed, got {} {:?}",
        reply.status, reply.body
    );
}

async fn index(
    router: &MultiServiceRouter,
    collection_id: &str,
    bytes: &[u8],
    external_image_id: Option<&str>,
) -> Reply {
    let mut body = json!({
        "CollectionId": collection_id,
        "Image": bytes_image(bytes),
    });
    if let Some(external) = external_image_id {
        body["ExternalImageId"] = json!(external);
    }
    call(router, "IndexFaces", body).await
}

/// The `FaceRecords` of a successful `IndexFaces`.
fn records(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: indexing a readable image answers 200, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.field("FaceRecords").is_some(),
        "case {case:?}: FaceRecords is always present, got {:?}",
        reply.body
    );
    reply.list("FaceRecords")
}

fn number(case: &str, value: &Value, path: &[&str]) -> f64 {
    let mut cursor = value;
    for step in path {
        cursor = cursor
            .get(step)
            .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be present in {value:?}"));
    }
    cursor
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be a number in {value:?}"))
}

/// The `FaceId` of an enrolled face, which is what the collection lists it under.
fn face_id(case: &str, face: &Value) -> String {
    face.get("FaceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: an enrolled face carries its id, got {face:?}"))
        .to_string()
}

/// The box a record's enrolled face carries, which is the box the detection measured.
fn face_box(case: &str, record: &Value) -> ReportedBox {
    let rect = record
        .get("Face")
        .and_then(|face| face.get("BoundingBox"))
        .unwrap_or_else(|| panic!("case {case:?}: an enrolled face carries a box, got {record:?}"));
    (
        number(case, rect, &["Left"]),
        number(case, rect, &["Top"]),
        number(case, rect, &["Width"]),
        number(case, rect, &["Height"]),
    )
}

/// Assert that `record` is the enrolment of `subject` as the test drew them: its box holds
/// the drawn eyes and mouth, is about the size of the drawn head, and is a region of the
/// picture rather than the whole picture.
fn assert_is_the_subject(case: &str, record: &Value, subject: &Subject, photograph: &Photograph) {
    let (width, height) = (f64::from(photograph.width), f64::from(photograph.height));
    let (left, top, box_width, box_height) = face_box(case, record);
    let (right, bottom) = (left + box_width, top + box_height);
    for (name, (x, y)) in [
        ("the left eye", subject.left_eye()),
        ("the right eye", subject.right_eye()),
        ("the mouth", subject.mouth_centre()),
    ] {
        let (x, y) = (f64::from(x) / width, f64::from(y) / height);
        assert!(
            x >= left && x <= right && y >= top && y <= bottom,
            "case {case:?}: the enrolled box must hold {name} at ({x:.3}, {y:.3}), got \
             ({left:.3}, {top:.3}) {box_width:.3}x{box_height:.3}"
        );
    }
    let (_, _, head_rx, head_ry) = subject.head();
    for (name, reported, drawn) in [
        ("width", box_width, f64::from(2.0 * head_rx) / width),
        ("height", box_height, f64::from(2.0 * head_ry) / height),
    ] {
        let (low, high) = BOX_SIDE_TOLERANCE;
        assert!(
            reported >= low * drawn && reported <= high * drawn,
            "case {case:?}: the enrolled box's {name} must be about the drawn head's \
             {drawn:.3}, got {reported:.3}"
        );
    }
    assert!(
        box_width * box_height < 0.95,
        "case {case:?}: a face's box is a region of the picture, not the whole of it, got \
         ({left:.3}, {top:.3}) {box_width:.3}x{box_height:.3}"
    );
    let confidence = number(case, record, &["Face", "Confidence"]);
    assert!(
        (0.0..=100.0).contains(&confidence),
        "case {case:?}: an enrolled face carries the confidence it was detected with, got \
         {confidence}"
    );
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// One `FaceRecord` per person the photograph holds, each over that person's own face —
/// whatever else the photograph holds, and read back through the collection.
#[tokio::test]
async fn index_faces_enrols_one_face_per_person_in_a_photograph() {
    let router = harness();
    let cases: Vec<(&str, Photograph, ImageFormat)> = vec![
        (
            "a head-and-shoulders portrait",
            Photograph::portrait(),
            ImageFormat::Jpeg,
        ),
        (
            "the same portrait as a PNG",
            Photograph::portrait(),
            ImageFormat::Png,
        ),
        (
            "a close crop",
            Photograph::portrait().with_subjects(vec![Subject::new(158.0, 120.0, 96.0)]),
            ImageFormat::Jpeg,
        ),
        (
            "a small figure in a wide frame",
            Photograph::portrait()
                .with_size(640, 400)
                .with_subjects(vec![Subject::new(430.0, 150.0, 44.0)]),
            ImageFormat::Jpeg,
        ),
        (
            "two people at different distances",
            Photograph::portrait().with_size(480, 320).with_subjects(vec![
                Subject::new(140.0, 120.0, 56.0),
                Subject::new(360.0, 135.0, 36.0),
            ]),
            ImageFormat::Jpeg,
        ),
        (
            "a dim room",
            Photograph::portrait().with_light(40, 70, 120),
            ImageFormat::Jpeg,
        ),
        (
            "a bright room",
            Photograph::portrait().with_light(200, 230, 170).with_grain(4),
            ImageFormat::Png,
        ),
        (
            "heavier grain",
            Photograph::portrait().with_grain(14),
            ImageFormat::Jpeg,
        ),
    ];

    for (index_of_case, (case, photograph, format)) in cases.into_iter().enumerate() {
        let collection_id = format!("photographs-{index_of_case}");
        create_collection(&router, &collection_id).await;
        let bytes = photograph.bytes(format);
        let reply = index(&router, &collection_id, &bytes, Some("one-upload")).await;
        let found = records(case, &reply);
        let expected = photograph.subjects.len();
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: the picture holds {expected} face(s), so that many are enrolled, \
             got {} with boxes {:?}",
            found.len(),
            found
                .iter()
                .map(|record| face_box(case, record))
                .collect::<Vec<_>>()
        );
        // Largest first, which is the order the drawn people are compared in.
        let mut drawn = photograph.subjects.clone();
        drawn.sort_by(|left, right| right.eye_span.total_cmp(&left.eye_span));
        for (record, subject) in found.iter().zip(drawn.iter()) {
            assert_is_the_subject(case, record, subject, &photograph);
        }

        // What was enrolled is what the collection now holds: the same faces, by id.
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": collection_id }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(expected as i64),
            "case {case:?}: the collection holds the faces that were enrolled, got {:?}",
            described.body
        );
        let listed = call(&router, "ListFaces", json!({ "CollectionId": collection_id })).await;
        let listed_ids: Vec<String> = listed
            .list("Faces")
            .iter()
            .map(|face| face_id(case, face))
            .collect();
        let enrolled_ids: Vec<String> = found
            .iter()
            .map(|record| {
                face_id(
                    case,
                    record
                        .get("Face")
                        .unwrap_or_else(|| panic!("case {case:?}: a record carries its face")),
                )
            })
            .collect();
        for id in &enrolled_ids {
            assert!(
                listed_ids.contains(id),
                "case {case:?}: an enrolled face is one the collection lists, {id:?} is not in \
                 {listed_ids:?}"
            );
        }
        assert_eq!(
            listed_ids.len(),
            enrolled_ids.len(),
            "case {case:?}: the collection lists exactly the enrolled faces, got {listed_ids:?} \
             for {enrolled_ids:?}"
        );
    }
}

/// A photograph with nobody in it enrols nobody: neither the grain and the gradient of an
/// empty frame nor a subject photographed from behind is a face.
#[tokio::test]
async fn a_photograph_of_nobody_enrols_nobody() {
    let router = harness();
    let cases: Vec<(&str, Photograph)> = vec![
        ("an empty frame", Photograph::portrait().with_subjects(Vec::new())),
        (
            "an empty frame, heavier grain",
            Photograph::portrait()
                .with_subjects(Vec::new())
                .with_grain(16),
        ),
        (
            "a subject photographed from behind",
            Photograph::portrait()
                .with_subjects(vec![Subject::new(160.0, 128.0, 60.0).turned_away()]),
        ),
        (
            "two subjects photographed from behind",
            Photograph::portrait().with_size(480, 320).with_subjects(vec![
                Subject::new(140.0, 120.0, 56.0).turned_away(),
                Subject::new(360.0, 135.0, 36.0).turned_away(),
            ]),
        ),
    ];

    for (index_of_case, (case, photograph)) in cases.into_iter().enumerate() {
        let collection_id = format!("nobody-{index_of_case}");
        create_collection(&router, &collection_id).await;
        let bytes = photograph.bytes(ImageFormat::Jpeg);
        let reply = index(&router, &collection_id, &bytes, None).await;
        let found = records(case, &reply);
        assert!(
            found.is_empty(),
            "case {case:?}: no face was photographed, so none is enrolled, got {:?}",
            found
                .iter()
                .map(|record| face_box(case, record))
                .collect::<Vec<_>>()
        );
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": collection_id }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: nothing was enrolled, so the collection holds nothing, got {:?}",
            described.body
        );
    }
}

/// The same bytes answer the same way: indexing one photograph twice enrols the same number
/// of faces with the same boxes, so a client that indexes an upload twice sees one face per
/// person each time.
#[tokio::test]
async fn the_same_photograph_enrols_the_same_faces_every_time() {
    let router = harness();
    let photograph = Photograph::portrait();
    let bytes = photograph.bytes(ImageFormat::Jpeg);
    let mut boxes: Vec<Vec<ReportedBox>> = Vec::new();
    for attempt in 0..3 {
        let collection_id = format!("repeat-{attempt}");
        create_collection(&router, &collection_id).await;
        let case = format!("attempt {attempt}");
        let reply = index(&router, &collection_id, &bytes, None).await;
        let found = records(&case, &reply);
        assert_eq!(
            found.len(),
            1,
            "{case}: the photograph holds one face, so one is enrolled, got {found:?}"
        );
        boxes.push(
            found
                .iter()
                .map(|record| face_box(&case, record))
                .collect::<Vec<_>>(),
        );
    }
    for (attempt, found) in boxes.iter().enumerate().skip(1) {
        assert_eq!(
            found, &boxes[0],
            "attempt {attempt}: the same bytes are the same measurement, expected {:?}, got \
             {found:?}",
            boxes[0]
        );
    }
}
