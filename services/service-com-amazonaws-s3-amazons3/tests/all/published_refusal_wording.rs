// SPDX-License-Identifier: CC0-1.0

//! Two refusals whose `<Message>` is a FIXED sentence S3 publishes, pinned against that
//! sentence rather than against a diagnostic.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a client branches on and a human reads: the status, the `<Code>` and the
//! `<Message>` of S3's error document.
//!
//! The rule under test: for a refusal whose cause carries no values of its own, the message
//! is the published sentence for that cause — the same text whatever the request named, with
//! nothing of the emulator's own in it (no Rust rendering of a value, no member path, no
//! quoted debug form). Anything matching on S3's message, and any operator reading it, sees
//! the sentence the service sends.
//!
//! The two causes, each over a table:
//!
//! 1. A `CompleteMultipartUpload` whose parts list does not ascend: the parts are assembled
//!    "in ascending order by part number" (that operation's own documentation), so a list
//!    that does not ascend is `InvalidPartOrder` (400).
//! 2. A `PutBucketEncryption` rule naming a `KMSMasterKeyID` beside an algorithm that is not
//!    a KMS one: "This parameter is allowed if and only if SSEAlgorithm is set to aws:kms or
//!    aws:kms:dsse" (`ApplyServerSideEncryptionByDefault$KMSMasterKeyID`), so the key is a
//!    value that is not valid for the parameter carrying it — `InvalidArgument` (400).
//!
//! The rows differ along what each rule generalises over: HOW the list fails to ascend (a
//! descending pair, a repeated number, a descent in the middle of a longer list) and WHICH
//! non-KMS algorithm the key was named beside (every value `ServerSideEncryption` publishes
//! that is not a KMS one), plus the complement of each — an ascending list and a KMS
//! algorithm are served.

use service_com_amazonaws_s3_amazons3::ops::types::SERVER_SIDE_ENCRYPTION_VALUES;
use service_com_amazonaws_s3_amazons3::state::KMS_SSE_ALGORITHMS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, text};

/// The `<Message>` S3 sends with `InvalidPartOrder` — the FIXED text that code's entry in
/// Amazon S3's list of error codes publishes, which names no part number.
const INVALID_PART_ORDER_MESSAGE: &str =
    "The list of parts was not in ascending order. Parts must be ordered by part number.";

/// The `<Message>` S3 sends when a default-encryption rule names a KMS key beside an
/// algorithm that is not a KMS one — FIXED text, naming neither the algorithm nor the key.
const KMS_KEY_WITHOUT_KMS_ALGORITHM_MESSAGE: &str =
    "a KMSMasterKeyID is not applicable if the default sse algorithm is not aws:kms or \
     aws:kms:dsse";

/// A KMS key ARN of the form `KMSMasterKeyID` documents ("you can specify the key ID, key
/// alias, or the Amazon Resource Name (ARN) of the KMS key").
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// Upload one part under `upload_id`, answering the entity tag the store recorded for it.
async fn upload_part(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
) -> String {
    let uploaded = send_bytes(
        router,
        "PUT",
        &format!("/{name}/{key}?partNumber={part_number}&uploadId={upload_id}"),
        &[],
        format!("part {part_number} of the upload").as_bytes(),
    )
    .await;
    assert_eq!(
        uploaded.status,
        200,
        "uploading part {part_number} must answer HTTP 200, got {} with body {:?}",
        uploaded.status,
        text(&uploaded)
    );
    uploaded
        .header("etag")
        .expect("a part upload must report an ETag header")
}

/// The parts-list payload naming each `(part number, entity tag)` pair in the order given.
fn completion_payload(parts: &[(i32, String)]) -> String {
    let mut payload = String::from("<CompleteMultipartUpload>");
    for (part_number, etag) in parts {
        payload.push_str(&format!(
            "<Part><PartNumber>{part_number}</PartNumber><ETag>{etag}</ETag></Part>"
        ));
    }
    payload.push_str("</CompleteMultipartUpload>");
    payload
}

