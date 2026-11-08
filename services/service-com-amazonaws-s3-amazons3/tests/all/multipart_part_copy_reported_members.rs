// SPDX-License-Identifier: CC0-1.0

//! What a part recorded by `UploadPartCopy` REPORTS beside its entity tag: the per-part
//! digest, and the encryption of the upload it was recorded into.
//!
//! Every assertion drives a real HTTP request through the router (the same harness the
//! multipart suites in this crate own), so what is pinned is the wire behaviour — the
//! `CopyPartResult` elements, the bound response headers and what a later `ListParts`
//! reports for the part.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A copy records a part of the bytes it TOOK from the source, so the digest it reports
//!    is the digest of exactly those bytes under the algorithm the upload's initiation
//!    settled — "this checksum is present if the multipart upload request was created with
//!    the … checksum algorithm" (`CopyPartResult`'s `Checksum*` members) — carried in the
//!    `CopyPartResult` member that algorithm names and in no other, reported identically by
//!    the later read of the part, and absent altogether from an upload initiated under no
//!    algorithm. The digests travel in the payload: this output binds no `Checksum*` header
//!    (`smithy_query(query="operation",operation="UploadPartCopy")`).
//! 2. The encryption a copy reports is the encryption of the UPLOAD the part was recorded
//!    into, settled when that upload was initiated, whatever encryption the source object
//!    it copied bytes from was stored under.
//!
//! Every expected digest is DERIVED here from the copied bytes through the hash crate that
//! implements the algorithm (the one derivation this crate's part-checksum suite owns), so
//! no assertion reads a value back out of the code under test.

use service_com_amazonaws_s3_amazons3::state::MIN_MULTIPART_PART_BYTES;

use super::bucket_lifecycle::router;
use super::bucket_tagging::unescaped;
use super::multipart_part_checksums::{
    digest, initiate, list_parts, part_element, ALGORITHMS, CHECKSUM_HEADERS,
};
use super::multipart_part_copy::upload_part_copy;
use super::object_and_tagging_roundtrip::{bucket, expected_etag, put_object, text};
use super::object_partial_reads::seeded_payload;

use super::object_encryption::KEY_ARN;

/// The bound headers an upload's encryption is reported in. This output binds three of the
/// four an object's encryption travels in: it carries no `SSEKMSEncryptionContext` member
/// (`smithy_query(query="operation",operation="UploadPartCopy")`).
const ENCRYPTION_HEADERS: [&str; 3] = [
    "x-amz-server-side-encryption",
    "x-amz-server-side-encryption-aws-kms-key-id",
    "x-amz-server-side-encryption-bucket-key-enabled",
];

/// Which bytes of a source a row's copy takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taken {
    /// The whole object at the small source key, which is what a copy naming no range takes.
    Whole,
    /// The inclusive byte range `(first, last)` of the large source key — the only kind of
    /// source a ranged copy is served from.
    Range(usize, usize),
}

/// One row of the digest table: the case name, the algorithm the upload's initiation settles
/// (`None` for an initiation that names none) and which source bytes the copy takes.
struct DigestRow {
    case: String,
    algorithm: Option<&'static str>,
    taken: Taken,
}

/// The table: every value `ChecksumAlgorithm` publishes, each taken both as a whole object
/// and as a byte range of another source, plus the two rows whose upload settles no
/// algorithm at all.
fn digest_rows() -> Vec<DigestRow> {
    let mut rows = Vec::new();
    for algorithm in ALGORITHMS {
        for taken in [Taken::Whole, Taken::Range(1_024, 2_047)] {
            rows.push(DigestRow {
                case: format!("{algorithm} over {taken:?}"),
                algorithm: Some(algorithm),
                taken,
            });
        }
    }
    for taken in [Taken::Whole, Taken::Range(0, 9)] {
        rows.push(DigestRow {
            case: format!("no algorithm at all, over {taken:?}"),
            algorithm: None,
            taken,
        });
    }
    rows
}

