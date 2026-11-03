// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PER-PART digest of a multipart upload: what `UploadPart`
//! computes over the bytes it received, what it refuses, and what every later reader of the
//! part reports.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rule under test, asserted over a SET of inputs rather than one request: a write that
//! records bytes under a checksum algorithm computes the digest of THOSE bytes under that
//! algorithm, reports it in the member the algorithm names, and refuses a declared digest
//! that is not that value with 400 `BadDigest` while recording nothing. For a part the
//! bytes are the part's own — "the Base64 encoded, … checksum of the part"
//! (`UploadPartOutput`'s `Checksum*` members and `Part`'s) — never the object the parts
//! assemble into, whose own digest the completion computes under the type the initiation
//! settled.
//!
//! The rows differ along:
//!
//! * the ALGORITHM: every value `ChecksumAlgorithm` publishes
//!   (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumAlgorithm")` reports
//!   `[CRC32, CRC32C, SHA1, SHA256, CRC64NVME, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]`),
//!   including the values the recorded cases never named;
//! * HOW the algorithm is named: by the initiation alone, by this request's
//!   `x-amz-sdk-checksum-algorithm`, or by the individual `x-amz-checksum-<algorithm>`
//!   header carrying the client's own digest;
//! * the BYTES: a different body per row, and parts of different sizes within one upload,
//!   so a per-part digest cannot be confused with another part's or with the object's.
//!
//! Every expected digest is DERIVED in this suite from the part's bytes through the hash
//! crate that implements the algorithm, so the assertions do not read a value back out of
//! the code under test.

use aws_smithy_types::base64;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::multipart_upload_completion::completion_payload;
use super::object_and_tagging_roundtrip::{bucket, text};

/// "Part size: 5 MiB to 5 GiB. There is no minimum size limit on the last part of your
/// multipart upload" (Amazon S3 User Guide, *Amazon S3 multipart upload limits*) — the size
/// every part but the last of a completed upload has to reach.
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The code S3 answers a declared digest that is not the digest of the bytes it received:
/// `BadDigest`, HTTP 400 (Amazon S3 *Error responses*, list of error codes).
const BAD_DIGEST: &str = "BadDigest";

/// Every `x-amz-checksum-*` response header the model binds, so a row that expects exactly
/// ONE of them can assert that no other carries a value.
const CHECKSUM_HEADERS: [&str; 10] = [
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

/// Every value `ChecksumAlgorithm` publishes, which is the whole space a part's digest can
/// be computed under.
const ALGORITHMS: [&str; 10] = [
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

/// The Base64 digest of `data` under `algorithm`, derived here rather than read back from
/// the service: the five checksums the AWS SDK implements come from `aws-smithy-checksums`,
/// the SHA-2 digests from `sha2`, `MD5` from `md-5` and the three `XXHASH` values from
/// `xxhash-rust`, each big-endian for an integer-valued digest, which is what a Base64
/// checksum value carries.
fn digest(algorithm: &str, data: &[u8]) -> String {
    let bytes: Vec<u8> = match algorithm {
        "CRC32" | "CRC32C" | "CRC64NVME" | "SHA1" => {
            let parsed: aws_smithy_checksums::ChecksumAlgorithm = algorithm
                .to_ascii_lowercase()
                .parse()
                .expect("the SDK implements this algorithm");
            let mut checksum = parsed.into_impl();
            checksum.update(data);
            checksum.finalize().to_vec()
        }
        "SHA256" => {
            use sha2::Digest as _;
            sha2::Sha256::digest(data).to_vec()
        }
        "SHA512" => {
            use sha2::Digest as _;
            sha2::Sha512::digest(data).to_vec()
        }
        "MD5" => {
            use md5::Digest as _;
            md5::Md5::digest(data).to_vec()
        }
        "XXHASH64" => xxhash_rust::xxh64::xxh64(data, 0).to_be_bytes().to_vec(),
        "XXHASH3" => xxhash_rust::xxh3::xxh3_64(data).to_be_bytes().to_vec(),
        "XXHASH128" => xxhash_rust::xxh3::xxh3_128(data).to_be_bytes().to_vec(),
        other => panic!("case {other:?}: the suite names an algorithm it cannot derive"),
    };
    base64::encode(bytes)
}

/// The header an algorithm's digest travels on, derived the way every `Checksum*` member of
/// this model binds one: `x-amz-checksum-` followed by the algorithm in lowercase.
fn checksum_header(algorithm: &str) -> String {
    format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase())
}

/// `len` bytes that depend on `seed`, so each row's part carries its own bytes and its own
/// digest.
fn filler(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, carrying the checksum headers
/// a row names.
async fn initiate(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> String {
    let initiated = send(router, "POST", &format!("/{name}/{key}?uploads"), headers, "").await;
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

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`.
async fn upload_part(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{name}/{key}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPart"),
        headers,
        body,
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`, the read-back of what
/// the part writes recorded.
async fn list_parts(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}/{key}?uploadId={upload_id}&x-id=ListParts"),
        &[],
        "",
    )
    .await
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`, with the parts list as
/// the payload and whatever checksum headers the row carries.
async fn complete(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{name}/{key}?uploadId={upload_id}"),
        headers,
        &completion_payload(parts),
    )
    .await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`, which is what shows whether a refused completion
/// stored an object after all.
async fn head_object(router: &MultiServiceRouter, name: &str, key: &str) -> Answer {
    send(router, "HEAD", &format!("/{name}/{key}"), &[], "").await
}

/// `GET /<bucket>/<key>?attributes` — `GetObjectAttributes`, asking for the object's
/// checksum container and the parts it was assembled from.
async fn object_attributes(router: &MultiServiceRouter, name: &str, key: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}/{key}?attributes"),
        &[("x-amz-object-attributes", "Checksum, ObjectParts, ObjectSize")],
        "",
    )
    .await
}

/// The text of the `<element>` inside the `<Part>` that reports `part_number`, or `None`
/// when that part reports no such element.
fn part_element(answer: &Answer, part_number: i32, element: &str) -> Option<String> {
    let body = text(answer);
    body.split("<Part>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</Part>").next())
        .find(|chunk| {
            chunk.contains(&format!("<PartNumber>{part_number}</PartNumber>"))
        })
        .and_then(|chunk| {
            chunk
                .split_once(&format!("<{element}>"))
                .and_then(|(_, rest)| rest.split_once(&format!("</{element}>")))
                .map(|(value, _)| value.to_string())
        })
}

