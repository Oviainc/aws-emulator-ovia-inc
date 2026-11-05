// SPDX-License-Identifier: CC0-1.0

//! `DetectFaces` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.DetectFaces`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `DetectFaces` reports one `FaceDetail` per face MEASURED in the
//! submitted image, and reports only what it measured — a box where the face is, the
//! detector's own agreement score, the landmarks and the roll its eye line and mouth give,
//! and the brightness and sharpness of the face's own pixels — so an image that carries no
//! face is answered with an empty list, a member nothing measured (every classified
//! attribute, and a yaw or a pitch) is absent whatever `Attributes` asks for, and an image
//! this service cannot read at all is refused with the modelled error for why. The image's
//! bytes are the request's own `Bytes` or the object this emulator's Amazon S3 holds at the
//! `S3Object` named, and both are read alike. Nothing is persisted: a second identical call
//! answers identically and the service's stored collections are untouched.
//!
//! Every portrait below is DRAWN by the test from known tones and distances — a head, two
//! eyes, a mouth — and encoded as a real PNG or JPEG, so each assertion compares the
//! service's answer against where the test put the face rather than against a recorded
//! answer. The tables span what the rule generalises over: how many faces an image carries
//! (none, one, two of different sizes), what structure it carries (a head with no features,
//! eyes with no mouth, flat tone, noise), the face's size, position, tilt and tone, the
//! encoded format, how the image was submitted (inline, from an S3 object), which part of
//! an address does not resolve, and what `Attributes` asks for.
//!
//! The generated sweep (`a_drawn_face_is_found_wherever_it_is_drawn`) runs a bounded number
//! of cases from the pinned [`SWEEP_SEED`] through a three-line xorshift rather than pulling
//! in a generator crate: the cases are enumerated deterministically, so a failure names the
//! seed and the case index and reproduces exactly.
//!
//! The portraits, the blur, the encoder and the object-store harness below are what every
//! suite of this crate that needs a drawn image uses, so one picture is drawn one way
//! whichever operation is being exercised.

use std::io::Cursor;
use std::sync::Arc;

use image::{DynamicImage, GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, call_in, Reply};

/// The Region every call here is signed in. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1, which is also the Region this service serves a
/// request carrying no credential scope in, so one Region label serves every call.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what claims a request for the object
/// store. No authenticator is registered, so the signature is never checked.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket the staged images below live in, and a bucket nothing here creates.
pub(crate) const BUCKET: &str = "rekognition-images";
const ABSENT_BUCKET: &str = "rekognition-images-missing";

/// The seed the generated-case sweep starts from, and how many cases it runs.
const SWEEP_SEED: u64 = 0x4445_5445_4354_4653;
const SWEEP_CASES: usize = 24;

/// What a face DRAWN to the proportions the detector reads must agree with them to, on the
/// 0..100 scale its `Confidence` carries: well above the detector's own floor, because every
/// portrait here places its eyes level with each other and its mouth where the proportions
/// put it. It is not 100: a portrait drawn with its eye line tilted agrees less with a level
/// one, which is the point of that term.
const WELL_DRAWN_CONFIDENCE: f64 = 65.0;

/// How far the measured `Quality.Brightness` of a face may sit from the tone its skin was
/// drawn in, in percentage points: the box is mostly skin, with the features and a little
/// background in it.
const BRIGHTNESS_TOLERANCE: f64 = 25.0;

/// Every `FaceDetail` member a classifier would have had to judge, which is nothing this
/// emulator runs: none of them may appear on a reported face
/// (`smithy_query(query="shape",shape="FaceDetail")` lists them all).
const CLASSIFIED_MEMBERS: [&str; 12] = [
    "AgeRange",
    "Smile",
    "Eyeglasses",
    "Sunglasses",
    "Gender",
    "Beard",
    "Mustache",
    "EyesOpen",
    "MouthOpen",
    "Emotions",
    "FaceOccluded",
    "EyeDirection",
];

// ---------------------------------------------------------------------------
// The portraits the tests draw
// ---------------------------------------------------------------------------

/// A reported `BoundingBox`, as the `(left, top, width, height)` fractions it carries.
pub(crate) type ReportedBox = (f64, f64, f64, f64);

/// One face as the TEST draws it: where its eyes are, how far apart, how far the eye line
/// is rotated clockwise in the image, and whether its features are drawn at all.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrawnFace {
    eyes_x: f32,
    eyes_y: f32,
    eye_span: f32,
    roll: f32,
    eyes: bool,
    mouth: bool,
}

