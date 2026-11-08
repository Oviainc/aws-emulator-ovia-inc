// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal three calls are refused with, when they cannot be served: the code a client
//! branches on and the sentence it reads, over a set of inputs rather than one request.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, `<Code>`, `<Message>`) and what the store holds afterwards.
//!
//! The rules under test:
//!
//! 1. A call barred by the STORAGE CLASS the object's bytes are in is refused with
//!    `InvalidObjectState` (HTTP 403, the shape's own `@httpError`) and the sentence S3 words
//!    for the object the class barred — the one for the object being READ, and the one for
//!    the object being copied FROM. "Before using an object as a source object for the copy
//!    operation, you must restore a copy of it if … the storage class of the source object is
//!    `GLACIER` or `DEEP_ARCHIVE`" (`CopyObjectRequest$StorageClass`), so a copy is barred
//!    exactly while a read of the same version is, and a restore that has fallen due lifts
//!    both bars.
//! 2. A `CompleteMultipartUpload` naming a part the upload holds nothing under, or an entity
//!    tag that is not the named part's, is refused with `InvalidPart` and the FIXED text
//!    Amazon S3's list of error codes publishes for that code — reproduced byte for byte,
//!    including the two spaces between its sentences — while a parts list that is not in
//!    ascending order is the sibling `InvalidPartOrder` with its own sentence.
//! 3. A call addressing the S3 Metadata configuration of a bucket that carries none — one
//!    that never had one, or one whose configuration a delete has taken away — is refused
//!    with `MetadataConfigurationNotFound` (HTTP 404) and "The metadata configuration was not
//!    found", which is the terminal answer a delete-then-verify loop waits for. The bucket
//!    stays reusable: a fresh create reads back again.

use std::time::Duration;

use service_com_amazonaws_s3_amazons3::state::{
    ARCHIVED_STORAGE_CLASSES, INVALID_OBJECT_STATE_COPY_SOURCE_MESSAGE,
    INVALID_OBJECT_STATE_READ_MESSAGE, METADATA_CONFIGURATION_NOT_FOUND_CODE,
    METADATA_CONFIGURATION_NOT_FOUND_MESSAGE, RESTORE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, text as element_text,
    wait_for_the_tables, XMLNS,
};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::seeded_payload;

/// The code every storage-class refusal in this suite carries: the modelled
/// `InvalidObjectState`, spelled out here rather than read from the crate under test.
const INVALID_OBJECT_STATE: &str = "InvalidObjectState";

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The `<Message>` S3 sends with `InvalidPart`, from its published list of error codes —
/// TWO spaces after the first sentence's period, which is how S3 words it.
const INVALID_PART_MESSAGE: &str =
    "One or more of the specified parts could not be found.  The part may not have been \
     uploaded, or the specified entity tag may not match the part's entity tag.";

/// The `<Message>` S3 sends with `InvalidPartOrder`, from the same list.
const INVALID_PART_ORDER_MESSAGE: &str =
    "The list of parts was not in ascending order. Parts must be ordered by part number.";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(router, "PUT", &format!("/{bucket_name}/{key}"), &all, b"").await
}

/// A blocking sleep until a restore's temporary copy has fallen due — the span the store
/// records plus slack for a loaded machine. A blocking sleep rather than
/// `tokio::time::sleep(..).await`, for the reason the restore suite gives: the workspace's
/// tokio carries only the `macros`/`rt` features, so its timer driver is not built.
fn wait_for_the_copy() {
    std::thread::sleep(RESTORE_COMPLETION_DELAY + Duration::from_millis(250));
}