/// How a row names the algorithm the part's digest is computed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedBy {
    /// The initiation alone named it: "this checksum algorithm must be the same for all
    /// parts" (`UploadPartRequest$ChecksumAlgorithm`), so a part of such an upload carries
    /// a digest even when its own request names nothing.
    Initiation,
    /// This request's `x-amz-sdk-checksum-algorithm` header.
    AlgorithmHeader,
    /// This request's individual `x-amz-checksum-<algorithm>` header, carrying the client's
    /// own digest of the same bytes.
    DeclaredDigest,
}

/// RULE: a part write answers the digest of THAT part's own bytes, in the member the
/// algorithm in force names and in no other, however the algorithm was named.
#[tokio::test]
async fn a_part_write_answers_the_digest_of_that_parts_own_bytes() {
    let router = router();
    let name = "multipart-part-checksum-bucket";
    bucket(&router, name).await;

    for (seed, algorithm) in ALGORITHMS.iter().enumerate() {
        for named_by in [
            NamedBy::Initiation,
            NamedBy::AlgorithmHeader,
            NamedBy::DeclaredDigest,
        ] {
            let case = format!("{algorithm} named by {named_by:?}");
            let key = format!("parts/{algorithm}-{named_by:?}.bin");
            // The bytes differ per row, so a digest that belonged to another row's part
            // could not pass this assertion.
            let body = filler(1024 + seed * 7, seed as u8);
            let expected = digest(algorithm, &body);

            let upload_id = initiate(
                &router,
                name,
                &key,
                &[("x-amz-checksum-algorithm", algorithm)],
            )
            .await;
            let header = checksum_header(algorithm);
            let headers: Vec<(&str, &str)> = match named_by {
                NamedBy::Initiation => Vec::new(),
                NamedBy::AlgorithmHeader => {
                    vec![("x-amz-sdk-checksum-algorithm", *algorithm)]
                }
                NamedBy::DeclaredDigest => vec![(header.as_str(), expected.as_str())],
            };
            let uploaded = upload_part(&router, name, &key, &upload_id, 1, &headers, &body).await;

            assert_eq!(
                uploaded.status,
                200,
                "case {case:?}: expected HTTP 200 from UploadPart, got {} with body {:?}",
                uploaded.status,
                text(&uploaded)
            );
            let answered = uploaded.header(&header);
            assert_eq!(
                answered.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {header} = {expected:?}, got {answered:?}"
            );
            // ONE member carries a digest: an object, and a part of one, is stored with the
            // checksum of a single algorithm.
            for other in CHECKSUM_HEADERS
                .iter()
                .filter(|other| **other != header.as_str())
            {
                let carried = uploaded.header(other);
                assert_eq!(
                    carried, None,
                    "case {case:?}: no other checksum header may carry a value, {other} \
                     carried {carried:?}"
                );
            }

            // The read-back reports the same per-part value: "the Base64 encoded, …
            // checksum of the part" (`Part`'s own `Checksum*` members).
            let listed = list_parts(&router, name, &key, &upload_id).await;
            assert_eq!(
                listed.status,
                200,
                "case {case:?}: expected HTTP 200 from ListParts, got {} with body {:?}",
                listed.status,
                text(&listed)
            );
            let element = format!("Checksum{}", algorithm);
            let reported = part_element(&listed, 1, &element);
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the listing to report <{element}> = {expected:?}, \
                 got {reported:?}"
            );
        }
    }
}

