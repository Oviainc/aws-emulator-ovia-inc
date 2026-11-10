// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the part-write of the multipart upload loop: what `UploadPart`
//! records is what `ListParts` reports.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A part upload records the request's bytes as the part its `PartNumber` addresses on
//!    the named upload, and answers that part's own entity tag — the quoted MD5 of those
//!    bytes. A read of the upload then reports every recorded part, in ascending
//!    part-number order, each with the number, the entity tag and the size of the bytes
//!    stored for it, whatever order the parts arrived in.
//! 2. A part number addresses ONE part: uploading again under a number the upload already
//!    holds replaces what that number held and adds no part beside it.
//! 3. A part belongs to the upload it was uploaded into: two uploads of one key hold their
//!    own parts, and neither reports the other's.
//! 4. A read is a read: two reads of one upload report the same thing, and neither changes
//!    it.
//! 5. A page of parts is the parts above `part-number-marker`, at most `max-parts` of them,
//!    reporting the last part it carried as the next marker exactly when parts are left
//!    over.
//! 6. A request this emulator cannot serve as asked is refused rather than recorded: a part
//!    number outside the published 1..=10,000 range with 400 `InvalidArgument`, a
//!    `Content-MD5` that is not the digest of the bytes with 400 `BadDigest`, an upload id
//!    the store never minted — or one named under another key — with 404 `NoSuchUpload`, a
//!    bucket this emulator does not hold with 404 `NoSuchBucket`, SSE-C members that do not
//!    name one usable key with 400 `InvalidArgument`, and `x-amz-expected-bucket-owner`
//!    naming another account with 403.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The lowest part number a part may carry: "Part number of part being uploaded. This is a
/// positive integer between 1 and 10,000" (`UploadPartRequest$PartNumber`, from
/// `smithy_query(query="operation",operation="UploadPart")`).
const MIN_PART_NUMBER: i32 = 1;

/// The other end of that published range.
const MAX_PART_NUMBER: i32 = 10_000;

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: String,
}

impl Answer {
    /// The text of the first `<name>` element in the body, or `None` when the body carries
    /// none — which is how an absent output member reaches the wire.
    fn element(&self, name: &str) -> Option<String> {
        element_of(&self.body, name)
    }

    /// The wire error code: the `<Code>` element of S3's restXml error document (the model
    /// sets `noErrorWrapping`, so it is the document's own child).
    fn error_code(&self) -> String {
        self.element("Code").unwrap_or_else(|| self.body.clone())
    }

    /// A bound response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// Every `<Part>` the body reports, in the order it reports them.
    fn parts(&self) -> Vec<ReportedPart> {
        self.body
            .split("<Part>")
            .skip(1)
            .filter_map(|chunk| chunk.split("</Part>").next())
            .map(|chunk| ReportedPart {
                part_number: element_of(chunk, "PartNumber").and_then(|text| text.parse().ok()),
                etag: element_of(chunk, "ETag").map(|etag| unquote(&etag)),
                size: element_of(chunk, "Size").and_then(|text| text.parse().ok()),
                last_modified: element_of(chunk, "LastModified"),
            })
            .collect()
    }
}

/// One part as a read of the upload reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedPart {
    part_number: Option<i32>,
    etag: Option<String>,
    size: Option<i64>,
    last_modified: Option<String>,
}

/// The text of the first `<name>` element in `document`.
fn element_of(document: &str, name: &str) -> Option<String> {
    document
        .split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(value, _)| value.to_string())
}

/// An entity tag with the double quotes S3 wraps it in removed, however they reached the
/// wire (a header carries them literally, an XML element may escape them).
fn unquote(etag: &str) -> String {
    etag.replace("&quot;", "").replace('"', "")
}

/// Dispatch one request, carrying `headers` and `body` beside what every request needs.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// `PUT /<bucket>` — `CreateBucket` in the default Region.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let created = send(router, "PUT", &format!("/{bucket}"), &[], Vec::new()).await;
    assert_eq!(
        created.status, 200,
        "the bucket the rows upload parts into must be created first, got {} with {}",
        created.status, created.body
    );
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(router: &MultiServiceRouter, bucket: &str, key: &str) -> String {
    let initiated = send(
        router,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        initiated.status, 200,
        "an initiation must answer HTTP 200, got {} with {}",
        initiated.status, initiated.body
    );
    initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId")
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`.
async fn upload_part(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPart"),
        headers,
        body.to_vec(),
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`. The `x-id` literal is
/// the one the model binds this operation under
/// (`smithy_query(query="operation",operation="ListParts")` reports
/// `uri: /{Bucket}/{Key+}?x-id=ListParts`), which is what distinguishes it from a read of
/// the object at the same address.
async fn list_parts(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    upload_id: &str,
    query: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}/{key}?uploadId={upload_id}&x-id=ListParts{query}"),
        &[],
        Vec::new(),
    )
    .await
}

