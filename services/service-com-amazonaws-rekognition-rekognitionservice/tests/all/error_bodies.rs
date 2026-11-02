// SPDX-License-Identifier: CC0-1.0

//! The error BODIES this service answers a refused request with, end to end over the
//! wire (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Operation>`),
//! dispatched through the same [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: a refused request is answered with the modelled exception for
//! the class of failure, and that exception's body names the class TWICE — in the
//! protocol envelope's `__type` and in the shape's own `Code` member, both as the
//! exception's shape name — beside a `Message` worded the way the service words that
//! failure for that operation. Two classes are kept apart: a collection the service
//! does not hold is `ResourceNotFoundException`, while a `FaceId` a collection that DOES
//! exist does not hold is `InvalidParameterException` — the resource named by the
//! request is there, so what the request got wrong is a parameter value.
//!
//! The tables span what the rule generalises over: which operation is called, which
//! modelled exception the failure falls into (absent collection, duplicate collection,
//! unminted pagination token, unknown resource ARN, unreadable image, unknown face id),
//! and the form of the identifier in the request (shortest and longest the model allows,
//! every character class its pattern admits, an id another collection's face carries, an
//! id this service minted and then deleted).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper and the reduced reply all come from the
// collection-lifecycle suite rather than being built a second time here.
use crate::face_collections::{bytes_image, call, harness, one_face_png, Reply};

/// A collection this suite creates, so every "the collection exists" row has one.
const EXISTING_COLLECTION: &str = "error-bodies-existing";

/// A `FaceId` of the modelled form (`pattern
/// ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that this service
/// never minted.
const UNMINTED_FACE_ID: &str = "00000000-0000-0000-0000-000000000000";
const OTHER_UNMINTED_FACE_ID: &str = "deadbeef-0000-4000-8000-0123456789ab";

/// `CollectionId` is `len 1..255, pattern ^[a-zA-Z0-9_.\-]+$`
/// (`smithy_query(query="operation",operation="DescribeCollection")`), so the ids below
/// span its bounds and every character class it admits.
fn absent_collection_ids() -> Vec<String> {
    vec![
        "a".to_string(),
        "my-faces".to_string(),
        "My_Collection.2026-v2".to_string(),
        "0".repeat(255),
    ]
}

/// What every refused request must carry: the status, the class named in both places,
/// and a message that says something.
fn assert_classified(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        expected,
        "case {case:?}: expected __type {expected}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Code").as_deref(),
        Some(expected),
        "case {case:?}: expected Code {expected}, got {:?}",
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

/// A router holding one collection with one indexed face, answering `(FaceId, ARN)`.
async fn seeded() -> (MultiServiceRouter, String, String) {
    let router = harness();
    let created = call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": EXISTING_COLLECTION }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the seeded collection must be created, got {:?}",
        created.body
    );
    let arn = created
        .string("CollectionArn")
        .expect("the created collection reports its ARN");
    let indexed = call(
        &router,
        "IndexFaces",
        json!({
            "CollectionId": EXISTING_COLLECTION,
            "Image": bytes_image(&one_face_png(61)),
        }),
    )
    .await;
    assert_eq!(
        indexed.status, 200,
        "the seeded face must be indexed, got {:?}",
        indexed.body
    );
    let face_id = indexed.list("FaceRecords")[0]
        .get("Face")
        .and_then(|face| face.get("FaceId"))
        .and_then(Value::as_str)
        .expect("the indexed face reports its FaceId")
        .to_string();
    (router, face_id, arn)
}

