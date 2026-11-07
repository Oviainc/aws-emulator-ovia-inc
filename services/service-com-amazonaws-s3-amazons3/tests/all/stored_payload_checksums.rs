// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the checksum this service computes over the bytes it STORED, and
//! reports back on every write and read of them.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle and object-round-trip suites in this crate already own), so what is
//! pinned is the wire behaviour — the `x-amz-checksum-*` headers, `x-amz-checksum-type`,
//! the `Checksum*` elements of a completion body and the `ChecksumAlgorithm` of a listed
//! annotation — rather than any handler's internals.
//!
//! The two rules under test, with their sources:
//!
//! * A multipart upload completes into an object carrying an object-level checksum whether
//!   or not the upload configured one: under the algorithm the initiation named, and under
//!   the default algorithm otherwise — `CRC64NVME`, whose checksum "is always a full object
//!   checksum" (`CompleteMultipartUploadOutput$ChecksumCRC64NVME`) and whose member is the
//!   one `Checksum*` member documented without the "only present if the checksum was
//!   uploaded with the object" caveat its siblings carry
//!   (`GetObjectOutput$ChecksumCRC32`). The completion reports the value it computed over
//!   the assembled bytes with `ChecksumType` `FULL_OBJECT`, and every later read that
//!   enables the checksum mode reports the same pair — "to retrieve the checksum, this mode
//!   must be enabled" (`GetObjectRequest$ChecksumMode`).
//! * An annotation write computes the digest of the payload it stored under the algorithm
//!   the request named — an individual `Checksum*` member first, because "if you provide an
//!   individual checksum, Amazon S3 ignores any provided ChecksumAlgorithm parameter"
//!   (`PutObjectRequest$ChecksumAlgorithm`), else `ChecksumAlgorithm` itself — records it
//!   with the annotation, and reports it on the write, on a read that enabled the checksum
//!   mode (`GetObjectAnnotation` binds `ChecksumMode` as its own
//!   `requestValidationModeMember`) and as the `ChecksumAlgorithm` of the entry a listing
//!   reports for it.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_annotations::put_annotation;
use super::object_partial_reads::{multipart_object, seeded_payload};

/// Every `x-amz-checksum-*` response header the model binds, so a case that expects exactly
/// one of them can assert that none of the others is there.
const EVERY_CHECKSUM_HEADER: [&str; 10] = [
    "x-amz-checksum-crc32",
    "x-amz-checksum-crc32c",
    "x-amz-checksum-crc64nvme",
    "x-amz-checksum-sha1",
    "x-amz-checksum-sha256",
    "x-amz-checksum-sha512",
    "x-amz-checksum-md5",
    "x-amz-checksum-xxhash64",
    "x-amz-checksum-xxhash3",
    "x-amz-checksum-xxhash128",
];

/// The `ChecksumAlgorithm` value an object or an annotation stored under no algorithm of its
/// own carries, and the `Checksum*` member its value travels in
/// (`ChecksumCRC64NVME -> x-amz-checksum-crc64nvme`).
const DEFAULT_ALGORITHM: &str = "CRC64NVME";

/// The `ChecksumType` a digest over the whole of a payload carries, one of the two values
/// `ChecksumType` publishes (`[COMPOSITE, FULL_OBJECT]`).
const FULL_OBJECT: &str = "FULL_OBJECT";
/// See [`FULL_OBJECT`].
const COMPOSITE: &str = "COMPOSITE";

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The input the CRC-64/NVME algorithm publishes its CHECK VALUE over: the nine ASCII bytes
/// `123456789`, the check input every entry of the CRC catalogue (Greg Cook, *Catalogue of
/// parametrised CRC algorithms*) states its check value for.
const CRC64NVME_CHECK_INPUT: &[u8] = b"123456789";

