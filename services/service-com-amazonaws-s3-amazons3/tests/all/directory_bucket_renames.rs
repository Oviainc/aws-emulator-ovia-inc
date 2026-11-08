// SPDX-License-Identifier: CC0-1.0

//! S3 Express One Zone: renaming an object IN a directory bucket.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour — status, error code, and the bytes a later read serves.
//!
//! The rules under test, with their sources:
//!
//! * `RenameObject` "renames an existing object in a directory bucket that uses the S3
//!   Express One Zone storage class. You can use `RenameObject` by specifying an existing
//!   object's name as the source and the new name of the object as the destination within the
//!   same directory bucket" (its own documentation) — so after a rename the destination holds
//!   what the source held and the source holds nothing;
//! * the source arrives in `x-amz-rename-source`, as the object's name with or without the
//!   bucket in front of it and with the one leading slash the member's pattern admits;
//! * a source the bucket does not hold is `NoSuchKey` (404);
//! * "to prevent overwriting an object, you can use the `If-None-Match` conditional header
//!   … Amazon S3 then returns a `412 Precondition Failed` error if the object with the
//!   specified name already exists", and the source and destination conditional members are
//!   refused the same way;
//! * "`RenameObject` supports idempotency using a client token … if you retry a request that
//!   completed successfully using the same client token and the same parameters, the retry
//!   succeeds without performing any further actions. If you retry a successful request using
//!   the same client token, but one or more of the parameters are different, the retry fails
//!   and an `IdempotentParameterMismatch` error is returned"
//!   (`RenameObjectRequest$ClientToken`);
//! * a rename addressed to a GENERAL PURPOSE bucket asks for functionality that endpoint does
//!   not implement, and a bucket this emulator does not hold is `NoSuchBucket` (404).

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// `PUT /<bucket>/<key>?renameObject` — `RenameObject`, carrying the source in
/// `x-amz-rename-source` and whatever conditional or idempotency headers the case names.
async fn rename_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-rename-source", source)];
    all.extend_from_slice(headers);
    send(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}?renameObject"),
        &all,
        "",
    )
    .await
}

/// The object `key` holds, as text, or `None` when the bucket holds nothing at that key.
async fn stored(router: &MultiServiceRouter, bucket_name: &str, key: &str) -> Option<String> {
    let read = get_object(router, bucket_name, key, "", &[]).await;
    (read.status == 200).then(|| text(&read))
}

/// RULE: a rename MOVES the object inside one directory bucket — the destination holds what
/// the source held, and the source holds nothing — whichever of the forms the source name is
/// written in.
///
/// The rows differ along the form of `x-amz-rename-source` (the key alone, the key with the
/// bucket in front of it, the same with the leading slash the member's pattern admits) and
/// along the shape of the keys (a flat name and one with slashes in it).
#[tokio::test]
async fn a_rename_moves_the_object_to_its_new_name() {
    let router = router();
    let name = "renames--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    // (case, the source key, the destination key, how the source is written)
    let rows: [(&str, &str, &str, &str); 4] = [
        ("the object's name alone", "flat-source.txt", "flat-destination.txt", "flat-source.txt"),
        (
            "the bucket in front of the name",
            "qualified-source.txt",
            "qualified-destination.txt",
            "renames--use1-az4--x-s3/qualified-source.txt",
        ),
        (
            "a leading slash before the bucket",
            "slashed-source.txt",
            "logs/2026/slashed-destination.txt",
            "/renames--use1-az4--x-s3/slashed-source.txt",
        ),
        (
            "a key with slashes in it",
            "logs/2026/january.log",
            "archive/january.log",
            "logs/2026/january.log",
        ),
    ];
    for (case, source_key, destination_key, source) in rows {
        let body = format!("the bytes of {source_key}");
        let stored_source = put_object(&router, name, source_key, body.as_bytes(), &[]).await;
        assert_eq!(
            stored_source.status, 200,
            "case {case:?}: the source must be stored first, got {} with body {:?}",
            stored_source.status,
            text(&stored_source)
        );

        let renamed = rename_object(&router, name, destination_key, source, &[]).await;
        assert_eq!(
            renamed.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            renamed.status,
            text(&renamed)
        );

        // Read back through the implemented reader, which is what makes the move real
        // rather than reported.
        assert_eq!(
            stored(&router, name, destination_key).await.as_deref(),
            Some(body.as_str()),
            "case {case:?}: the destination {destination_key:?} must hold the source's bytes, \
             got {:?}",
            stored(&router, name, destination_key).await
        );
        assert_eq!(
            stored(&router, name, source_key).await,
            None,
            "case {case:?}: the source {source_key:?} must hold nothing after the rename, got \
             {:?}",
            stored(&router, name, source_key).await
        );
    }
}