/// RULE: a part's digest is the digest of its OWN bytes — so the parts of one upload carry
/// different values, each the digest of what that part holds, and none of them is the
/// object's own checksum (the composition the completion computes).
#[tokio::test]
async fn each_part_of_one_upload_carries_its_own_digest() {
    let router = router();
    let name = "multipart-part-scoped-checksum-bucket";
    bucket(&router, name).await;
    let key = "parts/per-part.bin";
    let algorithm = "SHA256";

    // Parts of different sizes, the last one exempt from the published minimum, so a value
    // computed over the wrong span could not pass.
    let parts = [
        filler(MIN_MULTIPART_PART_BYTES, 3),
        filler(MIN_MULTIPART_PART_BYTES + 17, 5),
        b"the tail of it".to_vec(),
    ];
    let numbers = [1, 2, 3];

    let upload_id = initiate(
        &router,
        name,
        key,
        &[("x-amz-checksum-algorithm", algorithm)],
    )
    .await;
    let header = checksum_header(algorithm);
    let mut named: Vec<(i32, String)> = Vec::new();
    let mut answered: Vec<String> = Vec::new();
    for (number, body) in numbers.iter().zip(parts.iter()) {
        let uploaded = upload_part(&router, name, key, &upload_id, *number, &[], body).await;
        assert_eq!(
            uploaded.status,
            200,
            "part {number}: expected HTTP 200 from UploadPart, got {} with body {:?}",
            uploaded.status,
            text(&uploaded)
        );
        let expected = digest(algorithm, body);
        let carried = uploaded.header(&header);
        assert_eq!(
            carried.as_deref(),
            Some(expected.as_str()),
            "part {number}: expected {header} = {expected:?}, got {carried:?}"
        );
        answered.push(expected);
        named.push((
            *number,
            uploaded
                .header("etag")
                .expect("a part upload must report an ETag header"),
        ));
    }
    // Different bytes, different digests: nothing collapsed the three parts into one value.
    answered.sort();
    answered.dedup();
    assert_eq!(
        answered.len(),
        3,
        "three parts of three different bodies must answer three different digests, got \
         {answered:?}"
    );

    // And the OBJECT's checksum is the composition over the parts' digests — "concatenates
    // the … checksums … and then calculates the checksum of these concatenated values"
    // (Amazon S3 User Guide, *Checking object integrity*), reported "with the total number
    // of parts" after a dash — which is no part's own digest.
    let completed = complete(&router, name, key, &upload_id, &named, &[]).await;
    assert_eq!(
        completed.status,
        200,
        "the completion must answer HTTP 200, got {} with body {:?}",
        completed.status,
        text(&completed)
    );
    let mut digests: Vec<u8> = Vec::new();
    for part in parts.iter() {
        digests.extend_from_slice(&base64::decode(digest(algorithm, part)).expect("Base64"));
    }
    let composed = format!("{}-{}", digest(algorithm, &digests), parts.len());
    let object = completed.element(&format!("Checksum{algorithm}"));
    assert_eq!(
        object.as_deref(),
        Some(composed.as_str()),
        "the object's checksum must be the composition over its parts' digests {composed:?}, \
         got {object:?}"
    );
    for (number, part) in numbers.iter().zip(parts.iter()) {
        let own = digest(algorithm, part);
        assert_ne!(
            object.as_deref(),
            Some(own.as_str()),
            "part {number}: the object's checksum must not be a part's own digest, both are \
             {own:?}"
        );
    }

    // A read of the object's ATTRIBUTES states the count in `TotalPartsCount` beside the
    // checksum container, so the container carries the composition's DIGEST alone — while
    // each part it reports still carries that part's own digest.
    let attributes = object_attributes(&router, name, key).await;
    assert_eq!(
        attributes.status,
        200,
        "expected HTTP 200 from GetObjectAttributes, got {} with body {:?}",
        attributes.status,
        text(&attributes)
    );
    let element = format!("Checksum{algorithm}");
    let attributed = attributes.element(&element);
    let digest_alone = composed
        .rsplit_once('-')
        .map(|(digest, _)| digest.to_string())
        .expect("a composition carries the part count after a dash");
    assert_eq!(
        attributed.as_deref(),
        Some(digest_alone.as_str()),
        "a read of the attributes states the digest alone {digest_alone:?}, got {attributed:?}"
    );
    assert_eq!(
        attributes
            .element("PartsCount")
            .and_then(|count| count.parse::<usize>().ok()),
        Some(parts.len()),
        "the attributes state how many parts the object holds, got {:?}",
        attributes.element("PartsCount")
    );
    for (number, part) in numbers.iter().zip(parts.iter()) {
        let own = digest(algorithm, part);
        let reported = part_element(&attributes, *number, &element);
        assert_eq!(
            reported.as_deref(),
            Some(own.as_str()),
            "part {number}: the attributes must report that part's own digest {own:?}, got \
             {reported:?}"
        );
    }
}

