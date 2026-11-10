// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for initiating a multipart upload and reading that upload back:
//! `CreateMultipartUpload` records an upload and `ListParts` reports the one it recorded.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. An initiation records a NEW upload resource in the named bucket under a freshly
//!    minted, store-wide unique opaque upload id, and answers with that id and the address
//!    the upload was recorded under. Two initiations are two uploads, whatever they name:
//!    one key may have several in progress at once, each with its own id and its own
//!    recorded configuration.
//! 2. A read of an upload by its id reports the configuration the initiation settled — the
//!    storage class (`STANDARD` when the request named none), the checksum configuration it
//!    asked for — and the parts it holds, which is none until a part is uploaded.
//! 3. A read is a read: two reads of one upload report the same thing, and neither changes
//!    it.
//! 4. An address this emulator does not hold is refused with the documented 404 code: a
//!    bucket that is not there is `NoSuchBucket`, and an upload id the store never minted
//!    — or one named under another key — is `NoSuchUpload`.
//! 5. A request asking for something this emulator does not serve is refused rather than
//!    answered: SSE-C members that name no usable key with 400 `InvalidArgument`, a KMS-only
//!    member beside no KMS algorithm with 400 `InvalidArgument`, a tag set past an object's
//!    published limit with 400, and `x-amz-expected-bucket-owner` naming another account
//!    with 403.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::DEFAULT_SSE_ALGORITHM;
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The `StorageClass` values this suite names, taken from the model's own enum
/// (`smithy_query(query="shape",shape="StorageClass")` reports `STANDARD`,
/// `REDUCED_REDUNDANCY`, `STANDARD_IA`, `ONEZONE_IA`, `INTELLIGENT_TIERING`, `GLACIER`,
/// `DEEP_ARCHIVE`, `OUTPOSTS`, `GLACIER_IR`, `SNOW`, `EXPRESS_ONEZONE`, `FSX_OPENZFS`,
/// `FSX_ONTAP`, `AWS_BACKUP_WARM`, `AWS_BACKUP_LOW_COST_WARM`) — including entries the
/// issue cited nowhere, so the rows span the member rather than one recorded value.
const STORAGE_CLASSES: &[&str] = &[
    "STANDARD",
    "STANDARD_IA",
    "INTELLIGENT_TIERING",
    "GLACIER_IR",
    "REDUCED_REDUNDANCY",
];

/// The `ChecksumAlgorithm` values this suite names, from the model's own enum
/// (`smithy_query(query="shape",shape="ChecksumAlgorithm")`: `CRC32`, `CRC32C`,
/// `CRC64NVME`, `MD5`, `SHA1`, `SHA256`, `SHA512`, `XXHASH128`, `XXHASH3`, `XXHASH64`).
const CHECKSUM_ALGORITHMS: &[&str] = &["CRC32", "SHA256", "CRC64NVME", "XXHASH64"];

/// The longest object key this emulator records, in UTF-8 bytes: the published S3 limit
/// ("The name for a key is a sequence of Unicode characters whose UTF-8 encoding is at
/// most 1,024 bytes long", Amazon S3 User Guide, "Creating object key names").
const MAX_OBJECT_KEY_BYTES: usize = 1_024;

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
    /// The text of the first `<name>` element in the body, or `None` when the body
    /// carries none — which is how an absent output member reaches the wire.
    fn element(&self, name: &str) -> Option<String> {
        self.body
            .split_once(&format!("<{name}>"))
            .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
            .map(|(value, _)| value.to_string())
    }

    /// The wire error code: the `<Code>` element of S3's restXml error document (the
    /// model sets `noErrorWrapping`, so it is the document's own child).
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
}