impl DrawnFace {
    /// A whole face: a head, two eyes `eye_span` apart and a mouth below them.
    pub(crate) fn new(eyes_x: f32, eyes_y: f32, eye_span: f32) -> Self {
        Self {
            eyes_x,
            eyes_y,
            eye_span,
            roll: 0.0,
            eyes: true,
            mouth: true,
        }
    }

    pub(crate) fn rolled(mut self, roll: f32) -> Self {
        self.roll = roll;
        self
    }

    fn without_eyes(mut self) -> Self {
        self.eyes = false;
        self
    }

    fn without_mouth(mut self) -> Self {
        self.mouth = false;
        self
    }

    /// The eye line's direction, as the test rotated it.
    fn direction(&self) -> (f32, f32) {
        let (sin, cos) = self.roll.to_radians().sin_cos();
        (cos, sin)
    }

    fn left_eye(&self) -> (f32, f32) {
        let (cos, sin) = self.direction();
        (
            self.eyes_x - cos * self.eye_span / 2.0,
            self.eyes_y - sin * self.eye_span / 2.0,
        )
    }

    fn right_eye(&self) -> (f32, f32) {
        let (cos, sin) = self.direction();
        (
            self.eyes_x + cos * self.eye_span / 2.0,
            self.eyes_y + sin * self.eye_span / 2.0,
        )
    }

    /// The mouth sits one eye span down the face's own axis.
    fn mouth_centre(&self) -> (f32, f32) {
        let (cos, sin) = self.direction();
        (
            self.eyes_x - sin * self.eye_span,
            self.eyes_y + cos * self.eye_span,
        )
    }

    /// The head the test draws: an ellipse 2.2 eye spans wide and 3.0 tall, with the eye
    /// line a little above its middle. DELIBERATELY not the proportions the detector
    /// derives its box from, so an assertion about the box compares it against a drawn
    /// head rather than against the service's own constants.
    fn head(&self) -> (f32, f32, f32, f32) {
        (
            self.eyes_x,
            self.eyes_y + 0.15 * self.eye_span,
            1.1 * self.eye_span,
            1.5 * self.eye_span,
        )
    }
}

/// One synthetic image: its size, its three tones and the faces drawn on it.
#[derive(Debug, Clone)]
pub(crate) struct Portrait {
    pub(crate) width: u32,
    pub(crate) height: u32,
    background: u8,
    skin: u8,
    feature: u8,
    faces: Vec<DrawnFace>,
}

impl Portrait {
    /// One centred face on a 320x320 image, which is the base every case varies from.
    pub(crate) fn one_face() -> Self {
        Self {
            width: 320,
            height: 320,
            background: 110,
            skin: 190,
            feature: 30,
            faces: vec![DrawnFace::new(160.0, 150.0, 56.0)],
        }
    }

    /// An image with no face on it at all.
    pub(crate) fn blank(width: u32, height: u32, tone: u8) -> Self {
        Self {
            width,
            height,
            background: tone,
            skin: tone,
            feature: tone,
            faces: Vec::new(),
        }
    }

    pub(crate) fn with_faces(mut self, faces: Vec<DrawnFace>) -> Self {
        self.faces = faces;
        self
    }

    pub(crate) fn with_tones(mut self, background: u8, skin: u8, feature: u8) -> Self {
        self.background = background;
        self.skin = skin;
        self.feature = feature;
        self
    }

    pub(crate) fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// The pixels of this portrait.
    pub(crate) fn render(&self) -> GrayImage {
        let mut image = GrayImage::from_pixel(self.width, self.height, Luma([self.background]));
        for face in &self.faces {
            let (cx, cy, rx, ry) = face.head();
            fill_ellipse(&mut image, cx, cy, rx, ry, self.skin);
        }
        // The features go on after every head, so one face's head cannot paint over
        // another's eyes where two faces touch.
        for face in &self.faces {
            if face.eyes {
                let radius = 0.18 * face.eye_span;
                for (x, y) in [face.left_eye(), face.right_eye()] {
                    fill_ellipse(&mut image, x, y, radius, radius, self.feature);
                }
            }
            if face.mouth {
                let (x, y) = face.mouth_centre();
                fill_ellipse(
                    &mut image,
                    x,
                    y,
                    0.35 * face.eye_span,
                    0.12 * face.eye_span,
                    self.feature,
                );
            }
        }
        image
    }

