// SPDX-License-Identifier: CC0-1.0

//! HOW MANY checksums one write may declare.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle, object and integrity suites in this crate already own), so what is
//! pinned is the wire behaviour — status, error code, `<Message>` — and whether anything was
//! stored afterwards.
//!
//! The rule under test: a write declares ONE checksum. "If you provide an individual
//! checksum, Amazon S3 ignores any provided `ChecksumAlgorithm` parameter"
//! (`PutObjectRequest$ChecksumAlgorithm`) names the single algorithm a write's checksum is
//! computed under, and a stored object carries one checksum, so a request carrying several
//! `x-amz-checksum-*` headers asks for something no stored object can report: HTTP 400
//! `InvalidRequest`, "Expecting a single x-amz-checksum- header. Multiple checksum Types are
//! not allowed." Nothing is stored, and the refusal does not depend on the VALUES — every
//! header a row carries is the correct digest of the payload, so the count alone is what is
//! wrong.
//!
//! The rows differ along what the rule generalises over: WHICH algorithms are declared
//! together (every published `ChecksumAlgorithm` appears in a pair), how many at once, the
//! payload whose digests they are, and which write carries them — `PutObject` and
//! `UploadPart` both judge a declared checksum through the one rule.

use service_com_amazonaws_s3_amazons3::state::{
    checksum_header_name, full_object_checksum, INVALID_REQUEST_CODE,
    MULTIPLE_CHECKSUM_TYPES_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// Every value the model's `ChecksumAlgorithm` enum publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumAlgorithm",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports exactly these ten), each of which binds its own `x-amz-checksum-*` request
/// header. The pairs below walk the whole list, so no algorithm is left untested.
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

/// The payloads the rule is exercised over: the rows differ along the size and byte range of
/// the body whose digests are declared.
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("the empty payload", Vec::new()),
        ("a short text", b"two-checksums".to_vec()),
        (
            "a kilobyte spanning every byte value",
            (0..1024u32).map(|index| (index % 256) as u8).collect(),
        ),
    ]
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`, the reader that shows
/// whether a refused part write recorded anything.
async fn list_parts(router: &MultiServiceRouter, name: &str, key: &str, upload_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}/{key}?uploadId={upload_id}&x-id=ListParts"),
        &[],
        "",
    )
    .await
}

#[tokio::test]
async fn a_write_declaring_more_than_one_checksum_is_refused_and_stores_nothing() {
    let router = router();
    let name = "checksum-count-bucket";
    bucket(&router, name).await;

    for (payload_index, (payload_case, payload)) in payloads().into_iter().enumerate() {
        // Every adjacent pair of published algorithms, wrapping round, so each algorithm is
        // declared beside another one at least once.
        for (index, algorithm) in CHECKSUM_ALGORITHMS.iter().enumerate() {
            let other = CHECKSUM_ALGORITHMS[(index + 1) % CHECKSUM_ALGORITHMS.len()];
            let case = format!("{payload_case} with {algorithm} beside {other}");
            let (Some(first), Some(second)) = (
                full_object_checksum(algorithm, &payload),
                full_object_checksum(other, &payload),
            ) else {
                panic!("case {case:?}: the store must compute every published algorithm");
            };
            let (first_header, second_header) = (
                checksum_header_name(algorithm),
                checksum_header_name(other),
            );
            let key = format!("counted/{payload_index}-{algorithm}-{other}.bin");
            let refused = put_object(
                &router,
                name,
                &key,
                &payload,
                &[
                    (first_header.as_str(), first.as_str()),
                    (second_header.as_str(), second.as_str()),
                ],
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
                INVALID_REQUEST_CODE,
                "case {case:?}: expected the published code, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.error_message(),
                MULTIPLE_CHECKSUM_TYPES_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                refused.error_message()
            );
            let absent = get_object(&router, name, &key, "", &[]).await;
            assert_eq!(
                absent.status, 404,
                "case {case:?}: a refused write must store nothing, got GetObject {}",
                absent.status
            );

            // ONE of the same headers, carrying the same correct digest, is served — so the
            // refusal above is about the count and not about the algorithms or their values.
            let accepted = put_object(
                &router,
                name,
                &key,
                &payload,
                &[(first_header.as_str(), first.as_str())],
            )
            .await;
            assert_eq!(
                accepted.status,
                200,
                "case {case:?}: one checksum header must be served, got {} with body {:?}",
                accepted.status,
                text(&accepted)
            );
        }

        // Three at once is refused the same way: the rule is "a single header", not "at most
        // two".
        let case = format!("{payload_case} with three checksums at once");
        let headers: Vec<(String, String)> = CHECKSUM_ALGORITHMS
            .iter()
            .take(3)
            .map(|algorithm| {
                (
                    checksum_header_name(algorithm),
                    full_object_checksum(algorithm, &payload)
                        .unwrap_or_else(|| panic!("case {case:?}: {algorithm} is computable")),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let refused = put_object(
            &router,
            name,
            &format!("counted/{payload_index}-three.bin"),
            &payload,
            &borrowed,
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
            refused.error_message(),
            MULTIPLE_CHECKSUM_TYPES_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn the_same_count_rule_holds_for_a_part_of_a_multipart_upload() {
    let router = router();
    let name = "checksum-count-part-bucket";
    bucket(&router, name).await;
    let key = "counted/part.bin";
    let initiated = send(&router, "POST", &format!("/{name}/{key}?uploads"), &[], "").await;
    assert_eq!(
        initiated.status,
        200,
        "the initiation must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    let payload = b"part-bytes".to_vec();
    for (index, algorithm) in CHECKSUM_ALGORITHMS.iter().enumerate().take(4) {
        let other = CHECKSUM_ALGORITHMS[(index + 1) % CHECKSUM_ALGORITHMS.len()];
        let case = format!("a part declaring {algorithm} beside {other}");
        let first = full_object_checksum(algorithm, &payload)
            .unwrap_or_else(|| panic!("case {case:?}: {algorithm} is computable"));
        let second = full_object_checksum(other, &payload)
            .unwrap_or_else(|| panic!("case {case:?}: {other} is computable"));
        let refused = send_bytes(
            &router,
            "PUT",
            &format!("/{name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"),
            &[
                (checksum_header_name(algorithm).as_str(), first.as_str()),
                (checksum_header_name(other).as_str(), second.as_str()),
            ],
            &payload,
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
            refused.error_message(),
            MULTIPLE_CHECKSUM_TYPES_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
        // And the upload holds no part: a refused part write records nothing.
        let listed = list_parts(&router, name, key, &upload_id).await;
        assert!(
            !text(&listed).contains("<Part>"),
            "case {case:?}: a refused part write must record no part, got body {:?}",
            text(&listed)
        );
    }
}