/// The check value that catalogue publishes for `CRC-64/NVME` — the algorithm the NVM
/// Express specification defines and the one S3's `CRC64NVME` checksum is: the digest of
/// [`CRC64NVME_CHECK_INPUT`] is `0xAE8B14860A799888`. Written here as the published constant
/// so this suite pins the emulator's value against the ALGORITHM rather than against another
/// implementation of it.
const CRC64NVME_CHECK_VALUE: u64 = 0xAE8B_1486_0A79_9888;

/// The Base64 digest of `data` under the `ChecksumAlgorithm` value `algorithm`, as every
/// `Checksum*` member carries it: "the Base64 encoded, … checksum".
///
/// Computed through `aws-smithy-checksums`, the AWS SDK's own implementation of these
/// algorithms, over the bytes this suite wrote — so the expectation is a digest of the test's
/// own data rather than anything read back out of the emulator.
fn digest_of(algorithm: &str, data: &[u8]) -> String {
    let parsed: aws_smithy_checksums::ChecksumAlgorithm = algorithm
        .to_ascii_lowercase()
        .parse()
        .unwrap_or_else(|error| panic!("{algorithm} must be a checksum this crate computes: {error}"));
    let mut checksum = parsed.into_impl();
    checksum.update(data);
    aws_smithy_types::base64::encode(checksum.finalize())
}

/// The `x-amz-checksum-*` header one `ChecksumAlgorithm` value's digest travels on: every
/// `Checksum*` member of this model binds `x-amz-checksum-` followed by its algorithm in
/// lowercase.
fn header_of(algorithm: &str) -> String {
    format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase())
}

/// Assert that `answer` carries `expected` on the header `algorithm`'s value travels on, the
/// checksum type `checksum_type`, and NONE of the other nine checksum headers.
fn reports_only(answer: &Answer, case: &str, algorithm: &str, expected: &str, checksum_type: &str) {
    let header = header_of(algorithm);
    assert_eq!(
        answer.header(&header).as_deref(),
        Some(expected),
        "case {case:?}: expected {header} to report {expected:?}, got {:?}",
        answer.header(&header)
    );
    assert_eq!(
        answer.header("x-amz-checksum-type").as_deref(),
        Some(checksum_type),
        "case {case:?}: expected the checksum type {checksum_type:?}, got {:?}",
        answer.header("x-amz-checksum-type")
    );
    for other in EVERY_CHECKSUM_HEADER {
        if other == header {
            continue;
        }
        assert_eq!(
            answer.header(other),
            None,
            "case {case:?}: one payload carries the checksum of ONE algorithm, so {other} \
             must be absent, got {:?}",
            answer.header(other)
        );
    }
}

/// Assert that `answer` carries none of the checksum headers at all, nor a checksum type.
fn reports_no_checksum(answer: &Answer, case: &str) {
    for header in EVERY_CHECKSUM_HEADER {
        assert_eq!(
            answer.header(header),
            None,
            "case {case:?}: expected no checksum header, got {header} = {:?}",
            answer.header(header)
        );
    }
    assert_eq!(
        answer.header("x-amz-checksum-type"),
        None,
        "case {case:?}: expected no checksum type, got {:?}",
        answer.header("x-amz-checksum-type")
    );
}

/// `GET /<bucket>/<key>?annotation&annotationName=<name>&x-id=GetObjectAnnotation` with the
/// headers the case carries — `GetObjectAnnotation`, which is where `x-amz-checksum-mode`
/// travels.
async fn get_annotation_with(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    name: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = format!("/{bucket}/{key}?annotation&annotationName={name}&x-id=GetObjectAnnotation");
    send_bytes(router, "GET", &uri, headers, b"").await
}

/// `GET /<bucket>/<key>?annotation&x-id=ListObjectAnnotations` — `ListObjectAnnotations`.
async fn list_annotations(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    let uri = format!("/{bucket}/{key}?annotation&x-id=ListObjectAnnotations");
    send_bytes(router, "GET", &uri, &[], b"").await
}

