// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KEY a request addresses: the `{Key+}` label below the bucket
//! segment, decoded once and otherwise verbatim — the trailing slash of a "folder" key
//! included.
//!
//! The rule under test, asserted over a SET of keys rather than over one call:
//!
//! * An object key is "a sequence of Unicode characters whose UTF-8 encoding is at most
//!   1,024 bytes long" (`smithy_query(query="doc",shape="ObjectKey")`), and `/` is one of
//!   them: it carries no structure of its own, so a key may hold a slash anywhere, the LAST
//!   position included — which is exactly how the zero-byte "folder" objects a console or a
//!   CLI creates are named. Every operation that addresses an object binds it from the same
//!   greedy label (`smithy_query(query="operation",operation="PutObject,GetObject")` reports
//!   `Key: {httpLabel}` on the `/{Bucket}/{Key+}` route), so the key a write stores, the key
//!   a read answers, the key a listing reports and the key a copy source resolves against
//!   are one key.
//! * A key that differs from another only by a trailing slash is ANOTHER key: it is stored,
//!   read and reported on its own, and a reference to one of the two never resolves to the
//!   object of the other.
//! * The label's own decode runs exactly once: a `%XX` escape in the path stands for the byte
//!   it spells and nothing is decoded a second time, so a key whose own name contains `%20`
//!   is addressed as `%2520`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! behaviour.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The bytes every object in this suite holds.
const BODY: &[u8] = b"key-label-subject";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket: &str,
    destination_key: &str,
    copy_source: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket}/{destination_key}"),
        &[("x-amz-copy-source", copy_source)],
        b"",
    )
    .await
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`.
async fn delete_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send(router, "DELETE", &format!("/{bucket}/{key}"), &[], "").await
}

/// The keys a listing of `bucket` reports, in the order the document carries them.
async fn listed_keys(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let listed = send(router, "GET", &format!("/{bucket}?list-type=2"), &[], "").await;
    assert_eq!(
        listed.status,
        200,
        "a listing must answer HTTP 200, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    listed.elements("Key")
}

#[tokio::test]
async fn a_key_is_stored_and_reported_exactly_as_it_was_addressed() {
    let router = router();
    let name = "key-label-verbatim";
    bucket(&router, name).await;

    // (case, the key as addressed, the key that differs from it only by its trailing
    // slashes). The rows differ along how many trailing slashes the key carries and along
    // what stands before them — a top-level folder, a folder below one, a file name that
    // ends in a slash, and a key ending in two of them.
    let rows: [(&str, &str, &str); 4] = [
        ("a top-level folder key", "sourc/", "sourc"),
        ("a folder below another", "sourc/reports/", "sourc/reports"),
        (
            "a file name that ends in a slash",
            "sourc/reports/q1 final.txt/",
            "sourc/reports/q1 final.txt",
        ),
        ("two trailing slashes", "archive//", "archive"),
    ];

    for (case, key, shorter) in rows {
        let written = put_object(&router, name, &path_form(key), BODY, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the write of {key:?} must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );

        // The key reads back under the name it was written with …
        let read = get_object(&router, name, &path_form(key), "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: {key:?} must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: {key:?} must hold the bytes written to it, got {:?}",
            text(&read)
        );
        // … the listing reports that same name …
        let listed = listed_keys(&router, name).await;
        assert!(
            listed.iter().any(|reported| reported == key),
            "case {case:?}: the listing must report {key:?} verbatim, got {listed:?}"
        );
        // … and the name without the trailing slashes is another key, which holds nothing.
        let other = get_object(&router, name, &path_form(shorter), "", &[]).await;
        assert_eq!(
            other.status, 404,
            "case {case:?}: {shorter:?} is another key and holds nothing, got {} with \
             body {:?}",
            other.status,
            text(&other)
        );
        // So is a copy whose source names it: the reference resolves to nothing rather than
        // to the object the request never named.
        let copied = copy_object(
            &router,
            name,
            "copies/attempt.txt",
            &format!("{name}/{shorter}"),
        )
        .await;
        assert_eq!(
            copied.status, 404,
            "case {case:?}: a copy source naming {shorter:?} must resolve to nothing, got \
             {} with body {:?}",
            copied.status,
            text(&copied)
        );
        assert_eq!(
            copied.element("Key").as_deref(),
            Some(shorter),
            "case {case:?}: the refusal must name the key the source resolved to, got {:?}",
            copied.element("Key")
        );

        // And a delete takes THAT key away, not its shorter neighbour.
        let deleted = delete_object(&router, name, &path_form(key)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete of {key:?} must be served, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        let gone = get_object(&router, name, &path_form(key), "", &[]).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: {key:?} must be gone after its delete, got {} with body {:?}",
            gone.status,
            text(&gone)
        );
        let after = listed_keys(&router, name).await;
        assert!(
            !after.iter().any(|reported| reported == key),
            "case {case:?}: the listing must stop reporting {key:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_percent_escape_in_the_path_is_decoded_exactly_once() {
    let router = router();
    let name = "key-label-escapes";
    bucket(&router, name).await;

    // The path carries `%2520`, which decodes ONCE to the three characters `%20`: the key
    // stored is `reports/q1%202024%20final.txt`.
    let written = put_object(&router, name, "reports/q1%25202024%2520final.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the write must be served, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let listed = listed_keys(&router, name).await;
    assert_eq!(
        listed,
        vec!["reports/q1%202024%20final.txt".to_string()],
        "the listing must report the key one decode produced, got {listed:?}"
    );
    // So the path that spells the SPACE names another key, which holds nothing.
    let spaced = get_object(&router, name, "reports/q1%202024%20final.txt", "", &[]).await;
    assert_eq!(
        spaced.status, 404,
        "the spelled-out space names another key, got {} with body {:?}",
        spaced.status,
        text(&spaced)
    );
}

/// `key` as a request PATH carries it: a space written as its escape, so the request line is
/// a request line. Every other character of a key this suite uses stands for itself.
fn path_form(key: &str) -> String {
    key.replace(' ', "%20")
}