#[tokio::test]
async fn a_call_barred_by_the_objects_storage_class_is_refused_with_that_storage_class_refusal() {
    let router = router();
    let name = "archived-source-bucket";
    bucket(&router, name).await;

    // One row per archived storage class, so the rule is pinned over the class list rather
    // than over the one class the report named.
    for class in ARCHIVED_STORAGE_CLASSES {
        let key = format!("archive/{}.bin", class.to_ascii_lowercase());
        let payload = seeded_payload(0x5ad0_1234, 2_048);
        let stored = put_object(&router, name, &key, &payload, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "case {class:?}: the fixture write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        let etag = stored
            .header("etag")
            .expect("a write reports the object's ETag");

        // The in-place re-class is a copy that CHANGES something, so it is served and the
        // object keeps its bytes: same size, same entity tag, new storage class.
        let reclassed = copy_object(
            &router,
            name,
            &key,
            &format!("{name}/{key}"),
            &[("x-amz-storage-class", class)],
        )
        .await;
        assert_eq!(
            reclassed.status,
            200,
            "case {class:?}: the in-place re-class must be served, got {} with body {:?}",
            reclassed.status,
            text(&reclassed)
        );

        // A READ of the archived version is barred, with the sentence for the object being
        // read.
        let read = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            read.status, 403,
            "case {class:?}: a read of an archived object is refused with 403, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            INVALID_OBJECT_STATE,
            "case {class:?}: expected the storage-class refusal, got {:?}",
            read.error_code()
        );
        assert_eq!(
            read.error_message(),
            INVALID_OBJECT_STATE_READ_MESSAGE,
            "case {class:?}: expected the sentence for the object being read, got {:?}",
            read.error_message()
        );

        // And so is every copy whose SOURCE is that version — back onto itself, or out to
        // another key — with the same code and the sentence worded for the source object.
        for (where_to, destination) in [
            ("back onto itself", key.clone()),
            ("out to another key", format!("{key}.copy")),
        ] {
            let copied = copy_object(
                &router,
                name,
                &destination,
                &format!("{name}/{key}"),
                &[("x-amz-storage-class", "STANDARD")],
            )
            .await;
            assert_eq!(
                copied.status,
                403,
                "case {class:?} {where_to}: a copy from an archived source is refused with \
                 403, got {} with body {:?}",
                copied.status,
                text(&copied)
            );
            assert_eq!(
                copied.error_code(),
                INVALID_OBJECT_STATE,
                "case {class:?} {where_to}: expected the storage-class refusal, got {:?}",
                copied.error_code()
            );
            assert_eq!(
                copied.error_message(),
                INVALID_OBJECT_STATE_COPY_SOURCE_MESSAGE,
                "case {class:?} {where_to}: expected the sentence for the source object, \
                 got {:?}",
                copied.error_message()
            );
        }

        // A refused copy wrote nothing, and the archived object is untouched: same class,
        // same tag, same size.
        let absent = get_object(&router, name, &format!("{key}.copy"), "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {class:?}: a refused copy must record nothing, got {}",
            absent.status
        );
        let described = send(&router, "HEAD", &format!("/{name}/{key}"), &[], "").await;
        assert_eq!(
            described.header("x-amz-storage-class").as_deref(),
            Some(class),
            "case {class:?}: the source must still be in its storage class, got {:?}",
            described.header("x-amz-storage-class")
        );
        assert_eq!(
            described.header("etag"),
            Some(etag.clone()),
            "case {class:?}: the source must still carry its entity tag, got {:?}",
            described.header("etag")
        );
        assert_eq!(
            described.header("content-length"),
            Some(payload.len().to_string()),
            "case {class:?}: the source must still be its own size, got {:?}",
            described.header("content-length")
        );
    }

    // A source in a storage class that bars nothing copies, which is what makes the rows
    // above the storage class's doing rather than the copy's.
    let live = "archive/live.bin";
    put_object(&router, name, live, b"live bytes", &[]).await;
    let copied = copy_object(
        &router,
        name,
        "archive/live.copy",
        &format!("{name}/{live}"),
        &[],
    )
    .await;
    assert_eq!(
        copied.status,
        200,
        "case \"a source that is not archived\": the copy must be served, got {} with body \
         {:?}",
        copied.status,
        text(&copied)
    );

    // And the bar is lifted by a restore that has fallen due: the temporary copy a restore
    // makes available is what the source object's storage class was barring the copy for.
    let restored_key = "archive/restorable.bin";
    put_object(&router, name, restored_key, b"restorable bytes", &[]).await;
    copy_object(
        &router,
        name,
        restored_key,
        &format!("{name}/{restored_key}"),
        &[("x-amz-storage-class", "DEEP_ARCHIVE")],
    )
    .await;
    let asked = send(
        &router,
        "POST",
        &format!("/{name}/{restored_key}?restore"),
        &[],
        &format!("<RestoreRequest xmlns=\"{XMLNS}\"><Days>1</Days></RestoreRequest>"),
    )
    .await;
    assert_eq!(
        asked.status,
        202,
        "case \"a restore of the archived source\": expected the 202 of a retrieval this call \
         initiated, got {} with body {:?}",
        asked.status,
        text(&asked)
    );
    wait_for_the_copy();
    let after_restore = copy_object(
        &router,
        name,
        "archive/restored.copy",
        &format!("{name}/{restored_key}"),
        &[("x-amz-storage-class", "STANDARD")],
    )
    .await;
    assert_eq!(
        after_restore.status,
        200,
        "case \"a copy from a restored source\": the copy must be served once the temporary \
         copy is available, got {} with body {:?}",
        after_restore.status,
        text(&after_restore)
    );
}