/// RULE: a rename is refused for what it names — a source the bucket does not hold, a bucket
/// that is not a directory bucket, a bucket this emulator does not hold, and a source value
/// that names no key at all — and a refused rename leaves both keys exactly as they were.
#[tokio::test]
async fn a_rename_is_refused_for_the_address_it_names() {
    let router = router();
    let name = "renamerefusals--use1-az4--x-s3";
    let general_purpose = "renamerefusals-general-purpose";
    directory_bucket(&router, name, "use1-az4").await;
    bucket(&router, general_purpose).await;
    let stored_source = put_object(&router, general_purpose, "source.txt", b"body", &[]).await;
    assert_eq!(
        stored_source.status, 200,
        "the general purpose source must be stored first, got {} with body {:?}",
        stored_source.status,
        text(&stored_source)
    );

    // (case, the bucket addressed, the source written, the status and code expected)
    let rows: [(&str, &str, &str, u16, &str); 4] = [
        (
            "a source this directory bucket does not hold",
            name,
            "absent.txt",
            404,
            "NoSuchKey",
        ),
        (
            "a source naming no key at all",
            name,
            "/",
            400,
            "InvalidRequest",
        ),
        (
            "a rename addressed to a general purpose bucket",
            general_purpose,
            "source.txt",
            501,
            "NotImplemented",
        ),
        (
            "a bucket this emulator does not hold",
            "renamerefusals-absent--use1-az4--x-s3",
            "source.txt",
            404,
            "NoSuchBucket",
        ),
    ];
    for (case, addressed, source, status, code) in rows {
        let refused = rename_object(&router, addressed, "destination.txt", source, &[]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (status, code),
            "case {case:?}: expected {status} {code:?}, got {} {:?} with body {:?}",
            refused.status,
            refused.error_code(),
            text(&refused)
        );
        assert_eq!(
            stored(&router, addressed, "destination.txt").await,
            None,
            "case {case:?}: a refused rename must store nothing at the destination, got {:?}",
            stored(&router, addressed, "destination.txt").await
        );
    }
    assert_eq!(
        stored(&router, general_purpose, "source.txt").await.as_deref(),
        Some("body"),
        "a refused rename must leave the source where it was, got {:?}",
        stored(&router, general_purpose, "source.txt").await
    );
}

/// One row of the conditional-rename table: the case, the keys it renames between (its own,
/// so no row depends on what another left behind), whether the destination key already holds
/// an object, the conditional headers the rename carries, and the status expected.
type ConditionRow = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    &'static [(&'static str, &'static str)],
    u16,
);