/// The `<AnnotationEntry>` fragment of a listing body that names `annotation` — the one
/// entry a row's assertions are about, so the entries of the object's OTHER annotations are
/// never read as this one's.
fn entry_of(body: &str, annotation: &str) -> String {
    let named = format!("<AnnotationName>{annotation}</AnnotationName>");
    body.split("<AnnotationEntry>")
        .find(|fragment| fragment.contains(&named))
        .unwrap_or_else(|| {
            panic!("the listing must carry an entry for {annotation:?}, got body {body:?}")
        })
        .to_string()
}

/// A `HEAD` of the object, with the headers the case carries — `HeadObject`.
async fn head_object_with(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), headers, b"").await
}

#[tokio::test]
async fn an_upload_naming_no_algorithm_completes_into_a_default_full_object_checksum() {
    let router = router();
    let name = "default-object-checksum-bucket";
    bucket(&router, name).await;

    // The rows differ along what the rule generalises over: how many parts the object was
    // assembled from and how large they are — one part, the published check input of the
    // default algorithm, two parts at the published minimum, and three.
    let rows: Vec<(&str, Vec<Vec<u8>>)> = vec![
        (
            "one part",
            vec![b"the whole of this object arrived as one part".to_vec()],
        ),
        (
            "one part of the published check input",
            vec![CRC64NVME_CHECK_INPUT.to_vec()],
        ),
        (
            "two parts, the first at the published minimum",
            vec![
                seeded_payload(0x1234_5678, MIN_MULTIPART_PART_BYTES),
                b"and a tail".to_vec(),
            ],
        ),
        (
            "three parts",
            vec![
                seeded_payload(0x9abc_def0, MIN_MULTIPART_PART_BYTES),
                seeded_payload(0x0f0f_0f0f, MIN_MULTIPART_PART_BYTES),
                seeded_payload(0x5555_aaaa, 4_096),
            ],
        ),
    ];

    for (index, (case, parts)) in rows.iter().enumerate() {
        let key = format!("assembled/{index}.bin");
        // No initiation header at all: the upload names neither an algorithm nor a type.
        let completed = multipart_object(&router, name, &key, parts, &[], &[]).await;
        assert_eq!(
            completed.status,
            200,
            "case {case:?}: expected the completion to answer HTTP 200, got {} with body {:?}",
            completed.status,
            text(&completed)
        );

        let assembled = parts.concat();
        let expected = digest_of(DEFAULT_ALGORITHM, &assembled);
        assert_eq!(
            completed.element("ChecksumCRC64NVME").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the completion to report ChecksumCRC64NVME {expected:?}, \
             got {:?}",
            completed.element("ChecksumCRC64NVME")
        );
        assert_eq!(
            completed.element("ChecksumType").as_deref(),
            Some(FULL_OBJECT),
            "case {case:?}: a default checksum is over the whole object, got {:?}",
            completed.element("ChecksumType")
        );
        // A full-object value is the digest alone: the `-<parts>` suffix belongs to a
        // composition over the parts' checksums.
        assert!(
            !expected.contains('-'),
            "case {case:?}: expected no part-count suffix on {expected:?}"
        );
        for other in [
            "ChecksumCRC32",
            "ChecksumCRC32C",
            "ChecksumSHA1",
            "ChecksumSHA256",
            "ChecksumSHA512",
            "ChecksumMD5",
        ] {
            assert_eq!(
                completed.element(other),
                None,
                "case {case:?}: one object carries the checksum of ONE algorithm, so {other} \
                 must be absent, got {:?}",
                completed.element(other)
            );
        }

        // Every later read of the object reports the same pair — to a read that asked for it.
        let enabled = [("x-amz-checksum-mode", "ENABLED")];
        let read = get_object(&router, name, &key, "", &enabled).await;
        let headed = head_object_with(&router, name, &key, &enabled).await;
        for (reader, answer) in [("GetObject", read), ("HeadObject", headed)] {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected {reader} to answer HTTP 200, got {}",
                answer.status
            );
            reports_only(
                &answer,
                &format!("{case} read back by {reader}"),
                DEFAULT_ALGORITHM,
                &expected,
                FULL_OBJECT,
            );
        }
        // And nothing at all to a read that did not enable the mode.
        let unasked = get_object(&router, name, &key, "", &[]).await;
        reports_no_checksum(&unasked, &format!("{case} read without the checksum mode"));
    }

    // The published check value anchors the VALUE the default algorithm computes against the
    // algorithm's own definition rather than against another implementation of it.
    let anchored = get_object(
        &router,
        name,
        "assembled/1.bin",
        "",
        &[("x-amz-checksum-mode", "ENABLED")],
    )
    .await;
    let published = aws_smithy_types::base64::encode(CRC64NVME_CHECK_VALUE.to_be_bytes());
    assert_eq!(
        anchored.header("x-amz-checksum-crc64nvme").as_deref(),
        Some(published.as_str()),
        "case \"the published CRC-64/NVME check value\": expected {published:?}, got {:?}",
        anchored.header("x-amz-checksum-crc64nvme")
    );
}

