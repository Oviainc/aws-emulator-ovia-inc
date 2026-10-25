// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Content-Type` an object carries.
//!
//! The rule: S3 never infers a type from the key. An object written with no `Content-Type`
//! is stored as `binary/octet-stream` — the default the Amazon S3 User Guide documents for
//! an upload that specifies none — and every later read of it reports exactly that, so a
//! caller sees the same type whatever the key's suffix says. An object written WITH a
//! `Content-Type` reports the one the write carried, unchanged.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};

/// The type an object written with no `Content-Type` carries.
const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// `PUT /<bucket>/<key>` — `PutObject`, with exactly the headers the case names.
async fn put_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send_bytes(router, "PUT", &format!("/{bucket}/{key}"), headers, body).await
}

/// `GET /<bucket>/<key>` — `GetObject`.
async fn get_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}/{key}?x-id=GetObject"), &[], "").await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`.
async fn head_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send(router, "HEAD", &format!("/{bucket}/{key}"), &[], "").await
}

#[tokio::test]
async fn an_object_written_with_no_content_type_reports_the_documented_default() {
    let router = router();
    let bucket = "content-type-default";
    create_bucket(&router, bucket, None, None).await;

    // The rows differ along the one thing a type could wrongly be inferred from: the key's
    // SUFFIX. Every well-known suffix, a suffix no registry knows, a key with a dot but no
    // suffix, a key with no dot at all, and one whose suffix is in the middle of the key —
    // all stored with no `Content-Type` at all, so each must read back as the documented
    // default.
    for (case, key) in [
        ("a .csv key, which a sniffer would call text/csv", "report.csv"),
        ("a .json key", "payload.json"),
        ("a .xml key", "document.xml"),
        ("a .html key", "index.html"),
        ("a .png key", "image.png"),
        ("a .txt key", "notes.txt"),
        ("a suffix no registry knows", "blob.zzz"),
        ("a trailing dot and no suffix", "trailing."),
        ("no dot at all", "nosuffix"),
        ("a suffix in the middle of the key", "a.csv/nested/part"),
    ] {
        let stored = put_object(&router, bucket, key, &[], b"a,b\n1,2\n").await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the write must succeed, got {} with body {:?}",
            stored.status,
            String::from_utf8_lossy(&stored.body)
        );

        for (operation, answer) in [
            ("HeadObject", head_object(&router, bucket, key).await),
            ("GetObject", get_object(&router, bucket, key).await),
        ] {
            assert_eq!(
                answer.status, 200,
                "case {case:?}/{operation}: the read must succeed, got {}",
                answer.status
            );
            assert_eq!(
                answer.header("content-type").as_deref(),
                Some(DEFAULT_CONTENT_TYPE),
                "case {case:?}/{operation}: an object written with no Content-Type reports \
                 the documented default, never a type guessed from the key, got {:?}",
                answer.header("content-type")
            );
        }
    }
}

#[tokio::test]
async fn an_object_written_with_a_content_type_reports_the_one_it_was_given() {
    let router = router();
    let bucket = "content-type-given";
    create_bucket(&router, bucket, None, None).await;

    // The write's own header wins, whatever the key says — including a type that
    // CONTRADICTS the suffix, and the default named explicitly.
    for (case, key, content_type) in [
        ("a type matching the suffix", "report.csv", "text/csv"),
        ("a type contradicting the suffix", "report.csv", "application/json"),
        ("a type on a key with no suffix", "plain", "text/plain"),
        ("a type with parameters", "doc.txt", "text/plain; charset=utf-8"),
        (
            "the default named explicitly",
            "explicit.csv",
            DEFAULT_CONTENT_TYPE,
        ),
    ] {
        let stored = put_object(
            &router,
            bucket,
            key,
            &[("content-type", content_type)],
            b"bytes",
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the write must succeed, got {} with body {:?}",
            stored.status,
            String::from_utf8_lossy(&stored.body)
        );

        for (operation, answer) in [
            ("HeadObject", head_object(&router, bucket, key).await),
            ("GetObject", get_object(&router, bucket, key).await),
        ] {
            assert_eq!(
                answer.header("content-type").as_deref(),
                Some(content_type),
                "case {case:?}/{operation}: the read must report the type the write \
                 carried, got {:?}",
                answer.header("content-type")
            );
        }
    }
}

#[tokio::test]
async fn a_copy_that_replaces_metadata_without_a_content_type_takes_the_default() {
    let router = router();
    let bucket = "content-type-copy";
    create_bucket(&router, bucket, None, None).await;

    let stored = put_object(
        &router,
        bucket,
        "source.csv",
        &[("content-type", "text/csv")],
        b"a,b\n",
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the source must be stored first, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    // A `COPY` directive carries the source's type over; a `REPLACE` that names no type
    // replaces it with the default a write naming none gets, never with the source's.
    for (case, directive, expected) in [
        ("the COPY directive carries the source's type", "COPY", "text/csv"),
        (
            "a REPLACE naming no type takes the default",
            "REPLACE",
            DEFAULT_CONTENT_TYPE,
        ),
    ] {
        let key = format!("copy-{directive}.csv");
        let copied = send_bytes(
            &router,
            "PUT",
            &format!("/{bucket}/{key}?x-id=CopyObject"),
            &[
                ("x-amz-copy-source", &format!("{bucket}/source.csv")),
                ("x-amz-metadata-directive", directive),
            ],
            b"",
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: the copy must succeed, got {} with body {:?}",
            copied.status,
            String::from_utf8_lossy(&copied.body)
        );
        let read = head_object(&router, bucket, &key).await;
        assert_eq!(
            read.header("content-type").as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            read.header("content-type")
        );
    }
}