/// One row of the part table: the case name, the part number it is uploaded under, the
/// bytes it carries, and the hex MD5 those bytes are PUBLISHED to hash to (`None` where the
/// row's digest is not a published vector, in which case the row is checked against the tag
/// the upload itself reported).
struct PartCase {
    case: &'static str,
    part_number: i32,
    body: Vec<u8>,
    md5_hex: Option<&'static str>,
}

/// The part table. The rows differ along what the rule generalises over — the size of the
/// part (empty, one byte, a sentence, 128 KiB), what it holds (text, every byte value,
/// bytes that are not UTF-8 at all), the part number (both published bounds and numbers
/// with gaps between them) and the ORDER the parts arrive in, which is deliberately not
/// ascending.
///
/// Each published digest is a vector of RFC 1321's own test suite (appendix A.5, "MD5 test
/// suite"), NOT a value read back out of this emulator; S3's entity tag is that digest in
/// double quotes.
fn part_cases() -> Vec<PartCase> {
    vec![
        PartCase {
            case: "the sentence vector, uploaded first under a middle number",
            part_number: 5,
            body: b"message digest".to_vec(),
            md5_hex: Some("f96b697d7cb7938d525a2f31aaf161d0"),
        },
        PartCase {
            case: "the empty body, the low end of what a part may carry",
            part_number: MIN_PART_NUMBER,
            body: Vec::new(),
            md5_hex: Some("d41d8cd98f00b204e9800998ecf8427e"),
        },
        PartCase {
            case: "one byte",
            part_number: 2,
            body: b"a".to_vec(),
            md5_hex: Some("0cc175b9c0f1b6a831c399e269772661"),
        },
        PartCase {
            case: "the alphabet vector",
            part_number: 10,
            body: b"abcdefghijklmnopqrstuvwxyz".to_vec(),
            md5_hex: Some("c3fcd3d76192e4007dfb496cca67e13b"),
        },
        PartCase {
            case: "the alphanumeric vector, under the part number before the maximum",
            part_number: MAX_PART_NUMBER - 1,
            body: b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".to_vec(),
            md5_hex: Some("d174ab98d277d9f5a5611c2c9f419d9f"),
        },
        PartCase {
            case: "the digits vector, under the published maximum part number",
            part_number: MAX_PART_NUMBER,
            body: b"123456789012345678901234567890123456789012345678901234567890\
                    12345678901234567890"
                .to_vec(),
            md5_hex: Some("57edf4a22be3c955ac49da2e2107b67a"),
        },
        PartCase {
            case: "every byte value, which is not UTF-8 at all",
            part_number: 3,
            body: (0u8..=255).collect(),
            md5_hex: None,
        },
        PartCase {
            case: "128 KiB, a part far larger than one request's headers",
            part_number: 4,
            body: b"multipart-".repeat(13_108),
            md5_hex: None,
        },
    ]
}

