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
//!   is refused, and after `DeleteCollection` the id names nothing;
//! * `IndexFaces` stores one face per supplied image and reports exactly what it
//!   stored — a fresh uuid `FaceId`/`ImageId` and the `ExternalImageId` carried through
//!   — and reports NO value a detector would have had to measure (no `BoundingBox`, no
//!   `Confidence`, no `FaceDetail`, no `OrientationCorrection`);
//! * a page is cut out of the collection's insertion order: `MaxResults` caps it (and
//!   its published lower bound of 0 asks for an empty page, answered with no cursor), a
//!   returned `NextToken` resumes exactly where the page ended whatever happens in
//!   between, and a token the service did not mint is refused;
//! * `SearchFaces` reports, out of the same collection only, the faces indexed from the
//!   SAME image content as the searched face, at `Similarity` 100 — never the searched
//!   face, never another collection's, never a face from different content, whatever
//!   the threshold — capped by `MaxFaces`;
//! * `DeleteFaces` reports only the ids it actually removed, and the removal shows up in
//!   every later read;
//! * the store is bounded: past its collection and per-collection face capacity a
//!   create or an index is refused with the modelled quota error and stores nothing.
//!
//! The identity sweep (`an_indexed_face_*`) runs a bounded number of generated cases
//! from the pinned [`SWEEP_SEED`] through a three-line xorshift rather than pulling in a
//! generator crate: the cases are enumerated deterministically, so a failure names the
//! seed and the case index and reproduces exactly.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;
use service_com_amazonaws_rekognition_rekognitionservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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
const SWEEP_CASES: usize = 128;

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
/// collection holds, and which it does not.
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

/// The ARN a collection created in `region` must carry, per the format the bootstrap
/// issue and the service's `arnNamespace` give:
/// `arn:aws:rekognition:<region>:<account>:collection/<id>`.
fn expected_arn(region: &str, collection_id: &str) -> String {
    format!(
        "arn:aws:rekognition:{region}:{}:collection/{collection_id}",
        engine::emulator_account_id()
    )
}

async fn create_collection(router: &MultiServiceRouter, collection_id: &str) -> Reply {
    call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await
}