/// Every modelled failure this service can be driven into names its class in `__type`
/// AND in the body's own `Code` member.
#[tokio::test]
async fn every_modeled_error_names_its_class_in_code_and_type() {
    let (router, face_id, arn) = seeded().await;
    let absent = "error-bodies-absent";
    let token = "6e6f742d612d746f6b656e";
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "DescribeCollection on an absent collection",
            "DescribeCollection",
            json!({ "CollectionId": absent }),
            "ResourceNotFoundException",
        ),
        (
            "DeleteCollection on an absent collection",
            "DeleteCollection",
            json!({ "CollectionId": absent }),
            "ResourceNotFoundException",
        ),
        (
            "ListFaces on an absent collection",
            "ListFaces",
            json!({ "CollectionId": absent }),
            "ResourceNotFoundException",
        ),
        (
            "DeleteFaces on an absent collection",
            "DeleteFaces",
            json!({ "CollectionId": absent, "FaceIds": [UNMINTED_FACE_ID] }),
            "ResourceNotFoundException",
        ),
        (
            "IndexFaces on an absent collection",
            "IndexFaces",
            json!({ "CollectionId": absent, "Image": bytes_image(b"any-image") }),
            "ResourceNotFoundException",
        ),
        (
            "SearchFaces on an absent collection",
            "SearchFaces",
            json!({ "CollectionId": absent, "FaceId": UNMINTED_FACE_ID }),
            "ResourceNotFoundException",
        ),
        (
            "ListTagsForResource on an ARN no stored collection carries",
            "ListTagsForResource",
            json!({ "ResourceArn": format!("{arn}-nothing") }),
            "ResourceNotFoundException",
        ),
        (
            "CreateCollection on an id the store already holds",
            "CreateCollection",
            json!({ "CollectionId": EXISTING_COLLECTION }),
            "ResourceAlreadyExistsException",
        ),
        (
            "ListFaces resuming from a token no list minted",
            "ListFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "NextToken": token }),
            "InvalidPaginationTokenException",
        ),
        (
            "ListCollections resuming from a token no list minted",
            "ListCollections",
            json!({ "NextToken": token }),
            "InvalidPaginationTokenException",
        ),
        (
            "IndexFaces with an image identifying no content",
            "IndexFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "Image": {} }),
            "InvalidParameterException",
        ),
        (
            "DetectFaces on bytes that are not an image",
            "DetectFaces",
            json!({ "Image": bytes_image(b"not-an-image-at-all") }),
            "InvalidImageFormatException",
        ),
        (
            "SearchFaces on a face id the existing collection does not hold",
            "SearchFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "FaceId": UNMINTED_FACE_ID }),
            "InvalidParameterException",
        ),
        (
            "SearchFaces on a face id of the right form but never minted",
            "SearchFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "FaceId": OTHER_UNMINTED_FACE_ID }),
            "InvalidParameterException",
        ),
    ];

    for (case, operation, body, expected) in cases {
        let reply = call(&router, operation, body).await;
        assert_classified(case, &reply, expected);
    }

    // The seeded face is still searchable, so none of the refusals above changed what a
    // later caller sees.
    let ok = call(
        &router,
        "SearchFaces",
        json!({ "CollectionId": EXISTING_COLLECTION, "FaceId": face_id }),
    )
    .await;
    assert_eq!(
        ok.status, 200,
        "the seeded face must still be searchable, got {:?}",
        ok.body
    );
}

/// A collection the store does not hold is refused with the wording the service uses
/// for the operation called: the collection-resource operations word it
/// `Collection:<CollectionId> not found!`, the face operations word it
/// `The collection id: <CollectionId> does not exist`. Both interpolate the request's
/// own id and nothing else.
#[tokio::test]
async fn an_absent_collection_is_worded_the_way_the_operation_words_it() {
    let router = harness();
    for collection_id in absent_collection_ids() {
        let resource_message = format!("Collection:{collection_id} not found!");
        let faces_message = format!("The collection id: {collection_id} does not exist");
        let cases: Vec<(&str, Value, &String)> = vec![
            (
                "DescribeCollection",
                json!({ "CollectionId": collection_id }),
                &resource_message,
            ),
            (
                "DeleteCollection",
                json!({ "CollectionId": collection_id }),
                &resource_message,
            ),
            (
                "ListFaces",
                json!({ "CollectionId": collection_id }),
                &faces_message,
            ),
            (
                "DeleteFaces",
                json!({ "CollectionId": collection_id, "FaceIds": [UNMINTED_FACE_ID] }),
                &faces_message,
            ),
            (
                "IndexFaces",
                json!({ "CollectionId": collection_id, "Image": bytes_image(b"any-image") }),
                &faces_message,
            ),
            (
                "SearchFaces",
                json!({ "CollectionId": collection_id, "FaceId": UNMINTED_FACE_ID }),
                &faces_message,
            ),
        ];
        for (operation, body, expected_message) in cases {
            let case = format!("{operation} on {collection_id}");
            let reply = call(&router, operation, body).await;
            assert_classified(&case, &reply, "ResourceNotFoundException");
            assert_eq!(
                reply.string("Message").as_deref(),
                Some(expected_message.as_str()),
                "case {case:?}: expected Message {expected_message:?}, got {:?}",
                reply.body
            );
        }
    }
}