#[tokio::test]
async fn uploaded_parts_answer_their_own_tag_and_read_back_in_part_number_order() {
    let router = router();
    let bucket = "multipart-part-upload-bucket";
    create_bucket(&router, bucket).await;
    let key = "nested/path/assembled.bin";
    let upload_id = initiate(&router, bucket, key).await;

    // What each row recorded, so the read-back is judged against it: the part number, the
    // entity tag the upload answered and the size of the bytes it carried.
    let mut recorded: Vec<(i32, String, i64)> = Vec::new();
    for row in part_cases() {
        let PartCase {
            case,
            part_number,
            body,
            md5_hex,
        } = row;

        let uploaded = upload_part(&router, bucket, key, &upload_id, part_number, &[], &body).await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: expected HTTP 200 from UploadPart, got {} with body {:?}",
            uploaded.status, uploaded.body
        );
        let etag = uploaded
            .header("etag")
            .unwrap_or_else(|| panic!("case {case:?}: a part upload must report an ETag header"));
        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "case {case:?}: an entity tag is reported inside double quotes, got {etag:?}"
        );
        let digest = unquote(&etag);
        if let Some(published) = md5_hex {
            assert_eq!(
                digest, published,
                "case {case:?}: the part's entity tag must be the MD5 of its bytes, \
                 expected {published:?}, got {digest:?}"
            );
        }
        assert!(
            digest.len() == 32 && digest.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: an entity tag is a 32-digit hex MD5, got {digest:?}"
        );
        recorded.push((
            part_number,
            digest,
            i64::try_from(body.len()).expect("a test body fits in an i64"),
        ));
    }

    // The read-back reports every recorded part, in ascending part-number order — not the
    // order the rows uploaded them in.
    let listed = list_parts(&router, bucket, key, &upload_id, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListParts, got {} with body {:?}",
        listed.status, listed.body
    );
    let reported = listed.parts();
    recorded.sort_by_key(|(part_number, _, _)| *part_number);
    assert_eq!(
        reported.len(),
        recorded.len(),
        "the read-back must report every uploaded part, expected {} parts, got {:?}",
        recorded.len(),
        reported
    );
    for (part, (part_number, digest, size)) in reported.iter().zip(recorded.iter()) {
        assert_eq!(
            part.part_number,
            Some(*part_number),
            "parts are reported in ascending part-number order, expected {part_number}, \
             got {part:?}"
        );
        assert_eq!(
            part.etag.as_deref(),
            Some(digest.as_str()),
            "part {part_number}: the read-back must report the entity tag the upload \
             answered, expected {digest:?}, got {part:?}"
        );
        assert_eq!(
            part.size,
            Some(*size),
            "part {part_number}: the read-back must report the size of the bytes stored, \
             expected {size}, got {part:?}"
        );
        assert!(
            part.last_modified.is_some(),
            "part {part_number}: the read-back must report when the part was uploaded, \
             got {part:?}"
        );
    }
    assert_eq!(
        listed.element("IsTruncated").as_deref(),
        Some("false"),
        "a page that carried every part is not truncated, got {:?}",
        listed.element("IsTruncated")
    );
    assert_eq!(
        listed.element("NextPartNumberMarker"),
        None,
        "a page that carried every part reports no next marker, got {:?}",
        listed.element("NextPartNumberMarker")
    );

    // A read is a read: the second one reports exactly what the first did.
    let again = list_parts(&router, bucket, key, &upload_id, "").await;
    assert_eq!(
        again.parts(),
        reported,
        "two reads of one upload must report the same parts, got {:?} then {:?}",
        reported,
        again.parts()
    );
}

#[tokio::test]
async fn a_part_number_addresses_one_part_so_a_second_upload_replaces_it() {
    let router = router();
    let bucket = "multipart-part-overwrite-bucket";
    create_bucket(&router, bucket).await;
    let key = "overwritten.bin";
    let upload_id = initiate(&router, bucket, key).await;

    // Three rows over one number: each is the whole part afterwards, whatever the one
    // before it held — a larger body, a smaller one, and the empty body.
    let rows: &[(&str, &[u8], &str)] = &[
        (
            "the alphabet vector",
            b"abcdefghijklmnopqrstuvwxyz",
            "c3fcd3d76192e4007dfb496cca67e13b",
        ),
        ("one byte", b"a", "0cc175b9c0f1b6a831c399e269772661"),
        ("the empty body", b"", "d41d8cd98f00b204e9800998ecf8427e"),
    ];
    for (case, body, published) in rows {
        let uploaded = upload_part(&router, bucket, key, &upload_id, 7, &[], body).await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: expected HTTP 200 from UploadPart, got {} with body {:?}",
            uploaded.status, uploaded.body
        );

        let listed = list_parts(&router, bucket, key, &upload_id, "").await;
        let reported = listed.parts();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a part number addresses one part, so the upload must still \
             hold exactly one, got {reported:?}"
        );
        assert_eq!(
            reported[0].etag.as_deref(),
            Some(*published),
            "case {case:?}: the part must report the tag of the bytes last uploaded \
             under its number, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].size,
            Some(i64::try_from(body.len()).expect("a test body fits in an i64")),
            "case {case:?}: the part must report the size of the bytes last uploaded \
             under its number, got {:?}",
            reported[0]
        );
    }
}