/// RULE: a conditional rename is performed only while every condition the request carries
/// holds, and is refused `PreconditionFailed` (412) when one does not — the destination's
/// conditions judged against what the destination key holds, the source's against the object
/// being renamed.
///
/// The rows differ along which member carries the condition (destination and source, entity
/// tag and wildcard) and along whether it holds.
#[tokio::test]
async fn a_conditional_rename_holds_its_conditions() {
    let router = router();
    let name = "renameconditions--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    // (case, the source key, the destination key, whether the destination already holds an
    // object, the headers, the status expected)
    let rows: [ConditionRow; 6] = [
        (
            "If-None-Match:* onto a destination holding nothing",
            "none-match-free-source.txt",
            "none-match-free-destination.txt",
            false,
            &[("If-None-Match", "*")],
            200,
        ),
        (
            "If-None-Match:* onto a destination that already exists",
            "none-match-held-source.txt",
            "none-match-held-destination.txt",
            true,
            &[("If-None-Match", "*")],
            412,
        ),
        (
            "If-Match naming a tag the destination does not carry",
            "match-other-tag-source.txt",
            "match-other-tag-destination.txt",
            true,
            &[("If-Match", "\"00000000000000000000000000000000\"")],
            412,
        ),
        (
            "If-Match onto a destination holding nothing",
            "match-free-source.txt",
            "match-free-destination.txt",
            false,
            &[("If-Match", "*")],
            412,
        ),
        (
            "a source entity tag that matches",
            "source-tag-source.txt",
            "source-tag-destination.txt",
            false,
            &[("x-amz-rename-source-if-match", "*")],
            200,
        ),
        (
            "a source entity tag that does not match",
            "source-other-tag-source.txt",
            "source-other-tag-destination.txt",
            false,
            &[(
                "x-amz-rename-source-if-match",
                "\"00000000000000000000000000000000\"",
            )],
            412,
        ),
    ];
    for (case, source_key, destination_key, destination_held, headers, status) in rows {
        let stored_source = put_object(&router, name, source_key, b"source bytes", &[]).await;
        assert_eq!(
            stored_source.status, 200,
            "case {case:?}: the source must be stored first, got {}",
            stored_source.status
        );
        if destination_held {
            let held = put_object(&router, name, destination_key, b"held bytes", &[]).await;
            assert_eq!(
                held.status, 200,
                "case {case:?}: the destination must be stored first, got {}",
                held.status
            );
        }

        let renamed = rename_object(&router, name, destination_key, source_key, headers).await;
        assert_eq!(
            renamed.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            renamed.status,
            text(&renamed)
        );
        if status != 200 {
            assert_eq!(
                renamed.error_code(),
                "PreconditionFailed",
                "case {case:?}: expected PreconditionFailed, got {:?}",
                renamed.error_code()
            );
            assert_eq!(
                stored(&router, name, source_key).await.as_deref(),
                Some("source bytes"),
                "case {case:?}: a refused rename must leave the source where it was, got {:?}",
                stored(&router, name, source_key).await
            );
        }
    }
}

/// RULE: a client token makes a rename idempotent — a retry naming the same token and the
/// same parameters succeeds without renaming anything again, and one naming the same token
/// with other parameters is refused `IdempotentParameterMismatch`.
///
/// The rows differ along which parameter the retry changes (the destination, the source, and
/// nothing at all).
#[tokio::test]
async fn a_client_token_makes_a_rename_idempotent() {
    let router = router();
    let name = "renametokens--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;
    for key in ["first.txt", "second.txt"] {
        let stored_source = put_object(&router, name, key, key.as_bytes(), &[]).await;
        assert_eq!(
            stored_source.status, 200,
            "the object {key:?} must be stored first, got {}",
            stored_source.status
        );
    }

    let token = "client-token-for-one-rename";
    let first = rename_object(
        &router,
        name,
        "renamed.txt",
        "first.txt",
        &[("x-amz-client-token", token)],
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first rename under the token must be performed, got {} with body {:?}",
        first.status,
        text(&first)
    );

    // (case, the destination the retry names, the source the retry names, the status and the
    // code expected)
    let rows: [(&str, &str, &str, u16, &str); 3] = [
        ("the same parameters", "renamed.txt", "first.txt", 200, ""),
        (
            "another destination",
            "renamed-again.txt",
            "first.txt",
            400,
            "IdempotentParameterMismatch",
        ),
        (
            "another source",
            "renamed.txt",
            "second.txt",
            400,
            "IdempotentParameterMismatch",
        ),
    ];
    for (case, destination, source, status, code) in rows {
        let retried = rename_object(
            &router,
            name,
            destination,
            source,
            &[("x-amz-client-token", token)],
        )
        .await;
        assert_eq!(
            retried.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            retried.status,
            text(&retried)
        );
        if !code.is_empty() {
            assert_eq!(
                retried.error_code(),
                code,
                "case {case:?}: expected {code:?}, got {:?}",
                retried.error_code()
            );
        }
    }

    // The retry that named the same parameters renamed nothing a second time: the object the
    // first call moved is where it was, and the object the mismatching retries named is
    // untouched.
    assert_eq!(
        stored(&router, name, "renamed.txt").await.as_deref(),
        Some("first.txt"),
        "the destination must still hold what the one performed rename moved, got {:?}",
        stored(&router, name, "renamed.txt").await
    );
    assert_eq!(
        stored(&router, name, "second.txt").await.as_deref(),
        Some("second.txt"),
        "a refused retry must leave the source it named where it was, got {:?}",
        stored(&router, name, "second.txt").await
    );
    assert_eq!(
        stored(&router, name, "renamed-again.txt").await,
        None,
        "a refused retry must store nothing, got {:?}",
        stored(&router, name, "renamed-again.txt").await
    );
}