/// A `FaceId` a collection that EXISTS does not hold is a parameter refusal, worded with
/// fixed text that names no identifier — and it stays a parameter refusal however the id
/// came to be absent (never minted, minted for another collection, deleted from this
/// one), while the collection itself being absent stays the not-found refusal.
#[tokio::test]
async fn a_face_id_the_collection_does_not_hold_is_a_parameter_refusal() {
    const EXPECTED_MESSAGE: &str = "faceId was not found in the collection.";
    let (router, seeded_face_id, _) = seeded().await;

    // A second collection with its own face, so "a face id of another collection" is a
    // real id rather than an invented one.
    let other_collection = "error-bodies-other";
    let created = call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": other_collection }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the second collection must be created, got {:?}",
        created.body
    );
    let indexed = call(
        &router,
        "IndexFaces",
        json!({
            "CollectionId": other_collection,
            "Image": bytes_image(&one_face_png(62)),
        }),
    )
    .await;
    let other_face_id = indexed.list("FaceRecords")[0]
        .get("Face")
        .and_then(|face| face.get("FaceId"))
        .and_then(Value::as_str)
        .expect("the indexed face reports its FaceId")
        .to_string();

    // And a face this service minted into the seeded collection and then deleted.
    let deleted_face_id = {
        let indexed = call(
            &router,
            "IndexFaces",
            json!({
                "CollectionId": EXISTING_COLLECTION,
                "Image": bytes_image(&one_face_png(63)),
            }),
        )
        .await;
        let face_id = indexed.list("FaceRecords")[0]
            .get("Face")
            .and_then(|face| face.get("FaceId"))
            .and_then(Value::as_str)
            .expect("the indexed face reports its FaceId")
            .to_string();
        let removed = call(
            &router,
            "DeleteFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "FaceIds": [face_id.clone()] }),
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "the doomed face must be deleted, got {:?}",
            removed.body
        );
        face_id
    };

    let cases: Vec<(&str, String)> = vec![
        ("a face id this service never minted", UNMINTED_FACE_ID.to_string()),
        (
            "another face id this service never minted",
            OTHER_UNMINTED_FACE_ID.to_string(),
        ),
        ("a face id another collection holds", other_face_id),
        ("a face id deleted from this collection", deleted_face_id),
    ];
    for (case, face_id) in cases {
        let reply = call(
            &router,
            "SearchFaces",
            json!({ "CollectionId": EXISTING_COLLECTION, "FaceId": face_id }),
        )
        .await;
        assert_classified(case, &reply, "InvalidParameterException");
        assert_eq!(
            reply.string("Message").as_deref(),
            Some(EXPECTED_MESSAGE),
            "case {case:?}: expected Message {EXPECTED_MESSAGE:?}, got {:?}",
            reply.body
        );
    }

    // The same request against a collection the store does not hold is the OTHER class:
    // the two failures stay distinguishable.
    let reply = call(
        &router,
        "SearchFaces",
        json!({ "CollectionId": "error-bodies-absent", "FaceId": seeded_face_id }),
    )
    .await;
    assert_classified(
        "SearchFaces on an absent collection",
        &reply,
        "ResourceNotFoundException",
    );
}
