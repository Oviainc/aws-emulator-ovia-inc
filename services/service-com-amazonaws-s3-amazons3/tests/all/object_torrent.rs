// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetObjectTorrent` — the `?torrent` subresource of an object.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, error code, error message — rather than any handler's internals.
//!
//! The rules under test:
//!
//! * Amazon S3 serves no BitTorrent delivery of an object, so a torrent request against a
//!   bucket it holds is refused with HTTP 405 `MethodNotAllowed` ("the specified method is
//!   not allowed against this resource"), whatever the key names and whatever the object
//!   at it is — the refusal is about the delivery mechanism, not about the object, so it
//!   never depends on the object's size, its form or whether the bucket holds that key at
//!   all. No `Body` is answered, in place of an empty one a client would read as a torrent
//!   with no peers.
//! * The request's own address is judged first: a bucket this emulator does not hold is
//!   `NoSuchBucket` (404), and an `x-amz-expected-bucket-owner` that is not the account it
//!   serves is `AccessDenied` (403) — "the request fails with the HTTP status code 403
//!   Forbidden (access denied)", that member's own documentation.
//! * The call reports and changes nothing: every object the bucket held before it is
//!   readable afterwards, byte for byte, and a key it did not hold is still absent.

use service_com_amazonaws_s3_amazons3::state::{
    METHOD_NOT_ALLOWED_CODE, METHOD_NOT_ALLOWED_MESSAGE, METHOD_NOT_ALLOWED_STATUS,
    NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE, NO_SUCH_KEY_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object};

/// An account id that is NOT the one this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has.
const NOT_THE_OWNER: &str = "209876543210";

/// `GET /<bucket>/<key>?torrent` — `GetObjectTorrent`, with whatever headers the case
/// names.
async fn get_object_torrent(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket}/{key}?torrent"),
        headers,
        b"",
    )
    .await
}

/// One row of the torrent table: the case, the key as the request URI spells it, how many
/// bytes to store at it first (`None` for a key the bucket must NOT hold), and the headers
/// the torrent request carries.
struct Row {
    case: &'static str,
    key: &'static str,
    stored: Option<usize>,
    headers: Vec<(&'static str, &'static str)>,
}

/// The rows: one torrent request per thing the rule generalises over — where the key sits,
/// how it is spelled on the wire, how big the object at it is, whether the bucket holds it
/// at all, and which of the operation's optional headers the request carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a key at the bucket root",
            key: "torrent-target.bin",
            stored: Some(5),
            headers: Vec::new(),
        },
        Row {
            case: "a key under several path segments",
            key: "deep/path/to/object.iso",
            stored: Some(16),
            headers: Vec::new(),
        },
        Row {
            case: "a key the request URI carries percent-encoded",
            key: "spaced%20key.txt",
            stored: Some(3),
            headers: Vec::new(),
        },
        Row {
            case: "a key spelled like the subresource itself",
            key: "torrent",
            stored: Some(4),
            headers: Vec::new(),
        },
        Row {
            case: "an object of no bytes at all",
            key: "empty.bin",
            stored: Some(0),
            headers: Vec::new(),
        },
        Row {
            case: "an object of many kilobytes",
            key: "large.bin",
            stored: Some(70_000),
            headers: Vec::new(),
        },
        Row {
            case: "a key the bucket does not hold",
            key: "never-written.bin",
            stored: None,
            headers: Vec::new(),
        },
        Row {
            // "Confirms that the requester knows that they will be charged for the
            // request" (`GetObjectTorrentRequest$RequestPayer`) — an optional header that
            // changes who pays, not what is served.
            case: "a request confirming that the requester pays",
            key: "requester-pays.bin",
            stored: Some(7),
            headers: vec![("x-amz-request-payer", "requester")],
        },
        Row {
            // The owner condition HOLDS here, so the request gets past it and meets the
            // same refusal as a request naming no owner at all.
            case: "a request naming the bucket's own owner",
            key: "right-owner.bin",
            stored: Some(9),
            headers: vec![(
                "x-amz-expected-bucket-owner",
                // `emulator_account_id` is this emulator's one account, borrowed for the
                // program's lifetime, so it sits in a table of `&'static str` headers.
                engine::emulator_account_id(),
            )],
        },
    ]
}