    /// This portrait as PNG bytes, which is how most cases submit it.
    pub(crate) fn png(&self) -> Vec<u8> {
        encode(&self.render(), ImageFormat::Png)
    }
}

/// Fill the ellipse of radii `rx`/`ry` around `(cx, cy)` with `tone`.
pub(crate) fn fill_ellipse(image: &mut GrayImage, cx: f32, cy: f32, rx: f32, ry: f32, tone: u8) {
    let (width, height) = (image.width() as i64, image.height() as i64);
    let x_from = ((cx - rx).floor() as i64).max(0);
    let x_to = ((cx + rx).ceil() as i64).min(width - 1);
    let y_from = ((cy - ry).floor() as i64).max(0);
    let y_to = ((cy + ry).ceil() as i64).min(height - 1);
    for y in y_from..=y_to {
        for x in x_from..=x_to {
            let dx = (x as f32 - cx) / rx.max(1.0);
            let dy = (y as f32 - cy) / ry.max(1.0);
            if dx * dx + dy * dy <= 1.0 {
                image.put_pixel(x as u32, y as u32, Luma([tone]));
            }
        }
    }
}

/// `image` encoded in `format`: the bytes a client would submit.
pub(crate) fn encode(image: &GrayImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image.clone())
        .write_to(&mut bytes, format)
        .expect("the portrait encodes");
    bytes.into_inner()
}

/// `image` averaged over a 3x3 window, `passes` times: the same picture, less sharp.
pub(crate) fn blurred(image: &GrayImage, passes: usize) -> GrayImage {
    let mut current = image.clone();
    for _ in 0..passes {
        let mut next = current.clone();
        for y in 1..current.height() - 1 {
            for x in 1..current.width() - 1 {
                let mut total = 0_u32;
                for window_y in (y - 1)..=(y + 1) {
                    for window_x in (x - 1)..=(x + 1) {
                        total += u32::from(current.get_pixel(window_x, window_y).0[0]);
                    }
                }
                next.put_pixel(x, y, Luma([(total / 9) as u8]));
            }
        }
        current = next;
    }
    current
}

// ---------------------------------------------------------------------------
// The emulator, and reading its answers
// ---------------------------------------------------------------------------

/// The emulator with this one service mounted on a shared engine.
fn harness() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// The emulator with this service and the emulator's own Amazon S3 on ONE engine — what the
/// assembled emulator runs, narrowed to the two services an S3-sourced image needs.
pub(crate) fn harness_with_object_store() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One path-style request to the emulator's Amazon S3 front door.
pub(crate) async fn s3(router: &MultiServiceRouter, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

async fn detect(router: &MultiServiceRouter, image: Value) -> Reply {
    call(router, "DetectFaces", json!({ "Image": image })).await
}

/// The `FaceDetails` of a successful answer, with the members every reported face must and
/// must not carry already checked.
fn faces_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectFaces must answer, got {:?}",
        reply.body
    );
    assert!(
        reply.field("OrientationCorrection").is_none(),
        "case {case:?}: OrientationCorrection is always null, got {:?}",
        reply.body
    );
    let faces = reply
        .field("FaceDetails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: FaceDetails must be present, got {:?}", reply.body));
    for face in &faces {
        for member in CLASSIFIED_MEMBERS {
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
    }
    faces
}

/// A float member of a JSON object, by path.
pub(crate) fn number(value: &Value, path: &[&str]) -> f64 {
    let mut current = value;
    for step in path {
        current = current
            .get(step)
            .unwrap_or_else(|| panic!("{path:?} must be present in {value:?}"));
    }
    current
        .as_f64()
        .unwrap_or_else(|| panic!("{path:?} must be a number in {value:?}"))
}

/// The landmark of `kind` on a reported face, as the fractions it carries.
fn landmark(case: &str, face: &Value, kind: &str) -> (f64, f64) {
    let found = face
        .get("Landmarks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: Landmarks must be present, got {face:?}"))
        .iter()
        .find(|entry| entry.get("Type").and_then(Value::as_str) == Some(kind))
        .unwrap_or_else(|| panic!("case {case:?}: the {kind} landmark must be reported, got {face:?}"))
        .clone();
    (number(&found, &["X"]), number(&found, &["Y"]))
}