/// Dispatch one request, carrying `headers` beside the ones every request needs.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
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
        .body(bytes::Bytes::new())
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
    let created = send(router, "PUT", &format!("/{bucket}"), &[]).await;
    assert_eq!(
        created.status, 200,
        "the bucket the rows initiate uploads in must be created first, got {} with {}",
        created.status, created.body
    );
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`.
async fn initiate(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "POST", &format!("/{bucket}/{key}?uploads"), headers).await
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
    )
    .await
}

/// One row of the initiation table: the case name, the key the upload is initiated for,
/// the headers the request carries, and what the recorded upload must then report.
struct InitiationCase {
    case: String,
    key: String,
    headers: Vec<(&'static str, &'static str)>,
    /// The `StorageClass` the read-back must report for the recorded upload.
    storage_class: &'static str,
    /// The `ChecksumAlgorithm` the response and the read-back must report, or `None` when
    /// the initiation named none (in which case neither reports one).
    checksum_algorithm: Option<&'static str>,
    /// The `x-amz-server-side-encryption` the response must report. An initiation that
    /// names none is recorded under the BUCKET's default encryption configuration — "Amazon
    /// S3 encrypts data with server-side encryption by using Amazon S3 managed keys (SSE-S3)
    /// by default" (`PutObjectRequest$ServerSideEncryption`) — so such a row reports
    /// [`DEFAULT_SSE_ALGORITHM`], exactly as a one-shot write into the same bucket does.
    sse_algorithm: Option<&'static str>,
}

/// The initiation table: the rows differ along what the rule generalises over — the shape
/// and the length of the key, the storage class, the checksum configuration, the
/// encryption configuration, the user metadata and the tag set the initiation carries.
fn initiation_cases() -> Vec<InitiationCase> {
    let mut cases = vec![
        InitiationCase {
            case: "a flat key, and not one optional member".to_string(),
            key: "plain.bin".to_string(),
            headers: Vec::new(),
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        },
        InitiationCase {
            case: "a nested key, a content type and user metadata".to_string(),
            key: "nested/path/to/report-2026.csv".to_string(),
            headers: vec![
                ("content-type", "text/csv"),
                ("cache-control", "max-age=60"),
                ("x-amz-meta-owner", "alice"),
                ("x-amz-meta-purpose", "round-trip"),
            ],
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        },
        InitiationCase {
            case: "a tag set in x-amz-tagging".to_string(),
            key: "tagged/object.parquet".to_string(),
            headers: vec![("x-amz-tagging", "Project=emulator&Stage=test")],
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        },
        InitiationCase {
            case: "server-side encryption with an S3 managed key".to_string(),
            key: "encrypted/sse-s3.bin".to_string(),
            headers: vec![("x-amz-server-side-encryption", "AES256")],
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some("AES256"),
        },
        InitiationCase {
            case: "server-side encryption with a KMS key and a bucket key".to_string(),
            key: "encrypted/sse-kms.bin".to_string(),
            headers: vec![
                ("x-amz-server-side-encryption", "aws:kms"),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "alias/emulator",
                ),
                ("x-amz-server-side-encryption-bucket-key-enabled", "true"),
            ],
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some("aws:kms"),
        },
        InitiationCase {
            // The ceiling itself: a key of exactly the published maximum is accepted,
            // and one byte past it is refused (see the refusal table below).
            case: format!("a key of exactly the {MAX_OBJECT_KEY_BYTES}-byte maximum"),
            key: "k".repeat(MAX_OBJECT_KEY_BYTES),
            headers: Vec::new(),
            storage_class: "STANDARD",
            checksum_algorithm: None,
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        },
    ];
    // One row per storage class the model declares that this suite names, so the recorded
    // class is read back off the upload rather than assumed to be the default.
    for storage_class in STORAGE_CLASSES {
        cases.push(InitiationCase {
            case: format!("the storage class {storage_class}"),
            key: format!("classes/{}.bin", storage_class.to_ascii_lowercase()),
            headers: vec![("x-amz-storage-class", storage_class)],
            storage_class,
            checksum_algorithm: None,
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        });
    }
    // One row per checksum algorithm, which is a CONFIGURATION of the upload: it is
    // reported back as the algorithm the upload was initiated with, never as a checksum
    // value (nothing here computes one, so no `Checksum*` member is reported anywhere).
    for algorithm in CHECKSUM_ALGORITHMS {
        cases.push(InitiationCase {
            case: format!("the checksum algorithm {algorithm}"),
            key: format!("checksums/{}.bin", algorithm.to_ascii_lowercase()),
            headers: vec![("x-amz-checksum-algorithm", algorithm)],
            storage_class: "STANDARD",
            checksum_algorithm: Some(algorithm),
            sse_algorithm: Some(DEFAULT_SSE_ALGORITHM),
        });
    }
    cases
}

#[tokio::test]
async fn an_initiated_upload_answers_a_fresh_id_and_reads_back_as_it_was_initiated() {
    // One row initiates under a KMS key named by alias, and an initiation settles the
    // encryption the finished object is stored under — so that reference is resolved against
    // KMS before the upload is recorded, and the alias is made with KMS's own operations over
    // the same engine ([`super::kms_key_references`]).
    let router = super::kms_key_references::router_with_kms();
    let (key_id, _) =
        super::kms_key_references::created_key(&router, "the key this upload names").await;
    super::kms_key_references::created_alias(&router, "alias/emulator", &key_id).await;
    let bucket = "multipart-initiation-bucket";
    create_bucket(&router, bucket).await;

    let mut minted: Vec<(String, String)> = Vec::new();
    for row in initiation_cases() {
        let InitiationCase {
            case,
            key,
            headers,
            storage_class,
            checksum_algorithm,
            sse_algorithm,
        } = row;

        let initiated = initiate(&router, bucket, &key, &headers).await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: expected HTTP 200 from CreateMultipartUpload, got {} with \
             body {:?}",
            initiated.status, initiated.body
        );
        assert_eq!(
            initiated.element("Bucket").as_deref(),
            Some(bucket),
            "case {case:?}: the response must report the bucket the upload was recorded \
             in, got {:?}",
            initiated.element("Bucket")
        );
        assert_eq!(
            initiated.element("Key").as_deref(),
            Some(key.as_str()),
            "case {case:?}: the response must report the key the upload was initiated \
             for, got {:?}",
            initiated.element("Key")
        );
        let upload_id = initiated
            .element("UploadId")
            .unwrap_or_else(|| panic!("case {case:?}: an initiation must report an UploadId"));
        assert!(
            !upload_id.is_empty(),
            "case {case:?}: the reported UploadId must identify the upload, got {upload_id:?}"
        );
        assert!(
            upload_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '-'
                    || character == '_'),
            "case {case:?}: an upload id travels back in the uploadId query parameter, so \
             it must be URL-safe, got {upload_id:?}"
        );
        assert_eq!(
            initiated.header("x-amz-server-side-encryption"),
            sse_algorithm.map(str::to_string),
            "case {case:?}: the response must report the encryption the upload was \
             recorded under, got {:?}",
            initiated.header("x-amz-server-side-encryption")
        );
        assert_eq!(
            initiated.header("x-amz-checksum-algorithm"),
            checksum_algorithm.map(str::to_string),
            "case {case:?}: the response must report the checksum algorithm the upload \
             was initiated with, got {:?}",
            initiated.header("x-amz-checksum-algorithm")
        );

        // The round trip: the upload the initiation recorded is the one a read reports.
        let read = list_parts(&router, bucket, &key, &upload_id, "").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from ListParts on the upload just \
             initiated, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.element("UploadId").as_deref(),
            Some(upload_id.as_str()),
            "case {case:?}: the read must report the upload id it was asked about, got \
             {:?}",
            read.element("UploadId")
        );
        assert_eq!(
            read.element("Key").as_deref(),
            Some(key.as_str()),
            "case {case:?}: the read must report the key the upload was initiated for, \
             got {:?}",
            read.element("Key")
        );
        assert_eq!(
            read.element("Bucket").as_deref(),
            Some(bucket),
            "case {case:?}: the read must report the bucket the upload is in, got {:?}",
            read.element("Bucket")
        );
        assert_eq!(
            read.element("StorageClass").as_deref(),
            Some(storage_class),
            "case {case:?}: the read must report the storage class the upload was \
             recorded with, got {:?}",
            read.element("StorageClass")
        );
        assert_eq!(
            read.element("ChecksumAlgorithm"),
            checksum_algorithm.map(str::to_string),
            "case {case:?}: the read must report the checksum algorithm the upload was \
             initiated with, got {:?}",
            read.element("ChecksumAlgorithm")
        );
        assert_eq!(
            read.element("IsTruncated").as_deref(),
            Some("false"),
            "case {case:?}: a page that returned every part the upload holds is not \
             truncated, got {:?}",
            read.element("IsTruncated")
        );
        assert_eq!(
            read.element("MaxParts").as_deref(),
            Some("1000"),
            "case {case:?}: a read naming no max-parts reports the documented default of \
             1000, got {:?}",
            read.element("MaxParts")
        );
        assert!(
            read.element("Part").is_none(),
            "case {case:?}: nothing has uploaded a part to this upload, so the read must \
             report none, got {:?}",
            read.body
        );
        assert!(
            read.element("NextPartNumberMarker").is_none(),
            "case {case:?}: an untruncated page specifies no next marker, got {:?}",
            read.element("NextPartNumberMarker")
        );
        assert!(
            read.element("Initiator").is_some() && read.element("Owner").is_some(),
            "case {case:?}: a read reports who initiated the upload and who will own the \
             object, got {:?}",
            read.body
        );

        // A read changes nothing a later caller sees: the second read reports the first.
        let again = list_parts(&router, bucket, &key, &upload_id, "").await;
        assert_eq!(
            (again.status, again.element("UploadId"), again.element("StorageClass")),
            (read.status, read.element("UploadId"), read.element("StorageClass")),
            "case {case:?}: two reads of one upload must report the same thing, got {:?}",
            again.body
        );

        minted.push((case, upload_id));
    }

    // Every initiation minted an id of its own: no two uploads in this store share one.
    for (index, (left_case, left)) in minted.iter().enumerate() {
        for (right_case, right) in &minted[index + 1..] {
            assert_ne!(
                left, right,
                "cases {left_case:?} and {right_case:?} are two uploads, so their upload \
                 ids must differ, got {left:?}"
            );
        }
    }
}

#[tokio::test]
async fn several_uploads_of_one_key_are_several_uploads() {
    let router = router();
    let bucket = "multipart-concurrent-bucket";
    create_bucket(&router, bucket).await;
    let key = "shared/key.bin";

    // Each row initiates ANOTHER upload of the SAME key, with its own storage class, so
    // what is pinned is that an upload is a resource of its own: an initiation neither
    // replaces the upload already in progress at that key nor borrows its configuration.
    let mut recorded: Vec<(&str, String)> = Vec::new();
    for storage_class in STORAGE_CLASSES {
        let initiated = initiate(
            &router,
            bucket,
            key,
            &[("x-amz-storage-class", storage_class)],
        )
        .await;
        assert_eq!(
            initiated.status, 200,
            "case {storage_class:?}: expected HTTP 200 from a second initiation of one \
             key, got {} with body {:?}",
            initiated.status, initiated.body
        );
        let upload_id = initiated.element("UploadId").unwrap_or_else(|| {
            panic!("case {storage_class:?}: an initiation must report an UploadId")
        });
        recorded.push((storage_class, upload_id));
    }

    for (storage_class, upload_id) in &recorded {
        let read = list_parts(&router, bucket, key, upload_id, "").await;
        assert_eq!(
            read.status, 200,
            "case {storage_class:?}: every upload initiated for the key must still be \
             readable, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.element("StorageClass").as_deref(),
            Some(*storage_class),
            "case {storage_class:?}: each upload reports the class ITS initiation \
             recorded, got {:?}",
            read.element("StorageClass")
        );
    }

    let ids: Vec<&String> = recorded.iter().map(|(_, id)| id).collect();
    for (index, left) in ids.iter().enumerate() {
        for right in &ids[index + 1..] {
            assert_ne!(
                left, right,
                "two initiations of one key are two uploads, so their ids must differ, \
                 got {left:?}"
            );
        }
    }
}

/// One row of the refusal table: the case, the request to make, and the status and wire
/// error code it must be answered with.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    headers: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn an_address_or_a_feature_this_emulator_does_not_hold_is_refused() {
    let router = router();
    let bucket = "multipart-refusal-bucket";
    create_bucket(&router, bucket).await;
    let key = "refusals/object.bin";

    // One upload really in progress, so a refusal keyed on the upload is a refusal about
    // the id or the key rather than about an empty store.
    let initiated = initiate(&router, bucket, key, &[]).await;
    assert_eq!(
        initiated.status, 200,
        "the upload the refusal rows name must be initiated first, got {} with body {:?}",
        initiated.status, initiated.body
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    // 11 tags, one past the published object limit ("Amazon S3 limits the maximum number
    // of tags to 10 tags per object" — `PutObjectTagging`'s own documentation).
    let too_many_tags: String = (0..11)
        .map(|index| format!("k{index}=v{index}"))
        .collect::<Vec<_>>()
        .join("&");
    let over_long_key = "k".repeat(MAX_OBJECT_KEY_BYTES + 1);

    let cases = vec![
        RefusalCase {
            case: "an initiation into a bucket this emulator does not hold",
            method: "POST",
            uri: format!("/bucket-that-was-never-created/{key}?uploads"),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalCase {
            case: "a read of an upload in a bucket this emulator does not hold",
            method: "GET",
            uri: format!(
                "/bucket-that-was-never-created/{key}?uploadId={upload_id}&x-id=ListParts"
            ),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalCase {
            case: "a read naming an upload id this store never minted",
            method: "GET",
            uri: format!("/{bucket}/{key}?uploadId=an-id-nothing-minted&x-id=ListParts"),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "a read naming a real upload id under another key",
            method: "GET",
            uri: format!(
                "/{bucket}/another/key.bin?uploadId={upload_id}&x-id=ListParts"
            ),
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalCase {
            case: "an initiation whose expected bucket owner is another account",
            method: "POST",
            uri: format!("/{bucket}/{key}?uploads"),
            headers: vec![(
                "x-amz-expected-bucket-owner",
                "999999999999".to_string(),
            )],
            status: 403,
            code: "AccessDenied",
        },
        RefusalCase {
            case: "a read whose expected bucket owner is another account",
            method: "GET",
            uri: format!("/{bucket}/{key}?uploadId={upload_id}&x-id=ListParts"),
            headers: vec![(
                "x-amz-expected-bucket-owner",
                "999999999999".to_string(),
            )],
            status: 403,
            code: "AccessDenied",
        },
        RefusalCase {
            case: "an initiation whose SSE-C members name no usable key",
            method: "POST",
            uri: format!("/{bucket}/{key}?uploads"),
            headers: vec![(
                "x-amz-server-side-encryption-customer-algorithm",
                "AES256".to_string(),
            )],
            status: 400,
            code: "InvalidArgument",
        },
        RefusalCase {
            case: "an initiation asking to be charged for the request",
            method: "POST",
            uri: format!("/{bucket}/{key}?uploads"),
            headers: vec![("x-amz-request-payer", "requester".to_string())],
            status: 501,
            code: "NotImplemented",
        },
        RefusalCase {
            case: "an initiation naming a KMS key id beside no KMS algorithm",
            method: "POST",
            uri: format!("/{bucket}/{key}?uploads"),
            headers: vec![(
                "x-amz-server-side-encryption-aws-kms-key-id",
                "alias/emulator".to_string(),
            )],
            status: 400,
            code: "InvalidArgument",
        },
        RefusalCase {
            case: "an initiation whose tag set is one tag past an object's limit",
            method: "POST",
            uri: format!("/{bucket}/{key}?uploads"),
            headers: vec![("x-amz-tagging", too_many_tags)],
            status: 400,
            code: "BadRequest",
        },
        RefusalCase {
            case: "an initiation whose key is one byte past the published maximum",
            method: "POST",
            uri: format!("/{bucket}/{over_long_key}?uploads"),
            headers: Vec::new(),
            status: 400,
            code: "KeyTooLongError",
        },
        RefusalCase {
            case: "a read whose max-parts is not a count",
            method: "GET",
            uri: format!(
                "/{bucket}/{key}?uploadId={upload_id}&x-id=ListParts&max-parts=-1"
            ),
            headers: Vec::new(),
            status: 400,
            code: "InvalidArgument",
        },
    ];

    for case in cases {
        let headers: Vec<(&str, &str)> = case
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = send(&router, case.method, &case.uri, &headers).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            case.case, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            case.code,
            "case {:?}: expected the wire error code {}, got {:?}",
            case.case,
            case.code,
            answer.error_code()
        );
    }

    // Nothing the refusals named was recorded: the one upload really in progress is still
    // the only one a read can reach, and it is unchanged.
    let read = list_parts(&router, bucket, key, &upload_id, "").await;
    assert_eq!(
        read.status, 200,
        "the upload in progress must survive every refusal, got {} with body {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.element("StorageClass").as_deref(),
        Some("STANDARD"),
        "a refused initiation must not have touched the upload already in progress, got \
         {:?}",
        read.element("StorageClass")
    );
}

#[tokio::test]
async fn a_read_reports_the_page_bound_it_was_computed_under() {
    let router = router();
    let bucket = "multipart-page-bound-bucket";
    create_bucket(&router, bucket).await;
    let key = "paged/object.bin";
    let initiated = initiate(&router, bucket, key, &[]).await;
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    // The rows span the bound: the published default and maximum (1,000), a value inside
    // it, its low end, and one past it — which is answered with the maximum rather than
    // refused, the way `ListObjectsV2` answers a `max-keys` past its own page limit.
    let rows: &[(&str, Option<i32>, &str)] = &[
        ("no max-parts at all reports the default", None, "1000"),
        ("the low end of the bound", Some(0), "0"),
        ("a value inside the bound", Some(7), "7"),
        ("the published maximum", Some(1000), "1000"),
        ("one past the published maximum", Some(1001), "1000"),
        ("far past the published maximum", Some(100_000), "1000"),
    ];

    for (case, max_parts, expected) in rows {
        let query = match max_parts {
            Some(value) => format!("&max-parts={value}"),
            None => String::new(),
        };
        let read = list_parts(&router, bucket, key, &upload_id, &query).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from ListParts, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.element("MaxParts").as_deref(),
            Some(*expected),
            "case {case:?}: the read must report the bound the page was computed under, \
             got {:?}",
            read.element("MaxParts")
        );
    }
}