/// `size` bytes to store at a key: a repeating pattern, so an object read back is compared
/// against bytes a handler could not have guessed.
fn body(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

#[tokio::test]
async fn a_torrent_request_is_refused_as_a_method_the_resource_does_not_allow() {
    let router = router();
    let name = "object-torrent";
    bucket(&router, name).await;

    let rows = rows();

    // Every row that names a stored object is written first, so the torrent request meets
    // a real object rather than an empty bucket.
    for row in &rows {
        let Some(size) = row.stored else {
            continue;
        };
        let stored = put_object(
            &router,
            name,
            row.key,
            &body(size),
            &[("content-type", "application/octet-stream")],
        )
        .await;
        assert_eq!(
            stored.status,
            200,
            "case {:?}: the object must be stored first, got {} with body {:?}",
            row.case,
            stored.status,
            String::from_utf8_lossy(&stored.body)
        );
    }

    for row in &rows {
        let answer = get_object_torrent(&router, name, row.key, &row.headers).await;
        let code = answer.error_code();
        let message = answer.error_message();
        assert_eq!(
            answer.status, METHOD_NOT_ALLOWED_STATUS,
            "case {:?}: expected HTTP {METHOD_NOT_ALLOWED_STATUS}, got {} with body {:?}",
            row.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code, METHOD_NOT_ALLOWED_CODE,
            "case {:?}: expected the error code {METHOD_NOT_ALLOWED_CODE:?}, got {code:?} with \
             body {:?}",
            row.case,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            message, METHOD_NOT_ALLOWED_MESSAGE,
            "case {:?}: expected the message {METHOD_NOT_ALLOWED_MESSAGE:?}, got {message:?}",
            row.case
        );
    }

    // The wire contract spelled out, independent of the constants the table above reads:
    // the code, the status and the message Amazon S3's published "Error responses"
    // `ErrorCodeList` carries for this refusal, so a change to a crate constant cannot move
    // the expectation and the handler together.
    let literal = get_object_torrent(&router, name, "torrent-target.bin", &[]).await;
    assert_eq!(
        literal.status,
        405,
        "the published status for this refusal is 405, got {} with body {:?}",
        literal.status,
        String::from_utf8_lossy(&literal.body)
    );
    assert_eq!(
        literal.error_code(),
        "MethodNotAllowed",
        "the published code for this refusal is \"MethodNotAllowed\", got {:?}",
        literal.error_code()
    );
    assert_eq!(
        literal.error_message(),
        "The specified method is not allowed against this resource.",
        "the published message for this refusal is the one its list entry carries, got {:?}",
        literal.error_message()
    );

    // The refusals reported nothing about the objects and changed nothing: each stored
    // object reads back byte for byte, and the key the bucket never held is still absent.
    for row in &rows {
        let read = get_object(&router, name, row.key, "", &[]).await;
        match row.stored {
            Some(size) => {
                assert_eq!(
                    read.status, 200,
                    "case {:?}: the object must still be readable, got {} with body {:?}",
                    row.case,
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.body.as_ref(),
                    body(size).as_slice(),
                    "case {:?}: the stored bytes must be unchanged, got {} bytes",
                    row.case,
                    read.body.len()
                );
            }
            None => {
                let code = read.error_code();
                assert_eq!(
                    code, NO_SUCH_KEY_CODE,
                    "case {:?}: the key must still be absent, got {code:?} with status {}",
                    row.case, read.status
                );
            }
        }
    }
}

#[tokio::test]
async fn a_torrent_request_naming_a_bucket_that_does_not_exist_is_the_bucket_404() {
    let router = router();
    // No bucket is created at all: the address, not the delivery mechanism, is what the
    // request is answered about — and the key form does not change that.
    for (case, key) in [
        ("a key at the bucket root", "object.bin"),
        ("a key under several path segments", "deep/path/object.bin"),
        ("a key the request URI carries percent-encoded", "spaced%20key.txt"),
    ] {
        let answer = get_object_torrent(&router, "object-torrent-absent-bucket", key, &[]).await;
        let code = answer.error_code();
        let message = answer.error_message();
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code, NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected the error code {NO_SUCH_BUCKET_CODE:?}, got {code:?}"
        );
        assert_eq!(
            message, NO_SUCH_BUCKET_MESSAGE,
            "case {case:?}: expected the message {NO_SUCH_BUCKET_MESSAGE:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_torrent_request_whose_expected_owner_is_not_the_account_is_denied() {
    let router = router();
    let name = "object-torrent-owner";
    bucket(&router, name).await;
    let key = "owned.bin";
    let bytes = body(12);
    let stored = put_object(
        &router,
        name,
        key,
        &bytes,
        &[("content-type", "application/octet-stream")],
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );
    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the foreign account id must not be the account this emulator serves"
    );

    // (case, the bucket the request names) — the owner mismatch is answered for a bucket
    // this emulator holds and for one it does not, since neither is owned by that account.
    for (case, named) in [
        ("a bucket this emulator holds", name),
        ("a bucket it does not hold", "object-torrent-owner-absent"),
    ] {
        let answer = get_object_torrent(
            &router,
            named,
            key,
            &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
        )
        .await;
        let code = answer.error_code();
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code, "AccessDenied",
            "case {case:?}: expected the error code \"AccessDenied\", got {code:?}"
        );
    }

    // The denied calls left the object exactly as it was.
    let read = get_object(&router, name, key, "", &[]).await;
    assert_eq!(
        read.status,
        200,
        "the object must still be readable, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.body.as_ref(),
        bytes.as_slice(),
        "the stored bytes must be unchanged, got {} bytes",
        read.body.len()
    );
}