/// The reported box of a face, as `(left, top, width, height)` fractions.
fn bounding_box(case: &str, face: &Value) -> ReportedBox {
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

/// Assert that the face reported is the face the test DREW: its box encloses the drawn eyes
/// and mouth without claiming much more than the drawn head, its landmarks sit where the
/// features were drawn, its roll is the angle the eye line was drawn at, and its quality and
/// confidence are inside the ranges the model allows.
fn assert_describes(case: &str, face: &Value, drawn: &DrawnFace, portrait: &Portrait) {
    let (width, height) = (f64::from(portrait.width), f64::from(portrait.height));
    let (left, top, box_width, box_height) = bounding_box(case, face);
    let (right, bottom) = (left + box_width, top + box_height);
    for (name, (x, y)) in [
        ("the left eye", drawn.left_eye()),
        ("the right eye", drawn.right_eye()),
        ("the mouth", drawn.mouth_centre()),
    ] {
        let (x, y) = (f64::from(x) / width, f64::from(y) / height);
        assert!(
            x >= left && x <= right && y >= top && y <= bottom,
            "case {case:?}: the box must enclose {name} at ({x:.3}, {y:.3}), \
             got ({left:.3}, {top:.3}) {box_width:.3}x{box_height:.3}"
        );
    }
    let (_, _, head_half_width, head_half_height) = drawn.head();
    let drawn_width = f64::from(2.0 * head_half_width) / width;
    let drawn_height = f64::from(2.0 * head_half_height) / height;
    for (name, reported, expected) in [
        ("width", box_width, drawn_width),
        ("height", box_height, drawn_height),
    ] {
        assert!(
            reported >= 0.6 * expected && reported <= 1.4 * expected,
            "case {case:?}: the box's {name} must be about the drawn head's {expected:.3}, \
             got {reported:.3}"
        );
    }

    for (kind, (x, y)) in [
        ("eyeLeft", drawn.left_eye()),
        ("eyeRight", drawn.right_eye()),
    ] {
        let (reported_x, reported_y) = landmark(case, face, kind);
        let (expected_x, expected_y) = (f64::from(x) / width, f64::from(y) / height);
        assert!(
            (reported_x - expected_x).abs() <= 0.03 && (reported_y - expected_y).abs() <= 0.03,
            "case {case:?}: {kind} must be measured at ({expected_x:.3}, {expected_y:.3}), \
             got ({reported_x:.3}, {reported_y:.3})"
        );
    }
    let (mouth_x, mouth_y) = drawn.mouth_centre();
    let (mouth_left, _) = landmark(case, face, "mouthLeft");
    let (mouth_right, mouth_right_y) = landmark(case, face, "mouthRight");
    let expected_x = f64::from(mouth_x) / width;
    assert!(
        mouth_left <= expected_x && expected_x <= mouth_right,
        "case {case:?}: the mouth landmarks must span the drawn mouth at {expected_x:.3}, \
         got {mouth_left:.3}..{mouth_right:.3}"
    );
    assert!(
        (mouth_right_y - f64::from(mouth_y) / height).abs() <= 0.05,
        "case {case:?}: the mouth must be measured at {:.3}, got {mouth_right_y:.3}",
        f64::from(mouth_y) / height
    );

    let roll = number(face, &["Pose", "Roll"]);
    assert!(
        (roll - f64::from(drawn.roll)).abs() <= 4.0,
        "case {case:?}: the roll of the eye line was drawn at {}, got {roll}",
        drawn.roll
    );

    let confidence = number(face, &["Confidence"]);
    assert!(
        (WELL_DRAWN_CONFIDENCE..=100.0).contains(&confidence),
        "case {case:?}: a face drawn to the proportions the detector reads must agree well \
         with them, expected at least {WELL_DRAWN_CONFIDENCE}, got {confidence}"
    );
    // The box is mostly the drawn skin, so its measured brightness tracks that tone: a
    // brightness that did not come from these pixels cannot sit this close to it.
    let brightness = number(face, &["Quality", "Brightness"]);
    let skin = f64::from(portrait.skin) / 255.0 * 100.0;
    assert!(
        (brightness - skin).abs() <= BRIGHTNESS_TOLERANCE,
        "case {case:?}: skin drawn at {skin:.1}% of full brightness must measure within \
         {BRIGHTNESS_TOLERANCE} of it, got {brightness}"
    );
    let sharpness = number(face, &["Quality", "Sharpness"]);
    assert!(
        sharpness > 0.0 && sharpness <= 100.0,
        "case {case:?}: the drawn features give the face edges to measure, so its sharpness \
         is above zero, got {sharpness}"
    );
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// What an image carries is what is reported: one `FaceDetail` per drawn face, largest
/// first, and nothing at all for an image with no face structure on it.
#[tokio::test]
async fn detect_faces_reports_the_faces_the_image_carries() {
    let router = harness();
    let one = Portrait::one_face();
    let two = Portrait::one_face().with_size(480, 320).with_faces(vec![
        DrawnFace::new(130.0, 150.0, 56.0),
        DrawnFace::new(350.0, 140.0, 36.0),
    ]);
    let cases: Vec<(&str, Portrait, usize)> = vec![
        ("one centred face", one, 1),
        ("two faces of different sizes", two, 2),
        (
            "a tilted face",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(10.0)]),
            1,
        ),
        (
            "a face tilted the other way",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(-10.0)]),
            1,
        ),
        (
            "a small face in a large image",
            Portrait::one_face()
                .with_size(640, 480)
                .with_faces(vec![DrawnFace::new(300.0, 220.0, 56.0)]),
            1,
        ),
        (
            "a close-up face larger than the frame",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 140.0, 110.0)]),
            1,
        ),
        ("a flat light image", Portrait::blank(320, 320, 235), 0),
        ("a flat dark image", Portrait::blank(320, 320, 12), 0),
        (
            "a head with no features",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0)
                .without_eyes()
                .without_mouth()]),
            0,
        ),
        (
            "eyes with no mouth",
            Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).without_mouth()]),
            0,
        ),
        (
            "a mouth with no eyes",
            Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).without_eyes()]),
            0,
        ),
    ];

    for (case, portrait, expected) in cases {
        let reply = detect(&router, bytes_image(&portrait.png())).await;
        let faces = faces_of(case, &reply);
        assert_eq!(
            faces.len(),
            expected,
            "case {case:?}: expected {expected} face(s), got {faces:?}"
        );
        // The faces a drawn portrait carries, largest drawn first: the order the answer
        // must be in, since DetectFaces reports the largest faces of an image.
        let mut drawn = portrait.faces.clone();
        drawn.sort_by(|left, right| right.eye_span.total_cmp(&left.eye_span));
        for (face, drawn) in faces.iter().zip(drawn.iter()) {
            assert_describes(case, face, drawn, &portrait);
        }
    }
}

