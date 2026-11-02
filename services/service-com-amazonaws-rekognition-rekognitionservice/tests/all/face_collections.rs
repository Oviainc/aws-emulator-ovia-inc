// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Rekognition's face-collection lifecycle over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Operation>`),
//! dispatched through the same [`MultiServiceRouter`] the production emulator mounts.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body: nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! Each test states a RULE and asserts it over a TABLE of inputs, of which the
//! `my-faces` / `alice` / `bob` case from the bootstrap issue is one row:
//!
//! * a collection is a stored resource: what `CreateCollection` recorded (its ARN, its
//!   creation time, its tags, its face model version) is what `DescribeCollection`,
//!   `ListCollections` and `ListTagsForResource` report, a second create of the same id
//!   is refused, and after `DeleteCollection` the id names nothing — with the create's own
//!   answer carrying the collection's ARN WITHOUT the leading `arn:` label, the form
//!   Rekognition's `CreateCollection` reports, which still names the collection when it is
//!   handed back to the tagging path;
//! * `IndexFaces` stores one face per face the DETECTOR found in the supplied image — so
//!   an image carrying none stores none and an image carrying several stores each — and
//!   reports what it stored: a fresh uuid `FaceId`/`ImageId`, the `ExternalImageId`
//!   carried through and the box and confidence measured for the face, with every member
//!   nothing measured left absent;
//! * a page is cut out of the collection's insertion order: `MaxResults` caps it (and
//!   its published lower bound of 0 asks for an empty page, answered with no cursor), a
//!   returned `NextToken` resumes exactly where the page ended whatever happens in
//!   between, and a token the service did not mint is refused;
//! * `SearchFaces` reports, out of the same collection only, the faces indexed from the
//!   SAME image content as the searched face, at `Similarity` 100 — never the searched
//!   face, never another collection's, never a face from different content, whatever
//!   the threshold — capped by `MaxFaces`;
//! * `DeleteFaces` reports only the ids it actually removed, always as a present list,
//!   and the removal shows up in every later read;
//! * the store is bounded: past its collection and per-collection face capacity a
//!   create or an index is refused with the modelled quota error and stores nothing.
//!
//! The identity sweep (`an_indexed_face_*`) runs a bounded number of generated cases
//! from the pinned [`SWEEP_SEED`] through a three-line xorshift rather than pulling in a
//! generator crate: the cases are enumerated deterministically, so a failure names the
//! seed and the case index and reproduces exactly.
//!
//! Every image submitted here is a PORTRAIT drawn by the test — a head, two eyes, a mouth,
//! encoded as a real PNG or JPEG — through the harness the detection suite owns, so what
//! the collection ends up holding is compared against what the test drew.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;
use service_com_amazonaws_rekognition_rekognitionservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The drawn portraits, their encoder and the object-store harness come from the detection
// suite, so a picture is drawn one way whichever operation is being exercised.
use crate::face_detection::{
    blurred, encode, harness_with_object_store, s3, DrawnFace, Portrait, BUCKET,
};
use image::ImageFormat;

/// The region every call signs with unless the test is exercising the region rule.
const REGION: &str = "us-east-1";

/// `MaxResults` is `range 0..4096` on `ListFaces` / `ListCollections`
/// (`smithy_query(query="operation", operation="ListFaces,ListCollections")`).
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 4096;

/// `MaxFaces` is `range 1..4096` on `SearchFaces` (same model query).
const MAX_FACES_MIN: i64 = 1;
const MAX_FACES_MAX: i64 = 4096;

/// `FaceMatchThreshold` is `Percent`, `range 0..100` (same model query).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// The seed the generated-case sweep starts from, and the number of cases it runs.
const SWEEP_SEED: u64 = 0x5245_4b4f_474e_4954;
const SWEEP_CASES: usize = 24;

/// The face model version a collection created now is associated with.
/// `CreateCollection` "associates [a new collection] with the latest version of the face
/// model version", and the generation Rekognition answers a create with is 7.0 — the
/// value a recorded create of a new collection reports.
const LATEST_FACE_MODEL_VERSION: &str = "7.0";

/// The side of the portraits this suite draws, and the eye span it draws them with: the
/// smallest pair the detection suite pins as found at every scale, so a test that indexes
/// many faces stays cheap.
const PORTRAIT_SIDE: u32 = 160;
const PORTRAIT_EYE_SPAN: f32 = 28.0;

