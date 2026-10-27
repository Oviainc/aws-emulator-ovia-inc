// SPDX-License-Identifier: CC0-1.0

//! The integrity value a write DECLARES is judged before anything is stored.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle, object and annotation suites in this crate already own), so what
//! is pinned is the wire behaviour — status, error code, `<Message>`, and whether the key
//! exists afterwards — rather than any handler's internals.
//!
//! The rule under test: a `Content-MD5` or `x-amz-checksum-*` value on a write is checked
//! for its SHAPE first — a value that does not Base64-decode to the digest width the named
//! algorithm publishes cannot be a digest of anything, so it is refused before any
//! comparison — and only then against the digest of the payload actually received. The two
//! refusals are different answers because a client branches on them differently:
//!
//! * a malformed `x-amz-checksum-*` value is `InvalidRequest` (400), "Value for
//!   `x-amz-checksum-<algorithm>` header is invalid." — fix the client;
//! * a malformed `Content-MD5` is `InvalidDigest` (400), the code Amazon S3's *Error
//!   responses* list publishes for that header;
//! * a well-formed value that is not the digest of the bytes is `BadDigest` (400), naming
//!   the algorithm — recompute the digest and retry the same bytes.
//!
//! Nothing is stored by any of them, and a write whose declared value IS the digest is
//! accepted, with the checksum reported back and re-reported by a later read that enables
//! the checksum mode ("to retrieve the checksum, this mode must be enabled",
//! `GetObjectRequest$ChecksumMode`).

use md5::{Digest as _, Md5};
use service_com_amazonaws_s3_amazons3::state::{
    checksum_header_name, content_md5, full_object_checksum, CONTENT_MD5_MISMATCH_MESSAGE,
    INVALID_DIGEST_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;
use super::bucket_lifecycle::router;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_annotations::{get_annotation, put_annotation};

/// Every value the model's `ChecksumAlgorithm` enum publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumAlgorithm",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports exactly these ten), each of which binds its own `x-amz-checksum-*` request
/// header. The table below spans all of them, including the ones the issue that prompted
/// this suite never named.
const CHECKSUM_ALGORITHMS: [&str; 10] = [
    "CRC32",
    "CRC32C",
    "CRC64NVME",
    "SHA1",
    "SHA256",
    "SHA512",
    "MD5",
    "XXHASH64",
    "XXHASH3",
    "XXHASH128",
];

/// The payloads every integrity rule is exercised over: the rows differ along the SIZE and
/// the byte range of the body whose digest is declared — an empty body, one byte, a short
/// text, and a kilobyte spanning the whole byte range.
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("the empty payload", Vec::new()),
        ("a single byte", vec![0x00]),
        ("a short text", b"the quick brown fox".to_vec()),
        (
            "a kilobyte spanning every byte value",
            (0..1024u32).map(|index| (index % 256) as u8).collect(),
        ),
    ]
}

/// The Base64 digest of `data` under `algorithm`, as the header carries it.
///
/// Computed through the store's own helper, which is what the handler computes with — and
/// which [`the_digest_helper_matches_the_published_vectors`] pins against digests published
/// outside this process, so the table below rests on an outside expectation rather than on
/// the helper agreeing with itself.
fn digest(algorithm: &str, data: &[u8]) -> String {
    full_object_checksum(algorithm, data)
        .unwrap_or_else(|| panic!("the store must compute a {algorithm} checksum"))
}

/// The same digest with its LAST BYTE dropped: still Base64, still the right alphabet, and
/// a width no digest of this algorithm has.
fn digest_one_byte_short(algorithm: &str, data: &[u8]) -> String {
    let decoded = ::aws_smithy_types::base64::decode(digest(algorithm, data))
        .expect("the store's own checksum must be Base64");
    ::aws_smithy_types::base64::encode(&decoded[..decoded.len() - 1])
}

/// Whether the key exists at all, as a client sees it: a read of it answers 404 `NoSuchKey`
/// when the refused write stored nothing.
async fn key_is_absent(router: &MultiServiceRouter, bucket_name: &str, key: &str) -> bool {
    let read = get_object(router, bucket_name, key, "", &[]).await;
    read.status == 404 && read.error_code() == "NoSuchKey"
}

/// Assert one refused write: the status, the wire code, the `<Message>`, and that the key it
/// named holds nothing afterwards.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    bucket_name: &str,
    key: &str,
    refused: &Answer,
    code: &str,
    message: &str,
) {
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        refused.status,
        text(refused)
    );
    assert_eq!(
        refused.error_code(),
        code,
        "case {case:?}: expected the {code} code, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.error_message(),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        refused.error_message()
    );
    assert!(
        key_is_absent(router, bucket_name, key).await,
        "case {case:?}: a refused write must store nothing, but {key:?} reads back"
    );
}