#[tokio::test]
async fn a_parts_list_that_does_not_ascend_carries_the_published_sentence() {
    let router = router();
    bucket(&router, "order-bucket").await;

    // The orders a list can arrive in. Every row names parts the upload really holds, so
    // what each is refused for is the ORDER rather than the part.
    let rows: [(&str, &[i32], bool); 6] = [
        ("ascending", &[1, 2, 3], true),
        ("a descending pair", &[2, 1], false),
        ("a descent in the middle of a longer list", &[1, 3, 2], false),
        ("one part number named twice", &[1, 1], false),
        ("the same number three times", &[2, 2, 2], false),
        ("fully descending", &[3, 2, 1], false),
    ];

    for (index, (case, order, ascends)) in rows.iter().enumerate() {
        let key = format!("ordered-{index}.bin");
        let initiated = send(
            &router,
            "POST",
            &format!("/order-bucket/{key}?uploads"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: the initiation must answer HTTP 200, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        let upload_id = initiated
            .element("UploadId")
            .expect("an initiation must report an UploadId");
        // Three parts, so a row can name any of them in any order. They are smaller than
        // the published minimum part size, which only the LAST part of a list is exempt
        // from — so the rows that ascend name parts whose sizes the completion then
        // refuses; the ORDER refusal is judged first, which is what these rows pin.
        let mut tags: Vec<(i32, String)> = Vec::new();
        for part_number in 1..=3 {
            let etag = upload_part(&router, "order-bucket", &key, &upload_id, part_number).await;
            tags.push((part_number, etag));
        }
        let named: Vec<(i32, String)> = order
            .iter()
            .map(|number| {
                let (_, etag) = tags
                    .iter()
                    .find(|(held, _)| held == number)
                    .expect("every row names a part the upload holds");
                (*number, etag.clone())
            })
            .collect();

        let answer = send(
            &router,
            "POST",
            &format!("/order-bucket/{key}?uploadId={upload_id}"),
            &[],
            &completion_payload(&named),
        )
        .await;
        if *ascends {
            assert_ne!(
                answer.error_code(),
                "InvalidPartOrder",
                "case {case:?}: an ascending list must not be refused for its order, got body \
                 {:?}",
                text(&answer)
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "InvalidPartOrder",
                "case {case:?}: expected the InvalidPartOrder code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                INVALID_PART_ORDER_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                answer.error_message()
            );
            // A refused completion writes nothing at the key.
            let absent = get_object(&router, "order-bucket", &key, "", &[]).await;
            assert_eq!(
                absent.status, 404,
                "case {case:?}: a refused completion must store nothing, got {}",
                absent.status
            );
        }
    }
}

#[tokio::test]
async fn a_kms_key_beside_a_non_kms_algorithm_carries_the_published_sentence() {
    let router = router();
    let created = create_bucket(&router, "encryption-wording", None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // Every value `ServerSideEncryption` publishes, so the rows span the whole enum rather
    // than the one algorithm the issue's recorded call named: a KMS key beside a KMS
    // algorithm is served, and beside every other algorithm it is refused with the same
    // sentence.
    assert!(
        SERVER_SIDE_ENCRYPTION_VALUES.len() > KMS_SSE_ALGORITHMS.len(),
        "the model must publish more algorithms than the KMS ones, got {:?}",
        SERVER_SIDE_ENCRYPTION_VALUES
    );
    for algorithm in SERVER_SIDE_ENCRYPTION_VALUES {
        let kms = KMS_SSE_ALGORITHMS.contains(algorithm);
        let payload = format!(
            "<ServerSideEncryptionConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <Rule><ApplyServerSideEncryptionByDefault>\
             <SSEAlgorithm>{algorithm}</SSEAlgorithm>\
             <KMSMasterKeyID>{KMS_KEY_ARN}</KMSMasterKeyID>\
             </ApplyServerSideEncryptionByDefault></Rule>\
             </ServerSideEncryptionConfiguration>"
        );
        let answer = send(
            &router,
            "PUT",
            "/encryption-wording?encryption",
            &[],
            &payload,
        )
        .await;
        if kms {
            assert_eq!(
                answer.status, 200,
                "case {algorithm:?}: a KMS key beside a KMS algorithm must be accepted, got {} \
                 with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {algorithm:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {algorithm:?}: expected the InvalidArgument code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            KMS_KEY_WITHOUT_KMS_ALGORITHM_MESSAGE,
            "case {algorithm:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
    }

    // The sentence is the SAME however many rules the container carries and wherever the
    // offending rule sits in it, because the cause carries nothing of the request.
    let payload = format!(
        "<ServerSideEncryptionConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Rule><ApplyServerSideEncryptionByDefault>\
         <SSEAlgorithm>aws:kms</SSEAlgorithm><KMSMasterKeyID>{KMS_KEY_ARN}</KMSMasterKeyID>\
         </ApplyServerSideEncryptionByDefault></Rule>\
         <Rule><ApplyServerSideEncryptionByDefault>\
         <SSEAlgorithm>AES256</SSEAlgorithm><KMSMasterKeyID>{KMS_KEY_ARN}</KMSMasterKeyID>\
         </ApplyServerSideEncryptionByDefault></Rule>\
         </ServerSideEncryptionConfiguration>"
    );
    let refused: Answer = send(
        &router,
        "PUT",
        "/encryption-wording?encryption",
        &[],
        &payload,
    )
    .await;
    assert_eq!(
        refused.error_message(),
        KMS_KEY_WITHOUT_KMS_ALGORITHM_MESSAGE,
        "case \"the second rule of two\": expected the published sentence, got {:?}",
        refused.error_message()
    );
}