/// The emulator, with this one service mounted on a shared engine.
pub(crate) fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the
    // SigV4 credential scope below, which is also where an ARN's region comes from.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Reply {
    /// The wire error code (`__type`), without its namespace.
    pub(crate) fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.field(name).and_then(Value::as_str).map(str::to_string)
    }

    pub(crate) fn list(&self, name: &str) -> Vec<Value> {
        self.field(name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// One row of the collection-lifecycle table: an id, the tags created with it and the
/// region the call is signed in.
struct CollectionCase {
    collection_id: String,
    tags: Vec<(&'static str, &'static str)>,
    region: &'static str,
}

/// One row of the `DeleteFaces` table: which ids the request names, which of them the
/// collection held and so were removed, and whether the answer reports any face that
/// survived the request.
struct DeleteCase {
    case: &'static str,
    requested: Vec<String>,
    deleted: Vec<String>,
    unsuccessful: Vec<String>,
}

pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Reply {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/rekognition/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("rekognition.{region}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RekognitionService.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    Reply { status, body }
}

pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Reply {
    call_in(router, REGION, operation, body).await
}

/// An image passed inline, the way an SDK passes `Image.Bytes` on a JSON protocol:
/// base64 of the bytes.
pub(crate) fn bytes_image(content: &[u8]) -> Value {
    json!({ "Bytes": aws_smithy_types::base64::encode(content) })
}

/// The ARN a collection created in `region` is STORED and described under, per the format
/// the bootstrap issue and the service's `arnNamespace` give:
/// `arn:aws:rekognition:<region>:<account>:collection/<id>`.
fn expected_arn(region: &str, collection_id: &str) -> String {
    format!(
        "arn:aws:rekognition:{region}:{}:collection/{collection_id}",
        engine::emulator_account_id()
    )
}

/// The ARN `CreateCollection`'s own answer reports: the same ARN without the leading
/// `arn:` label, which is the form that operation reports it in.
fn expected_created_arn(region: &str, collection_id: &str) -> String {
    expected_arn(region, collection_id)
        .strip_prefix("arn:")
        .expect("the stored ARN carries the label")
        .to_string()
}

/// Where [`one_face_portrait`] puts the eyes of the one face it draws for `seed`.
fn one_face_eyes(seed: u32) -> (f32, f32) {
    let across = (seed % 9) as f32 - 4.0;
    let down = ((seed / 9) % 9) as f32 - 4.0;
    let side = f32::from(u16::try_from(PORTRAIT_SIDE).expect("the side is small"));
    (side / 2.0 + across, side * 0.47 + down)
}

/// A portrait carrying exactly ONE drawn face, placed at `seed`'s own spot: two seeds are
/// two different images, and one seed repeated is the same image content again — which is
/// what the same-image relation `SearchFaces` reports is about.
pub(crate) fn one_face_portrait(seed: u32) -> Portrait {
    let (eyes_x, eyes_y) = one_face_eyes(seed);
    Portrait::one_face()
        .with_size(PORTRAIT_SIDE, PORTRAIT_SIDE)
        .with_faces(vec![DrawnFace::new(eyes_x, eyes_y, PORTRAIT_EYE_SPAN)])
}

/// The PNG bytes of [`one_face_portrait`].
pub(crate) fn one_face_png(seed: u32) -> Vec<u8> {
    one_face_portrait(seed).png()
}

async fn create_collection(router: &MultiServiceRouter, collection_id: &str) -> Reply {
    call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await
}

/// Index the one-face portrait `seed` draws and answer the `FaceId` it minted.
async fn index_face(
    router: &MultiServiceRouter,
    collection_id: &str,
    seed: u32,
    external_image_id: Option<&str>,
) -> String {
    let mut body = json!({
        "CollectionId": collection_id,
        "Image": bytes_image(&one_face_png(seed)),
    });
    if let Some(external) = external_image_id {
        body["ExternalImageId"] = json!(external);
    }
    let reply = call(router, "IndexFaces", body).await;
    assert_eq!(
        reply.status, 200,
        "indexing into {collection_id} must succeed, got {:?}",
        reply.body
    );
    let records = reply.list("FaceRecords");
    assert_eq!(
        records.len(),
        1,
        "case {collection_id}/{external_image_id:?}: the portrait carries one face, so one \
         face must be indexed, got {records:?}"
    );
    face_member(&records[0], "FaceId")
}

/// A member of the `Face` inside a `FaceRecord`, as a string.
fn face_member(record: &Value, member: &str) -> String {
    record
        .get("Face")
        .and_then(|face| face.get(member))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn face_id_of(face: &Value) -> String {
    face.get("FaceId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(FaceId, Similarity)` pairs of a `SearchFaces` answer.
fn matches_of(reply: &Reply) -> Vec<(String, f64)> {
    reply
        .list("FaceMatches")
        .iter()
        .map(|entry| {
            (
                face_id_of(entry.get("Face").unwrap_or(&Value::Null)),
                entry
                    .get("Similarity")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
            )
        })
        .collect()
}

/// The shape `FaceId` is constrained to:
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="operation", operation="SearchFaces")`), plus the version and
/// variant nibbles of the random uuid form the service mints — so an id that is merely
/// hex and hyphens does not pass.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            == [
                groups[0].len(),
                groups[1].len(),
                groups[2].len(),
                groups[3].len(),
                groups[4].len(),
            ]
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        && groups[2].starts_with('4')
        && groups[3]
            .starts_with(['8', '9', 'a', 'b'])
}

/// Every `FaceDetail` member a classifier would have had to judge: nothing here runs one,
/// so a reported face carries none of them
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

/// Assert that a reported face carries its minted identity and the two measurements the
/// detector takes for a stored face, and that the box is a box of fractions of the image.
fn assert_measured_face(case: &str, face: &Value) {
    assert!(
        is_uuid(&face_id_of(face)),
        "case {case:?}: FaceId must be a uuid, got {face:?}"
    );
    let confidence = face
        .get("Confidence")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN);
    assert!(
        (0.0..=PERCENT_MAX).contains(&confidence),
        "case {case:?}: the detector's measured confidence must be a Percent, got {face:?}"
    );
    let rect = reported_box(case, face);
    assert!(
        rect.0 >= 0.0
            && rect.1 >= 0.0
            && rect.2 > 0.0
            && rect.3 > 0.0
            && rect.0 + rect.2 <= 1.0 + 1e-3
            && rect.1 + rect.3 <= 1.0 + 1e-3,
        "case {case:?}: the box must be the fractions of the image the face covers, \
         got {rect:?}"
    );
}

/// A reported `BoundingBox`, as the `(left, top, width, height)` fractions it carries.
fn reported_box(case: &str, face: &Value) -> (f64, f64, f64, f64) {
    let rect = face
        .get("BoundingBox")
        .unwrap_or_else(|| panic!("case {case:?}: a detected face carries a box, got {face:?}"));
    let member = |name: &str| {
        rect.get(name)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: the box carries {name}, got {rect:?}"))
    };
    (
        member("Left"),
        member("Top"),
        member("Width"),
        member("Height"),
    )
}

/// Assert that the box reported for `face` is where the test drew the face: the drawn eye
/// centre, in the fractions of the image the box is expressed in, lies inside it.
fn assert_box_covers(case: &str, face: &Value, size: (u32, u32), eyes: (f32, f32)) {
    let (left, top, width, height) = reported_box(case, face);
    let x = f64::from(eyes.0) / f64::from(size.0);
    let y = f64::from(eyes.1) / f64::from(size.1);
    assert!(
        (left..=left + width).contains(&x) && (top..=top + height).contains(&y),
        "case {case:?}: the drawn eyes at ({x:.3}, {y:.3}) must lie inside the reported box \
         ({left:.3}, {top:.3}, {width:.3}, {height:.3})"
    );
}

/// Assert that a reported `FaceDetail` carries what this detector measures and nothing a
/// classifier would have had to judge.
fn assert_measured_detail(case: &str, detail: &Value) {
    for member in ["BoundingBox", "Confidence", "Landmarks", "Pose", "Quality"] {
        assert!(
            detail.get(member).is_some_and(|value| !value.is_null()),
            "case {case:?}: the detail must carry the measured {member}, got {detail:?}"
        );
    }
    for member in CLASSIFIED_MEMBERS {
        assert!(
            detail.get(member).is_none_or(Value::is_null),
            "case {case:?}: {member} is judged by no classifier here, so it must be absent, \
             got {detail:?}"
        );
    }
}

/// The `Quality.Sharpness` a reported `FaceDetail` measured.
fn measured_sharpness(case: &str, detail: &Value) -> f64 {
    detail
        .get("Quality")
        .and_then(|quality| quality.get("Sharpness"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the detail measures a sharpness, got {detail:?}"))
}

/// The sweep's generator: xorshift64, so every case is enumerated from the pinned seed
/// and a failure reproduces exactly.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[tokio::test]
async fn a_collection_is_a_stored_resource_every_reader_agrees_on() {
    let router = harness();

    // Rows differ along what the rule generalises over: the id's form and length
    // (including the modelled 1- and 255-character bounds of `CollectionId`), the number
    // of tags, and the region the request is signed in — which is where the ARN's region
    // comes from. `my-faces` is the bootstrap issue's own row.
    let long_id = "c".repeat(255);
    let cases: Vec<CollectionCase> = vec![
        CollectionCase {
            collection_id: "my-faces".to_string(),
            tags: vec![],
            region: REGION,
        },
        CollectionCase {
            collection_id: "Team.Faces_2026-Q1".to_string(),
            tags: vec![("env", "test"), ("owner", "qa")],
            region: "eu-west-1",
        },
        CollectionCase {
            collection_id: "x".to_string(),
            tags: vec![("only", "one")],
            region: "us-west-2",
        },
        CollectionCase {
            collection_id: long_id,
            tags: vec![],
            region: "ap-southeast-2",
        },
    ];

    for CollectionCase {
        collection_id,
        tags,
        region,
    } in &cases
    {
        let tag_map: serde_json::Map<String, Value> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect();
        let mut body = json!({ "CollectionId": collection_id });
        if !tag_map.is_empty() {
            body["Tags"] = Value::Object(tag_map.clone());
        }

        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_secs_f64();
        let created = call_in(&router, region, "CreateCollection", body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {collection_id:?}: create must succeed, got {:?}",
            created.body
        );
        assert_eq!(
            created.field("StatusCode").and_then(Value::as_i64),
            Some(200),
            "case {collection_id:?}: StatusCode must report the successful create, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("CollectionArn"),
            Some(expected_created_arn(region, collection_id)),
            "case {collection_id:?}: the create's ARN must name the request's region and the \
             collection, without the arn: label, got {:?}",
            created.body
        );
        let face_model_version = created.string("FaceModelVersion").unwrap_or_else(|| {
            panic!(
                "case {collection_id:?}: create must report a FaceModelVersion, got {:?}",
                created.body
            )
        });
        assert_eq!(
            face_model_version, LATEST_FACE_MODEL_VERSION,
            "case {collection_id:?}: a new collection is associated with the latest face model \
             version, got {face_model_version:?}"
        );

        // A second create of the same id is a duplicate, not an update.
        let again = call_in(&router, region, "CreateCollection", body).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, "ResourceAlreadyExistsException"),
            "case {collection_id:?}: re-creating the id must be refused, got {:?}",
            again.body
        );

        let described = call_in(
            &router,
            region,
            "DescribeCollection",
            json!({ "CollectionId": collection_id }),
        )
        .await;
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_secs_f64();
        assert_eq!(
            described.status, 200,
            "case {collection_id:?}: describe must succeed, got {:?}",
            described.body
        );
        assert_eq!(
            described.string("CollectionARN"),
            Some(expected_arn(region, collection_id)),
            "case {collection_id:?}: describe must report the ARN create minted, got {:?}",
            described.body
        );
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(0),
            "case {collection_id:?}: a new collection holds no faces, got {:?}",
            described.body
        );
        assert_eq!(
            described.field("UserCount").and_then(Value::as_i64),
            Some(0),
            "case {collection_id:?}: a new collection holds no users, got {:?}",
            described.body
        );
        assert_eq!(
            described.string("FaceModelVersion"),
            Some(face_model_version.clone()),
            "case {collection_id:?}: describe must echo the created face model version, got {:?}",
            described.body
        );
        let created_at = described
            .field("CreationTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        assert!(
            created_at >= before && created_at <= after,
            "case {collection_id:?}: CreationTimestamp must be the time of the create \
             ({before}..={after}), got {created_at}"
        );

        // Both forms of the ARN name the collection: the canonical one every reader
        // reports, and the one the create handed the caller to tag the resource by.
        for (arn_case, resource_arn) in [
            ("the stored ARN", expected_arn(region, collection_id)),
            (
                "the ARN the create answered with",
                expected_created_arn(region, collection_id),
            ),
        ] {
            let listed_tags = call_in(
                &router,
                region,
                "ListTagsForResource",
                json!({ "ResourceArn": resource_arn }),
            )
            .await;
            assert_eq!(
                listed_tags.status, 200,
                "case {collection_id:?}/{arn_case}: the ARN must resolve, got {:?}",
                listed_tags.body
            );
            assert_eq!(
                listed_tags.field("Tags").cloned().unwrap_or(json!({})),
                Value::Object(tag_map.clone()),
                "case {collection_id:?}/{arn_case}: the tags create persisted must be the tags \
                 reported, got {:?}",
                listed_tags.body
            );
        }
    }

    // An ARN no stored resource carries is not found.
    let stray = call(
        &router,
        "ListTagsForResource",
        json!({ "ResourceArn": expected_arn(REGION, "never-created") }),
    )
    .await;
    assert_eq!(
        (stray.status, stray.error_code().as_str()),
        (400, "ResourceNotFoundException"),
        "an ARN naming no stored resource must not be found, got {:?}",
        stray.body
    );

    // Every live collection, and only those, is listed — with each one's face model
    // version in the matching position.
    let listed = call(&router, "ListCollections", json!({})).await;
    let ids: Vec<String> = listed
        .list("CollectionIds")
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    let expected_ids: BTreeSet<String> = cases
        .iter()
        .map(|case| case.collection_id.clone())
        .collect();
    assert_eq!(
        ids.iter().cloned().collect::<BTreeSet<String>>(),
        expected_ids,
        "ListCollections must list exactly the live collections, got {ids:?}"
    );
    assert_eq!(
        listed.list("FaceModelVersions").len(),
        ids.len(),
        "FaceModelVersions must carry one entry per listed collection, got {:?}",
        listed.body
    );

    // Deleting one removes it from every reader, and frees the id for re-use.
    let deleted = call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "my-faces" }),
    )
    .await;
    assert_eq!(
        (
            deleted.status,
            deleted.field("StatusCode").and_then(Value::as_i64)
        ),
        (200, Some(200)),
        "delete must report the successful removal, got {:?}",
        deleted.body
    );
    for (operation, body) in [
        ("DescribeCollection", json!({ "CollectionId": "my-faces" })),
        ("ListFaces", json!({ "CollectionId": "my-faces" })),
        (
            "IndexFaces",
            json!({ "CollectionId": "my-faces", "Image": bytes_image(b"after-delete") }),
        ),
        (
            "DeleteFaces",
            json!({
                "CollectionId": "my-faces",
                "FaceIds": ["00000000-0000-4000-8000-000000000000"],
            }),
        ),
        ("DeleteCollection", json!({ "CollectionId": "my-faces" })),
    ] {
        let reply = call(&router, operation, body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation}: a deleted collection must not be found, got {:?}",
            reply.body
        );
    }
    let relisted = call(&router, "ListCollections", json!({})).await;
    assert!(
        !relisted
            .list("CollectionIds")
            .iter()
            .any(|id| id.as_str() == Some("my-faces")),
        "a deleted collection must not be listed, got {:?}",
        relisted.body
    );
    assert_eq!(
        create_collection(&router, "my-faces").await.status,
        200,
        "the id of a deleted collection must be free again"
    );
}

/// One step of the tagging round trip: which form of the resource's ARN the request uses,
/// what it does, and the tags the resource must carry afterwards.
struct TagStep {
    case: &'static str,
    label_less_arn: bool,
    operation: &'static str,
    body: Value,
    expected: Value,
}

/// The tags `ListTagsForResource` reports for the resource `resource_arn` names.
async fn tags_of(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let reply = call(
        router,
        "ListTagsForResource",
        json!({ "ResourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "{resource_arn} must resolve, got {:?}",
        reply.body
    );
    reply.field("Tags").cloned().unwrap_or(json!({}))
}

/// A resource's tags are a stored set: `TagResource` adds and updates keys in it,
/// `UntagResource` removes them, and `ListTagsForResource` reports exactly what is left —
/// whichever form of the resource's ARN the request names it by.
#[tokio::test]
async fn tags_are_a_stored_set_the_tagging_operations_change() {
    let router = harness();
    let created = call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": "tagged", "Tags": { "env": "test" } }),
    )
    .await;
    assert_eq!(
        created.string("CollectionArn"),
        Some(expected_created_arn(REGION, "tagged")),
        "the create reports the ARN a caller then tags by, got {:?}",
        created.body
    );
    // A second collection, so every step can show that one resource's tags are its own.
    create_collection(&router, "untouched").await;
    call(
        &router,
        "TagResource",
        json!({
            "ResourceArn": expected_arn(REGION, "untouched"),
            "Tags": { "keep": "me" },
        }),
    )
    .await;

    assert_eq!(
        tags_of(&router, &expected_created_arn(REGION, "tagged")).await,
        json!({ "env": "test" }),
        "the create's own tags are the set the round trip starts from"
    );

    // Rows differ along what the request changes — several new keys, a key the resource
    // already carries, none at all, one key removed, a key removed twice, a key never
    // set, every key removed — and along which form of the ARN names the resource.
    let steps: Vec<TagStep> = vec![
        TagStep {
            case: "two new keys",
            label_less_arn: true,
            operation: "TagResource",
            body: json!({ "Tags": { "owner": "qa", "team": "vision" } }),
            expected: json!({ "env": "test", "owner": "qa", "team": "vision" }),
        },
        TagStep {
            case: "a key the resource already carries",
            label_less_arn: false,
            operation: "TagResource",
            body: json!({ "Tags": { "env": "prod" } }),
            expected: json!({ "env": "prod", "owner": "qa", "team": "vision" }),
        },
        TagStep {
            case: "no tags at all",
            label_less_arn: true,
            operation: "TagResource",
            body: json!({ "Tags": {} }),
            expected: json!({ "env": "prod", "owner": "qa", "team": "vision" }),
        },
        TagStep {
            case: "one key removed",
            label_less_arn: false,
            operation: "UntagResource",
            body: json!({ "TagKeys": ["owner"] }),
            expected: json!({ "env": "prod", "team": "vision" }),
        },
        TagStep {
            case: "the same key again, beside one never set",
            label_less_arn: true,
            operation: "UntagResource",
            body: json!({ "TagKeys": ["owner", "never-set"] }),
            expected: json!({ "env": "prod", "team": "vision" }),
        },
        TagStep {
            case: "no keys at all",
            label_less_arn: false,
            operation: "UntagResource",
            body: json!({ "TagKeys": [] }),
            expected: json!({ "env": "prod", "team": "vision" }),
        },
        TagStep {
            case: "every remaining key",
            label_less_arn: true,
            operation: "UntagResource",
            body: json!({ "TagKeys": ["env", "team"] }),
            expected: json!({}),
        },
        TagStep {
            case: "a key added back after the set was emptied",
            label_less_arn: false,
            operation: "TagResource",
            body: json!({ "Tags": { "env": "test" } }),
            expected: json!({ "env": "test" }),
        },
    ];

    for TagStep {
        case,
        label_less_arn,
        operation,
        body,
        expected,
    } in steps
    {
        let arn = if label_less_arn {
            expected_created_arn(REGION, "tagged")
        } else {
            expected_arn(REGION, "tagged")
        };
        let mut request = body;
        request["ResourceArn"] = json!(arn);
        let reply = call(&router, operation, request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {operation} must succeed, got {:?}",
            reply.body
        );
        // Both ARN forms report the one stored set.
        for arn in [
            expected_arn(REGION, "tagged"),
            expected_created_arn(REGION, "tagged"),
        ] {
            assert_eq!(
                tags_of(&router, &arn).await,
                expected,
                "case {case:?}: {arn} must report the tags the request left, after {operation}"
            );
        }
        assert_eq!(
            tags_of(&router, &expected_arn(REGION, "untouched")).await,
            json!({ "keep": "me" }),
            "case {case:?}: another resource's tags are its own"
        );
    }

    // An ARN no live resource carries is not found, by either tagging operation, and
    // nothing is written for it.
    for (operation, body) in [
        (
            "TagResource",
            json!({
                "ResourceArn": expected_arn(REGION, "never-created"),
                "Tags": { "env": "test" },
            }),
        ),
        (
            "UntagResource",
            json!({
                "ResourceArn": expected_created_arn(REGION, "never-created"),
                "TagKeys": ["env"],
            }),
        ),
    ] {
        let reply = call(&router, operation, body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation} on an ARN naming nothing: got {:?}",
            reply.body
        );
    }

    // And a deleted collection carries no tags to change.
    call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "tagged" }),
    )
    .await;
    let after_delete = call(
        &router,
        "TagResource",
        json!({
            "ResourceArn": expected_arn(REGION, "tagged"),
            "Tags": { "env": "test" },
        }),
    )
    .await;
    assert_eq!(
        (after_delete.status, after_delete.error_code().as_str()),
        (400, "ResourceNotFoundException"),
        "tagging a deleted collection must be refused, got {:?}",
        after_delete.body
    );
}

/// One row of the `IndexFaces` table: the image submitted, its size, where the test drew
/// each of its faces (largest first) and the external id the request carried.
struct IndexCase {
    case: &'static str,
    bytes: Vec<u8>,
    size: (u32, u32),
    drawn: Vec<(f32, f32)>,
    external: Option<&'static str>,
}

#[tokio::test]
async fn index_faces_stores_the_faces_the_detector_found() {
    let router = harness();
    create_collection(&router, "my-faces").await;

    // Rows differ along what the image CARRIES — one drawn face, the same portrait again,
    // another portrait, two faces of different sizes, no face at all — how it is encoded
    // (PNG, JPEG) and whether the request names an `ExternalImageId`. How many faces the
    // test drew is how many must be indexed.
    let pair = Portrait::one_face().with_size(480, 320).with_faces(vec![
        DrawnFace::new(130.0, 150.0, 56.0),
        DrawnFace::new(350.0, 140.0, 36.0),
    ]);
    let cases: Vec<IndexCase> = vec![
        IndexCase {
            case: "one drawn face, PNG",
            bytes: one_face_png(1),
            size: (PORTRAIT_SIDE, PORTRAIT_SIDE),
            drawn: vec![one_face_eyes(1)],
            external: Some("alice"),
        },
        IndexCase {
            case: "the same portrait again",
            bytes: one_face_png(1),
            size: (PORTRAIT_SIDE, PORTRAIT_SIDE),
            drawn: vec![one_face_eyes(1)],
            external: Some("alice.twin"),
        },
        IndexCase {
            case: "another portrait, no external id",
            bytes: one_face_png(2),
            size: (PORTRAIT_SIDE, PORTRAIT_SIDE),
            drawn: vec![one_face_eyes(2)],
            external: None,
        },
        IndexCase {
            case: "one drawn face, JPEG",
            bytes: encode(&one_face_portrait(3).render(), ImageFormat::Jpeg),
            size: (PORTRAIT_SIDE, PORTRAIT_SIDE),
            drawn: vec![one_face_eyes(3)],
            external: Some("carol"),
        },
        IndexCase {
            case: "two faces of different sizes",
            bytes: pair.png(),
            size: (480, 320),
            drawn: vec![(130.0, 150.0), (350.0, 140.0)],
            external: Some("pair"),
        },
        IndexCase {
            case: "an image carrying no face",
            bytes: Portrait::blank(240, 240, 200).png(),
            size: (240, 240),
            drawn: vec![],
            external: Some("nobody"),
        },
    ];

    let mut seen_face_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_image_ids: BTreeSet<String> = BTreeSet::new();
    let mut stored_so_far = 0_i64;

    for IndexCase {
        case,
        bytes,
        size,
        drawn,
        external,
    } in &cases
    {
        let mut body = json!({ "CollectionId": "my-faces", "Image": bytes_image(bytes) });
        if let Some(external) = external {
            body["ExternalImageId"] = json!(external);
        }
        let reply = call(&router, "IndexFaces", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: indexing must succeed, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("FaceModelVersion").as_deref(),
            Some(LATEST_FACE_MODEL_VERSION),
            "case {case:?}: the collection's face model version must be reported, got {:?}",
            reply.body
        );

        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            drawn.len(),
            "case {case:?}: {} face(s) were drawn, so {} must be indexed, got {records:?}",
            drawn.len(),
            drawn.len()
        );
        assert!(
            reply.list("UnindexedFaces").is_empty(),
            "case {case:?}: no filter and no cap applied, so UnindexedFaces is empty, got {:?}",
            reply.body
        );
        assert!(
            reply.field("OrientationCorrection").is_none(),
            "case {case:?}: no orientation information is reported, got {:?}",
            reply.body
        );

        // One `ImageId` per call, shared by every face indexed from that image.
        let image_ids: BTreeSet<String> = records
            .iter()
            .map(|record| face_member(record, "ImageId"))
            .collect();
        assert_eq!(
            image_ids.len(),
            usize::from(!records.is_empty()),
            "case {case:?}: every face of one image shares that image's id, got {records:?}"
        );
        for image_id in &image_ids {
            assert!(
                is_uuid(image_id),
                "case {case:?}: ImageId must be a uuid, got {image_id:?}"
            );
            assert!(
                seen_image_ids.insert(image_id.clone()),
                "case {case:?}: every indexed image gets a fresh ImageId, {image_id:?} was \
                 seen before"
            );
        }

        for (at, eyes) in drawn.iter().enumerate() {
            let record = &records[at];
            let face = record.get("Face").cloned().unwrap_or(Value::Null);
            assert_measured_face(case, &face);
            // The faces come back largest first, so the box at this position is the one
            // the test drew at this position.
            assert_box_covers(case, &face, *size, *eyes);
            let detail = record
                .get("FaceDetail")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: a detected face carries its detail"));
            assert_measured_detail(case, &detail);
            assert_eq!(
                detail.get("BoundingBox"),
                face.get("BoundingBox"),
                "case {case:?}: the stored face's box is the box that was measured, \
                 got {record:?}"
            );

            let face_id = face_id_of(&face);
            assert!(
                seen_face_ids.insert(face_id.clone()),
                "case {case:?}: every indexed face gets a fresh FaceId, {face_id:?} was \
                 seen before"
            );
            assert_eq!(
                face.get("ExternalImageId").and_then(Value::as_str),
                *external,
                "case {case:?}: ExternalImageId must be carried through unchanged, got {face:?}"
            );
            assert_eq!(
                face.get("IndexFacesModelVersion").and_then(Value::as_str),
                Some(LATEST_FACE_MODEL_VERSION),
                "case {case:?}: the stored face must carry the collection's face model \
                 version, got {face:?}"
            );

            // What was reported is what was stored: the whole face comes back through
            // `ListFaces` unchanged.
            let listed = call(&router, "ListFaces", json!({ "CollectionId": "my-faces" })).await;
            let stored = listed
                .list("Faces")
                .into_iter()
                .find(|candidate| face_id_of(candidate) == face_id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the indexed face must be listed, got {:?}",
                        listed.body
                    )
                });
            assert_eq!(
                stored, face,
                "case {case:?}: ListFaces must report the stored face exactly as the index \
                 reported it"
            );
        }

        stored_so_far += drawn.len() as i64;
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": "my-faces" }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(stored_so_far),
            "case {case:?}: FaceCount must count the faces stored so far, got {:?}",
            described.body
        );
    }
}

/// One row of the `MaxFaces` table: the image submitted, the cap the request names, and
/// how many of the image's faces that leaves indexed and reported as unindexed.
struct CapCase<'a> {
    case: &'static str,
    bytes: &'a [u8],
    max_faces: Option<i64>,
    indexed: usize,
    unindexed: usize,
}

impl<'a> CapCase<'a> {
    fn new(
        case: &'static str,
        bytes: &'a [u8],
        max_faces: Option<i64>,
        indexed: usize,
        unindexed: usize,
    ) -> Self {
        Self {
            case,
            bytes,
            max_faces,
            indexed,
            unindexed,
        }
    }
}

/// `MaxFaces` caps how many of an image's faces are indexed, and the faces it left no
/// room for are reported as unindexed rather than dropped.
#[tokio::test]async fn index_faces_caps_what_it_indexes_at_max_faces() {
    let router = harness();
    let pair = Portrait::one_face().with_size(480, 320).with_faces(vec![
        DrawnFace::new(130.0, 150.0, 56.0),
        DrawnFace::new(350.0, 140.0, 36.0),
    ]);
    let pair_bytes = pair.png();
    let one_bytes = one_face_png(4);

    // Rows: the cap below, at and above the number of faces drawn, plus the model's own
    // maximum and no cap at all.
    let cases: Vec<CapCase> = vec![
        CapCase::new("two faces, cap of one", &pair_bytes, Some(1), 1, 1),
        CapCase::new("two faces, cap of two", &pair_bytes, Some(2), 2, 0),
        CapCase::new("two faces, cap of three", &pair_bytes, Some(3), 2, 0),
        CapCase::new(
            "two faces, the largest cap the model allows",
            &pair_bytes,
            Some(MAX_FACES_MAX),
            2,
            0,
        ),
        CapCase::new("two faces, no cap", &pair_bytes, None, 2, 0),
        CapCase::new("one face, cap of one", &one_bytes, Some(MAX_FACES_MIN), 1, 0),
        CapCase::new("one face, cap of three", &one_bytes, Some(3), 1, 0),
    ];

    for CapCase {
        case,
        bytes,
        max_faces,
        indexed,
        unindexed,
    } in cases
    {
        let collection = format!("cap-{}", case.replace([' ', ','], "-"));
        assert_eq!(
            create_collection(&router, &collection).await.status,
            200,
            "case {case:?}: the collection must be created"
        );
        let mut body = json!({ "CollectionId": collection, "Image": bytes_image(bytes) });
        if let Some(max) = max_faces {
            body["MaxFaces"] = json!(max);
        }
        let reply = call(&router, "IndexFaces", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: indexing must succeed, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.list("FaceRecords").len(),
            indexed,
            "case {case:?}: the cap must limit what is indexed, got {:?}",
            reply.body
        );
        let left_out = reply.list("UnindexedFaces");
        assert_eq!(
            left_out.len(),
            unindexed,
            "case {case:?}: the faces the cap left no room for must be reported, got {:?}",
            reply.body
        );
        for entry in &left_out {
            assert_eq!(
                entry.get("Reasons").and_then(Value::as_array),
                Some(&vec![json!("EXCEEDS_MAX_FACES")]),
                "case {case:?}: a face past the cap is EXCEEDS_MAX_FACES, got {entry:?}"
            );
            assert_measured_detail(
                case,
                entry
                    .get("FaceDetail")
                    .unwrap_or_else(|| panic!("case {case:?}: an unindexed face carries its detail")),
            );
        }
        // Only the faces that were indexed are in the collection.
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": collection }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(indexed as i64),
            "case {case:?}: only the indexed faces are stored, got {:?}",
            described.body
        );
    }
}

/// A face whose measured sharpness is below the bar the `QualityFilter` sets is not
/// indexed, and is reported as unindexed with the reason for the quality that was
/// measured.
#[tokio::test]
async fn index_faces_filters_a_face_below_the_quality_bar() {
    let router = harness();
    let sharp = one_face_portrait(5);
    let images: Vec<(&str, Vec<u8>)> = vec![
        ("a sharp portrait", sharp.png()),
        (
            "the same portrait blurred",
            encode(&blurred(&sharp.render(), 3), ImageFormat::Png),
        ),
    ];

    // Every level the model names, plus "no filter at all"
    // (`smithy_query(query="shape",shape="QualityFilter")`: NONE | AUTO | LOW | MEDIUM |
    // HIGH).
    let filters: [Option<&str>; 6] = [
        None,
        Some("NONE"),
        Some("LOW"),
        Some("MEDIUM"),
        Some("HIGH"),
        Some("AUTO"),
    ];

    for (image_case, bytes) in &images {
        // What the detector measures for this image, learnt from the one filter that
        // filters nothing: the bar is compared against this.
        let unfiltered = format!("unfiltered-{}", image_case.replace(' ', "-"));
        create_collection(&router, &unfiltered).await;
        let measured = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": unfiltered,
                "Image": bytes_image(bytes),
                "QualityFilter": "NONE",
            }),
        )
        .await;
        let records = measured.list("FaceRecords");
        assert_eq!(
            records.len(),
            1,
            "case {image_case:?}: the drawn face is found whatever its sharpness, got {:?}",
            measured.body
        );
        let sharpness = measured_sharpness(
            image_case,
            records[0]
                .get("FaceDetail")
                .expect("a detected face carries its detail"),
        );

        for filter in filters {
            let case = format!("{image_case} with QualityFilter={filter:?}");
            let bar = f64::from(state::face_detection::quality_bar(filter));
            let collection = format!("quality-{}", case.replace([' ', '"', '=', ')', '('], "-"));
            assert_eq!(
                create_collection(&router, &collection).await.status,
                200,
                "case {case:?}: the collection must be created"
            );
            let mut body = json!({ "CollectionId": collection, "Image": bytes_image(bytes) });
            if let Some(filter) = filter {
                body["QualityFilter"] = json!(filter);
            }
            let reply = call(&router, "IndexFaces", body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: indexing must succeed, got {:?}",
                reply.body
            );
            let indexed = usize::from(sharpness >= bar);
            assert_eq!(
                reply.list("FaceRecords").len(),
                indexed,
                "case {case:?}: a face measuring {sharpness:.3} sharp is indexed exactly when \
                 it meets the bar of {bar}, got {:?}",
                reply.body
            );
            let left_out = reply.list("UnindexedFaces");
            assert_eq!(
                left_out.len(),
                1 - indexed,
                "case {case:?}: a face the filter removed must be reported, got {:?}",
                reply.body
            );
            for entry in &left_out {
                assert_eq!(
                    entry.get("Reasons").and_then(Value::as_array),
                    Some(&vec![json!("LOW_SHARPNESS")]),
                    "case {case:?}: the quality this detector measures is sharpness, \
                     got {entry:?}"
                );
            }
            let described = call(
                &router,
                "DescribeCollection",
                json!({ "CollectionId": collection }),
            )
            .await;
            assert_eq!(
                described.field("FaceCount").and_then(Value::as_i64),
                Some(indexed as i64),
                "case {case:?}: a filtered face is not stored either, got {:?}",
                described.body
            );
        }
    }
}

/// An image staged in the emulator's own Amazon S3 is indexed exactly like the same bytes
/// submitted inline, and an address the store does not serve is refused.
#[tokio::test]
async fn index_faces_reads_an_s3_sourced_image_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    let bytes = one_face_png(6);
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/portrait.png"), &bytes).await,
        200,
        "the image object must be stored"
    );
    create_collection(&router, "sourced").await;

    let mut boxes: Vec<(&str, Value)> = Vec::new();
    for (case, image) in [
        ("inline bytes", bytes_image(&bytes)),
        (
            "the same bytes as an S3 object",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "portrait.png" } }),
        ),
    ] {
        let reply = call(
            &router,
            "IndexFaces",
            json!({ "CollectionId": "sourced", "Image": image }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: indexing must succeed, got {:?}",
            reply.body
        );
        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: the staged portrait carries one face, got {records:?}"
        );
        let face = records[0].get("Face").cloned().unwrap_or(Value::Null);
        assert_measured_face(case, &face);
        assert_box_covers(
            case,
            &face,
            (PORTRAIT_SIDE, PORTRAIT_SIDE),
            one_face_eyes(6),
        );
        boxes.push((
            case,
            face.get("BoundingBox").cloned().unwrap_or(Value::Null),
        ));
    }
    assert_eq!(
        boxes[0].1, boxes[1].1,
        "one image answers alike whichever way it was submitted, got {boxes:?}"
    );

    // An address the store does not serve is refused, and stores nothing.
    let refused = call(
        &router,
        "IndexFaces",
        json!({
            "CollectionId": "sourced",
            "Image": { "S3Object": { "Bucket": BUCKET, "Name": "no-such-key.png" } },
        }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InvalidS3ObjectException"),
        "an object the store does not hold must be refused, got {:?}",
        refused.body
    );
    let described = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": "sourced" }),
    )
    .await;
    assert_eq!(
        described.field("FaceCount").and_then(Value::as_i64),
        Some(2),
        "the two indexed faces are the only ones stored, got {:?}",
        described.body
    );
}

#[tokio::test]
async fn index_faces_refuses_what_it_cannot_read() {
    let router = harness();
    create_collection(&router, "my-faces").await;

    // Rows: an `Image` that carries no content at all, an empty inline blob, an S3
    // object missing its key, one missing its bucket, and a collection that does not
    // exist. Each names the modelled error for its own reason.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "neither Bytes nor S3Object",
            json!({ "CollectionId": "my-faces", "Image": {} }),
            "InvalidParameterException",
            "an image with no content cannot be indexed",
        ),
        (
            "empty Bytes",
            json!({ "CollectionId": "my-faces", "Image": bytes_image(b"") }),
            // `ImageBlob` is `len 1..5242880`, so an empty blob never reaches the
            // handler: the modelled-constraint validator refuses it at the boundary.
            "ValidationException",
            "an empty blob carries no image",
        ),
        (
            "S3Object without Name",
            json!({ "CollectionId": "my-faces", "Image": { "S3Object": { "Bucket": "faces" } } }),
            "InvalidS3ObjectException",
            "an S3 object needs a key",
        ),
        (
            "S3Object without Bucket",
            json!({ "CollectionId": "my-faces", "Image": { "S3Object": { "Name": "a.jpg" } } }),
            "InvalidS3ObjectException",
            "an S3 object needs a bucket",
        ),
        (
            "unknown collection",
            json!({ "CollectionId": "no-such-collection", "Image": bytes_image(b"jpeg") }),
            "ResourceNotFoundException",
            "faces can only be indexed into a collection that exists",
        ),
        (
            "unknown collection and an unreadable image",
            json!({ "CollectionId": "no-such-collection", "Image": {} }),
            "ResourceNotFoundException",
            "the resource is resolved before the request's own parameters",
        ),
        (
            "inline bytes that are not an image",
            json!({ "CollectionId": "my-faces", "Image": bytes_image(b"not-an-image-at-all") }),
            "InvalidImageFormatException",
            "a face can only be detected in an image this service decodes",
        ),
        (
            "an S3 object the store does not serve",
            json!({
                "CollectionId": "my-faces",
                "Image": { "S3Object": { "Bucket": "no-such-bucket", "Name": "no-such-key" } },
            }),
            "InvalidS3ObjectException",
            "an object whose bytes cannot be obtained holds no face to index",
        ),
    ];

    for (case, body, expected_code, why) in cases {
        let reply = call(&router, "IndexFaces", body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, expected_code),
            "case {case:?} ({why}): got {:?}",
            reply.body
        );
    }

    let described = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": "my-faces" }),
    )
    .await;
    assert_eq!(
        described.field("FaceCount").and_then(Value::as_i64),
        Some(0),
        "a refused IndexFaces must store nothing, got {:?}",
        described.body
    );
}

#[tokio::test]
async fn list_faces_pages_the_stored_order_exactly() {
    let router = harness();
    create_collection(&router, "my-faces").await;

    let mut order = Vec::new();
    for (seed, external) in [
        (11, "one"),
        (12, "two"),
        (13, "three"),
        (14, "four"),
        (15, "five"),
    ] {
        order.push(index_face(&router, "my-faces", seed, Some(external)).await);
    }

    // Rows differ along the page size, from one face per page to more than the
    // collection holds, plus the model's maximum and "no MaxResults at all".
    for max_results in [
        Some(1),
        Some(2),
        Some(3),
        Some(5),
        Some(7),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "CollectionId": "my-faces" });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(cursor) = &token {
                body["NextToken"] = json!(cursor);
            }
            let reply = call(&router, "ListFaces", body).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?}: page {pages} must succeed, got {:?}",
                reply.body
            );
            assert!(
                reply.string("FaceModelVersion").is_some(),
                "case MaxResults={max_results:?}: every page reports the face model version, \
                 got {:?}",
                reply.body
            );
            let page: Vec<String> = reply.list("Faces").iter().map(face_id_of).collect();
            if let Some(max) = max_results {
                assert!(
                    page.len() as i64 <= max,
                    "case MaxResults={max_results:?}: a page must not exceed the cap, got {} faces",
                    page.len()
                );
            }
            walked.extend(page);
            pages += 1;
            token = reply.string("NextToken");
            assert_eq!(
                token.is_some(),
                walked.len() < order.len(),
                "case MaxResults={max_results:?}: a NextToken must come back exactly while \
                 faces remain, walked {} of {}",
                walked.len(),
                order.len()
            );
            if token.is_none() {
                break;
            }
            assert!(
                pages <= order.len() + 1,
                "case MaxResults={max_results:?}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, order,
            "case MaxResults={max_results:?}: the pages must spell the stored order with no \
             repeats and no gaps"
        );
    }

    // `MaxResults` 0 is inside the modelled range: it asks for no faces, and is
    // answered with none and with no cursor — a cursor would name the position the
    // caller is already at.
    let empty_page = call(
        &router,
        "ListFaces",
        json!({ "CollectionId": "my-faces", "MaxResults": MAX_RESULTS_MIN }),
    )
    .await;
    assert_eq!(
        (
            empty_page.status,
            empty_page.list("Faces").len(),
            empty_page.string("NextToken")
        ),
        (200, 0, None),
        "case MaxResults=0: asking for no faces answers none and ends the iteration, got {:?}",
        empty_page.body
    );

    // `FaceIds` filters, whatever the subset's size and whether it names ids the
    // collection does not hold; the order stays the stored one.
    let unknown = "11111111-1111-4111-8111-111111111111".to_string();
    let filter_cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one id", vec![order[2].clone()], vec![order[2].clone()]),
        (
            "two ids out of order",
            vec![order[3].clone(), order[0].clone()],
            vec![order[0].clone(), order[3].clone()],
        ),
        (
            "an unknown id mixed in",
            vec![unknown.clone(), order[1].clone()],
            vec![order[1].clone()],
        ),
        ("only unknown ids", vec![unknown.clone()], vec![]),
        ("every id", order.clone(), order.clone()),
    ];
    for (case, face_ids, expected) in filter_cases {
        let reply = call(
            &router,
            "ListFaces",
            json!({ "CollectionId": "my-faces", "FaceIds": face_ids }),
        )
        .await;
        let got: Vec<String> = reply.list("Faces").iter().map(face_id_of).collect();
        assert_eq!(
            got, expected,
            "case {case:?}: FaceIds must select exactly the stored faces it names, got {:?}",
            reply.body
        );
    }

    // `UserId` selects the faces associated with that user. `AssociateFaces` is not
    // implemented, so no stored face is associated with one and the filter is empty.
    let by_user = call(
        &router,
        "ListFaces",
        json!({ "CollectionId": "my-faces", "UserId": "someone" }),
    )
    .await;
    assert!(
        by_user.list("Faces").is_empty(),
        "no stored face is associated with a user, got {:?}",
        by_user.body
    );

    // A cursor names the next page's first face, so a face deleted after the token was
    // issued neither hides nor repeats a face the caller has yet to see.
    let first_page = call(
        &router,
        "ListFaces",
        json!({ "CollectionId": "my-faces", "MaxResults": 2 }),
    )
    .await;
    let token = first_page
        .string("NextToken")
        .expect("a token while faces remain");
    let dropped = call(
        &router,
        "DeleteFaces",
        json!({ "CollectionId": "my-faces", "FaceIds": [order[2]] }),
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "deleting a face must succeed, got {:?}",
        dropped.body
    );
    let resumed = call(
        &router,
        "ListFaces",
        json!({ "CollectionId": "my-faces", "NextToken": token }),
    )
    .await;
    let got: Vec<String> = resumed.list("Faces").iter().map(face_id_of).collect();
    assert_eq!(
        got,
        order[3..].to_vec(),
        "a resumed page must continue after the faces already reported, got {:?}",
        resumed.body
    );

    // A token this service did not mint names no position and is refused — including
    // a `ListCollections` cursor, which belongs to another list.
    let collections_cursor = state::encode_cursor(state::COLLECTIONS_CURSOR_SCOPE, "my-faces");
    for token in [
        "not-a-token",
        "zz",
        "6162",
        "ffffffffffffffffffffffffffff",
        "0",
        collections_cursor.as_str(),
    ] {
        let reply = call(
            &router,
            "ListFaces",
            json!({ "CollectionId": "my-faces", "NextToken": token }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidPaginationTokenException"),
            "case {token:?}: a token the service did not mint must be refused, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn list_collections_pages_the_id_order_exactly() {
    let router = harness();
    let ids = ["alpha", "bravo", "charlie", "delta"];
    for id in ids {
        assert_eq!(create_collection(&router, id).await.status, 200);
    }

    for max_results in [Some(1), Some(2), Some(3), Some(4), Some(9), None] {
        let mut walked: Vec<String> = Vec::new();
        let mut versions: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({});
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(cursor) = &token {
                body["NextToken"] = json!(cursor);
            }
            let reply = call(&router, "ListCollections", body).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?}: a page must succeed, got {:?}",
                reply.body
            );
            let page: Vec<String> = reply
                .list("CollectionIds")
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect();
            let page_versions: Vec<String> = reply
                .list("FaceModelVersions")
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                page.len(),
                page_versions.len(),
                "case MaxResults={max_results:?}: one face model version per listed collection, \
                 got {:?}",
                reply.body
            );
            walked.extend(page);
            versions.extend(page_versions);
            token = reply.string("NextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
            "case MaxResults={max_results:?}: the pages must spell every live collection once"
        );
        assert!(
            versions.iter().all(|version| !version.is_empty()),
            "case MaxResults={max_results:?}: every listed collection reports its model version, \
             got {versions:?}"
        );
    }

    // A token this list did not mint names no position — including an even-length hex
    // string that decodes to something it never issued, and a `ListFaces` cursor.
    let faces_cursor = state::encode_sequence_cursor(0);
    for token in [
        "nope",
        "zz",
        "@@",
        // Hex of "ab": it decodes, but this list never minted it.
        "6162",
        faces_cursor.as_str(),
    ] {
        let reply = call(&router, "ListCollections", json!({ "NextToken": token })).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidPaginationTokenException"),
            "case {token:?}: a token the service did not mint must be refused, got {:?}",
            reply.body
        );
    }

    // The published lower bound of `MaxResults` asks for an empty page, and is answered
    // with one and no cursor.
    let empty_page = call(
        &router,
        "ListCollections",
        json!({ "MaxResults": MAX_RESULTS_MIN }),
    )
    .await;
    assert_eq!(
        (
            empty_page.status,
            empty_page.list("CollectionIds").len(),
            empty_page.string("NextToken")
        ),
        (200, 0, None),
        "case MaxResults=0: asking for no collections answers none and ends the iteration, \
         got {:?}",
        empty_page.body
    );
}

#[tokio::test]
async fn search_faces_reports_the_faces_indexed_from_the_same_image() {
    let router = harness();
    create_collection(&router, "my-faces").await;
    create_collection(&router, "other-faces").await;

    // alice, bob and eve are indexed from the SAME image content; carol and dave come
    // from other images, and the stranger is the same content in ANOTHER collection.
    let alice = index_face(&router, "my-faces", 21, Some("alice")).await;
    let bob = index_face(&router, "my-faces", 21, Some("bob")).await;
    let carol = index_face(&router, "my-faces", 22, Some("carol")).await;
    let dave = index_face(&router, "my-faces", 23, Some("dave")).await;
    let eve = index_face(&router, "my-faces", 21, Some("eve")).await;
    let stranger = index_face(&router, "other-faces", 21, Some("stranger")).await;

    // The rule holds at every threshold the model allows, including both published
    // bounds and the 80% default the member documents (sent and omitted): an
    // identical-content face is a match at 100, a different-content face never is.
    for threshold in [
        Some(PERCENT_MIN),
        Some(1.0),
        Some(80.0),
        Some(99.5),
        Some(PERCENT_MAX),
        None,
    ] {
        let mut body = json!({ "CollectionId": "my-faces", "FaceId": alice, "MaxFaces": MAX_FACES_MAX });
        if let Some(threshold) = threshold {
            body["FaceMatchThreshold"] = json!(threshold);
        }
        let reply = call(&router, "SearchFaces", body).await;
        assert_eq!(
            reply.status, 200,
            "case threshold={threshold:?}: searching a stored face must succeed, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("SearchedFaceId"),
            Some(alice.clone()),
            "case threshold={threshold:?}: SearchedFaceId must echo the face looked up, got {:?}",
            reply.body
        );
        assert!(
            reply.string("FaceModelVersion").is_some(),
            "case threshold={threshold:?}: the collection's face model version must be reported, \
             got {:?}",
            reply.body
        );
        let matched = matches_of(&reply);
        assert_eq!(
            matched.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec![bob.clone(), eve.clone()],
            "case threshold={threshold:?}: exactly the faces indexed from the same image, in the \
             collection's order, got {matched:?}"
        );
        assert!(
            matched
                .iter()
                .all(|(_, similarity)| (*similarity - PERCENT_MAX).abs() < 1e-3),
            "case threshold={threshold:?}: an identical-content match is reported at 100, \
             got {matched:?}"
        );
        for absent in [&alice, &carol, &dave, &stranger] {
            assert!(
                !matched.iter().any(|(id, _)| id == absent),
                "case threshold={threshold:?}: the searched face, a face from different content \
                 and another collection's face are never matches, got {matched:?}"
            );
        }
        for entry in &reply.list("FaceMatches") {
            assert_measured_face("search match", entry.get("Face").unwrap_or(&Value::Null));
        }
    }

    // `MaxFaces` caps how many matches are reported.
    for max_faces in [MAX_FACES_MIN, 2, 3, MAX_FACES_MAX] {
        let reply = call(
            &router,
            "SearchFaces",
            json!({
                "CollectionId": "my-faces",
                "FaceId": alice,
                "FaceMatchThreshold": PERCENT_MIN,
                "MaxFaces": max_faces,
            }),
        )
        .await;
        let got: Vec<String> = matches_of(&reply).into_iter().map(|(id, _)| id).collect();
        let want: Vec<String> = vec![bob.clone(), eve.clone()]
            .into_iter()
            .take(usize::try_from(max_faces).unwrap_or(usize::MAX))
            .collect();
        assert_eq!(
            got, want,
            "case MaxFaces={max_faces}: the cap must limit the reported matches, got {:?}",
            reply.body
        );
    }

    // The relation is symmetric and excludes only the searched face itself.
    let from_bob = call(
        &router,
        "SearchFaces",
        json!({ "CollectionId": "my-faces", "FaceId": bob }),
    )
    .await;
    assert_eq!(
        matches_of(&from_bob)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>(),
        [alice.clone(), eve.clone()].into_iter().collect::<BTreeSet<_>>(),
        "the same-image relation must be symmetric, got {:?}",
        from_bob.body
    );

    // A face from content nothing else shares has no matches at all — the operation
    // reports none rather than inventing a graded one.
    let from_carol = call(
        &router,
        "SearchFaces",
        json!({ "CollectionId": "my-faces", "FaceId": carol, "FaceMatchThreshold": PERCENT_MIN }),
    )
    .await;
    assert!(
        matches_of(&from_carol).is_empty(),
        "a face whose image content is unique has no matches, got {:?}",
        from_carol.body
    );

    // A face id the collection does not hold names no face to search from — including
    // one that is stored in a DIFFERENT collection. The collection named exists in those
    // two rows, so the id is a parameter the service rejects; a collection the service
    // does not hold at all is the not-found refusal instead.
    for (case, collection, face_id, expected) in [
        (
            "never indexed",
            "my-faces",
            "22222222-2222-4222-8222-222222222222",
            "InvalidParameterException",
        ),
        (
            "another collection's face",
            "my-faces",
            stranger.as_str(),
            "InvalidParameterException",
        ),
        (
            "unknown collection",
            "no-such-collection",
            alice.as_str(),
            "ResourceNotFoundException",
        ),
    ] {
        let reply = call(
            &router,
            "SearchFaces",
            json!({ "CollectionId": collection, "FaceId": face_id }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, expected),
            "case {case:?}: got {:?}",
            reply.body
        );
    }

    // And a deleted face stops being a match.
    call(
        &router,
        "DeleteFaces",
        json!({ "CollectionId": "my-faces", "FaceIds": [bob, eve] }),
    )
    .await;
    let after_delete = call(
        &router,
        "SearchFaces",
        json!({ "CollectionId": "my-faces", "FaceId": alice }),
    )
    .await;
    assert!(
        matches_of(&after_delete).is_empty(),
        "a deleted face must stop being a match, got {:?}",
        after_delete.body
    );
}

#[tokio::test]
async fn delete_faces_reports_only_the_ids_it_removed() {
    let router = harness();
    create_collection(&router, "my-faces").await;

    let mut stored = Vec::new();
    for seed in 31..35u32 {
        stored.push(index_face(&router, "my-faces", seed, None).await);
    }
    let unknown = "33333333-3333-4333-8333-333333333333".to_string();

    // Rows differ along what the request names: one stored id, a stored and an unknown
    // id, only unknown ids, the same id twice, and the rest of the collection. An id the
    // collection does not hold is nothing to delete, so it is reported in neither list.
    let cases: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "one stored id",
            requested: vec![stored[0].clone()],
            deleted: vec![stored[0].clone()],
            unsuccessful: vec![],
        },
        DeleteCase {
            case: "a stored and an unknown id",
            requested: vec![stored[1].clone(), unknown.clone()],
            deleted: vec![stored[1].clone()],
            unsuccessful: vec![],
        },
        DeleteCase {
            case: "only unknown ids",
            requested: vec![unknown.clone()],
            deleted: vec![],
            unsuccessful: vec![],
        },
        DeleteCase {
            case: "the same id twice",
            requested: vec![stored[2].clone(), stored[2].clone()],
            deleted: vec![stored[2].clone()],
            unsuccessful: vec![],
        },
        DeleteCase {
            case: "the rest of the collection",
            requested: vec![stored[3].clone()],
            deleted: vec![stored[3].clone()],
            unsuccessful: vec![],
        },
    ];

    let mut remaining = stored.clone();
    for DeleteCase {
        case,
        requested,
        deleted: expected_deleted,
        unsuccessful: expected_unsuccessful,
    } in cases
    {
        let reply = call(
            &router,
            "DeleteFaces",
            json!({ "CollectionId": "my-faces", "FaceIds": requested }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: delete must answer, got {:?}",
            reply.body
        );
        assert!(
            reply.field("DeletedFaces").is_some(),
            "case {case:?}: DeletedFaces is always reported, empty when nothing matched, \
             got {:?}",
            reply.body
        );
        let deleted: Vec<String> = reply
            .list("DeletedFaces")
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            deleted, expected_deleted,
            "case {case:?}: only ids the collection held are reported deleted, got {:?}",
            reply.body
        );
        let unsuccessful: Vec<(String, Vec<String>)> = reply
            .list("UnsuccessfulFaceDeletions")
            .iter()
            .map(|entry| {
                (
                    entry
                        .get("FaceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    entry
                        .get("Reasons")
                        .and_then(Value::as_array)
                        .map(|reasons| {
                            reasons
                                .iter()
                                .map(|reason| reason.as_str().unwrap_or_default().to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            unsuccessful
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            expected_unsuccessful,
            "case {case:?}: only a face the collection still holds is an unsuccessful \
             deletion, got {:?}",
            reply.body
        );
        assert!(
            reply.field("UnsuccessfulFaceDeletions").is_some(),
            "case {case:?}: the member is always reported, empty when every requested id \
             was removed or named no face, got {:?}",
            reply.body
        );

        remaining.retain(|id| !expected_deleted.contains(id));
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": "my-faces" }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(remaining.len() as i64),
            "case {case:?}: FaceCount must follow the removal, got {:?}",
            described.body
        );
        let listed = call(&router, "ListFaces", json!({ "CollectionId": "my-faces" })).await;
        assert_eq!(
            listed
                .list("Faces")
                .iter()
                .map(face_id_of)
                .collect::<Vec<_>>(),
            remaining,
            "case {case:?}: ListFaces must report exactly the surviving faces, got {:?}",
            listed.body
        );
    }

    let unknown_collection = call(
        &router,
        "DeleteFaces",
        json!({ "CollectionId": "no-such-collection", "FaceIds": [unknown] }),
    )
    .await;
    assert_eq!(
        (
            unknown_collection.status,
            unknown_collection.error_code().as_str()
        ),
        (400, "ResourceNotFoundException"),
        "deleting from a collection that does not exist must be refused, got {:?}",
        unknown_collection.body
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_capacity() {
    let router = harness();

    // Faces: at the per-collection capacity an index is refused with the modelled quota
    // error, the collection is unchanged, and deleting a face makes room again. One
    // portrait, drawn once, is submitted for every face: filling the store is about the
    // bound, not about the pictures.
    create_collection(&router, "full").await;
    let portrait = one_face_png(41);
    for face in 0..state::MAX_FACES_PER_COLLECTION {
        let reply = call(
            &router,
            "IndexFaces",
            json!({ "CollectionId": "full", "Image": bytes_image(&portrait) }),
        )
        .await;
        assert_eq!(
            reply.list("FaceRecords").len(),
            1,
            "face {face}: the portrait's one face must be indexed, got {:?}",
            reply.body
        );
    }
    let refused = call(
        &router,
        "IndexFaces",
        json!({ "CollectionId": "full", "Image": bytes_image(&portrait) }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "ServiceQuotaExceededException"),
        "case faces={}: indexing past the capacity must be refused, got {:?}",
        state::MAX_FACES_PER_COLLECTION,
        refused.body
    );
    let described = call(&router, "DescribeCollection", json!({ "CollectionId": "full" })).await;
    assert_eq!(
        described.field("FaceCount").and_then(Value::as_i64),
        Some(state::MAX_FACES_PER_COLLECTION as i64),
        "a refused index must store nothing, got {:?}",
        described.body
    );
    let listed = call(
        &router,
        "ListFaces",
        json!({ "CollectionId": "full", "MaxResults": 1 }),
    )
    .await;
    let victim = face_id_of(&listed.list("Faces")[0]);
    call(
        &router,
        "DeleteFaces",
        json!({ "CollectionId": "full", "FaceIds": [victim] }),
    )
    .await;
    let after_room = call(
        &router,
        "IndexFaces",
        json!({ "CollectionId": "full", "Image": bytes_image(&portrait) }),
    )
    .await;
    assert_eq!(
        after_room.status, 200,
        "freeing a face must make room for another, got {:?}",
        after_room.body
    );

    // Collections: at the capacity a create is refused the same way, and deleting one
    // makes room again.
    for index in 1..state::MAX_COLLECTIONS {
        let id = format!("collection-{index:04}");
        assert_eq!(
            create_collection(&router, &id).await.status,
            200,
            "case {id}: creating up to the capacity must succeed"
        );
    }
    let over = create_collection(&router, "one-too-many").await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "ServiceQuotaExceededException"),
        "case collections={}: creating past the capacity must be refused, got {:?}",
        state::MAX_COLLECTIONS,
        over.body
    );
    let listed = call(&router, "ListCollections", json!({})).await;
    assert_eq!(
        listed.list("CollectionIds").len(),
        state::MAX_COLLECTIONS,
        "a refused create must store nothing, got {:?}",
        listed.body
    );
    call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "collection-0001" }),
    )
    .await;
    assert_eq!(
        create_collection(&router, "one-too-many").await.status,
        200,
        "deleting a collection must make room for another"
    );
}

#[tokio::test]
async fn the_faces_a_collection_holds_are_the_faces_its_images_carry() {
    let router = harness();
    create_collection(&router, "sweep").await;

    // A bounded sweep of generated portraits: each case draws none, one or two faces, at
    // generated positions, sizes and tones, and the collection must end up holding exactly
    // the faces the DETECTOR found in that image — the ones `DetectFaces` reports for the
    // same pixels, which always include the ones the test drew. Seed and case index are in
    // every message, so a failure reproduces exactly.
    let mut generator = SWEEP_SEED;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut held = 0_i64;
    for index in 0..SWEEP_CASES {
        let case = format!("seed {SWEEP_SEED:#x} case {index}");
        let count = (next(&mut generator) % 3) as usize;
        // A skin tone anywhere across the range, with its background either side of it
        // and features darker than it: the structure is what is detected, not a tone.
        let skin = 70 + (next(&mut generator) % 160) as u8;
        let background = if next(&mut generator).is_multiple_of(2) {
            skin.saturating_sub(60)
        } else {
            skin.saturating_add(60)
        };
        let feature = skin.saturating_sub(70);
        // Two slots far enough apart to hold two faces, the first always drawn larger
        // than the second, so the order the answer must be in is known.
        let slots: [(f32, f32, f32); 2] = [
            (
                130.0 + (next(&mut generator) % 17) as f32 - 8.0,
                150.0 + (next(&mut generator) % 17) as f32 - 8.0,
                40.0 + (next(&mut generator) % 17) as f32,
            ),
            (
                350.0 + (next(&mut generator) % 17) as f32 - 8.0,
                140.0 + (next(&mut generator) % 17) as f32 - 8.0,
                28.0 + (next(&mut generator) % 9) as f32,
            ),
        ];
        let drawn = &slots[..count];
        let portrait = Portrait::one_face()
            .with_size(480, 320)
            .with_tones(background, skin, feature)
            .with_faces(
                drawn
                    .iter()
                    .map(|(x, y, span)| DrawnFace::new(*x, *y, *span))
                    .collect(),
            );

        let bytes = portrait.png();
        // What the detector reports for these pixels, through the operation whose whole
        // job is reporting them: the faces IndexFaces must index are exactly these.
        let detected = call(&router, "DetectFaces", json!({ "Image": bytes_image(&bytes) })).await;
        assert_eq!(
            detected.status, 200,
            "{case}: detecting must succeed, got {:?}",
            detected.body
        );
        let details = detected.list("FaceDetails");

        let reply = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": "sweep",
                "Image": bytes_image(&bytes),
                "ExternalImageId": format!("case-{index}"),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "{case}: indexing must succeed, got {:?}",
            reply.body
        );
        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            details.len(),
            "{case}: {} face(s) drawn at {drawn:?} in tones {skin} on {background}: indexing \
             must store the faces the detector found, got {records:?} for {details:?}",
            count
        );
        for (at, detail) in details.iter().enumerate() {
            let face = records[at].get("Face").cloned().unwrap_or(Value::Null);
            assert_measured_face(&case, &face);
            assert_eq!(
                records[at].get("FaceDetail"),
                Some(detail),
                "{case}: the indexed face carries the detail that was measured for it"
            );
            assert_eq!(
                face.get("BoundingBox"),
                detail.get("BoundingBox"),
                "{case}: the stored face's box is the measured box, got {face:?}"
            );
            let face_id = face_id_of(&face);
            assert!(
                seen.insert(face_id.clone()),
                "{case}: FaceId {face_id} was minted twice"
            );
        }
        // And the faces the test drew are among them: a drawn face is a face the service
        // reports a box around.
        for (x, y, _) in drawn {
            assert!(
                records.iter().any(|record| {
                    let face = record.get("Face").cloned().unwrap_or(Value::Null);
                    let (left, top, width, height) = reported_box(&case, &face);
                    let fx = f64::from(*x) / 480.0;
                    let fy = f64::from(*y) / 320.0;
                    (left..=left + width).contains(&fx) && (top..=top + height).contains(&fy)
                }),
                "{case}: the face drawn at ({x}, {y}) must be indexed with a box around it, \
                 got {records:?}"
            );
        }

        held += details.len() as i64;
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": "sweep" }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(held),
            "{case}: the collection holds exactly the faces that were indexed, got {:?}",
            described.body
        );
    }
    assert!(
        held > 0,
        "seed {SWEEP_SEED:#x}: the sweep must have drawn some faces, held {held}"
    );

    // Re-indexing one image mints a fresh face from the same content, which is what the
    // same-image relation `SearchFaces` reports, at 100 and in both directions.
    create_collection(&router, "twins").await;
    let first = index_face(&router, "twins", 51, Some("first")).await;
    let second = index_face(&router, "twins", 51, Some("second")).await;
    assert_ne!(
        first, second,
        "re-indexing the same image must mint a fresh FaceId"
    );
    for (from, to) in [
        (first.clone(), second.clone()),
        (second.clone(), first.clone()),
    ] {
        let reply = call(
            &router,
            "SearchFaces",
            json!({ "CollectionId": "twins", "FaceId": from }),
        )
        .await;
        assert_eq!(
            matches_of(&reply),
            vec![(to.clone(), PERCENT_MAX)],
            "the same content must match at 100 both ways, got {:?}",
            reply.body
        );
    }
    let other = index_face(&router, "twins", 52, Some("other")).await;
    let reply = call(
        &router,
        "SearchFaces",
        json!({
            "CollectionId": "twins",
            "FaceId": other,
            "FaceMatchThreshold": PERCENT_MIN,
            "MaxFaces": MAX_FACES_MAX,
        }),
    )
    .await;
    assert!(
        matches_of(&reply).is_empty(),
        "a face from image content nothing else shares has no matches, got {:?}",
        reply.body
    );
}