/// RULE: the digests the table declares are the ones the published test vectors state, so
/// everything the suite then asserts rests on a value computed outside this process.
///
/// The vectors: MD5 of `"abc"` from RFC 1321 Appendix A.5, and SHA-256 of `"abc"` from the
/// NIST FIPS 180-4 example (also the first vector of the SHA-2 test suite).
#[test]
fn the_digest_helper_matches_the_published_vectors() {
    let rows: [(&str, &str, &[u8], &str); 3] = [
        (
            "MD5 of \"abc\" (RFC 1321 A.5)",
            "MD5",
            b"abc",
            "900150983cd24fb0d6963f7d28e17f72",
        ),
        (
            "SHA256 of \"abc\" (FIPS 180-4)",
            "SHA256",
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "MD5 of the empty string (RFC 1321 A.5)",
            "MD5",
            b"",
            "d41d8cd98f00b204e9800998ecf8427e",
        ),
    ];
    for (case, algorithm, data, hex_digest) in rows {
        let expected = ::aws_smithy_types::base64::encode(
            hex::decode(hex_digest).expect("the published vector must be hex"),
        );
        let computed = digest(algorithm, data);
        assert_eq!(
            computed, expected,
            "case {case:?}: expected {expected:?}, got {computed:?}"
        );
    }
    // `Content-MD5` carries that same MD5 digest, Base64 encoded, so the helper the
    // Content-MD5 rows declare their value with is pinned to the same vector.
    assert_eq!(
        content_md5(b"abc"),
        digest("MD5", b"abc"),
        "case \"Content-MD5 of \\\"abc\\\"\": the header's digest must be the MD5 one, got {:?}",
        content_md5(b"abc")
    );
}

/// RULE: an `x-amz-checksum-*` value is judged for shape, then against the bytes — for
/// every algorithm the model publishes and every payload size, with nothing stored by a
/// refused write.
#[tokio::test]
async fn a_declared_checksum_is_judged_for_shape_and_then_against_the_bytes() {
    let router = router();
    let bucket_name = "integrity-checksum-bucket";
    bucket(&router, bucket_name).await;

    for algorithm in CHECKSUM_ALGORITHMS {
        let header = checksum_header_name(algorithm);
        for (payload_case, payload) in payloads() {
            // The ACCEPTED write: the declared value IS the digest of the bytes, so the
            // object is stored and the checksum is reported back.
            let key = format!("checked/{algorithm}/{}.bin", payload.len());
            let accepted = put_object(
                &router,
                bucket_name,
                &key,
                &payload,
                &[(header.as_str(), digest(algorithm, &payload).as_str())],
            )
            .await;
            let case = format!("{algorithm} over {payload_case}: the true digest");
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: expected the write to be accepted, got {} with body {:?}",
                accepted.status,
                text(&accepted)
            );
            assert_eq!(
                accepted.header(&header),
                Some(digest(algorithm, &payload)),
                "case {case:?}: expected the write to report the checksum it computed, got {:?}",
                accepted.header(&header)
            );
            // And every later read that asks for it reports the same value.
            let read = get_object(
                &router,
                bucket_name,
                &key,
                "",
                &[("x-amz-checksum-mode", "ENABLED")],
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the read to answer HTTP 200, got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.header(&header),
                Some(digest(algorithm, &payload)),
                "case {case:?}: expected the read to report the stored checksum, got {:?}",
                read.header(&header)
            );

            // The SHAPE refusals: three values that cannot be a digest of this algorithm —
            // one three bytes wide whatever the algorithm, one of the right alphabet and
            // the wrong width, and one that is not Base64 at all.
            let malformed: [(&str, String); 3] = [
                ("a value three bytes wide", "YWJj".to_string()),
                (
                    "the true digest one byte short",
                    digest_one_byte_short(algorithm, &payload),
                ),
                ("a value that is not Base64", "not~base64~at~all".to_string()),
            ];
            for (malformed_case, value) in malformed {
                let key = format!(
                    "malformed/{algorithm}/{}/{}.bin",
                    payload.len(),
                    malformed_case.replace(' ', "-")
                );
                let refused = put_object(
                    &router,
                    bucket_name,
                    &key,
                    &payload,
                    &[(header.as_str(), value.as_str())],
                )
                .await;
                assert_refused(
                    &router,
                    &format!("{algorithm} over {payload_case}: {malformed_case}"),
                    bucket_name,
                    &key,
                    &refused,
                    "InvalidRequest",
                    &format!("Value for {header} header is invalid."),
                )
                .await;
            }

            // The COMPARISON refusal: a well-formed digest of this very algorithm, over
            // other bytes, is `BadDigest` naming the algorithm.
            let key = format!("mismatched/{algorithm}/{}.bin", payload.len());
            let other = [payload.as_slice(), b"and one more thing"].concat();
            let refused = put_object(
                &router,
                bucket_name,
                &key,
                &payload,
                &[(header.as_str(), digest(algorithm, &other).as_str())],
            )
            .await;
            assert_refused(
                &router,
                &format!("{algorithm} over {payload_case}: the digest of other bytes"),
                bucket_name,
                &key,
                &refused,
                "BadDigest",
                &format!("The {algorithm} you specified did not match the calculated checksum."),
            )
            .await;
        }
    }
}