/// Index one image and answer the `FaceId` it minted.
async fn index_face(
    router: &MultiServiceRouter,
    collection_id: &str,
    content: &[u8],
    external_image_id: Option<&str>,
) -> String {
    let mut body = json!({ "CollectionId": collection_id, "Image": bytes_image(content) });
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
        "case {collection_id}/{external_image_id:?}: expected one indexed face, got {records:?}"
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

/// Every member a `Face` must NOT carry here, because only a detector could have
/// measured it. The two `FaceDetail`-only members are checked on the record itself.
const DETECTOR_ONLY_FACE_MEMBERS: [&str; 2] = ["BoundingBox", "Confidence"];

/// Assert that a reported face carries its identity and nothing a detector measures.
fn assert_identity_only(case: &str, face: &Value) {
    assert!(
        is_uuid(&face_id_of(face)),
        "case {case:?}: FaceId must be a uuid, got {face:?}"
    );
    for member in DETECTOR_ONLY_FACE_MEMBERS {
        assert!(
            face.get(member).is_none_or(Value::is_null),
            "case {case:?}: {member} is measured by no detector here, so it must be absent, \
             got {face:?}"
        );
    }
}

/// Deterministic generated image content: case `index` of the sweep seeded with
/// [`SWEEP_SEED`]. A three-line xorshift, so a failing case index reproduces exactly.
fn sweep_image(index: usize) -> Vec<u8> {
    let mut state = SWEEP_SEED ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::with_capacity(32);
    for _ in 0..4 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_be_bytes());
    }
    out
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
            Some(expected_arn(region, collection_id)),
            "case {collection_id:?}: ARN must name the request's region and the collection, \
             got {:?}",
            created.body
        );
        let face_model_version = created.string("FaceModelVersion").unwrap_or_else(|| {
            panic!(
                "case {collection_id:?}: create must report a FaceModelVersion, got {:?}",
                created.body
            )
        });
        assert!(
            !face_model_version.is_empty(),
            "case {collection_id:?}: FaceModelVersion must not be empty, got {face_model_version:?}"
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

        let listed_tags = call_in(
            &router,
            region,
            "ListTagsForResource",
            json!({ "ResourceArn": expected_arn(region, collection_id) }),
        )
        .await;
        assert_eq!(
            listed_tags.status, 200,
            "case {collection_id:?}: the created ARN must resolve, got {:?}",
            listed_tags.body
        );
        assert_eq!(
            listed_tags.field("Tags").cloned().unwrap_or(json!({})),
            Value::Object(tag_map),
            "case {collection_id:?}: the tags create persisted must be the tags reported, got {:?}",
            listed_tags.body
        );
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

#[tokio::test]
async fn index_faces_reports_exactly_the_face_it_stored() {
    let router = harness();
    create_collection(&router, "my-faces").await;

    // Rows differ along the image's form (inline bytes, an S3 object), the content
    // itself, the presence of `ExternalImageId` and the `MaxFaces` cap. The last row
    // repeats the first row's content under a different external id: its minted ids must
    // still be fresh.
    let cases: Vec<(&str, Value, Option<&str>, Option<i64>)> = vec![
        (
            "alice inline",
            bytes_image(b"jpeg-alice"),
            Some("alice"),
            None,
        ),
        ("bob inline", bytes_image(b"jpeg-bob"), Some("bob"), None),
        (
            "s3 object",
            json!({ "S3Object": { "Bucket": "faces", "Name": "carol.jpg" } }),
            Some("carol"),
            Some(MAX_FACES_MAX),
        ),
        ("no external id", bytes_image(b"jpeg-dave"), None, Some(1)),
        (
            "alice again",
            bytes_image(b"jpeg-alice"),
            Some("alice.twin"),
            None,
        ),
    ];

    let mut seen_face_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_image_ids: BTreeSet<String> = BTreeSet::new();

    for (index, (case, image, external, max_faces)) in cases.iter().enumerate() {
        let mut body = json!({ "CollectionId": "my-faces", "Image": image });
        if let Some(external) = external {
            body["ExternalImageId"] = json!(external);
        }
        if let Some(max_faces) = max_faces {
            body["MaxFaces"] = json!(max_faces);
        }
        let reply = call(&router, "IndexFaces", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: indexing must succeed, got {:?}",
            reply.body
        );

        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: one supplied image indexes one face, got {records:?}"
        );
        assert!(
            reply.list("UnindexedFaces").is_empty(),
            "case {case:?}: nothing was filtered out, so UnindexedFaces is empty, got {:?}",
            reply.body
        );
        assert!(
            reply.field("OrientationCorrection").is_none(),
            "case {case:?}: no orientation information is reported, got {:?}",
            reply.body
        );
        // Every member of `FaceDetail` is something a detector measures, so the record
        // carries none at all.
        assert!(
            records[0]
                .get("FaceDetail")
                .is_none_or(Value::is_null),
            "case {case:?}: FaceDetail is measured by no detector here, got {:?}",
            records[0]
        );

        let face = records[0].get("Face").cloned().unwrap_or(Value::Null);
        assert_identity_only(case, &face);
        let face_id = face_member(&records[0], "FaceId");
        let image_id = face_member(&records[0], "ImageId");
        assert!(
            is_uuid(&image_id),
            "case {case:?}: ImageId must be a uuid, got {image_id:?}"
        );
        assert!(
            seen_face_ids.insert(face_id.clone()),
            "case {case:?}: every indexed face gets a fresh FaceId, {face_id:?} was seen before"
        );
        assert!(
            seen_image_ids.insert(image_id.clone()),
            "case {case:?}: every indexed image gets a fresh ImageId, {image_id:?} was seen before"
        );
        assert_eq!(
            face.get("ExternalImageId").and_then(Value::as_str),
            *external,
            "case {case:?}: ExternalImageId must be carried through unchanged, got {face:?}"
        );
        assert_eq!(
            face.get("IndexFacesModelVersion").and_then(Value::as_str),
            reply.string("FaceModelVersion").as_deref(),
            "case {case:?}: the stored face must carry the collection's face model version, \
             got {face:?}"
        );

        // What was reported is what was stored: the face comes back through `ListFaces`
        // unchanged, and the count follows.
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
        assert_identity_only(case, &stored);
        assert_eq!(
            (
                stored.get("ExternalImageId").and_then(Value::as_str),
                stored.get("ImageId").and_then(Value::as_str),
            ),
            (*external, Some(image_id.as_str())),
            "case {case:?}: ListFaces must report the stored face as indexed, got {stored:?}"
        );
        let described = call(
            &router,
            "DescribeCollection",
            json!({ "CollectionId": "my-faces" }),
        )
        .await;
        assert_eq!(
            described.field("FaceCount").and_then(Value::as_i64),
            Some(index as i64 + 1),
            "case {case:?}: FaceCount must count the faces stored so far, got {:?}",
            described.body
        );
    }
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
    for (content, external) in [
        (&b"jpeg-1"[..], "one"),
        (&b"jpeg-2"[..], "two"),
        (&b"jpeg-3"[..], "three"),
        (&b"jpeg-4"[..], "four"),
        (&b"jpeg-5"[..], "five"),
    ] {
        order.push(index_face(&router, "my-faces", content, Some(external)).await);
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
    let alice = index_face(&router, "my-faces", b"jpeg-alice", Some("alice")).await;
    let bob = index_face(&router, "my-faces", b"jpeg-alice", Some("bob")).await;
    let carol = index_face(&router, "my-faces", b"jpeg-carol", Some("carol")).await;
    let dave = index_face(&router, "my-faces", b"jpeg-dave", Some("dave")).await;
    let eve = index_face(&router, "my-faces", b"jpeg-alice", Some("eve")).await;
    let stranger = index_face(&router, "other-faces", b"jpeg-alice", Some("stranger")).await;

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
            assert_identity_only("search match", entry.get("Face").unwrap_or(&Value::Null));
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
    for index in 0..4u8 {
        let content = format!("jpeg-{index}");
        stored.push(index_face(&router, "my-faces", content.as_bytes(), None).await);
    }
    let unknown = "33333333-3333-4333-8333-333333333333".to_string();

    // Rows differ along what the request names: one stored id, a stored and an unknown
    // id, only unknown ids, the same id twice, and the rest of the collection.
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
            unsuccessful: vec![unknown.clone()],
        },
        DeleteCase {
            case: "only unknown ids",
            requested: vec![unknown.clone()],
            deleted: vec![],
            unsuccessful: vec![unknown.clone()],
        },
        DeleteCase {
            case: "the same id twice",
            requested: vec![stored[2].clone(), stored[2].clone()],
            deleted: vec![stored[2].clone()],
            unsuccessful: vec![stored[2].clone()],
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
            "case {case:?}: every id that survived must be reported, got {:?}",
            reply.body
        );
        assert!(
            unsuccessful
                .iter()
                .all(|(_, reasons)| reasons == &vec!["FACE_NOT_FOUND".to_string()]),
            "case {case:?}: an id the collection does not hold is FACE_NOT_FOUND, \
             got {unsuccessful:?}"
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
    // error, the collection is unchanged, and deleting a face makes room again.
    create_collection(&router, "full").await;
    for face in 0..state::MAX_FACES_PER_COLLECTION {
        let content = format!("jpeg-{face}");
        index_face(&router, "full", content.as_bytes(), None).await;
    }
    let refused = call(
        &router,
        "IndexFaces",
        json!({ "CollectionId": "full", "Image": bytes_image(b"one-too-many") }),
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
        json!({ "CollectionId": "full", "Image": bytes_image(b"one-too-many") }),
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
async fn an_indexed_face_carries_only_its_identity_whatever_the_image() {
    let router = harness();
    create_collection(&router, "sweep").await;

    // A bounded sweep of generated image contents: whatever the content, the stored face
    // carries its identity and no measured value, every minted id is fresh, and the
    // same-image relation `SearchFaces` reports is exactly content identity. Seed and
    // case index are in every message, so a failure reproduces exactly.
    let mut ids: Vec<(usize, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for case in 0..SWEEP_CASES {
        let content = sweep_image(case);
        let reply = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": "sweep",
                "Image": bytes_image(&content),
                "ExternalImageId": format!("case-{case}"),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "seed {SWEEP_SEED:#x} case {case}: indexing must succeed, got {:?}",
            reply.body
        );
        let records = reply.list("FaceRecords");
        assert_eq!(
            records.len(),
            1,
            "seed {SWEEP_SEED:#x} case {case}: one image indexes one face, got {records:?}"
        );
        let face = records[0].get("Face").cloned().unwrap_or(Value::Null);
        assert_identity_only(&format!("seed {SWEEP_SEED:#x} case {case}"), &face);
        let face_id = face_id_of(&face);
        assert!(
            seen.insert(face_id.clone()),
            "seed {SWEEP_SEED:#x} case {case}: FaceId {face_id} was minted twice"
        );
        ids.push((case, face_id));
    }

    // Each generated content is unique, so no face matches another.
    for (case, face_id) in ids.iter().step_by(16) {
        let reply = call(
            &router,
            "SearchFaces",
            json!({
                "CollectionId": "sweep",
                "FaceId": face_id,
                "FaceMatchThreshold": PERCENT_MIN,
                "MaxFaces": MAX_FACES_MAX,
            }),
        )
        .await;
        assert!(
            matches_of(&reply).is_empty(),
            "seed {SWEEP_SEED:#x} case {case}: a unique image has no matches, got {:?}",
            reply.body
        );
    }

    // Re-indexing the same content mints a fresh face that matches the first one, at
    // 100, in both directions.
    for (case, first) in ids.iter().step_by(16) {
        let content = sweep_image(*case);
        let second = index_face(&router, "sweep", &content, Some("second")).await;
        assert_ne!(
            &second, first,
            "seed {SWEEP_SEED:#x} case {case}: re-indexing must mint a fresh FaceId"
        );
        for (from, to) in [(first.clone(), second.clone()), (second, first.clone())] {
            let reply = call(
                &router,
                "SearchFaces",
                json!({ "CollectionId": "sweep", "FaceId": from }),
            )
            .await;
            assert_eq!(
                matches_of(&reply),
                vec![(to.clone(), PERCENT_MAX)],
                "seed {SWEEP_SEED:#x} case {case}: the same content must match at 100 both ways, \
                 got {:?}",
                reply.body
            );
        }
    }

    let described = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": "sweep" }),
    )
    .await;
    assert_eq!(
        described.field("FaceCount").and_then(Value::as_i64),
        Some((SWEEP_CASES + SWEEP_CASES.div_ceil(16)) as i64),
        "FaceCount must count every indexed face, got {:?}",
        described.body
    );
}