#[tokio::test]
async fn parts_belong_to_the_upload_they_were_uploaded_into() {
    let router = router();
    let bucket = "multipart-part-isolation-bucket";
    create_bucket(&router, bucket).await;
    let key = "two-uploads/one-key.bin";

    // Two uploads of ONE key, each holding a part under the SAME number: the number
    // addresses a part of its own upload, so neither read reports the other's part.
    let first = initiate(&router, bucket, key).await;
    let second = initiate(&router, bucket, key).await;
    assert_ne!(
        first, second,
        "two initiations of one key are two uploads, each with its own id"
    );

    let rows: &[(&str, &str, &[u8], &str)] = &[
        (
            "the first upload",
            &first,
            b"abc",
            "900150983cd24fb0d6963f7d28e17f72",
        ),
        (
            "the second upload",
            &second,
            b"message digest",
            "f96b697d7cb7938d525a2f31aaf161d0",
        ),
    ];
    for (case, upload_id, body, _) in rows {
        let uploaded = upload_part(&router, bucket, key, upload_id, 1, &[], body).await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: expected HTTP 200 from UploadPart, got {} with body {:?}",
            uploaded.status, uploaded.body
        );
    }
    for (case, upload_id, body, published) in rows {
        let listed = list_parts(&router, bucket, key, upload_id, "").await;
        let reported = listed.parts();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: an upload reports only the parts uploaded into it, \
             got {reported:?}"
        );
        assert_eq!(
            reported[0].etag.as_deref(),
            Some(*published),
            "case {case:?}: the part reported must be the one this upload holds, \
             got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].size,
            Some(i64::try_from(body.len()).expect("a test body fits in an i64")),
            "case {case:?}: the part reported must carry this upload's bytes, got {:?}",
            reported[0]
        );
    }
}

/// One row of the paging table: the case, the query it adds to the read, the part numbers
/// the page must carry, whether it is truncated and the next marker it must report.
struct PageCase {
    case: &'static str,
    query: &'static str,
    expected: Vec<i32>,
    truncated: bool,
    next_marker: Option<&'static str>,
}

#[tokio::test]
async fn a_page_of_parts_is_the_parts_above_the_marker_bounded_by_max_parts() {
    let router = router();
    let bucket = "multipart-part-paging-bucket";
    create_bucket(&router, bucket).await;
    let key = "paged.bin";
    let upload_id = initiate(&router, bucket, key).await;

    // Five parts, uploaded under numbers with gaps between them, so a marker is judged
    // against the numbers the upload really holds rather than a position in a list.
    let numbers = [1, 4, 9, 16, 25];
    for part_number in numbers {
        let uploaded = upload_part(
            &router,
            bucket,
            key,
            &upload_id,
            part_number,
            &[],
            format!("part-{part_number}").as_bytes(),
        )
        .await;
        assert_eq!(
            uploaded.status, 200,
            "part {part_number}: expected HTTP 200 from UploadPart, got {} with body {:?}",
            uploaded.status, uploaded.body
        );
    }

    // One row per page size the rule generalises over: a page smaller than the upload
    // holds, one exactly its size, one larger, and a marker past every part.
    let rows = vec![
        PageCase {
            case: "no marker and no bound",
            query: "",
            expected: numbers.to_vec(),
            truncated: false,
            next_marker: None,
        },
        PageCase {
            case: "a page of two from the start",
            query: "&max-parts=2",
            expected: vec![1, 4],
            truncated: true,
            next_marker: Some("4"),
        },
        PageCase {
            case: "a page of two above part 4",
            query: "&max-parts=2&part-number-marker=4",
            expected: vec![9, 16],
            truncated: true,
            next_marker: Some("16"),
        },
        PageCase {
            case: "the last page above part 16",
            query: "&max-parts=2&part-number-marker=16",
            expected: vec![25],
            truncated: false,
            next_marker: None,
        },
        PageCase {
            case: "a page as large as the upload holds",
            query: "&max-parts=5",
            expected: numbers.to_vec(),
            truncated: false,
            next_marker: None,
        },
        PageCase {
            case: "a page larger than the upload holds",
            query: "&max-parts=50",
            expected: numbers.to_vec(),
            truncated: false,
            next_marker: None,
        },
        PageCase {
            case: "a marker above every part",
            query: "&part-number-marker=25",
            expected: Vec::new(),
            truncated: false,
            next_marker: None,
        },
        PageCase {
            case: "a page of none, with parts left over",
            query: "&max-parts=0",
            expected: Vec::new(),
            truncated: true,
            next_marker: Some("0"),
        },
    ];
    for row in rows {
        let PageCase {
            case,
            query,
            expected,
            truncated,
            next_marker,
        } = row;
        let listed = list_parts(&router, bucket, key, &upload_id, query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListParts, got {} with body {:?}",
            listed.status, listed.body
        );
        let reported: Vec<Option<i32>> = listed
            .parts()
            .into_iter()
            .map(|part| part.part_number)
            .collect();
        let wanted: Vec<Option<i32>> = expected.iter().copied().map(Some).collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the part numbers {wanted:?}, got {reported:?}"
        );
        assert_eq!(
            listed.element("IsTruncated").as_deref(),
            Some(if truncated { "true" } else { "false" }),
            "case {case:?}: expected IsTruncated {truncated}, got {:?}",
            listed.element("IsTruncated")
        );
        assert_eq!(
            listed.element("NextPartNumberMarker").as_deref(),
            next_marker,
            "case {case:?}: expected the next marker {next_marker:?}, got {:?}",
            listed.element("NextPartNumberMarker")
        );
    }

    // The marker is a part NUMBER carried as a string, so a value that is not one — and a
    // negative one, which no part number can be — names no position at all and is refused
    // rather than listed.
    for (case, query) in [
        (
            "a marker that is not a number at all",
            "&part-number-marker=not-a-number",
        ),
        ("a negative marker", "&part-number-marker=-5"),
        ("a fractional marker", "&part-number-marker=4.5"),
        ("an empty marker", "&part-number-marker="),
    ] {
        let refused = list_parts(&router, bucket, key, &upload_id, query).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListParts, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case:?}: expected the wire error code \"InvalidArgument\", got {:?}",
            refused.error_code()
        );
    }
}