#[tokio::test]
async fn an_upload_naming_an_algorithm_keeps_it_rather_than_the_default() {
    let router = router();
    let name = "named-object-checksum-bucket";
    bucket(&router, name).await;

    // The algorithm the INITIATION named is what the object carries, under the type that
    // algorithm settles — so the default never displaces a configured upload's checksum.
    let parts = vec![b"one part, under an algorithm the initiation named".to_vec()];
    for (case, algorithm, expected_type) in [
        ("a composition by default", "SHA256", COMPOSITE),
        ("a CRC composed over the parts", "CRC32", COMPOSITE),
    ] {
        let key = format!("named/{}.bin", algorithm.to_ascii_lowercase());
        let completed = multipart_object(
            &router,
            name,
            &key,
            &parts,
            &[("x-amz-checksum-algorithm", algorithm)],
            &[],
        )
        .await;
        assert_eq!(
            completed.status,
            200,
            "case {case:?}: expected the completion to answer HTTP 200, got {} with body {:?}",
            completed.status,
            text(&completed)
        );
        assert_eq!(
            completed.element("ChecksumType").as_deref(),
            Some(expected_type),
            "case {case:?}: expected the type {expected_type:?} the algorithm settles, got {:?}",
            completed.element("ChecksumType")
        );
        assert!(
            completed.element(&format!("Checksum{algorithm}")).is_some(),
            "case {case:?}: expected the completion to report Checksum{algorithm}, got body {:?}",
            text(&completed)
        );
        assert_eq!(
            completed.element("ChecksumCRC64NVME"),
            None,
            "case {case:?}: an upload that named an algorithm carries no default checksum \
             beside it, got {:?}",
            completed.element("ChecksumCRC64NVME")
        );
    }
}

/// How the row's request names the algorithm its checksum is computed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// The `x-amz-sdk-checksum-algorithm` header — `ChecksumAlgorithm`.
    TheAlgorithm,
    /// The individual `x-amz-checksum-<algorithm>` header, carrying the client's own digest
    /// of the same payload.
    ItsOwnDigest,
    /// Neither: the write names no checksum algorithm at all.
    Nothing,
}

