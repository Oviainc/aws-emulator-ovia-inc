// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what S3 reports about a DELETE MARKER when a read cannot be served
//! from one: the headers the 404 for a key a marker hides carries, and the headers and the
//! `Allow` of the 405 a read addressed AT a marker earns.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire behaviour.
//!
//! The rules under test, with their sources:
//!
//! * "If the current version is a delete marker, Amazon S3 behaves as if the object was
//!   deleted" (`GetObject`'s own documentation), so a read naming no version answers 404 —
//!   and "if the specified version is a delete marker, the response returns a 405 Method Not
//!   Allowed error" for a read that names the marker's own version.
//! * Either answer REPORTS the marker, because the error document carries no member for it:
//!   `x-amz-delete-marker` "indicates whether the specified object is (true) or is not
//!   (false) a delete marker" (`GetObjectOutput$DeleteMarker`), beside the
//!   `x-amz-version-id` of the marker (`$VersionId`) and the `Last-Modified` of that version
//!   — a marker "has a key name and version ID like any other object" (Amazon S3 User Guide,
//!   *Working with delete markers*). Those headers are the only thing separating a key a
//!   marker hides from one that never existed, since both are a 404.
//! * A 405 names what the resource DOES allow: "the origin server MUST generate an `Allow`
//!   header field in a 405 response containing a list of the target resource's currently
//!   supported methods" (RFC 9110 §15.5.6), and the one method a delete marker supports is
//!   the DELETE that removes it ("to permanently delete a delete marker, you must include
//!   its version ID in a DeleteObject request").

use service_com_amazonaws_s3_amazons3::state::methods_allowed_on;
use service_com_amazonaws_s3_amazons3::state::DELETE_MARKER_RESOURCE_TYPE;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, set_versioning};

/// The bytes the hidden object was written with.
const BODY: &[u8] = b"hidden-by-a-marker";

/// `HEAD /<bucket>/<key><query>` — `HeadObject`.
async fn head_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}{query}"), &[], b"").await
}

#[tokio::test]
async fn a_read_barred_by_a_delete_marker_reports_the_marker() {
    let router = router();
    bucket(&router, "marker-headers").await;
    set_versioning(&router, "marker-headers", "Enabled").await;

    // Two keys, so the table spans both forms of the same rule: a key a marker hides and the
    // marker's own version.
    for key in ["hidden.bin", "deep/hidden.bin"] {
        let written = put_object(&router, "marker-headers", key, BODY, &[]).await;
        assert_eq!(
            written.status,
            200,
            "case {key:?}: the object the marker hides must be written first, got {} with \
             body {:?}",
            written.status,
            text(&written)
        );
        let deleted = delete_object(&router, "marker-headers", key, "").await;
        assert_eq!(
            deleted.status,
            204,
            "case {key:?}: the delete must lay a marker, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {key:?}: the delete must report that it laid a marker, got {:?}",
            deleted.header("x-amz-delete-marker")
        );
        let marker_version = deleted
            .header("x-amz-version-id")
            .expect("the delete must report the version id of the marker it laid");

        // The key, addressed without a version: the marker is the current version, so the
        // object is gone — and the answer says which version made it so.
        let hidden_query = String::new();
        // The marker itself, addressed by its own version: a 405, because a marker is not
        // something a read can be served from.
        let marker_query = format!("?versionId={marker_version}");
        for (case, query, status, code) in [
            ("the key the marker hides", &hidden_query, 404, "NoSuchKey"),
            (
                "the marker's own version",
                &marker_query,
                405,
                "MethodNotAllowed",
            ),
        ] {
            let read = get_object(&router, "marker-headers", key, query, &[]).await;
            let headed = head_object(&router, "marker-headers", key, query).await;
            for (operation, answer) in [("GetObject", &read), ("HeadObject", &headed)] {
                assert_eq!(
                    answer.status, status,
                    "case {key:?}/{case:?} on {operation}: expected HTTP {status}, got {} \
                     with body {:?}",
                    answer.status,
                    text(answer)
                );
                assert_eq!(
                    answer.header("x-amz-delete-marker").as_deref(),
                    Some("true"),
                    "case {key:?}/{case:?} on {operation}: the answer must report that the \
                     version is a delete marker, got {:?}",
                    answer.header("x-amz-delete-marker")
                );
                assert_eq!(
                    answer.header("x-amz-version-id").as_deref(),
                    Some(marker_version.as_str()),
                    "case {key:?}/{case:?} on {operation}: the answer must name the marker's \
                     version, got {:?}",
                    answer.header("x-amz-version-id")
                );
                assert!(
                    answer.header("last-modified").is_some(),
                    "case {key:?}/{case:?} on {operation}: the answer must report when the \
                     marker was written, got {:?}",
                    answer.header("last-modified")
                );
                // The 405 is the only one of the two that advertises a method set: a 404 says
                // the resource is not there, so there is nothing it supports.
                let allowed = match status {
                    405 => methods_allowed_on(DELETE_MARKER_RESOURCE_TYPE),
                    _ => None,
                };
                assert_eq!(
                    answer.header("allow").as_deref(),
                    allowed,
                    "case {key:?}/{case:?} on {operation}: expected the Allow header \
                     {allowed:?}, got {:?}",
                    answer.header("allow")
                );
            }
            assert_eq!(
                read.error_code(),
                code,
                "case {key:?}/{case:?}: expected the code {code:?}, got {:?}",
                read.error_code()
            );
        }

        // Removing the marker brings the object back, which is what the headers above told
        // the caller to do.
        let undeleted = delete_object(
            &router,
            "marker-headers",
            key,
            &format!("?versionId={marker_version}"),
        )
        .await;
        assert_eq!(
            undeleted.status,
            204,
            "case {key:?}: the marker must be removable by its own version, got {} with body \
             {:?}",
            undeleted.status,
            text(&undeleted)
        );
        let back = get_object(&router, "marker-headers", key, "", &[]).await;
        assert_eq!(
            back.status,
            200,
            "case {key:?}: removing the marker must bring the object back, got {} with body \
             {:?}",
            back.status,
            text(&back)
        );
        assert_eq!(
            back.header("x-amz-delete-marker"),
            None,
            "case {key:?}: a served read reports no delete marker, got {:?}",
            back.header("x-amz-delete-marker")
        );
    }
}

#[tokio::test]
async fn a_key_that_never_existed_reports_no_marker() {
    let router = router();
    bucket(&router, "no-marker-headers").await;
    set_versioning(&router, "no-marker-headers", "Enabled").await;

    // The negative control for the rule above: both answers are a 404, and the header is
    // what tells them apart.
    for (case, key) in [
        ("a key nothing was ever written to", "never.bin"),
        ("a key under a prefix nothing holds", "deep/never.bin"),
    ] {
        let read = get_object(&router, "no-marker-headers", key, "", &[]).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header("x-amz-delete-marker"),
            None,
            "case {case:?}: a key that never existed carries no delete-marker header, got \
             {:?}",
            read.header("x-amz-delete-marker")
        );
    }
}
