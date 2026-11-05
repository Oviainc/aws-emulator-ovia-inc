// SPDX-License-Identifier: CC0-1.0

//! What a general purpose bucket answers a request that addresses a RENAME.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour — status, error code, `<Message>`, the element that names what was refused
//! — and what the two keys hold afterwards.
//!
//! The rule under test: `RenameObject` "renames an existing object in a directory bucket that
//! uses the S3 Express One Zone storage class" (its own documentation) and is addressed by the
//! `?renameObject` query parameter, which the model pins on its URI
//! (`smithy_query(query="operation",operation="RenameObject")` reports
//! `http {method: "PUT", uri: "/{Bucket}/{Key+}?renameObject"}`). A general purpose bucket's
//! endpoint therefore implements nothing for that parameter: the request is refused with HTTP
//! 501 `NotImplemented`, "a query parameter you provided implies functionality that is not
//! implemented", with the `<QueryParameter>` naming it — and nothing is written, so a rename
//! a general purpose bucket cannot serve never lands as an ordinary write of the bytes the
//! request carried.
//!
//! The rows differ along what the rule generalises over: whether the request carries the
//! `x-amz-rename-source` header the rename's own route requires (which is what decides
//! whether the request is shaped like a rename or like an ordinary write), how the parameter
//! is spelled in the query string, whether another query parameter travels beside it, and
//! whether the destination key already holds an object.

use service_com_amazonaws_s3_amazons3::scaffolded::NOT_IMPLEMENTED_QUERY_PARAMETER_MESSAGE;
use service_com_amazonaws_s3_amazons3::service::object_write::RENAME_OBJECT_PARAMETER;
use service_com_amazonaws_s3_amazons3::state::{NOT_IMPLEMENTED_CODE, NOT_IMPLEMENTED_STATUS};

use super::bucket_lifecycle::{router, send_bytes};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The bytes every object in this suite holds, so a refused rename that had instead been
/// served as a write would be visible as a key holding the request's own payload.
const SOURCE: &[u8] = b"rename-source-payload";

/// The bytes a refused request carries as its body — what an ordinary write would have
/// stored at the key it addressed.
const WRITTEN_IF_SERVED: &[u8] = b"renamed-by-a-write";

/// One row: the case, the query string the request carries after the key, the headers it
/// carries, and whether the destination key already holds an object.
struct Row {
    case: &'static str,
    query: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    destination_held: bool,
}

/// The `x-amz-rename-source` the rename's own route requires
/// (`RenameObjectRequest$RenameSource`, `@required` and bound to that header, with the
/// pattern `^\/?.+\/.+$`), spelled as the source object's `<bucket>/<key>`.
const RENAME_SOURCE: &str = "general-purpose-renames/source.bin";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the bare parameter, with the rename source header the route requires",
            query: "?renameObject",
            headers: vec![("x-amz-rename-source", RENAME_SOURCE)],
            destination_held: false,
        },
        Row {
            case: "the bare parameter and no rename source header at all",
            query: "?renameObject",
            headers: Vec::new(),
            destination_held: false,
        },
        Row {
            case: "the parameter spelled with an empty value",
            query: "?renameObject=",
            headers: vec![("x-amz-rename-source", RENAME_SOURCE)],
            destination_held: false,
        },
        Row {
            case: "another parameter travelling in front of it",
            query: "?x-id=PutObject&renameObject",
            headers: Vec::new(),
            destination_held: false,
        },
        Row {
            case: "a destination key that already holds an object",
            query: "?renameObject",
            headers: vec![("x-amz-rename-source", RENAME_SOURCE)],
            destination_held: true,
        },
    ]
}