/// Noise is not a face: a flat canvas with every pixel perturbed from the pinned seed.
#[tokio::test]
async fn noise_is_not_a_face() {
    let router = harness();
    let mut state = SWEEP_SEED;
    let mut image = Portrait::blank(320, 320, 128).render();
    for pixel in image.pixels_mut() {
        let draw = next(&mut state);
        pixel.0[0] = (draw % 256) as u8;
    }
    let reply = detect(&router, bytes_image(&encode(&image, ImageFormat::Png))).await;
    let faces = faces_of("noise", &reply);
    assert!(
        faces.is_empty(),
        "seed {SWEEP_SEED:#x}: noise carries no face structure, got {faces:?}"
    );
}

/// One face answers alike however it is encoded and at whatever size it is drawn: the box
/// and the landmarks are fractions of the image, so they do not move with the scale.
#[tokio::test]
async fn one_face_answers_alike_at_every_scale_and_in_both_formats() {
    let router = harness();
    let cases: [(&str, u32, f32, ImageFormat); 6] = [
        ("160 px, PNG", 160, 28.0, ImageFormat::Png),
        ("320 px, PNG", 320, 56.0, ImageFormat::Png),
        ("640 px, PNG", 640, 112.0, ImageFormat::Png),
        ("160 px, JPEG", 160, 28.0, ImageFormat::Jpeg),
        ("320 px, JPEG", 320, 56.0, ImageFormat::Jpeg),
        ("640 px, JPEG", 640, 112.0, ImageFormat::Jpeg),
    ];
    let mut boxes: Vec<(&str, ReportedBox)> = Vec::new();
    for (case, side, eye_span, format) in cases {
        let centre = f32::from(u16::try_from(side).expect("the test's sides are small"));
        let drawn = DrawnFace::new(centre / 2.0, centre * 0.47, eye_span);
        let portrait = Portrait::one_face()
            .with_size(side, side)
            .with_faces(vec![drawn]);
        let bytes = encode(&portrait.render(), format);
        let reply = detect(&router, bytes_image(&bytes)).await;
        let faces = faces_of(case, &reply);
        assert_eq!(
            faces.len(),
            1,
            "case {case:?}: one drawn face must be found once, got {faces:?}"
        );
        assert_describes(case, &faces[0], &drawn, &portrait);
        boxes.push((case, bounding_box(case, &faces[0])));
    }
    let (first_case, first) = boxes[0];
    for (case, found) in &boxes[1..] {
        for (name, left, right) in [
            ("Left", first.0, found.0),
            ("Top", first.1, found.1),
            ("Width", first.2, found.2),
            ("Height", first.3, found.3),
        ] {
            assert!(
                (left - right).abs() <= 0.05,
                "case {case:?}: the same face drawn at a different scale or format must report \
                 the same {name} fraction as {first_case:?} ({left:.3}), got {right:.3}"
            );
        }
    }
}

