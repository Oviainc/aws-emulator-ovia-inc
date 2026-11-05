// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the request HEADERS of the object write and read path, and for the
//! envelope a refusal of one carries.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! call:
//!
//! 1. `x-amz-copy-source` "must be URL-encoded", so it is percent-DECODED and only then read
//!    as `<source-bucket>/<source-key>`: every spelling of one reference — the separator
//!    literal or escaped, a space escaped, one leading slash — names the same stored object,
//!    and a reference that resolves to nothing is reported about the thing that is missing
//!    (`NoSuchBucket` naming the decoded bucket, `NoSuchKey` naming the decoded key) rather
//!    than as a malformed header.
//! 2. A value that names no source object at all — a URI with a scheme, a first segment that
//!    is no bucket name, a subresource other than `versionId` — is a bad ARGUMENT: 400
//!    `InvalidArgument` with `Invalid copy source URI.` and the `<ArgumentName>` /
//!    `<ArgumentValue>` pair naming the header.
//! 3. A copy source whose `?versionId=` is not shaped like a version id is a request S3
//!    cannot serve: 400 `InvalidRequest` with `Invalid Request` — where a WELL-FORMED id the
//!    bucket holds no version for stays the 404 `NoSuchVersion` that reports the version.
//! 4. A copy source naming a DELETE MARKER by version id is 400 `InvalidRequest` with `The
//!    source of a copy request may not specifically refer to a delete marker by version
//!    id.`, which names the condition the version id created.
//! 5. A canned ACL outside the set the operation's own `ACL` member publishes is 400
//!    `InvalidArgument` with an EMPTY `<Message>` and the `<ArgumentName>` /
//!    `<ArgumentValue>` pair naming `x-amz-acl` and the value as sent — on every operation
//!    that takes one.
//! 6. User-defined metadata past the published ceiling is 400 `MetadataTooLarge` with `Your
//!    metadata headers exceed the maximum allowed metadata size`, plus the `<Size>` /
//!    `<MaxSizeAllowed>` pair: the request's own total and the limit it was judged against.
//! 7. A timestamp bound to a request HEADER arrives as an HTTP-date (RFC 9110's
//!    `IMF-fixdate`), which is the format every SDK sends a conditional header in: such a
//!    read is EVALUATED — served when the condition holds, and 412 `PreconditionFailed` with
//!    the `<Condition>` naming the header when it does not — rather than refused for not
//!    being an ISO 8601 date-time.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle, object and multipart suites in this crate own), so what is pinned is
//! the wire document.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::{
    MAX_OBJECT_METADATA_BYTES, VERSION_ID_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, set_versioning};

/// The bytes every source object in this suite holds.
const BODY: &[u8] = b"copy-source-header-subject";

/// The code S3 answers a request field carrying a value it cannot accept with.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The code S3 answers a request it cannot carry out as given with.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The published sentence a copy source that is no `<bucket>/<key>` reference earns.
const INVALID_COPY_SOURCE_URI: &str = "Invalid copy source URI.";

/// The published sentence a copy source that names a bucket and no object key earns.
const INVALID_COPY_SOURCE_KEY: &str = "Invalid copy source object key";

/// The published sentence a copy source whose version id is not one earns.
const COPY_SOURCE_INVALID_VERSION: &str = "Invalid Request";

/// The published sentence a copy source naming a delete marker by version id earns.
const COPY_SOURCE_DELETE_MARKER: &str =
    "The source of a copy request may not specifically refer to a delete marker by version id.";

/// The code and sentence a write whose user-defined metadata is too large earns.
const METADATA_TOO_LARGE: &str = "MetadataTooLarge";
const METADATA_TOO_LARGE_MESSAGE: &str =
    "Your metadata headers exceed the maximum allowed metadata size";