#[tokio::test]
async fn a_copied_part_reports_the_digest_of_the_bytes_it_took() {
    let router = router();
    let origin = "part-copy-digest-origin";
    let target = "part-copy-digest-target";
    bucket(&router, origin).await;
    bucket(&router, target).await;

    // Two sources: a small object a whole-object copy takes entire, and one past the
    // published minimum part size, which is the only kind a RANGED copy may be taken from.
    let small_key = "sources/small.bin";
    let large_key = "sources/large.bin";
    let small = seeded_payload(0x2024_1029, 777);
    let large = seeded_payload(0x0102_0304, MIN_MULTIPART_PART_BYTES + 1);
    for (key, body) in [(small_key, &small), (large_key, &large)] {
        let stored = put_object(&router, origin, key, body, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "the fixture write of {key:?} must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }

    for (index, row) in digest_rows().iter().enumerate() {
        let DigestRow {
            case,
            algorithm,
            taken,
        } = row;
        let case = case.as_str();
        let part_number = i32::try_from(index + 1).expect("the table is far shorter than 10,000");
        let key = format!("copies/{index}.bin");

        // The upload the part is recorded into, initiated with exactly the algorithm the row
        // names — the only thing that settles what a copied part's digest is computed under.
        let initiation: Vec<(&str, &str)> = algorithm
            .map(|algorithm| vec![("x-amz-checksum-algorithm", algorithm)])
            .unwrap_or_default();
        let upload_id = initiate(&router, target, &key, &initiation).await;

        let (source_key, expected_bytes, range) = match *taken {
            Taken::Whole => (small_key, small.clone(), None),
            Taken::Range(first, last) => (
                large_key,
                large[first..=last].to_vec(),
                Some(format!("bytes={first}-{last}")),
            ),
        };
        let headers: Vec<(&str, &str)> = match range.as_deref() {
            None => Vec::new(),
            Some(range) => vec![("x-amz-copy-source-range", range)],
        };
        let copied = upload_part_copy(
            &router,
            target,
            &key,
            &upload_id,
            part_number,
            &format!("{origin}/{source_key}"),
            &headers,
        )
        .await;

        assert_eq!(
            copied.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        // The part is of the bytes the copy took, which its entity tag says: the digest
        // asserted below is of the same bytes.
        let etag = expected_etag(&expected_bytes);
        assert_eq!(
            copied.element("ETag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected the tag of the copied bytes {etag:?}, got {:?}",
            copied.element("ETag")
        );
        // The source bucket keeps no versions, so the copied-from version has no id to
        // report: "the version of the source object that was copied, if you have enabled
        // versioning on the source bucket".
        assert_eq!(
            copied.header("x-amz-copy-source-version-id"),
            None,
            "case {case:?}: an unversioned source has no version id to report, got {:?}",
            copied.header("x-amz-copy-source-version-id")
        );

        // The digests travel in the payload container, never in a bound header: this output
        // binds no `Checksum*` header at all.
        for header in CHECKSUM_HEADERS {
            let carried = copied.header(header);
            assert_eq!(
                carried, None,
                "case {case:?}: this output binds no checksum header, {header} carried \
                 {carried:?}"
            );
        }

        // The read-back of the part, which has to report the same per-part value the copy
        // answered: "the Base64 encoded, … checksum of the part" (`Part`'s own members).
        let listed = list_parts(&router, target, &key, &upload_id).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: the upload must be readable, got {} with body {:?}",
            listed.status,
            text(&listed)
        );

        for candidate in ALGORITHMS {
            let element = format!("Checksum{candidate}");
            let expected = algorithm
                .filter(|settled| *settled == candidate)
                .map(|settled| digest(settled, &expected_bytes));
            let answered = copied.element(&element);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the CopyPartResult {element} to be {expected:?}, \
                 got {answered:?} in body {:?}",
                text(&copied)
            );
            let reported = part_element(&listed, part_number, &element);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the recorded part's {element} to be {expected:?}, \
                 got {reported:?} in body {:?}",
                text(&listed)
            );
            // The digest is of the PART's own bytes, so for a ranged copy it cannot be the
            // digest of the whole source object the range was taken out of.
            if let (Some(settled), Taken::Range(..)) = (algorithm, taken) {
                if *settled == candidate {
                    let whole = digest(settled, &large);
                    assert_ne!(
                        answered.as_deref(),
                        Some(whole.as_str()),
                        "case {case:?}: the digest must be of the copied range, not of the \
                         whole source object"
                    );
                }
            }
            // Only the member the settled algorithm names carries a value: the assertion
            // above holds the others to `None`, which is exactly their absence from the
            // container.
        }
    }
}

/// One row of the encryption table: the case name, the encryption headers the INITIATION
/// carries, and the bound headers the copy's answer must report.
struct EncryptionRow {
    case: &'static str,
    initiation: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, Option<&'static str>)>,
}

#[tokio::test]
async fn a_copied_part_reports_the_encryption_of_the_upload_it_was_recorded_into() {
    let router = router();
    let origin = "part-copy-encryption-origin";
    let target = "part-copy-encryption-target";
    bucket(&router, origin).await;
    bucket(&router, target).await;

    // The source is stored under the destination bucket's own default encryption, which is
    // what makes every row below a statement about the UPLOAD rather than about the source.
    let source_key = "sources/plain.bin";
    let payload = b"copy-source-bytes".to_vec();
    let stored = put_object(&router, origin, source_key, &payload, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the fixture write must succeed, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // The rows differ along what the rule generalises over: whether the initiation named an
    // algorithm at all, WHICH algorithm it named, whether it named a KMS key, and whether it
    // switched the Bucket Key on or off.
    let rows: Vec<EncryptionRow> = vec![
        EncryptionRow {
            case: "an initiation that named no encryption member",
            initiation: Vec::new(),
            expected: vec![
                // "Amazon S3 encrypts data with server-side encryption by using Amazon S3
                // managed keys (SSE-S3) by default" (`PutObjectRequest$ServerSideEncryption`),
                // which is the AES256 value of the `ServerSideEncryption` enum.
                ("x-amz-server-side-encryption", Some("AES256")),
                ("x-amz-server-side-encryption-aws-kms-key-id", None),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        },
        EncryptionRow {
            case: "an upload initiated under SSE-S3",
            initiation: vec![("x-amz-server-side-encryption", "AES256")],
            expected: vec![
                ("x-amz-server-side-encryption", Some("AES256")),
                ("x-amz-server-side-encryption-aws-kms-key-id", None),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        },
        EncryptionRow {
            case: "an upload initiated under SSE-KMS with the key named by ARN",
            initiation: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", KEY_ARN),
            ],
            expected: vec![
                ("x-amz-server-side-encryption", Some("aws:kms")),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    Some(KEY_ARN),
                ),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        },
        EncryptionRow {
            case: "an upload initiated under SSE-KMS with a Bucket Key switched on",
            initiation: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", KEY_ARN),
                ("x-amz-server-side-encryption-bucket-key-enabled", "true"),
            ],
            expected: vec![
                ("x-amz-server-side-encryption", Some("aws:kms")),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    Some(KEY_ARN),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    Some("true"),
                ),
            ],
        },
        EncryptionRow {
            case: "an upload initiated under dual-layer SSE-KMS",
            initiation: vec![
                ("x-amz-server-side-encryption", "aws:kms:dsse"),
                ("x-amz-server-side-encryption-aws-kms-key-id", KEY_ARN),
            ],
            expected: vec![
                ("x-amz-server-side-encryption", Some("aws:kms:dsse")),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    Some(KEY_ARN),
                ),
                ("x-amz-server-side-encryption-bucket-key-enabled", None),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = format!("copies/encrypted-{index}.bin");
        let upload_id = initiate(&router, target, &key, &row.initiation).await;
        let copied = upload_part_copy(
            &router,
            target,
            &key,
            &upload_id,
            1,
            &format!("{origin}/{source_key}"),
            &[],
        )
        .await;

        assert_eq!(
            copied.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        for (header, value) in &row.expected {
            let answered = copied.header(header);
            assert_eq!(
                answered.as_deref(),
                *value,
                "case {case:?}: expected {header} = {value:?}, got {answered:?}"
            );
        }
        // Every row pins every header an upload's encryption is reported in, so no row can
        // pass by leaving one of them out of its own expectations.
        for header in ENCRYPTION_HEADERS {
            assert!(
                row.expected.iter().any(|(named, _)| *named == header),
                "case {case:?}: the row must state what {header} reports, it names only {:?}",
                row.expected
            );
        }
        // Nothing about the part itself changed: the copy still recorded the source's bytes
        // under the part number it named, whatever the upload's encryption.
        let listed = list_parts(&router, target, &key, &upload_id).await;
        // The listing carries the tag with its quotes XML-escaped, as every element of a
        // served document is, so the comparison is made on the unescaped text.
        let reported = part_element(&listed, 1, "ETag").map(|value| unescaped(&value));
        let etag = expected_etag(&payload);
        assert_eq!(
            reported.as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected the recorded part to carry {etag:?}, got {reported:?} \
             in body {:?}",
            text(&listed)
        );
    }
}
