// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the `x-amz-copy-source` header of a copy is READ: the value is
//! one URL-encoded reference, so it is decoded as URL-encoded text and only then split into
//! the bucket, the key and the version it names.
//!
//! The rule under test, asserted over a SET of spellings rather than over one call:
//!
//! * The member's value "must be URL-encoded"
//!   (`smithy_query(query="doc",shape="CopyObjectRequest",member="CopySource")`, and the same
//!   sentence on `UploadPartCopyRequest$CopySource`), so every substitution URL encoding
//!   makes is undone once, over the whole value: a `%XX` escape stands for the byte it
//!   spells, and a `+` stands for a SPACE. A key that really carries a plus has to arrive as
//!   `%2B`, exactly as a key carrying a slash of its own has to arrive as `%2F` for the
//!   separator to stay the separator — and a value spelling the plus literally names the key
//!   with a space in that position, which is the key the copy then resolves (or fails to
//!   resolve) against.
//! * The reference is resolved in STATE afterwards: a decoded bucket-and-key pair the store
//!   does not hold is reported as the thing that is missing — `NoSuchKey` naming the DECODED
//!   key — and nothing is written at the destination.
//! * Both operations that take the member read it by the same rule, so a reference names the
//!   same object for the whole-object copy and for the part copy.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! document.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::set_versioning;

/// The bytes every source object in this suite holds.
const BODY: &[u8] = b"copy-source-encoding-subject";