#[tokio::test]
async fn a_rename_addressed_to_a_general_purpose_bucket_is_not_implemented() {
    let router = router();
    let name = "general-purpose-renames";
    bucket(&router, name).await;
    let stored = put_object(&router, name, "source.bin", SOURCE, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the source object must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    for (index, row) in rows().iter().enumerate() {
        let Row {
            case,
            query,
            headers,
            destination_held,
        } = row;
        let key = format!("renamed/row-{index}.bin");
        if *destination_held {
            let held = put_object(&router, name, &key, SOURCE, &[]).await;
            assert_eq!(
                held.status, 200,
                "case {case:?}: the destination fixture write must succeed, got {}",
                held.status
            );
        }

        let refused = send_bytes(
            &router,
            "PUT",
            &format!("/{name}/{key}{query}"),
            headers,
            WRITTEN_IF_SERVED,
        )
        .await;
        assert_eq!(
            refused.status,
            NOT_IMPLEMENTED_STATUS,
            "case {case:?}: expected HTTP {NOT_IMPLEMENTED_STATUS}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            NOT_IMPLEMENTED_CODE,
            "case {case:?}: expected the published code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            NOT_IMPLEMENTED_QUERY_PARAMETER_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
        // The sentence names the parameter CLASS and not the parameter, so the element is the
        // only part of the refusal that says which one the caller must drop.
        assert_eq!(
            refused.element("QueryParameter").as_deref(),
            Some(RENAME_OBJECT_PARAMETER),
            "case {case:?}: the refusal must name the parameter that asked for it, got {:?} \
             in {:?}",
            refused.element("QueryParameter"),
            text(&refused)
        );

        // Nothing was written: a destination that held nothing still holds nothing, and one
        // that held the source bytes still holds exactly those.
        let after = get_object(&router, name, &key, "", &[]).await;
        let expected = if *destination_held { 200 } else { 404 };
        assert_eq!(
            after.status, expected,
            "case {case:?}: expected the destination to answer {expected} after the refusal, \
             got {} with body {:?}",
            after.status,
            text(&after)
        );
        if *destination_held {
            assert_eq!(
                after.body.as_ref(),
                SOURCE,
                "case {case:?}: a refused rename must leave the destination's bytes alone"
            );
        }

        // And the source is still there, with its own bytes: nothing was moved.
        let source = get_object(&router, name, "source.bin", "", &[]).await;
        assert_eq!(
            source.body.as_ref(),
            SOURCE,
            "case {case:?}: a refused rename must leave the source exactly as it was, got \
             {:?}",
            text(&source)
        );
    }

    // The SAME write without the parameter is served, so the refusals above are about the
    // parameter and not about the request's path, headers or payload.
    let served = put_object(&router, name, "renamed/plain.bin", WRITTEN_IF_SERVED, &[]).await;
    assert_eq!(
        served.status,
        200,
        "an ordinary write of the same bytes must be served, got {} with body {:?}",
        served.status,
        text(&served)
    );

    // A bucket this emulator does not hold answers that, the way the write path answers the
    // same address: the kind of a bucket that is not there says nothing about the parameter.
    let absent = send_bytes(
        &router,
        "PUT",
        "/general-purpose-renames-absent/renamed.bin?renameObject",
        &[("x-amz-rename-source", RENAME_SOURCE)],
        WRITTEN_IF_SERVED,
    )
    .await;
    assert_eq!(
        absent.status,
        404,
        "a bucket this emulator does not hold answers 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "a bucket this emulator does not hold answers NoSuchBucket, got {:?}",
        absent.error_code()
    );
}

#[tokio::test]
async fn the_same_parameter_is_not_refused_for_the_parameter_on_a_directory_bucket() {
    let router = router();
    let name = "renames--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;
    let stored = put_object(&router, name, "source.bin", SOURCE, &[]).await;
    assert_eq!(
        stored.status, 200,
        "the source object must be written first, got {}",
        stored.status
    );

    // A directory bucket is the kind of bucket S3 serves a rename for, so the parameter
    // itself is not what its answer refuses: the refusal carries no `<QueryParameter>`
    // naming it. (The rename itself is this emulator's own stub, which is its own issue.)
    let answered = send_bytes(
        &router,
        "PUT",
        &format!("/{name}/renamed.bin?renameObject"),
        &[("x-amz-rename-source", "renames--use1-az4--x-s3/source.bin")],
        b"",
    )
    .await;
    assert_eq!(
        answered.element("QueryParameter"),
        None,
        "a bucket that serves renames must not refuse the parameter, got {:?}",
        text(&answered)
    );
}