/// The code and sentence a conditional request whose condition does not hold earns.
const PRECONDITION_FAILED: &str = "PreconditionFailed";
const PRECONDITION_FAILED_MESSAGE: &str =
    "At least one of the pre-conditions you specified did not hold";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`. The header is what
/// separates this route from `PutObject` on the same method and path, so it is always sent.
async fn copy_object(
    router: &MultiServiceRouter,
    destination_bucket: &str,
    destination_key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(
        router,
        "PUT",
        &format!("/{destination_bucket}/{destination_key}"),
        &all,
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

/// `PUT /<bucket>/<key>?partNumber=1&uploadId=<id>&x-id=UploadPartCopy` carrying
/// `x-amz-copy-source` — `UploadPartCopy`, the other operation that takes a copy source.
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

/// Assert `answer`'s document names `argument` and echoes `value`.
fn assert_names_argument(answer: &Answer, case: &str, argument: &str, value: &str) {
    assert_eq!(
        answer.element("ArgumentName").as_deref(),
        Some(argument),
        "case {case:?}: the envelope must name the field it rejected, got {:?} in {:?}",
        answer.element("ArgumentName"),
        text(answer)
    );
    assert_eq!(
        answer.element("ArgumentValue").as_deref(),
        Some(value),
        "case {case:?}: the envelope must carry the value for that field, got {:?} in {:?}",
        answer.element("ArgumentValue"),
        text(answer)
    );
}

#[tokio::test]
async fn every_spelling_of_one_copy_source_names_the_same_stored_object() {
    let router = router();
    let origin = "copy-header-origin";
    let destination = "copy-header-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    // A key carrying the two characters a client always percent-encodes: a slash of its own
    // and a space.
    let key = "source/file one.txt";
    let written = put_object(&router, origin, "source/file%20one.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the `x-amz-copy-source` as sent). Every row names `origin`/`key`; the rows
    // differ along WHICH characters the client escaped — the bucket/key separator, the
    // key's own slash, the space — and along the one leading slash the member's pattern
    // allows.
    let rows: [(&str, &str); 5] = [
        (
            "only the space escaped",
            "copy-header-origin/source/file%20one.txt",
        ),
        (
            "every slash escaped as well",
            "copy-header-origin%2Fsource%2Ffile%20one.txt",
        ),
        (
            "the key's own slash escaped, the separator literal",
            "copy-header-origin/source%2Ffile%20one.txt",
        ),
        (
            "a leading slash before the bucket",
            "/copy-header-origin/source/file%20one.txt",
        ),
        (
            "a leading slash and every slash escaped",
            "/copy-header-origin%2Fsource%2Ffile%20one.txt",
        ),
    ];

    for (index, (case, copy_source)) in rows.iter().enumerate() {
        let destination_key = format!("copies/{index}.txt");
        let copied = copy_object(&router, destination, &destination_key, copy_source, &[]).await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: a copy source that decodes to {origin:?}/{key:?} must be \
             served, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        let read = get_object(&router, destination, &destination_key, "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: the copy must hold the source's bytes, got {:?}",
            text(&read)
        );
    }

    // The part copy reads the member by the same rule, so a fully escaped reference names
    // the same object there too.
    let upload_id = initiate(&router, destination, "copies/assembled.txt").await;
    let part = upload_part_copy(
        &router,
        destination,
        "copies/assembled.txt",
        &upload_id,
        "copy-header-origin%2Fsource%2Ffile%20one.txt",
    )
    .await;
    assert_eq!(
        part.status, 200,
        "an escaped copy source must name the same object for a part copy, got {} with \
         body {:?}",
        part.status,
        text(&part)
    );
}

#[tokio::test]
async fn a_copy_source_that_resolves_to_nothing_reports_what_is_missing() {
    let router = router();
    let origin = "copy-missing-origin";
    let destination = "copy-missing-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the copy source as sent, the code, the sentence, the element the document
    // names the missing thing in, the value it must carry). Both rows are fully escaped —
    // the form the header is read in is what decides WHICH thing is reported missing.
    let rows: [(&str, &str, &str, &str, &str, &str); 4] = [
        (
            "an escaped reference to a bucket this emulator does not hold",
            "copy-missing-absent%2Fheld.txt",
            "NoSuchBucket",
            "The specified bucket does not exist",
            "BucketName",
            "copy-missing-absent",
        ),
        (
            "an escaped reference to a key the bucket does not hold",
            "copy-missing-origin%2Fnever%2Fwritten.txt",
            "NoSuchKey",
            "The specified key does not exist.",
            "Key",
            "never/written.txt",
        ),
        (
            "a literal reference to a bucket this emulator does not hold",
            "copy-missing-absent/held.txt",
            "NoSuchBucket",
            "The specified bucket does not exist",
            "BucketName",
            "copy-missing-absent",
        ),
        (
            "a literal reference to a key the bucket does not hold",
            "copy-missing-origin/never/written.txt",
            "NoSuchKey",
            "The specified key does not exist.",
            "Key",
            "never/written.txt",
        ),
    ];

    for (case, copy_source, code, message, element, named) in rows {
        let refused = copy_object(&router, destination, "copies/absent.txt", copy_source, &[]).await;
        assert_refused(&refused, case, 404, code, message);
        assert_eq!(
            refused.element(element).as_deref(),
            Some(named),
            "case {case:?}: the envelope must name the decoded {element} the copy source \
             named, got {:?} in {:?}",
            refused.element(element),
            text(&refused)
        );
    }
}

#[tokio::test]
async fn a_copy_source_that_names_no_object_is_refused_against_the_header() {
    let router = router();
    let origin = "copy-uri-origin";
    let destination = "copy-uri-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // (case, the copy source as sent, the sentence it earns). The rows differ along HOW a
    // value fails to name an object, which is what decides which half of the reading is at
    // fault: a URI scheme or an endpoint host in front of the reference, a first segment
    // that is no bucket name (too short, or carrying characters a bucket name cannot), an
    // empty bucket half and a subresource that is not `versionId` are no REFERENCE at all;
    // a bucket half that IS one with nothing after it names no object KEY — in the three
    // spellings of that, one of which the member's own pattern (`^\/?.+\/.+$`) rejects
    // before the operation is reached, so both layers are covered.
    let rows: [(&str, &str, &str); 11] = [
        (
            "an s3 URI",
            "s3://copy-uri-origin/held.txt",
            INVALID_COPY_SOURCE_URI,
        ),
        (
            "a virtual-hosted-style URL",
            "https://copy-uri-origin.s3.amazonaws.com/held.txt",
            INVALID_COPY_SOURCE_URI,
        ),
        (
            "a bucket half of two characters",
            "ab/held.txt",
            INVALID_COPY_SOURCE_URI,
        ),
        (
            "a bucket half carrying an underscore",
            "copy_uri_origin/held.txt",
            INVALID_COPY_SOURCE_URI,
        ),
        (
            "a bucket half in upper case",
            "COPY-URI-ORIGIN/held.txt",
            INVALID_COPY_SOURCE_URI,
        ),
        ("an empty bucket half", "//held.txt", INVALID_COPY_SOURCE_URI),
        (
            "a subresource other than versionId",
            "copy-uri-origin/held.txt?tagging",
            INVALID_COPY_SOURCE_URI,
        ),
        (
            "a bucket and no key at all",
            "copy-uri-origin",
            INVALID_COPY_SOURCE_KEY,
        ),
        (
            "a bucket and an empty key",
            "copy-uri-origin/",
            INVALID_COPY_SOURCE_KEY,
        ),
        (
            "a bucket and an empty key, the separator escaped",
            "copy-uri-origin%2F",
            INVALID_COPY_SOURCE_KEY,
        ),
        (
            "a leading slash, a bucket and no key",
            "/copy-uri-origin",
            INVALID_COPY_SOURCE_KEY,
        ),
    ];

    for (case, copy_source, message) in rows {
        let refused = copy_object(&router, destination, "copies/bad-uri.txt", copy_source, &[]).await;
        assert_refused(&refused, case, 400, INVALID_ARGUMENT, message);
        assert_names_argument(&refused, case, "x-amz-copy-source", "x-amz-copy-source");

        // The part copy reads the same member by the same rule.
        let upload_id = initiate(&router, destination, "copies/parts.txt").await;
        let part = upload_part_copy(
            &router,
            destination,
            "copies/parts.txt",
            &upload_id,
            copy_source,
        )
        .await;
        assert_refused(
            &part,
            &format!("{case}/UploadPartCopy"),
            400,
            INVALID_ARGUMENT,
            message,
        );
        assert_names_argument(
            &part,
            &format!("{case}/UploadPartCopy"),
            "x-amz-copy-source",
            "x-amz-copy-source",
        );
    }

    // Nothing was written by any refused copy.
    let absent = get_object(&router, destination, "copies/bad-uri.txt", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a refused copy must store nothing, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
}

#[tokio::test]
async fn a_copy_source_version_that_is_not_a_version_id_is_refused_as_a_bad_request() {
    let router = router();
    let origin = "copy-version-origin";
    let destination = "copy-version-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    set_versioning(&router, origin, "Enabled").await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    let minted = written
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket reports the version id it minted");

    // The tokens that are not version ids, each with the reason: a version id is an opaque
    // token of the published width and alphabet
    // (`state::version_id_is_well_formed`), and no issuer mints the zero one.
    let published_width = "a".repeat(VERSION_ID_CHARS);
    let malformed: Vec<(&str, String)> = vec![
        ("a word that is no id", "not-a-version".to_string()),
        (
            "one character short of the published minimum",
            published_width[..VERSION_ID_CHARS - 1].to_string(),
        ),
        (
            "standard Base64's slash, which the shape excludes",
            format!("{}%2F", &published_width[..VERSION_ID_CHARS - 1]),
        ),
        (
            "the zero token, which no issuer mints",
            "A".repeat(VERSION_ID_CHARS),
        ),
    ];

    for (case, version_id) in &malformed {
        let copy_source = format!("copy-version-origin/held.txt?versionId={version_id}");
        let refused = copy_object(
            &router,
            destination,
            "copies/malformed-version.txt",
            &copy_source,
            &[],
        )
        .await;
        assert_refused(
            &refused,
            case,
            400,
            INVALID_REQUEST,
            COPY_SOURCE_INVALID_VERSION,
        );

        let upload_id = initiate(&router, destination, "copies/version-parts.txt").await;
        let part = upload_part_copy(
            &router,
            destination,
            "copies/version-parts.txt",
            &upload_id,
            &copy_source,
        )
        .await;
        assert_refused(
            &part,
            &format!("{case}/UploadPartCopy"),
            400,
            INVALID_REQUEST,
            COPY_SOURCE_INVALID_VERSION,
        );
    }

    // A WELL-FORMED id the bucket holds no version for is the other condition, and keeps
    // its own code: the two are told apart by the token's SHAPE.
    let absent = "0".repeat(VERSION_ID_CHARS);
    let refused = copy_object(
        &router,
        destination,
        "copies/absent-version.txt",
        &format!("copy-version-origin/held.txt?versionId={absent}"),
        &[],
    )
    .await;
    assert_refused(
        &refused,
        "a well-formed id the bucket holds no version for",
        404,
        "NoSuchVersion",
        "The specified version does not exist.",
    );

    // And the id the store DID mint is served, so every row above was refused for the token
    // it named rather than for naming a version at all.
    let copied = copy_object(
        &router,
        destination,
        "copies/minted.txt",
        &format!("copy-version-origin/held.txt?versionId={minted}"),
        &[],
    )
    .await;
    assert_eq!(
        copied.status, 200,
        "the minted version must be copyable, got {} with body {:?}",
        copied.status,
        text(&copied)
    );
}

#[tokio::test]
async fn a_copy_source_naming_a_delete_marker_by_version_id_names_the_condition() {
    let router = router();
    let origin = "copy-marker-origin";
    let destination = "copy-marker-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    set_versioning(&router, origin, "Enabled").await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    let stored_version = written
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket reports the version id it minted");
    let deleted = delete_object(&router, origin, "held.txt", "").await;
    assert_eq!(
        deleted.status, 204,
        "the delete must lay a marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let marker = deleted
        .header("x-amz-version-id")
        .expect("a delete on a versioned bucket reports the marker's version id");

    // (case, the copy source as sent) — the marker named by version id, in both spellings
    // of the reference, so the refusal is about the version the value resolves to rather
    // than about the text.
    for (case, copy_source) in [
        (
            "the marker named by version id",
            format!("copy-marker-origin/held.txt?versionId={marker}"),
        ),
        (
            "the marker named by version id, reference escaped",
            format!("copy-marker-origin%2Fheld.txt?versionId={marker}"),
        ),
    ] {
        let refused = copy_object(
            &router,
            destination,
            "copies/marker.txt",
            &copy_source,
            &[],
        )
        .await;
        assert_refused(
            &refused,
            case,
            400,
            INVALID_REQUEST,
            COPY_SOURCE_DELETE_MARKER,
        );

        let upload_id = initiate(&router, destination, "copies/marker-parts.txt").await;
        let part = upload_part_copy(
            &router,
            destination,
            "copies/marker-parts.txt",
            &upload_id,
            &copy_source,
        )
        .await;
        assert_refused(
            &part,
            &format!("{case}/UploadPartCopy"),
            400,
            INVALID_REQUEST,
            COPY_SOURCE_DELETE_MARKER,
        );
    }

    // Naming NO version, with the marker as the current version, is the other condition:
    // "if the current version is a delete marker, Amazon S3 behaves as if the object was
    // deleted", which is the key not being there — and the answer names the marker that
    // makes it so in its two headers, on each call that resolves the key.
    for (case, answer) in [
        (
            "CopyObject",
            copy_object(
                &router,
                destination,
                "copies/current.txt",
                "copy-marker-origin/held.txt",
                &[],
            )
            .await,
        ),
        (
            "GetObject",
            get_object(&router, origin, "held.txt", "", &[]).await,
        ),
        (
            "HeadObject",
            send(&router, "HEAD", &format!("/{origin}/held.txt"), &[], "").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a key whose current version is a marker is not there, got {} \
             with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the answer must report that the version it resolved to is a \
             delete marker, got {:?}",
            answer.header("x-amz-delete-marker")
        );
        assert_eq!(
            answer.header("x-amz-version-id").as_deref(),
            Some(marker.as_str()),
            "case {case:?}: the answer must name the marker's own version, got {:?}",
            answer.header("x-amz-version-id")
        );
        // A HEAD carries no body, so only the two that do are checked for the document.
        if case != "HeadObject" {
            assert_eq!(
                answer.error_code(),
                "NoSuchKey",
                "case {case:?}: expected the NoSuchKey code, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the version UNDER the marker still copies, so the refusals above were about the
    // marker rather than about the key.
    let copied = copy_object(
        &router,
        destination,
        "copies/under-the-marker.txt",
        &format!("copy-marker-origin/held.txt?versionId={stored_version}"),
        &[],
    )
    .await;
    assert_eq!(
        copied.status, 200,
        "the version under the marker must be copyable, got {} with body {:?}",
        copied.status,
        text(&copied)
    );
}

#[tokio::test]
async fn a_canned_acl_outside_the_published_set_names_the_header_and_the_value() {
    let router = router();
    let origin = "acl-header-origin";
    bucket(&router, origin).await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let upload_id = initiate(&router, origin, "acl-upload.txt").await;

    // The values outside BOTH published sets (`ObjectCannedACL` and `BucketCannedACL`), so
    // one table covers every operation that takes one. The rows differ along HOW a value
    // misses the set: the SDK's own enum NAME rather than the wire value, a published value
    // differing only in case, a prefix of a published one, and a word of its own.
    let rejected = [
        "BucketOwnerFullControl",
        "PRIVATE",
        "public",
        "everyone-read",
    ];

    for value in rejected {
        // (operation, the request it arrives on) — every operation whose input binds `ACL`.
        let answers: Vec<(&str, Answer)> = vec![
            (
                "CopyObject",
                copy_object(
                    &router,
                    origin,
                    "copies/acl.txt",
                    "acl-header-origin/held.txt",
                    &[("x-amz-acl", value)],
                )
                .await,
            ),
            (
                "PutObject",
                put_object(
                    &router,
                    origin,
                    "written-with-acl.txt",
                    BODY,
                    &[("x-amz-acl", value)],
                )
                .await,
            ),
            (
                "CreateMultipartUpload",
                send(
                    &router,
                    "POST",
                    &format!("/{origin}/upload-with-acl.txt?uploads"),
                    &[("x-amz-acl", value)],
                    "",
                )
                .await,
            ),
            (
                "PutObjectAcl",
                send(
                    &router,
                    "PUT",
                    &format!("/{origin}/held.txt?acl"),
                    &[("x-amz-acl", value)],
                    "",
                )
                .await,
            ),
            (
                "PutBucketAcl",
                send(
                    &router,
                    "PUT",
                    &format!("/{origin}?acl"),
                    &[("x-amz-acl", value)],
                    "",
                )
                .await,
            ),
            (
                "CreateBucket",
                send(
                    &router,
                    "PUT",
                    "/acl-header-created",
                    &[("x-amz-acl", value)],
                    "",
                )
                .await,
            ),
            (
                "UploadPartCopy",
                send_bytes(
                    &router,
                    "PUT",
                    &format!(
                        "/{origin}/acl-upload.txt?partNumber=1&uploadId={upload_id}\
                         &x-id=UploadPartCopy"
                    ),
                    &[
                        ("x-amz-copy-source", "acl-header-origin/held.txt"),
                        ("x-amz-acl", value),
                    ],
                    b"",
                )
                .await,
            ),
        ];

        for (operation, answer) in &answers {
            // `UploadPartCopy` binds no `ACL` member at all
            // (`smithy_query(query="operation",operation="UploadPartCopy")`), so the header
            // is not a value it judges: it is ignored, exactly as any header an operation
            // binds nothing from is.
            if *operation == "UploadPartCopy" {
                assert_eq!(
                    answer.status, 200,
                    "case {value:?}/{operation}: an operation that binds no ACL must ignore \
                     the header, got {} with body {:?}",
                    answer.status,
                    text(answer)
                );
                continue;
            }
            let case = format!("{value}/{operation}");
            assert_refused(answer, &case, 400, INVALID_ARGUMENT, "");
            assert_names_argument(answer, &case, "x-amz-acl", value);
        }
    }

    // Nothing the refused requests named came into being.
    for (case, answer) in [
        (
            "the object a refused PutObject named",
            get_object(&router, origin, "written-with-acl.txt", "", &[]).await,
        ),
        (
            "the object a refused CopyObject named",
            get_object(&router, origin, "copies/acl.txt", "", &[]).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a refused write must store nothing, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }
    let bucket_refused = send(&router, "HEAD", "/acl-header-created", &[], "").await;
    assert_eq!(
        bucket_refused.status, 404,
        "a refused CreateBucket must create nothing, got {}",
        bucket_refused.status
    );

    // And a PUBLISHED value is still served on the same requests, so the rows above were
    // refused for being outside the set rather than for carrying the header at all.
    let served = copy_object(
        &router,
        origin,
        "copies/acl-served.txt",
        "acl-header-origin/held.txt",
        &[("x-amz-acl", "bucket-owner-full-control")],
    )
    .await;
    assert_eq!(
        served.status, 200,
        "a published object canned ACL must be served, got {} with body {:?}",
        served.status,
        text(&served)
    );
    let served = send(
        &router,
        "PUT",
        &format!("/{origin}?acl"),
        &[("x-amz-acl", "log-delivery-write")],
        "",
    )
    .await;
    assert_ne!(
        served.error_code(),
        INVALID_ARGUMENT,
        "a published BUCKET canned ACL must get past the value check and be judged by the \
         bucket's own ownership rule instead, got {:?} with body {:?}",
        served.error_code(),
        text(&served)
    );
    assert_eq!(
        served.error_code(),
        "AccessControlListNotSupported",
        "a bucket this emulator creates allows no ACLs, so a published value must reach \
         that refusal, got {} with body {:?}",
        served.status,
        text(&served)
    );
}

/// One `x-amz-meta-*` header whose key and value together measure `bytes` — the unit the
/// published ceiling is stated in ("the sum of the number of bytes in the UTF-8 encoding of
/// each key and value", Amazon S3 User Guide, which is what
/// `state::Object::metadata_bytes` measures).
fn metadata_header(bytes: usize) -> (String, String) {
    let key = "note";
    (
        format!("x-amz-meta-{key}"),
        "m".repeat(bytes - key.len()),
    )
}

#[tokio::test]
async fn metadata_past_the_published_ceiling_names_the_size_and_the_limit() {
    let router = router();
    let origin = "metadata-size-origin";
    bucket(&router, origin).await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the source object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // The totals the table spans: one byte past the ceiling, a little past it, and far
    // past it — so the reported `<Size>` is the request's own total rather than a constant.
    let totals = [
        MAX_OBJECT_METADATA_BYTES + 1,
        MAX_OBJECT_METADATA_BYTES + 97,
        MAX_OBJECT_METADATA_BYTES * 2,
    ];

    for total in totals {
        let (name, value) = metadata_header(total);
        let meta: (&str, &str) = (name.as_str(), value.as_str());
        // (operation, the request it arrives on) — every write that carries user-defined
        // metadata.
        let answers: Vec<(&str, Answer)> = vec![
            (
                "PutObject",
                put_object(&router, origin, "written.txt", BODY, &[meta]).await,
            ),
            (
                "CopyObject",
                copy_object(
                    &router,
                    origin,
                    "copies/metadata.txt",
                    "metadata-size-origin/held.txt",
                    &[("x-amz-metadata-directive", "REPLACE"), meta],
                )
                .await,
            ),
            (
                "CreateMultipartUpload",
                send(
                    &router,
                    "POST",
                    &format!("/{origin}/upload.txt?uploads"),
                    &[meta],
                    "",
                )
                .await,
            ),
        ];

        for (operation, answer) in &answers {
            let case = format!("{total} bytes/{operation}");
            assert_refused(
                answer,
                &case,
                400,
                METADATA_TOO_LARGE,
                METADATA_TOO_LARGE_MESSAGE,
            );
            assert_eq!(
                answer.element("Size").as_deref(),
                Some(total.to_string().as_str()),
                "case {case:?}: the envelope must report the request's own total, got {:?} \
                 in {:?}",
                answer.element("Size"),
                text(answer)
            );
            assert_eq!(
                answer.element("MaxSizeAllowed").as_deref(),
                Some(MAX_OBJECT_METADATA_BYTES.to_string().as_str()),
                "case {case:?}: the envelope must report the limit it was judged against, \
                 got {:?} in {:?}",
                answer.element("MaxSizeAllowed"),
                text(answer)
            );
        }
    }

    // Metadata exactly AT the ceiling is served, so every row above was refused for the
    // bytes it carried rather than for carrying metadata at all.
    let (name, value) = metadata_header(MAX_OBJECT_METADATA_BYTES);
    let served = put_object(
        &router,
        origin,
        "at-the-ceiling.txt",
        BODY,
        &[(name.as_str(), value.as_str())],
    )
    .await;
    assert_eq!(
        served.status, 200,
        "metadata at the ceiling must be served, got {} with body {:?}",
        served.status,
        text(&served)
    );
    let absent = get_object(&router, origin, "written.txt", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a refused write must store nothing, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
}

/// `epoch_seconds` as the HTTP-date (RFC 9110's `IMF-fixdate`) every SDK spells a
/// conditional header in.
fn http_date(epoch_seconds: i64) -> String {
    DateTime::from_secs(epoch_seconds)
        .fmt(Format::HttpDate)
        .expect("the test's own instant renders as an HTTP date")
}

/// `epoch_seconds` as the ISO 8601 `date-time` the same member also reads.
fn date_time(epoch_seconds: i64) -> String {
    DateTime::from_secs(epoch_seconds)
        .fmt(Format::DateTime)
        .expect("the test's own instant renders as an ISO 8601 date-time")
}

/// The instant `answer`'s `Last-Modified` reports, in whole seconds since the epoch — the
/// resolution an HTTP date carries.
fn last_modified_seconds(answer: &Answer) -> i64 {
    let rendered = answer
        .header("last-modified")
        .expect("a read reports the object's Last-Modified");
    DateTime::from_str(&rendered, Format::HttpDate)
        .unwrap_or_else(|error| {
            panic!("a reported Last-Modified must be an HTTP date, got {rendered:?}: {error}")
        })
        .secs()
}

#[tokio::test]
async fn a_conditional_header_sent_as_an_http_date_is_evaluated_rather_than_refused() {
    let router = router();
    let origin = "conditional-date-origin";
    let destination = "conditional-date-target";
    bucket(&router, origin).await;
    bucket(&router, destination).await;
    let written = put_object(&router, origin, "held.txt", BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let read = get_object(&router, origin, "held.txt", "", &[]).await;
    assert_eq!(
        read.status, 200,
        "the object must read back, got {} with body {:?}",
        read.status,
        text(&read)
    );
    let modified = last_modified_seconds(&read);

    // (case, the instant the header names, the status the read must answer). The rows span
    // the comparison the rule is about — before, at and after the stored version's
    // last-modified time — and each is sent in BOTH spellings a client may use: the
    // HTTP-date every SDK sends, and the ISO 8601 date-time the member's own declared
    // format reads.
    let rows: [(&str, i64, u16); 4] = [
        ("a second before the write", modified - 1, 412),
        ("an hour before the write", modified - 3_600, 412),
        ("the instant of the write", modified, 200),
        ("an hour after the write", modified + 3_600, 200),
    ];

    for (case, instant, status) in rows {
        for (spelling, value) in [
            ("an HTTP-date", http_date(instant)),
            ("an ISO 8601 date-time", date_time(instant)),
        ] {
            let answer = get_object(
                &router,
                origin,
                "held.txt",
                "",
                &[("If-Unmodified-Since", value.as_str())],
            )
            .await;
            let case = format!("{case}, as {spelling}");
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status} for {value:?}, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            if status == 412 {
                assert_refused(
                    &answer,
                    &case,
                    412,
                    PRECONDITION_FAILED,
                    PRECONDITION_FAILED_MESSAGE,
                );
                assert_eq!(
                    answer.element("Condition").as_deref(),
                    Some("If-Unmodified-Since"),
                    "case {case:?}: the envelope must name the header that did not hold, \
                     got {:?} in {:?}",
                    answer.element("Condition"),
                    text(&answer)
                );
            } else {
                assert_eq!(
                    answer.body.as_ref(),
                    BODY,
                    "case {case:?}: a condition that holds must serve the object, got {:?}",
                    text(&answer)
                );
            }
        }
    }

    // A conditional date a recipient cannot read states no condition at all, so the read is
    // served: "a recipient MUST ignore the If-Unmodified-Since header field if the received
    // field value is not a valid HTTP-date" (RFC 9110). The rows span what a client may put
    // there: a word, an empty value, and a date in a format HTTP does not define.
    for (case, value) in [
        ("a word", "yesterday"),
        ("an empty value", ""),
        ("a calendar date alone", "2026-11-04"),
        ("an epoch second count", "1788660000"),
    ] {
        for header in ["If-Unmodified-Since", "If-Modified-Since"] {
            let answer = get_object(&router, origin, "held.txt", "", &[(header, value)]).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}/{header}: a condition stated in a value that is no date must \
                 be ignored, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.body.as_ref(),
                BODY,
                "case {case:?}/{header}: the object must be served, got {:?}",
                text(&answer)
            );
        }
        let copied = copy_object(
            &router,
            destination,
            "copies/ignored-condition.txt",
            "conditional-date-origin/held.txt",
            &[("x-amz-copy-source-if-unmodified-since", value)],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}/CopyObject: a copy-source condition stated in a value that is no \
             date must be ignored, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
    }

    // A COPY's own conditional source headers are the same members, read the same way: the
    // copy is refused with the 412 that names the copy-source header, and served when the
    // condition holds.
    for (case, instant, status, condition) in [
        (
            "a copy whose source was modified since the instant named",
            modified - 1,
            412,
            Some("x-amz-copy-source-If-Unmodified-Since"),
        ),
        (
            "a copy whose source was not modified since the instant named",
            modified + 3_600,
            200,
            None,
        ),
    ] {
        let value = http_date(instant);
        let answer = copy_object(
            &router,
            destination,
            "copies/conditional.txt",
            "conditional-date-origin/held.txt",
            &[("x-amz-copy-source-if-unmodified-since", value.as_str())],
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for {value:?}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        if let Some(condition) = condition {
            assert_refused(
                &answer,
                case,
                412,
                PRECONDITION_FAILED,
                PRECONDITION_FAILED_MESSAGE,
            );
            assert_eq!(
                answer.element("Condition").as_deref(),
                Some(condition),
                "case {case:?}: the envelope must name the copy-source header that did not \
                 hold, got {:?} in {:?}",
                answer.element("Condition"),
                text(&answer)
            );
        }
    }
}