/// `Quality` and `Confidence` are MEASURED: brighter skin reports more brightness, a blurred
/// picture of the same face reports less sharpness, and a mouth away from where the
/// proportions put it agrees less with the reading of the pixels.
#[tokio::test]
async fn quality_and_confidence_follow_the_pixels() {
    let router = harness();
    let brighter = Portrait::one_face().with_tones(60, 220, 20);
    let darker = Portrait::one_face().with_tones(20, 90, 10);
    let mut measured: Vec<(&str, f64, f64)> = Vec::new();
    for (case, portrait) in [("brighter skin", &brighter), ("darker skin", &darker)] {
        let reply = detect(&router, bytes_image(&portrait.png())).await;
        let faces = faces_of(case, &reply);
        assert_eq!(
            faces.len(),
            1,
            "case {case:?}: the face must be found whatever the tone of its skin, got {faces:?}"
        );
        assert_describes(case, &faces[0], &portrait.faces[0], portrait);
        measured.push((
            case,
            number(&faces[0], &["Quality", "Brightness"]),
            number(&faces[0], &["Confidence"]),
        ));
    }
    assert!(
        measured[0].1 > measured[1].1,
        "brighter skin must measure brighter than darker skin, got {:?} and {:?}",
        measured[0],
        measured[1]
    );

    let sharp = Portrait::one_face();
    let sharp_face = sharp.png();
    let soft = encode(&blurred(&sharp.render(), 3), ImageFormat::Png);
    let mut sharpness: Vec<(&str, f64)> = Vec::new();
    for (case, bytes) in [("sharp", sharp_face), ("blurred", soft)] {
        let reply = detect(&router, bytes_image(&bytes)).await;
        let faces = faces_of(case, &reply);
        assert_eq!(
            faces.len(),
            1,
            "case {case:?}: the same face must be found in both, got {faces:?}"
        );
        sharpness.push((case, number(&faces[0], &["Quality", "Sharpness"])));
    }
    assert!(
        sharpness[0].1 > sharpness[1].1,
        "a blurred picture of one face must measure less sharp than a crisp one, \
         got {sharpness:?}"
    );

    // The same face with its mouth painted over and redrawn further down the axis: still a
    // face, but the reading agrees less with the proportions, so the measured confidence is
    // lower.
    let proportioned = Portrait::one_face();
    let displaced_bytes = {
        let mut image = proportioned.render();
        let face = proportioned.faces[0];
        let (x, y) = face.mouth_centre();
        fill_ellipse(
            &mut image,
            x,
            y,
            0.36 * face.eye_span,
            0.13 * face.eye_span,
            proportioned.skin,
        );
        fill_ellipse(
            &mut image,
            x,
            y + 0.35 * face.eye_span,
            0.35 * face.eye_span,
            0.12 * face.eye_span,
            proportioned.feature,
        );
        encode(&image, ImageFormat::Png)
    };
    let well = faces_of(
        "proportioned",
        &detect(&router, bytes_image(&proportioned.png())).await,
    );
    let off = faces_of(
        "displaced mouth",
        &detect(&router, bytes_image(&displaced_bytes)).await,
    );
    assert_eq!(
        (well.len(), off.len()),
        (1, 1),
        "both portraits carry one face, got {well:?} and {off:?}"
    );
    assert!(
        number(&well[0], &["Confidence"]) > number(&off[0], &["Confidence"]),
        "a mouth where the proportions put it must agree better than one away from it, \
         got {} and {}",
        number(&well[0], &["Confidence"]),
        number(&off[0], &["Confidence"])
    );
}