#[tokio::test]
async fn an_annotation_write_reports_the_checksum_of_the_payload_it_stored() {
    let router = router();
    let name = "annotation-checksum-bucket";
    bucket(&router, name).await;
    let key = "annotated/report.txt";
    let written = put_object(&router, name, key, b"the object the annotations hang on", &[]).await;
    assert_eq!(
        written.status,
        200,
        "the annotated object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // The rows differ along what the rule generalises over: the algorithm, how the request
    // names it, and the payload's size and encoding.
    let long = "a line of annotation text\n".repeat(2_048);
    let rows: Vec<(&str, Names, &str, &[u8])> = vec![
        (
            "a CRC named by the algorithm header",
            Names::TheAlgorithm,
            "CRC32",
            b"the annotation payload",
        ),
        (
            "a CRC named by the client's own digest",
            Names::ItsOwnDigest,
            "CRC32",
            b"the annotation payload",
        ),
        (
            "the castagnoli CRC over one byte",
            Names::TheAlgorithm,
            "CRC32C",
            b"x",
        ),
        (
            "the default algorithm, asked for by name",
            Names::TheAlgorithm,
            "CRC64NVME",
            b"a payload checksummed under the default algorithm",
        ),
        (
            "a SHA over multi-byte UTF-8 text",
            Names::TheAlgorithm,
            "SHA1",
            "annotation: \u{3b1}\u{3bd}\u{3b8}\u{3c1}\u{3c9}\u{3c0}\u{3bf}\u{3c2} \u{1f3af}"
                .as_bytes(),
        ),
        (
            "a SHA over a long payload, named by the client's own digest",
            Names::ItsOwnDigest,
            "SHA256",
            long.as_bytes(),
        ),
        (
            "a write that names no algorithm at all",
            Names::Nothing,
            "CRC32",
            b"an unchecksummed annotation payload",
        ),
    ];

    for (index, (case, names, algorithm, payload)) in rows.iter().enumerate() {
        let annotation = format!("note-{index}");
        let expected = digest_of(algorithm, payload);
        let header = header_of(algorithm);
        let headers: Vec<(&str, &str)> = match names {
            Names::TheAlgorithm => vec![("x-amz-sdk-checksum-algorithm", algorithm)],
            Names::ItsOwnDigest => vec![(header.as_str(), expected.as_str())],
            Names::Nothing => Vec::new(),
        };

        let put = put_annotation(&router, name, key, &annotation, payload, &headers).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the annotation write to answer HTTP 200, got {} with body \
             {:?}",
            put.status,
            text(&put)
        );

        let enabled = [("x-amz-checksum-mode", "ENABLED")];
        let read = get_annotation_with(&router, name, key, &annotation, &enabled).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the annotation read to answer HTTP 200, got {} with body \
             {:?}",
            read.status,
            text(&read)
        );
        let listed = list_annotations(&router, name, key).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected the listing to answer HTTP 200, got {} with body {:?}",
            listed.status,
            text(&listed)
        );

        if *names == Names::Nothing {
            reports_no_checksum(&put, case);
            reports_no_checksum(&read, &format!("{case} read back"));
            let entry = entry_of(&text(&listed), &annotation);
            assert!(
                !entry.contains("<ChecksumAlgorithm>"),
                "case {case:?}: an annotation recorded under no algorithm is listed without \
                 one, got entry {entry:?}"
            );
            continue;
        }

        reports_only(&put, case, algorithm, &expected, FULL_OBJECT);
        reports_only(
            &read,
            &format!("{case} read back"),
            algorithm,
            &expected,
            FULL_OBJECT,
        );
        // The listing names the algorithm whose member carries that value, read off the same
        // stored record.
        let entry = entry_of(&text(&listed), &annotation);
        let named = format!("<ChecksumAlgorithm>{algorithm}</ChecksumAlgorithm>");
        assert!(
            entry.contains(&named),
            "case {case:?}: expected the listed entry to name {algorithm:?}, got {entry:?}"
        );
        // And a read that did not enable the mode reports none of it, "to retrieve the
        // checksum, this mode must be enabled".
        let unasked = get_annotation_with(&router, name, key, &annotation, &[]).await;
        reports_no_checksum(&unasked, &format!("{case} read without the checksum mode"));
    }

    // Over the whole listing: one `ChecksumAlgorithm` per annotation whose write named an
    // algorithm, and none for the one that named none — the listing reports what each write
    // recorded rather than one answer for the object.
    let listed = list_annotations(&router, name, key).await;
    let body = text(&listed);
    let checksummed = rows
        .iter()
        .filter(|(_, names, _, _)| *names != Names::Nothing)
        .count();
    assert_eq!(
        body.matches("<ChecksumAlgorithm>").count(),
        checksummed,
        "case \"the whole listing\": expected one ChecksumAlgorithm per checksummed \
         annotation ({checksummed}), got body {body:?}"
    );
}