/// The code and sentence S3 answers a copy whose decoded reference names a key the source
/// bucket does not hold.
const NO_SUCH_KEY: &str = "NoSuchKey";
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// The code and sentence a value that is no `<bucket>/<key>` reference at all earns.
const INVALID_ARGUMENT: &str = "InvalidArgument";
const INVALID_COPY_SOURCE_URI: &str = "Invalid copy source URI.";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`. The header is what
/// separates this route from `PutObject` on the same method and path, so it is always sent.
async fn copy_object(
    router: &MultiServiceRouter,
    destination_bucket: &str,
    destination_key: &str,
    copy_source: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{destination_bucket}/{destination_key}"),
        &[("x-amz-copy-source", copy_source)],
        b"",
    )
    .await
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(router: &MultiServiceRouter, name: &str, key: &str) -> String {
    let initiated = send(router, "POST", &format!("/{name}/{key}?uploads"), &[], "").await;
    assert_eq!(
        initiated.status,
        200,
        "an initiation must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId")
}

/// `PUT /<bucket>/<key>?partNumber=1&uploadId=<id>` carrying `x-amz-copy-source` —
/// `UploadPartCopy`, the other operation that takes the same member.
async fn upload_part_copy(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    copy_source: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPartCopy"),
        &[("x-amz-copy-source", copy_source)],
        b"",
    )
    .await
}

/// What one spelling of a reference must resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolves {
    /// The object the row stored, so the copy is served and holds its bytes.
    StoredObject,
    /// A key the bucket does not hold — the decoded key this names — so the copy is refused
    /// and writes nothing.
    MissingKey(&'static str),
    /// No reference at all, because the decoded bucket half is no bucket name.
    NoReference,
}

/// Assert `answer` is a refusal with `status`, `code` and `message`.
fn assert_refused(answer: &Answer, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected the {code:?} code, got {:?} in {:?}",
        answer.error_code(),
        text(answer)
    );
    assert_eq!(
        answer.error_message(),
        message,
        "case {case:?}: expected the published sentence {message:?}, got {:?}",
        answer.error_message()
    );
}

#[tokio::test]
async fn a_copy_source_is_read_as_one_url_encoded_reference() {
    let router = router();
    let origin = "copy-encoding-origin";
    let destination = "copy-encoding-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;

    // (case, the key the source object is stored under, the `x-amz-copy-source` as sent,
    // what that value resolves to). The rows differ along WHICH substitution the client's
    // spelling leans on — a plus for a space, the `%20` escape, the literal character, the
    // escape of a plus the KEY carries, the separator escaped as well — and along which
    // half of the reference carries it, so the table spans the rule rather than one call.
    let rows: [(&str, &str, &str, Resolves); 9] = [
        (
            "a plus where the stored key has a space",
            "reports/q1 2024 final.txt",
            "copy-encoding-origin/reports/q1+2024+final.txt",
            Resolves::StoredObject,
        ),
        (
            "the %20 escape for that same space",
            "reports/q1 2024 final.txt",
            "copy-encoding-origin/reports/q1%202024%20final.txt",
            Resolves::StoredObject,
        ),
        (
            "the space left literal",
            "reports/q1 2024 final.txt",
            "copy-encoding-origin/reports/q1 2024 final.txt",
            Resolves::StoredObject,
        ),
        (
            "a plus for a space with the separator escaped too",
            "reports/q1 2024 final.txt",
            "copy-encoding-origin%2Freports%2Fq1+2024+final.txt",
            Resolves::StoredObject,
        ),
        (
            "a plus the stored key really carries, left literal",
            "reports/q1+2024.txt",
            "copy-encoding-origin/reports/q1+2024.txt",
            Resolves::MissingKey("reports/q1 2024.txt"),
        ),
        (
            "that same plus escaped as %2B",
            "reports/q1+2024.txt",
            "copy-encoding-origin/reports/q1%2B2024.txt",
            Resolves::StoredObject,
        ),
        (
            "an escaped plus with the separator escaped too",
            "reports/q1+2024.txt",
            "copy-encoding-origin%2Freports%2Fq1%2B2024.txt",
            Resolves::StoredObject,
        ),
        (
            "a plus in a key the store holds with neither plus nor space",
            "reports/q12024.txt",
            "copy-encoding-origin/reports/q1+2024.txt",
            Resolves::MissingKey("reports/q1 2024.txt"),
        ),
        (
            "a plus in the bucket half, which no bucket name may carry",
            "reports/q12024.txt",
            "copy+encoding+origin/reports/q12024.txt",
            Resolves::NoReference,
        ),
    ];

    for (index, (case, stored, copy_source, resolves)) in rows.iter().enumerate() {
        let written = put_object(
            &router,
            origin,
            &percent_encoded_path(stored),
            BODY,
            &[],
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the source object must be stored first, got {} with body {:?}",
            written.status,
            text(&written)
        );

        let destination_key = format!("copies/{index}.txt");
        let copied = copy_object(&router, destination, &destination_key, copy_source).await;
        match resolves {
            Resolves::StoredObject => {
                assert_eq!(
                    copied.status, 200,
                    "case {case:?}: a reference that decodes to {stored:?} must be served, \
                     got {} with body {:?}",
                    copied.status,
                    text(&copied)
                );
                let read = get_object(
                    &router,
                    destination,
                    &percent_encoded_path(&destination_key),
                    "",
                    &[],
                )
                .await;
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: the copy must hold the source's bytes, got {:?}",
                    text(&read)
                );
            }
            Resolves::MissingKey(decoded) => {
                assert_refused(&copied, case, 404, NO_SUCH_KEY, NO_SUCH_KEY_MESSAGE);
                assert_eq!(
                    copied.element("Key").as_deref(),
                    Some(*decoded),
                    "case {case:?}: the envelope must name the decoded key the value named, \
                     got {:?} in {:?}",
                    copied.element("Key"),
                    text(&copied)
                );
            }
            Resolves::NoReference => {
                assert_refused(
                    &copied,
                    case,
                    400,
                    INVALID_ARGUMENT,
                    INVALID_COPY_SOURCE_URI,
                );
            }
        }
        if *resolves != Resolves::StoredObject {
            let absent = get_object(
                &router,
                destination,
                &percent_encoded_path(&destination_key),
                "",
                &[],
            )
            .await;
            assert_eq!(
                absent.status, 404,
                "case {case:?}: a refused copy must store nothing, got {} with body {:?}",
                absent.status,
                text(&absent)
            );
        }

        // The PART copy reads the same member by the same rule, so the same spelling names
        // the same object there.
        let upload_key = format!("assembled/{index}.txt");
        let upload_id = initiate(&router, destination, &percent_encoded_path(&upload_key)).await;
        let part = upload_part_copy(
            &router,
            destination,
            &percent_encoded_path(&upload_key),
            &upload_id,
            copy_source,
        )
        .await;
        let expected_status = match resolves {
            Resolves::StoredObject => 200,
            Resolves::MissingKey(_) => 404,
            Resolves::NoReference => 400,
        };
        assert_eq!(
            part.status, expected_status,
            "case {case:?}/UploadPartCopy: expected HTTP {expected_status}, got {} with \
             body {:?}",
            part.status,
            text(&part)
        );
    }
}

#[tokio::test]
async fn a_version_qualified_copy_source_decodes_its_key_the_same_way() {
    let router = router();
    let origin = "copy-encoding-versioned";
    let destination = "copy-encoding-versioned-to";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    set_versioning(&router, origin, "Enabled").await;

    // A key carrying a plus of its own, so the reference has to escape it in BOTH halves of
    // the value: the key half and the `?versionId=` subresource the value appends.
    let key = "reports/q1+2024.txt";
    let written = put_object(&router, origin, &percent_encoded_path(key), BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source version must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let version_id = written
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket must report the version it stored");

    // (case, the `x-amz-copy-source` as sent, the status it earns). The escaped plus names
    // the stored key, so the version is found; the literal plus names a key with a space,
    // which holds no version at all.
    let rows: [(&str, String, u16); 2] = [
        (
            "the key's plus escaped, the version named",
            format!("{origin}/reports/q1%2B2024.txt?versionId={version_id}"),
            200,
        ),
        (
            "the key's plus left literal, the same version named",
            format!("{origin}/reports/q1+2024.txt?versionId={version_id}"),
            404,
        ),
    ];

    for (index, (case, copy_source, status)) in rows.iter().enumerate() {
        let destination_key = format!("copies/{index}.txt");
        let copied = copy_object(&router, destination, &destination_key, copy_source).await;
        assert_eq!(
            copied.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        if *status == 200 {
            let read = get_object(&router, destination, &destination_key, "", &[]).await;
            assert_eq!(
                read.body.as_ref(),
                BODY,
                "case {case:?}: the copy must hold the source version's bytes, got {:?}",
                text(&read)
            );
        }
    }
}

/// `key` as a request PATH carries it: the one substitution a path does not make for itself,
/// the space, written as its escape so the request line is a request line. A plus is left
/// alone, because a path's plus IS a plus — only the copy-source HEADER reads one as a space.
fn percent_encoded_path(key: &str) -> String {
    key.replace(' ', "%20")
}