/// `Attributes` asks for classifier judgements no classifier here makes, so the answer is
/// the same measurement whatever it asks for — and never a guessed attribute.
#[tokio::test]
async fn attributes_do_not_change_what_was_measured() {
    let router = harness();
    let portrait = Portrait::one_face();
    let image = bytes_image(&portrait.png());
    let cases: [(&str, Value); 5] = [
        ("no Attributes", json!({ "Image": image })),
        (
            "DEFAULT",
            json!({ "Image": image, "Attributes": ["DEFAULT"] }),
        ),
        ("ALL", json!({ "Image": image, "Attributes": ["ALL"] })),
        (
            "ALL and DEFAULT",
            json!({ "Image": image, "Attributes": ["ALL", "DEFAULT"] }),
        ),
        (
            "named attributes",
            json!({ "Image": image, "Attributes": ["AGE_RANGE", "GENDER", "FACE_OCCLUDED"] }),
        ),
    ];
    let mut answers: Vec<(&str, Vec<Value>)> = Vec::new();
    for (case, body) in cases {
        let reply = call(&router, "DetectFaces", body).await;
        let faces = faces_of(case, &reply);
        assert_eq!(
            faces.len(),
            1,
            "case {case:?}: the drawn face must be found, got {faces:?}"
        );
        answers.push((case, faces));
    }
    let (first_case, first) = &answers[0];
    for (case, faces) in &answers[1..] {
        assert_eq!(
            faces, first,
            "case {case:?}: the measurement cannot depend on what {first_case:?} asked for"
        );
    }
}

/// An image this service cannot read is refused with the modelled error for WHY, never with
/// an empty answer that would read as "no faces here".
#[tokio::test]
async fn an_unreadable_image_is_refused() {
    let router = harness();
    let png = Portrait::one_face().png();
    let cases: Vec<(&str, Value, &str)> = vec![
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
            "a format DetectFaces does not accept",
            bytes_image(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
            "InvalidImageFormatException",
        ),
        (
            // The model's own `ImageBlob` length constraint (`length 1..5242880`) refuses
            // this at the boundary, before the handler runs.
            "inline content with no bytes in it",
            json!({ "Bytes": "" }),
            "ValidationException",
        ),
        (
            "an image identifying no content",
            json!({}),
            "InvalidParameterException",
        ),
        (
            "an S3 object with no key",
            json!({ "S3Object": { "Bucket": BUCKET } }),
            "InvalidS3ObjectException",
        ),
    ];
    for (case, image, expected) in cases {
        let reply = detect(&router, image).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unreadable image is a client error, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            reply.body
        );
        assert!(
            ["Message", "message"]
                .iter()
                .any(|key| reply.string(key).is_some_and(|message| !message.is_empty())),
            "case {case:?}: the refusal must say why, got {:?}",
            reply.body
        );
    }
}

/// An image staged in the emulator's own Amazon S3 answers exactly like the same bytes
/// submitted inline, and an address the store does not serve is refused with the modelled
/// error for an unreadable object.
#[tokio::test]
async fn an_s3_sourced_image_answers_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    let cases: Vec<(&str, Portrait)> = vec![
        ("one centred face", Portrait::one_face()),
        (
            "two faces",
            Portrait::one_face().with_size(480, 320).with_faces(vec![
                DrawnFace::new(130.0, 150.0, 56.0),
                DrawnFace::new(350.0, 140.0, 36.0),
            ]),
        ),
        ("no face at all", Portrait::blank(240, 240, 200)),
    ];
    for (case, portrait) in cases {
        for key in ["portrait.png", "nested/prefix/portrait.png"] {
            let bytes = portrait.png();
            assert_eq!(
                s3(&router, &format!("/{BUCKET}/{key}"), &bytes).await,
                200,
                "case {case:?}: the image object must be stored"
            );
            let inline = faces_of(case, &detect(&router, bytes_image(&bytes)).await);
            let sourced = faces_of(
                case,
                &detect(
                    &router,
                    json!({ "S3Object": { "Bucket": BUCKET, "Name": key } }),
                )
                .await,
            );
            assert_eq!(
                sourced, inline,
                "case {case:?}, key {key:?}: an S3-sourced image must answer like the same \
                 bytes inline"
            );
        }
    }

    let unreadable: Vec<(&str, Value)> = vec![
        (
            "a bucket the store does not hold",
            json!({ "Bucket": ABSENT_BUCKET, "Name": "portrait.png" }),
        ),
        (
            "a key the bucket does not hold",
            json!({ "Bucket": BUCKET, "Name": "absent.png" }),
        ),
        (
            "a key that only prefixes a stored one",
            json!({ "Bucket": BUCKET, "Name": "portrait" }),
        ),
        (
            "a version the bucket never held",
            json!({ "Bucket": BUCKET, "Name": "portrait.png", "Version": "0123456789abcdef" }),
        ),
    ];
    for (case, object) in unreadable {
        let reply = detect(&router, json!({ "S3Object": object })).await;
        assert_eq!(
            reply.error_code(),
            "InvalidS3ObjectException",
            "case {case:?}: an object the store does not serve is refused as unreadable, \
             got {:?}",
            reply.body
        );
    }

    // The signing region is the one caller-supplied value that reaches the address the
    // in-process read is sent to, and a value that is not a Region identifier is not
    // reflected: the read still resolves the object this emulator holds.
    let bytes = Portrait::one_face().png();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/one-face.png"), &bytes).await,
        200,
        "the image object must be stored"
    );
    let inline = faces_of("inline", &detect(&router, bytes_image(&bytes)).await);
    let sourced = call_in(
        &router,
        "us-east-1.not-a-region.example.com",
        "DetectFaces",
        json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": "one-face.png" } } }),
    )
    .await;
    assert_eq!(
        faces_of("a signing region that is not one", &sourced),
        inline,
        "a request signed for no real Region is served in the default one, so its S3-sourced \
         image answers like the same bytes inline"
    );
}