/// RULE: `Content-MD5` is judged by the same two steps, with the codes and sentences AWS
/// publishes for that header — on `PutObject` and on `PutObjectAnnotation`, the two writes
/// whose payload the header describes.
#[tokio::test]
async fn content_md5_is_judged_for_shape_and_then_against_the_bytes() {
    let router = router();
    let bucket_name = "integrity-content-md5-bucket";
    bucket(&router, bucket_name).await;

    for (payload_case, payload) in payloads() {
        // The ACCEPTED write, and the object it stored read back byte for byte.
        let key = format!("checked/{}.bin", payload.len());
        let accepted = put_object(
            &router,
            bucket_name,
            &key,
            &payload,
            &[("content-md5", content_md5(&payload).as_str())],
        )
        .await;
        let case = format!("Content-MD5 over {payload_case}: the true digest");
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the write to be accepted, got {} with body {:?}",
            accepted.status,
            text(&accepted)
        );
        let read = get_object(&router, bucket_name, &key, "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            payload.as_slice(),
            "case {case:?}: expected the stored bytes back, got {} bytes",
            read.body.len()
        );

        // The two refusal classes, each with the code and the fixed sentence AWS publishes
        // for this header.
        let rows: [(&str, String, &str, String); 4] = [
            (
                "the digest of other bytes",
                content_md5(&[payload.as_slice(), b"extra"].concat()),
                "BadDigest",
                CONTENT_MD5_MISMATCH_MESSAGE.to_string(),
            ),
            (
                "the digest of the empty payload where the body is not empty",
                content_md5(b"not-the-body"),
                "BadDigest",
                CONTENT_MD5_MISMATCH_MESSAGE.to_string(),
            ),
            (
                "a value that is not Base64",
                "not-base64-at-all".to_string(),
                "InvalidDigest",
                INVALID_DIGEST_MESSAGE.to_string(),
            ),
            (
                "a Base64 value that is not 128 bits wide",
                ::aws_smithy_types::base64::encode(&Md5::digest(&payload)[..15]),
                "InvalidDigest",
                INVALID_DIGEST_MESSAGE.to_string(),
            ),
        ];
        for (refusal_case, value, code, message) in rows {
            let case = format!("Content-MD5 over {payload_case}: {refusal_case}");
            let key = format!(
                "refused/{}/{}.bin",
                payload.len(),
                refusal_case.replace(' ', "-")
            );
            let refused = put_object(
                &router,
                bucket_name,
                &key,
                &payload,
                &[("content-md5", value.as_str())],
            )
            .await;
            assert_refused(
                &router, &case, bucket_name, &key, &refused, code, &message,
            )
            .await;

            // The SAME header on the other write that carries one: an annotation's payload
            // is judged by the same rule, so the annotation is not recorded either.
            let annotated = format!("annotated/{}.bin", payload.len());
            let stored = put_object(&router, bucket_name, &annotated, b"parent", &[]).await;
            assert_eq!(
                stored.status, 200,
                "case {case:?}: the annotated object must be stored first, got {}",
                stored.status
            );
            let refused = put_annotation(
                &router,
                bucket_name,
                &annotated,
                "note",
                &payload,
                &[("content-md5", value.as_str())],
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected PutObjectAnnotation to refuse with HTTP 400, got {} \
                 with body {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected PutObjectAnnotation to answer {code}, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}: expected PutObjectAnnotation to answer {message:?}, got {:?}",
                refused.error_message()
            );
            let absent = get_annotation(&router, bucket_name, &annotated, "note").await;
            assert_eq!(
                absent.error_code(),
                "NoSuchAnnotation",
                "case {case:?}: a refused annotation write must record nothing, got {:?}",
                absent.error_code()
            );
        }
    }
}