/// One row of the refusal table: the case, the address it names, the part number it
/// uploads under, the headers it carries, and the status and wire error code it must be
/// refused with. A row names only what it varies; [`RefusalCase::against`] fills the rest
/// with a request that would otherwise be served.
struct RefusalCase {
    case: &'static str,
    bucket: &'static str,
    key: &'static str,
    upload_id: String,
    part_number: i32,
    headers: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

impl RefusalCase {
    /// A row against `bucket`/`key` that varies nothing else: the lowest valid part
    /// number, no extra header, and no refusal expected until the row names one.
    fn against(bucket: &'static str, key: &'static str) -> Self {
        Self {
            case: "",
            bucket,
            key,
            upload_id: String::new(),
            part_number: MIN_PART_NUMBER,
            headers: Vec::new(),
            status: 200,
            code: "",
        }
    }
}

#[tokio::test]
async fn a_part_upload_this_emulator_cannot_serve_as_asked_is_refused() {
    let router = router();
    let bucket = "multipart-part-refusal-bucket";
    create_bucket(&router, bucket).await;
    let key = "refused.bin";
    let upload_id = initiate(&router, bucket, key).await;
    let other_upload = initiate(&router, bucket, "another/key.bin").await;

    let rows = vec![
        RefusalCase {
            case: "the part number one below the published minimum",
            upload_id: upload_id.clone(),
            part_number: MIN_PART_NUMBER - 1,
            status: 400,
            code: "InvalidArgument",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "a negative part number",
            upload_id: upload_id.clone(),
            part_number: -7,
            status: 400,
            code: "InvalidArgument",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "the part number one above the published maximum",
            upload_id: upload_id.clone(),
            part_number: MAX_PART_NUMBER + 1,
            status: 400,
            code: "InvalidArgument",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "a Content-MD5 that is not the digest of the bytes",
            upload_id: upload_id.clone(),
            headers: vec![("content-md5", "rL0Y20zC+Fzt72VPzMSk2A==")],
            status: 400,
            code: "BadDigest",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "an upload id the store never minted",
            upload_id: "not-an-upload-id".to_string(),
            status: 404,
            code: "NoSuchUpload",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "an upload id named under another key",
            upload_id: other_upload.clone(),
            status: 404,
            code: "NoSuchUpload",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "a bucket this emulator does not hold",
            upload_id: upload_id.clone(),
            status: 404,
            code: "NoSuchBucket",
            ..RefusalCase::against("multipart-part-absent-bucket", key)
        },
        RefusalCase {
            // A part presenting SSE-C members that do not name one usable key is refused for
            // the member that does not hold — here the algorithm with no key beside it —
            // before the part is recorded (see
            // [`super::object_customer_key_encryption`] for the whole rule).
            case: "a customer-provided encryption key with no key beside it",
            upload_id: upload_id.clone(),
            headers: vec![(
                "x-amz-server-side-encryption-customer-algorithm",
                "AES256",
            )],
            status: 400,
            code: "InvalidArgument",
            ..RefusalCase::against(bucket, key)
        },
        RefusalCase {
            case: "x-amz-expected-bucket-owner naming another account",
            upload_id: upload_id.clone(),
            headers: vec![("x-amz-expected-bucket-owner", "123456789012")],
            status: 403,
            code: "AccessDenied",
            ..RefusalCase::against(bucket, key)
        },
    ];

    for row in rows {
        let RefusalCase {
            case,
            bucket: row_bucket,
            key: row_key,
            upload_id: row_upload,
            part_number,
            headers,
            status,
            code,
        } = row;
        let refused = upload_part(
            &router,
            row_bucket,
            row_key,
            &row_upload,
            part_number,
            &headers,
            b"abc",
        )
        .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the wire error code {code:?}, got {:?}",
            refused.error_code()
        );
        assert!(
            refused.element("Message").is_some_and(|m| !m.is_empty()),
            "case {case:?}: an S3 error document carries a Message, got {:?}",
            refused.body
        );
    }