/// RULE: a declared digest that is not the digest of the bytes received is refused 400
/// `BadDigest` and nothing is recorded — the part slot the request addressed stays empty.
#[tokio::test]
async fn a_wrong_declared_part_digest_is_refused_and_records_no_part() {
    let router = router();
    let name = "multipart-part-bad-digest-bucket";
    bucket(&router, name).await;

    for (seed, algorithm) in ALGORITHMS.iter().enumerate() {
        let case = format!("a wrong {algorithm}");
        let key = format!("bad-digest/{algorithm}.bin");
        let body = filler(512 + seed * 3, seed as u8);
        // A digest of the SAME WIDTH, so it is a well-formed value for the header and the
        // refusal is about the comparison rather than about the shape: the digest of other
        // bytes under the same algorithm.
        let wrong = digest(algorithm, b"not the bytes this part carries");
        let truthful = digest(algorithm, &body);
        assert_ne!(
            wrong, truthful,
            "case {case:?}: the suite's wrong digest must differ from the true one"
        );

        let upload_id = initiate(
            &router,
            name,
            &key,
            &[("x-amz-checksum-algorithm", algorithm)],
        )
        .await;
        let header = checksum_header(algorithm);
        let refused = upload_part(
            &router,
            name,
            &key,
            &upload_id,
            1,
            &[(header.as_str(), wrong.as_str())],
            &body,
        )
        .await;

        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            BAD_DIGEST,
            "case {case:?}: expected the error code {BAD_DIGEST}, got {:?}",
            refused.error_code()
        );
        let expected_message =
            format!("The {algorithm} you specified did not match the calculated checksum.");
        assert_eq!(
            refused.error_message(),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            refused.error_message()
        );

        // The slot stays empty: the upload reports no part at all.
        let listed = list_parts(&router, name, &key, &upload_id).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected HTTP 200 from ListParts, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        let reported = text(&listed).matches("<Part>").count();
        assert_eq!(
            reported, 0,
            "case {case:?}: a refused part write must record nothing, the upload reports \
             {reported} part(s)"
        );

        // The same bytes with the TRUE digest are recorded, so the refusal was about the
        // value and not about the header being present.
        let accepted = upload_part(
            &router,
            name,
            &key,
            &upload_id,
            1,
            &[(header.as_str(), truthful.as_str())],
            &body,
        )
        .await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: the true digest must be accepted, got {} with body {:?}",
            accepted.status,
            text(&accepted)
        );
    }
}