/// One row of the completion table: the case, the parts list it names and the code and
/// sentence the refusal must carry.
struct Completion {
    case: &'static str,
    /// The `(part number, which part's entity tag)` pairs the list names, where `None` names
    /// an entity tag no part of the upload carries.
    parts: Vec<(i32, Option<usize>)>,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_completion_that_cannot_match_a_named_part_is_refused_with_the_published_sentence() {
    let router = router();
    let name = "invalid-part-bucket";
    bucket(&router, name).await;

    // Two parts: the first at the published minimum, the last exempt from it.
    let parts = [
        seeded_payload(0x00c0_ffee, MIN_MULTIPART_PART_BYTES),
        seeded_payload(0x00de_caf0, 1_024),
    ];

    let rows: Vec<Completion> = vec![
        Completion {
            case: "a part number the upload holds nothing under",
            parts: vec![(1, Some(0)), (3, None)],
            code: "InvalidPart",
            message: INVALID_PART_MESSAGE,
        },
        Completion {
            case: "only a part number the upload holds nothing under",
            parts: vec![(7, None)],
            code: "InvalidPart",
            message: INVALID_PART_MESSAGE,
        },
        Completion {
            case: "an entity tag that is another part's",
            parts: vec![(1, Some(1))],
            code: "InvalidPart",
            message: INVALID_PART_MESSAGE,
        },
        Completion {
            case: "an entity tag that is no part's at all",
            parts: vec![(1, None), (2, Some(1))],
            code: "InvalidPart",
            message: INVALID_PART_MESSAGE,
        },
        Completion {
            case: "a parts list that is not in ascending order",
            parts: vec![(2, Some(1)), (1, Some(0))],
            code: "InvalidPartOrder",
            message: INVALID_PART_ORDER_MESSAGE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = format!("completions/row-{index}.bin");
        let initiated = send(&router, "POST", &format!("/{name}/{key}?uploads"), &[], "").await;
        assert_eq!(
            initiated.status,
            200,
            "case {case:?}: the initiation must answer HTTP 200, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        let upload_id = initiated
            .element("UploadId")
            .expect("an initiation must report an UploadId");

        let mut etags: Vec<String> = Vec::new();
        for (number, part) in parts.iter().enumerate() {
            let uploaded = send_bytes(
                &router,
                "PUT",
                &format!(
                    "/{name}/{key}?partNumber={}&uploadId={upload_id}",
                    number + 1
                ),
                &[],
                part,
            )
            .await;
            assert_eq!(
                uploaded.status,
                200,
                "case {case:?}: uploading part {} must be recorded, got {} with body {:?}",
                number + 1,
                uploaded.status,
                text(&uploaded)
            );
            etags.push(
                uploaded
                    .header("etag")
                    .expect("a part upload must report an ETag header"),
            );
        }

        let mut payload = String::from("<CompleteMultipartUpload>");
        for (number, which) in &row.parts {
            let etag = match which {
                Some(part) => etags[*part].clone(),
                None => String::from("\"00000000000000000000000000000000\""),
            };
            payload.push_str(&format!(
                "<Part><PartNumber>{number}</PartNumber><ETag>{etag}</ETag></Part>"
            ));
        }
        payload.push_str("</CompleteMultipartUpload>");
        let completed = send(
            &router,
            "POST",
            &format!("/{name}/{key}?uploadId={upload_id}"),
            &[],
            &payload,
        )
        .await;

        assert_eq!(
            completed.status,
            400,
            "case {case:?}: a completion that cannot be carried out is refused with 400, \
             got {} with body {:?}",
            completed.status,
            text(&completed)
        );
        assert_eq!(
            completed.error_code(),
            row.code,
            "case {case:?}: expected the code {:?}, got {:?}",
            row.code,
            completed.error_code()
        );
        assert_eq!(
            completed.error_message(),
            row.message,
            "case {case:?}: expected the published sentence {:?}, got {:?}",
            row.message,
            completed.error_message()
        );

        // A refused completion assembled nothing, and the upload is still there: the same
        // parts list completes once it names them correctly.
        let absent = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused completion must assemble nothing, got {}",
            absent.status
        );
        let correct = format!(
            "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{}</ETag></Part>\
             <Part><PartNumber>2</PartNumber><ETag>{}</ETag></Part></CompleteMultipartUpload>",
            etags[0], etags[1]
        );
        let retried = send(
            &router,
            "POST",
            &format!("/{name}/{key}?uploadId={upload_id}"),
            &[],
            &correct,
        )
        .await;
        assert_eq!(
            retried.status,
            200,
            "case {case:?}: the upload must survive a refused completion, got {} with body \
             {:?}",
            retried.status,
            text(&retried)
        );
    }
}

#[tokio::test]
async fn a_metadata_configuration_call_on_a_bucket_carrying_none_answers_the_not_found_code() {
    let router = router();

    // The `MetadataConfiguration` a create records, and the two updates addressing the
    // tables inside it.
    let configuration = format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\"><JournalTableConfiguration>\
         <RecordExpiration><Expiration>DISABLED</Expiration></RecordExpiration>\
         </JournalTableConfiguration></MetadataConfiguration>"
    );
    let journal_update = format!(
        "<JournalTableConfiguration xmlns=\"{XMLNS}\"><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days></RecordExpiration>\
         </JournalTableConfiguration>"
    );
    let inventory_update = format!(
        "<InventoryTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>ENABLED</ConfigurationState></InventoryTableConfiguration>"
    );

    // (the call's name, its method, the subresource it addresses, the body it carries) — one
    // read and the two mutations, so the rule is pinned over every call that addresses a
    // configuration rather than over the read alone.
    let calls: Vec<(&str, &str, &str, &str)> = vec![
        ("GetBucketMetadataConfiguration", "GET", "metadataConfiguration", ""),
        (
            "UpdateBucketMetadataJournalTableConfiguration",
            "PUT",
            "metadataJournalTable",
            journal_update.as_str(),
        ),
        (
            "UpdateBucketMetadataInventoryTableConfiguration",
            "PUT",
            "metadataInventoryTable",
            inventory_update.as_str(),
        ),
    ];

    // The two ways a bucket comes to carry no configuration: it never had one, and a delete
    // took the one it had away.
    for (index, (how, deleted)) in [("a bucket that never had one", false), ("a deleted configuration", true)]
        .iter()
        .enumerate()
    {
        let name = format!("metadata-not-found-{index}");
        bucket(&router, &name).await;
        if *deleted {
            let created =
                create_metadata_configuration(&router, &name, &[], &configuration).await;
            assert_eq!(
                created.status,
                200,
                "case {how:?}: the create must be served, got {} with body {:?}",
                created.status,
                text(&created)
            );
            let removed = send(
                &router,
                "DELETE",
                &format!("/{name}?metadataConfiguration"),
                &[],
                "",
            )
            .await;
            assert_eq!(
                removed.status, 204,
                "case {how:?}: the delete must answer the modelled 204, got {}",
                removed.status
            );
        }

        for (call, method, subresource, body) in &calls {
            let answer = send(
                &router,
                method,
                &format!("/{name}?{subresource}"),
                &[],
                body,
            )
            .await;
            assert_eq!(
                answer.status,
                404,
                "case {how:?}/{call}: expected HTTP 404, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                METADATA_CONFIGURATION_NOT_FOUND_CODE,
                "case {how:?}/{call}: expected the metadata configuration's own not-found \
                 code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
                "case {how:?}/{call}: expected the sentence S3 sends with it, got {:?}",
                answer.error_message()
            );
        }

        // The bucket stays reusable: a fresh create is recorded, and its journal table reaches
        // service. The configuration this row deleted was taken away while its journal table
        // was still being created, so no table was left in the managed namespace holding that
        // name and the fresh create provisions one afresh — the published already-exists
        // failure is for a name a table that reached service still holds, which the namespace
        // suites span.
        let again = create_metadata_configuration(&router, &name, &[], &configuration).await;
        assert_eq!(
            again.status,
            200,
            "case {how:?}: a fresh create must be served, got {} with body {:?}",
            again.status,
            text(&again)
        );
        wait_for_the_tables();
        let read = get_metadata_configuration(&router, &name, &[]).await;
        assert_eq!(
            read.status, 200,
            "case {how:?}: the fresh configuration must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        let document = String::from_utf8_lossy(&read.body).to_string();
        let journal = inside(&document, "JournalTableConfigurationResult").unwrap_or_else(|| {
            panic!("case {how:?}: the read must report a JournalTableConfigurationResult")
        });
        assert!(
            element_text(journal, "TableName").is_some_and(|table| !table.is_empty()),
            "case {how:?}: the journal table configuration must name its table, got {:?}",
            element_text(journal, "TableName")
        );
        assert_eq!(
            element_text(journal, "TableStatus").as_deref(),
            Some("ACTIVE"),
            "case {how:?}: expected the journal table to report ACTIVE, got {:?}",
            element_text(journal, "TableStatus")
        );
    }
}