/// `DetectFaces` persists nothing: the same call answers identically a second time, and the
/// collections the service does store are untouched by it.
#[tokio::test]
async fn detect_faces_persists_nothing() {
    let router = harness();
    let before = call(&router, "ListCollections", json!({})).await;
    let portrait = Portrait::one_face();
    let first = faces_of(
        "first call",
        &detect(&router, bytes_image(&portrait.png())).await,
    );
    let second = faces_of(
        "second call",
        &detect(&router, bytes_image(&portrait.png())).await,
    );
    assert_eq!(
        first, second,
        "a stateless measurement of one image cannot differ between two calls"
    );
    let after = call(&router, "ListCollections", json!({})).await;
    assert_eq!(
        after.list("CollectionIds"),
        before.list("CollectionIds"),
        "DetectFaces stores no resource, so the collection list cannot have changed"
    );
    assert!(
        before.list("CollectionIds").is_empty(),
        "no collection was ever created here, got {:?}",
        before.body
    );
}

/// A drawn face is found wherever, however large and in whatever tones it is drawn: the
/// generated sweep, enumerated from [`SWEEP_SEED`].
#[tokio::test]
async fn a_drawn_face_is_found_wherever_it_is_drawn() {
    let router = harness();
    let mut state = SWEEP_SEED;
    for index in 0..SWEEP_CASES {
        let case = format!("seed {SWEEP_SEED:#x} case {index}");
        let eye_span = 40.0 + (next(&mut state) % 30) as f32;
        // Inside the frame with room for the whole head, whose half-height is 1.45 spans.
        let margin_x = eye_span * 1.2;
        let margin_y = eye_span * 1.7;
        let eyes_x = margin_x + (next(&mut state) % (320 - 2 * margin_x as u64).max(1)) as f32;
        let eyes_y = margin_y + (next(&mut state) % (320 - 2 * margin_y as u64).max(1)) as f32;
        let roll = (next(&mut state) % 25) as f32 - 12.0;
        // A skin tone anywhere across the range, with its background either side of it and
        // features darker than it: the structure is what is detected, not a tone.
        let skin = 70 + (next(&mut state) % 160) as u8;
        let background = if next(&mut state).is_multiple_of(2) {
            skin.saturating_sub(60)
        } else {
            skin.saturating_add(60)
        };
        let feature = skin.saturating_sub(70);
        let drawn = DrawnFace::new(eyes_x, eyes_y, eye_span).rolled(roll);
        let portrait = Portrait::one_face()
            .with_tones(background, skin, feature)
            .with_faces(vec![drawn]);
        let reply = detect(&router, bytes_image(&portrait.png())).await;
        let faces = faces_of(&case, &reply);
        assert_eq!(
            faces.len(),
            1,
            "{case}: one face drawn at ({eyes_x:.0}, {eyes_y:.0}) span {eye_span:.0} tone \
             {skin} on {background} must be found once, got {faces:?}"
        );
        assert_describes(&case, &faces[0], &drawn, &portrait);
    }
}

/// The sweep's generator: xorshift64, so every case is enumerated from the pinned seed and a
/// failure reproduces exactly.
pub(crate) fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