/// RULE, at the other end of the same loop: the object checksum a completion DECLARES is
/// judged against the one the assembled object carries — a value that is not it is refused
/// 400 `BadDigest`, no object is stored, and the upload is left in progress with its parts.
#[tokio::test]
async fn a_wrong_declared_object_checksum_is_refused_and_stores_no_object() {
    let router = router();
    let name = "multipart-object-checksum-bucket";
    bucket(&router, name).await;

    // A full-object algorithm and a composite one, so the rule is asserted over both ways
    // an object's checksum is computed from its parts.
    for (algorithm, requested_type) in [("CRC64NVME", "FULL_OBJECT"), ("SHA256", "COMPOSITE")] {
        let case = format!("{algorithm}/{requested_type}");
        let key = format!("object-digest/{algorithm}.bin");
        let parts = [filler(MIN_MULTIPART_PART_BYTES, 23), b"and the rest".to_vec()];

        let upload_id = initiate(
            &router,
            name,
            &key,
            &[
                ("x-amz-checksum-algorithm", algorithm),
                ("x-amz-checksum-type", requested_type),
            ],
        )
        .await;
        let header = checksum_header(algorithm);
        let mut named: Vec<(i32, String)> = Vec::new();
        for (index, body) in parts.iter().enumerate() {
            let number = index as i32 + 1;
            let uploaded = upload_part(&router, name, &key, &upload_id, number, &[], body).await;
            assert_eq!(
                uploaded.status,
                200,
                "case {case:?}: expected HTTP 200 from UploadPart, got {} with body {:?}",
                uploaded.status,
                text(&uploaded)
            );
            // A FULL_OBJECT upload's part still answers the part's OWN digest, not the
            // object's.
            let own = digest(algorithm, body);
            let carried = uploaded.header(&header);
            assert_eq!(
                carried.as_deref(),
                Some(own.as_str()),
                "case {case:?}: part {number} must answer its own digest {own:?}, got \
                 {carried:?}"
            );
            named.push((
                number,
                uploaded
                    .header("etag")
                    .expect("a part upload must report an ETag header"),
            ));
        }

        // A value that is not the object's checksum: the digest of other bytes entirely.
        let wrong = digest(algorithm, b"not the object these parts assemble into");
        let refused = complete(
            &router,
            name,
            &key,
            &upload_id,
            &named,
            &[(header.as_str(), wrong.as_str())],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            BAD_DIGEST,
            "case {case:?}: expected the error code {BAD_DIGEST}, got {:?}",
            refused.error_code()
        );
        // A COMPLETION names the algorithm the way the header its value arrived on spells
        // it — lowercase — where a one-shot write's own mismatch names the
        // `ChecksumAlgorithm` enum value; the rest of the sentence is the same words.
        let expected_message = format!(
            "The {} you specified did not match the calculated checksum.",
            algorithm.to_ascii_lowercase()
        );
        assert_eq!(
            refused.error_message(),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            refused.error_message()
        );

        // Nothing was stored, and the upload still holds its parts.
        let head = head_object(&router, name, &key).await;
        assert_eq!(
            head.status, 404,
            "case {case:?}: a refused completion must store no object, HeadObject answered {}",
            head.status
        );
        let listed = list_parts(&router, name, &key, &upload_id).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: the upload must still be in progress, ListParts answered {} with \
             body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            text(&listed).matches("<Part>").count(),
            parts.len(),
            "case {case:?}: the upload must still hold every part it held"
        );

        // The TRUE object checksum — the full object's digest for a FULL_OBJECT upload, the
        // composition over the parts' digests for a COMPOSITE one — is accepted, and the
        // completion answers that same value.
        let truthful = if requested_type == "FULL_OBJECT" {
            digest(algorithm, &parts.concat())
        } else {
            let mut digests: Vec<u8> = Vec::new();
            for part in parts.iter() {
                digests.extend_from_slice(
                    &base64::decode(digest(algorithm, part)).expect("a Base64 digest"),
                );
            }
            format!("{}-{}", digest(algorithm, &digests), parts.len())
        };
        let completed = complete(
            &router,
            name,
            &key,
            &upload_id,
            &named,
            &[(header.as_str(), truthful.as_str())],
        )
        .await;
        assert_eq!(
            completed.status,
            200,
            "case {case:?}: the true object checksum must be accepted, got {} with body {:?}",
            completed.status,
            text(&completed)
        );
        let reported = completed.element(&format!("Checksum{algorithm}"));
        assert_eq!(
            reported.as_deref(),
            Some(truthful.as_str()),
            "case {case:?}: expected the completion to report {truthful:?}, got {reported:?}"
        );
    }
}