    // Nothing above was recorded: the upload still holds no part at all.
    let listed = list_parts(&router, bucket, key, &upload_id, "").await;
    assert_eq!(
        listed.parts(),
        Vec::new(),
        "a refused part upload records nothing, got {:?}",
        listed.parts()
    );
}

#[tokio::test]
async fn a_part_upload_carrying_the_digest_of_its_bytes_is_recorded() {
    let router = router();
    let bucket = "multipart-part-digest-bucket";
    create_bucket(&router, bucket).await;
    let key = "checked.bin";
    let upload_id = initiate(&router, bucket, key).await;

    // The Base64 MD5 digests of RFC 1321's own vectors (appendix A.5), which is the form
    // `Content-MD5` carries: "The Base64 encoded 128-bit MD5 digest of the part data"
    // (`UploadPartRequest$ContentMD5`). A write whose header matches its bytes is recorded,
    // and reports the same digest as its hex entity tag.
    let rows: &[(&str, &[u8], &str, &str)] = &[
        (
            "the empty body",
            b"",
            "1B2M2Y8AsgTpgAmY7PhCfg==",
            "d41d8cd98f00b204e9800998ecf8427e",
        ),
        (
            "one byte",
            b"a",
            "DMF1ucDxtqgxw5niaXcmYQ==",
            "0cc175b9c0f1b6a831c399e269772661",
        ),
        (
            "the alphabet vector",
            b"abcdefghijklmnopqrstuvwxyz",
            "w/zT12GS5AB9+0lsymfhOw==",
            "c3fcd3d76192e4007dfb496cca67e13b",
        ),
    ];
    for (index, (case, body, content_md5, published_hex)) in rows.iter().enumerate() {
        let part_number = i32::try_from(index).expect("a row index fits in an i32") + 1;
        let uploaded = upload_part(
            &router,
            bucket,
            key,
            &upload_id,
            part_number,
            &[("content-md5", content_md5)],
            body,
        )
        .await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: a part whose Content-MD5 is the digest of its bytes is \
             recorded, got {} with body {:?}",
            uploaded.status, uploaded.body
        );
        let etag = uploaded
            .header("etag")
            .unwrap_or_else(|| panic!("case {case:?}: a part upload must report an ETag header"));
        assert_eq!(
            unquote(&etag),
            *published_hex,
            "case {case:?}: expected the entity tag {published_hex:?}, got {etag:?}"
        );
    }

    let listed = list_parts(&router, bucket, key, &upload_id, "").await;
    let reported: Vec<Option<i32>> = listed
        .parts()
        .into_iter()
        .map(|part| part.part_number)
        .collect();
    assert_eq!(
        reported,
        vec![Some(1), Some(2), Some(3)],
        "every accepted part must read back, got {reported:?}"
    );
}
