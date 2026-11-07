// SPDX-License-Identifier: CC0-1.0

//! An OAC-signed CloudFront fetch of an SSE-KMS origin object, across CloudFront, Amazon S3
//! and KMS on one engine.
//!
//! RULE: an origin fetch a distribution's ORIGIN ACCESS CONTROL signs is made by CloudFront
//! as the `cloudfront.amazonaws.com` SERVICE PRINCIPAL, acting for that distribution — so
//! every authorization decision the origin's own service makes about it is decided for that
//! principal with `aws:SourceArn` naming the distribution, and not for the account whose
//! credential the emulator signs an in-process call with. The decision this suite is about
//! is the one a read of an SSE-KMS object needs beside the bucket's: serving such an object's
//! bytes performs `kms:Decrypt` on the customer-managed key it was stored under, so a key
//! whose policy does not grant that principal for that distribution refuses the fetch — S3
//! answers `403` with its own `AccessDenied` document and none of the object's bytes, which is
//! what the viewer is then served — and the very next fetch is served once the key policy
//! grants it. A service principal is no principal of the account: it holds no identity-based
//! policies and naming the account grants it nothing, which is why a key carrying only its
//! DEFAULT policy refuses the fetch while the account's own direct read of the same object is
//! served.
//!
//! Only a cross-service suite can pin this: the grant is written in KMS, the object and its
//! encryption are S3's, the distribution and its origin access control are CloudFront's, and
//! the subject is the identity the three of them agree the fetch was made as.
//! `cloudfront_oac_s3_origin_edge.rs` tables the BUCKET-policy half of the same rule (and
//! deliberately asserts nothing about the key policy); the S3 crate's own
//! `sse_kms_decrypt_authorization` suite tables the same key-policy decision for a caller
//! presenting a credential of its own.
//!
//! The rows span what the rule generalises over: where the key policy's grant comes from and
//! what it is scoped to (this distribution's ARN under `StringEquals`, an `ArnLike` pattern,
//! `kms:ViaService`, no condition at all, another distribution's ARN, another service
//! principal, an explicit `Deny` beside an allow, the default policy alone), which KMS action
//! it covers, and what encryption the object was stored under (a customer-managed CMK, an
//! SSE-KMS write naming no customer key, and SSE-S3, which asks KMS nothing). Each row is
//! driven through both
//! addressings a viewer reaches a distribution by — its published domain name in the host,
//! and that same name as the first path segment at the emulator's own endpoint — because what
//! the key policy decides cannot depend on which one the client could use.
//!
//! The requests are driven through the assembled router with request authentication off, as
//! every suite in this test crate drives it: what each setup call carries is a credential
//! scope, which is what names the Region. The VIEWER requests carry no credential at all,
//! which is exactly what makes the fetch's own identity the subject. Assembling the whole
//! router costs stack (see `boot_serve.rs`), so it is built — and the requests driven — on a
//! sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the origin host names and
/// the keys are resolved in.
const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version every route is under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// viewer request under it reaches the origin, so each row's answer is that fetch's own.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The service principal an origin access control makes CloudFront's origin request as, and
/// so the principal a bucket policy and a key policy grant to admit it.
const CLOUDFRONT_PRINCIPAL: &str = "cloudfront.amazonaws.com";

/// The code S3 refuses a read nothing granted with (Amazon S3 "Error responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// The emulator's OWN endpoint, which is the one address a client on this machine can reach:
/// a bare address, so it names no service and a request to it carries the distribution's own
/// published name in the PATH, where it cannot carry it in the host.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A response header's value, or the empty string.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The body as JSON (`Null` when it is empty or not JSON), for the awsJson services.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    /// The body as text — an object's bytes, or an error document.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The text of `<name>…</name>` in the body, or the empty string.
    fn element(&self, name: &str) -> String {
        let body = self.text();
        body.split_once(&format!("<{name}>"))
            .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
            .map_or(String::new(), |(text, _after)| text.to_string())
    }
}

/// The assembled emulator every call below is driven through, and the runtime it is driven
/// on: held as one value, so no call carries both.
struct Emulator {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

/// Dispatch one request to `host`, carrying the credential scope `signing_name` names when
/// it names one and nothing at all when it does not — which is how a VIEWER reaches an edge.
fn dispatch(
    router: &Emulator,
    signing_name: Option<&str>,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host);
    if let Some(signing_name) = signing_name {
        // The scope names the access key id, the service and the Region a handler builds its
        // ARNs in. The signature itself is not computed: this router runs with
        // authentication off.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = router.runtime.block_on(router.router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The PATH-STYLE S3 endpoint of this Region, which carries no bucket in its host: the
/// address a call naming its bucket in the PATH has to use.
fn regional_endpoint() -> String {
    format!("s3.{REGION}.amazonaws.com")
}

/// The host a bucket of this Region is addressed at VIRTUAL-HOSTED style, which is what an
/// S3 origin's `DomainName` carries.
fn origin_domain(bucket: &str) -> String {
    format!("{bucket}.s3.{REGION}.amazonaws.com")
}

/// One S3 call, addressed path-style.
fn s3(
    router: &Emulator,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    dispatch(
        router,
        Some("s3"),
        &regional_endpoint(),
        method,
        path,
        headers,
        body,
    )
}

/// One CloudFront control-plane call.
fn cloudfront(
    router: &Emulator,
    method: &str,
    path: &str,
    body: bytes::Bytes,
) -> Answer {
    dispatch(
        router,
        Some("cloudfront"),
        CLOUDFRONT_HOST,
        method,
        path,
        &[("content-type", "application/xml")],
        body,
    )
}

/// One `kms` awsJson1_1 call.
fn kms(
    router: &Emulator,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        Some("kms"),
        &format!("kms.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// `kms:CreateKey`, answering the new key's ARN.
fn create_key(router: &Emulator, case: &str) -> String {
    let created = kms(
        router,
        "TrentService.CreateKey",
        &json!({ "Description": "the CMK an origin object is stored under" }),
    );
    assert_eq!(
        created.status,
        200,
        "case {case:?}: CreateKey must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `kms:PutKeyPolicy`, replacing the key's own document.
fn put_key_policy(
    router: &Emulator,
    case: &str,
    key_arn: &str,
    policy: &str,
) {
    let stored = kms(
        router,
        "TrentService.PutKeyPolicy",
        &json!({ "KeyId": key_arn, "PolicyName": "default", "Policy": policy }),
    );
    assert_eq!(
        stored.status,
        200,
        "case {case:?}: PutKeyPolicy must succeed, got {} with {:?}",
        stored.status,
        stored.text()
    );
}

/// `s3:CreateBucket`.
fn create_bucket(
    router: &Emulator,
    case: &str,
    bucket: &str,
) {
    let created = s3(
        router,
        "PUT",
        &format!("/{bucket}"),
        &[],
        bytes::Bytes::new(),
    );
    assert_eq!(
        created.status,
        200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
}

/// `s3:PutBucketEncryption`, making SSE-KMS under `key_arn` the bucket's default — the
/// configuration an origin bucket holding encrypted objects carries.
fn put_bucket_encryption(
    router: &Emulator,
    case: &str,
    bucket: &str,
    key_arn: &str,
) {
    let configuration = format!(
        "<ServerSideEncryptionConfiguration><Rule>\
         <ApplyServerSideEncryptionByDefault>\
         <SSEAlgorithm>aws:kms</SSEAlgorithm><KMSMasterKeyID>{key_arn}</KMSMasterKeyID>\
         </ApplyServerSideEncryptionByDefault></Rule>\
         </ServerSideEncryptionConfiguration>"
    );
    let stored = s3(
        router,
        "PUT",
        &format!("/{bucket}?encryption"),
        &[("content-type", "application/xml")],
        bytes::Bytes::from(configuration),
    );
    assert!(
        (200..300).contains(&stored.status),
        "case {case:?}: PutBucketEncryption must succeed, got {} with {:?}",
        stored.status,
        stored.text()
    );
}

/// `s3:PutObject`, with the encryption headers the row names, asserting what the write
/// recorded for the object.
fn put_object(
    router: &Emulator,
    case: &str,
    bucket: &str,
    key: &str,
    body: &str,
    headers: &[(&str, &str)],
    expected_algorithm: &str,
) {
    let written = s3(
        router,
        "PUT",
        &format!("/{bucket}/{key}"),
        headers,
        bytes::Bytes::from(body.to_string()),
    );
    assert_eq!(
        written.status,
        200,
        "case {case:?}: PutObject({key:?}) must succeed, got {} with {:?}",
        written.status,
        written.text()
    );
    assert_eq!(
        written.header("x-amz-server-side-encryption"),
        expected_algorithm,
        "case {case:?}: expected the object stored under {expected_algorithm:?}, got {:?}",
        written.header("x-amz-server-side-encryption")
    );
}

/// `s3:PutBucketPolicy` granting the CloudFront principal the read of every object of
/// `bucket` for the distribution `distribution_arn` names — the document that makes the
/// origin fetch SIGNED, so what remains for a row to decide is the key policy alone.
fn grant_the_bucket(
    router: &Emulator,
    case: &str,
    bucket: &str,
    distribution_arn: &str,
) {
    let document = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\
         \"Sid\":\"AllowCloudFrontServicePrincipal\",\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{CLOUDFRONT_PRINCIPAL}\"}},\
         \"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::{bucket}/*\",\
         \"Condition\":{{\"StringEquals\":{{\"AWS:SourceArn\":\"{distribution_arn}\"}}}}}}]}}"
    );
    let stored = s3(
        router,
        "PUT",
        &format!("/{bucket}?policy"),
        &[("content-type", "application/json")],
        bytes::Bytes::from(document),
    );
    assert!(
        (200..300).contains(&stored.status),
        "case {case:?}: PutBucketPolicy must succeed, got {} with {:?}",
        stored.status,
        stored.text()
    );
}

/// `cloudfront:CreateOriginAccessControl` for an `s3` origin signed on every request,
/// answering the control's id.
fn create_origin_access_control(
    router: &Emulator,
    case: &str,
    name: &str,
) -> String {
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-control"),
        bytes::Bytes::from(format!(
            "<OriginAccessControlConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name><SigningProtocol>sigv4</SigningProtocol>\
             <SigningBehavior>always</SigningBehavior>\
             <OriginAccessControlOriginType>s3</OriginAccessControlOriginType>\
             </OriginAccessControlConfig>"
        )),
    );
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateOriginAccessControl must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.element("Id")
}

/// `cloudfront:CreateDistribution` over the S3 origin `bucket` with `control` attached,
/// answering the distribution's `(id, DomainName, ARN)`.
fn create_distribution(
    router: &Emulator,
    case: &str,
    caller_reference: &str,
    bucket: &str,
    control: &str,
) -> (String, String, String) {
    let created = cloudfront(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        bytes::Bytes::from(format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{caller_reference}</CallerReference>\
             <Origins><Quantity>1</Quantity><Items><Origin>\
             <Id>origin-bucket</Id><DomainName>{}</DomainName>\
             <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
             <OriginAccessControlId>{control}</OriginAccessControlId>\
             </Origin></Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
             <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
             </DefaultCacheBehavior>\
             <Comment>oac over an encrypted s3 origin</Comment>\
             <Enabled>true</Enabled></DistributionConfig>",
            origin_domain(bucket)
        )),
    );
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let (id, domain, arn) = (
        created.element("Id"),
        created.element("DomainName"),
        created.element("ARN"),
    );
    for (member, value) in [("Id", &id), ("DomainName", &domain), ("ARN", &arn)] {
        assert!(
            !value.is_empty(),
            "case {case:?}: the created distribution reports no {member}: {:?}",
            created.text()
        );
    }
    (id, domain, arn)
}

/// Assert through `GetDistribution` that the clock has deployed `id`.
fn assert_deployed(
    router: &Emulator,
    case: &str,
    id: &str,
) {
    let read = cloudfront(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        bytes::Bytes::new(),
    );
    assert_eq!(
        read.element("Status"),
        "Deployed",
        "case {case:?}: the distribution did not reach Deployed: {:?}",
        read.text()
    );
}

/// A key policy's statement allowing the ACCOUNT's own principals every KMS action — the
/// `Enable IAM User Permissions` statement a key's DEFAULT policy carries, which every row's
/// document keeps so the key stays manageable.
const ROOT_STATEMENT: &str = "{\"Sid\":\"Enable IAM User Permissions\",\"Effect\":\"Allow\",\
     \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Action\":\"kms:*\",\
     \"Resource\":\"*\"}";

/// A key policy of [`ROOT_STATEMENT`] plus the statements a row adds.
fn key_policy(statements: &[String]) -> String {
    let mut all = vec![ROOT_STATEMENT.to_string()];
    all.extend(statements.iter().cloned());
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        all.join(",")
    )
}

/// One key-policy statement about `principal` and `actions`, with the condition clause the
/// row names.
fn statement(effect: &str, principal: &str, actions: &str, condition: Option<&str>) -> String {
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":{{\"Service\":\"{principal}\"}},\
         \"Action\":{actions},\"Resource\":\"*\"{condition}}}"
    )
}

/// What encryption a row's object is written under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encryption {
    /// SSE-KMS under the row's own customer-managed CMK — the key whose policy decides.
    CustomerKey,
    /// SSE-KMS naming no key id, so the write names no customer-managed key and the read
    /// consults no key policy of this account's: what this row holds the rule to is that a
    /// fetch the key policy of a CUSTOMER key does not decide is left exactly as it was. What
    /// AWS does for an object under the AWS managed key for Amazon S3 behind an origin access
    /// control is a question of that key's own (uneditable) policy, which this emulator does
    /// not model and this suite does not claim.
    AwsManagedKey,
    /// SSE-S3, which asks KMS nothing at all.
    SseS3,
}

/// What the viewer must be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The origin's object, byte for byte.
    TheObject,
    /// S3's own refusal of the fetch: `403` with its `AccessDenied` document and no bytes.
    Refused,
}

/// One row: a bucket, an object, a distribution over it under an origin access control, and
/// the KEY POLICY the object's key carries — and what the viewer is then served.
struct Row {
    case: &'static str,
    /// This row's own bucket, so one row's grants cannot admit another's read.
    bucket: &'static str,
    key: &'static str,
    body: &'static str,
    encryption: Encryption,
    /// The key policy statements beside [`ROOT_STATEMENT`], built from the distribution's
    /// own ARN once it is known; `None` leaves the key its DEFAULT policy.
    statements: Option<fn(&str) -> Vec<String>>,
    expected: Expected,
}

/// The table: one row per shape of key-policy grant, action and object encryption.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a key policy naming no CloudFront principal at all",
            bucket: "oac-kms-default-policy",
            key: "reports/default-policy.csv",
            body: "the sse-kms object",
            encryption: Encryption::CustomerKey,
            statements: None,
            expected: Expected::Refused,
        },
        Row {
            case: "the decrypt granted to the CloudFront principal for this distribution",
            bucket: "oac-kms-granted",
            key: "reports/granted.csv",
            body: "the granted sse-kms object",
            encryption: Encryption::CustomerKey,
            statements: Some(|arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "[\"kms:Decrypt\",\"kms:GenerateDataKey*\"]",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                    )),
                )]
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant scoped by an ArnLike pattern this distribution matches",
            bucket: "oac-kms-arnlike",
            key: "arnlike/index.html",
            body: "<p>arnlike</p>",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "\"kms:Decrypt\"",
                    Some(
                        "{\"ArnLike\":{\"aws:SourceArn\":\
                         \"arn:aws:cloudfront::*:distribution/*\"}}",
                    ),
                )]
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant to the CloudFront principal with no condition at all",
            bucket: "oac-kms-unscoped",
            key: "deep/nested/page.txt",
            body: "unscoped grant",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "\"kms:Decrypt\"",
                    None,
                )]
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant confined to this service's own use of the key",
            bucket: "oac-kms-viaservice",
            key: "viaservice/plain.txt",
            body: "through s3 only",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "\"kms:Decrypt\"",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"kms:ViaService\":\"s3.{REGION}.amazonaws.com\"}}}}"
                    )),
                )]
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant scoped to ANOTHER distribution's ARN",
            bucket: "oac-kms-other-distribution",
            key: "other-distribution/plain.txt",
            body: "not for this distribution",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "\"kms:Decrypt\"",
                    Some(
                        "{\"StringEquals\":{\"AWS:SourceArn\":\
                         \"arn:aws:cloudfront::000000000000:distribution/E2NOTTHISONE\"}}",
                    ),
                )]
            }),
            expected: Expected::Refused,
        },
        Row {
            case: "a grant to another service principal",
            bucket: "oac-kms-other-principal",
            key: "other-principal/plain.txt",
            body: "granted to someone else",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![statement(
                    "Allow",
                    "lambda.amazonaws.com",
                    "\"kms:Decrypt\"",
                    None,
                )]
            }),
            expected: Expected::Refused,
        },
        Row {
            case: "a grant of another KMS action than the decrypt a read performs",
            bucket: "oac-kms-other-action",
            key: "other-action/plain.txt",
            body: "encrypt but not decrypt",
            encryption: Encryption::CustomerKey,
            statements: Some(|arn| {
                vec![statement(
                    "Allow",
                    CLOUDFRONT_PRINCIPAL,
                    "[\"kms:Encrypt\",\"kms:GenerateDataKey*\"]",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                    )),
                )]
            }),
            expected: Expected::Refused,
        },
        Row {
            case: "an explicit Deny of the CloudFront principal beside the allow",
            bucket: "oac-kms-explicit-deny",
            key: "explicit-deny/plain.txt",
            body: "denied outright",
            encryption: Encryption::CustomerKey,
            statements: Some(|_arn| {
                vec![
                    statement("Allow", CLOUDFRONT_PRINCIPAL, "\"kms:Decrypt\"", None),
                    statement("Deny", CLOUDFRONT_PRINCIPAL, "\"kms:Decrypt\"", None),
                ]
            }),
            expected: Expected::Refused,
        },
        Row {
            case: "an SSE-KMS object naming no customer-managed key",
            bucket: "oac-kms-aws-managed",
            key: "managed.txt",
            body: "the aws managed key object",
            encryption: Encryption::AwsManagedKey,
            statements: None,
            expected: Expected::TheObject,
        },
        Row {
            case: "an SSE-S3 object, which asks KMS nothing",
            bucket: "oac-kms-sse-s3",
            key: "sse-s3/plain.txt",
            body: "the sse-s3 object",
            encryption: Encryption::SseS3,
            statements: None,
            expected: Expected::TheObject,
        },
    ]
}

/// What one row left behind for the viewer phase.
struct Prepared {
    case: &'static str,
    key: &'static str,
    body: &'static str,
    bucket: &'static str,
    domain: String,
    id: String,
    expected: Expected,
}

#[test]
fn an_oac_signed_fetch_of_an_sse_kms_object_is_decided_by_the_key_policy() {
    std::thread::Builder::new()
        .name("cloudfront-oac-kms-s3-edge".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut assembled = build_router(engine);
            assembled.set_request_authentication(false);
            let router = Emulator {
                router: assembled,
                runtime: tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .expect("a current-thread runtime"),
            };

            let mut prepared: Vec<Prepared> = Vec::new();
            for (index, row) in rows().into_iter().enumerate() {
                create_bucket(&router, row.case, row.bucket);
                // The object's own encryption, and the key whose policy decides its read.
                let key_arn = match row.encryption {
                    Encryption::CustomerKey => {
                        let key_arn = create_key(&router, row.case);
                        assert!(
                            !key_arn.is_empty(),
                            "case {:?}: CreateKey reports no key ARN",
                            row.case
                        );
                        // The bucket's own default encryption, which is what a client
                        // configures and what an object written without headers inherits.
                        put_bucket_encryption(
                            &router, row.case, row.bucket, &key_arn,
                        );
                        Some(key_arn)
                    }
                    Encryption::AwsManagedKey | Encryption::SseS3 => None,
                };
                let (headers, algorithm): (Vec<(&str, &str)>, &str) = match row.encryption {
                    Encryption::CustomerKey => (Vec::new(), "aws:kms"),
                    Encryption::AwsManagedKey => (
                        vec![("x-amz-server-side-encryption", "aws:kms")],
                        "aws:kms",
                    ),
                    Encryption::SseS3 => (
                        vec![("x-amz-server-side-encryption", "AES256")],
                        "AES256",
                    ),
                };
                put_object(
                    &router,
                    row.case,
                    row.bucket,
                    row.key,
                    row.body,
                    &headers,
                    algorithm,
                );
                let control = create_origin_access_control(
                    &router,
                    row.case,
                    &format!("oac-kms-{index}"),
                );
                let (id, domain, arn) = create_distribution(
                    &router,
                    row.case,
                    &format!("oac-kms-reference-{index}"),
                    row.bucket,
                    &control,
                );
                // Both documents are written AFTER the distribution exists, because each is
                // scoped to its own ARN — the order a client has to follow.
                grant_the_bucket(&router, row.case, row.bucket, &arn);
                if let (Some(statements), Some(key_arn)) = (row.statements, key_arn.as_deref())
                {
                    put_key_policy(
                        &router,
                        row.case,
                        key_arn,
                        &key_policy(&statements(&arn)),
                    );
                }
                prepared.push(Prepared {
                    case: row.case,
                    key: row.key,
                    body: row.body,
                    bucket: row.bucket,
                    domain,
                    id,
                    expected: row.expected,
                });
            }

            // One wait for every row: the configurations were all submitted above, and the
            // clock owes each of them the same propagation.
            router
                .runtime
                .block_on(async { tokio::time::sleep(Duration::from_secs(3)).await });

            for row in &prepared {
                assert_deployed(&router, row.case, &row.id);
                // Both addressings a viewer reaches this edge by: the distribution's own
                // domain name in the host, and that same name as the first path segment at
                // the emulator's own endpoint, which is where a client that resolves no
                // `cloudfront.net` name carries it. What the key policy decides cannot depend
                // on which one was used.
                for (addressing, host, path) in [
                    (
                        "the distribution's domain name",
                        row.domain.clone(),
                        format!("/{}", row.key),
                    ),
                    (
                        "that domain name in the path, at the emulator's own endpoint",
                        EMULATOR_HOST.to_string(),
                        format!("/{}/{}", row.domain, row.key),
                    ),
                ] {
                    let case = format!("{} / {addressing}", row.case);
                    let case = case.as_str();
                    let viewed = dispatch(
                        &router,
                        None,
                        &host,
                        "GET",
                        &path,
                        &[],
                        bytes::Bytes::new(),
                    );
                    match row.expected {
                        Expected::TheObject => {
                            assert_eq!(
                                viewed.status,
                                200,
                                "case {case:?}: expected the origin's object through the edge, \
                                 got {} with {:?}",
                                viewed.status,
                                viewed.text()
                            );
                            assert_eq!(
                                viewed.text(),
                                row.body,
                                "case {case:?}: expected the ORIGIN's own bytes, got {:?}",
                                viewed.text()
                            );
                        }
                        Expected::Refused => {
                            assert_eq!(
                                viewed.status,
                                403,
                                "case {case:?}: expected the origin's refusal through the edge, \
                                 got {} with {:?}",
                                viewed.status,
                                viewed.text()
                            );
                            assert_eq!(
                                viewed.element("Code"),
                                ACCESS_DENIED,
                                "case {case:?}: expected S3's own {ACCESS_DENIED} document, got \
                                 {:?}",
                                viewed.text()
                            );
                            assert!(
                                !viewed.text().contains(row.body),
                                "case {case:?}: a refused read must not carry the object's \
                                 bytes, got {:?}",
                                viewed.text()
                            );
                        }
                    }
                }
                // The identity an OAC fetch is made as belongs to that fetch alone: the
                // ACCOUNT's own direct read of the very same object is decided against the
                // credential it presents, which the key's default policy delegates to this
                // account — so it is served whatever the row's key policy said about
                // CloudFront, and a direct read carrying a request id of its own invention is
                // served the same way.
                for (leg, extra) in [
                    ("a direct read", Vec::new()),
                    (
                        "a direct read naming a request id of its own",
                        vec![("x-amz-cf-id", "0123456789abcdef0123456789abcdef")],
                    ),
                ] {
                    let read = s3(
                        &router,
                        "GET",
                        &format!("/{}/{}", row.bucket, row.key),
                        &extra,
                        bytes::Bytes::new(),
                    );
                    assert_eq!(
                        read.status,
                        200,
                        "case {:?}: {leg} by the account must be served, got {} with {:?}",
                        row.case,
                        read.status,
                        read.text()
                    );
                    assert_eq!(
                        read.text(),
                        row.body,
                        "case {:?}: {leg} by the account must answer the object's bytes, got \
                         {:?}",
                        row.case,
                        read.text()
                    );
                }
            }

            // The very next fetch is served once the key policy grants it, and refused again
            // when that grant is taken away: the decision is the document in force at the
            // moment of the fetch, not a verdict reached when the distribution was created.
            let refused = prepared
                .iter()
                .find(|row| row.expected == Expected::Refused)
                .expect("the table carries a refused row");
            let key_arn = create_key(&router, refused.case);
            let bucket = "oac-kms-rescoped";
            let key = "rescoped.csv";
            let body = "the rescoped object";
            create_bucket(&router, "rescoping the key policy", bucket);
            put_bucket_encryption(
                &router,
                "rescoping the key policy",
                bucket,
                &key_arn,
            );
            put_object(
                &router,
                "rescoping the key policy",
                bucket,
                key,
                body,
                &[],
                "aws:kms",
            );
            let control = create_origin_access_control(
                &router,
                "rescoping the key policy",
                "oac-kms-rescoped",
            );
            let (id, domain, arn) = create_distribution(
                &router,
                "rescoping the key policy",
                "oac-kms-rescoped-reference",
                bucket,
                &control,
            );
            grant_the_bucket(&router, "rescoping the key policy", bucket, &arn);
            router
                .runtime
                .block_on(async { tokio::time::sleep(Duration::from_secs(3)).await });
            assert_deployed(&router, "rescoping the key policy", &id);

            let granting = key_policy(&[statement(
                "Allow",
                CLOUDFRONT_PRINCIPAL,
                "\"kms:Decrypt\"",
                Some(&format!(
                    "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                )),
            )]);
            let elsewhere = key_policy(&[statement(
                "Allow",
                CLOUDFRONT_PRINCIPAL,
                "\"kms:Decrypt\"",
                Some(
                    "{\"StringEquals\":{\"AWS:SourceArn\":\
                     \"arn:aws:cloudfront::000000000000:distribution/E2NOTTHISONE\"}}",
                ),
            )]);
            for (case, document, expected) in [
                ("before the key policy grants it", None, Expected::Refused),
                (
                    "once the key policy grants this distribution",
                    Some(&granting),
                    Expected::TheObject,
                ),
                (
                    "once that grant is rescoped away",
                    Some(&elsewhere),
                    Expected::Refused,
                ),
            ] {
                if let Some(document) = document {
                    put_key_policy(&router, case, &key_arn, document);
                }
                let viewed = dispatch(
                    &router,
                    None,
                    &domain,
                    "GET",
                    &format!("/{key}"),
                    &[],
                    bytes::Bytes::new(),
                );
                match expected {
                    Expected::TheObject => {
                        assert_eq!(
                            viewed.status,
                            200,
                            "case {case:?}: expected the object through the edge, got {} with \
                             {:?}",
                            viewed.status,
                            viewed.text()
                        );
                        assert_eq!(
                            viewed.text(),
                            body,
                            "case {case:?}: expected the ORIGIN's own bytes, got {:?}",
                            viewed.text()
                        );
                    }
                    Expected::Refused => {
                        assert_eq!(
                            viewed.status,
                            403,
                            "case {case:?}: expected the refusal through the edge, got {} with \
                             {:?}",
                            viewed.status,
                            viewed.text()
                        );
                        assert_eq!(
                            viewed.element("Code"),
                            ACCESS_DENIED,
                            "case {case:?}: expected S3's own {ACCESS_DENIED} document, got {:?}",
                            viewed.text()
                        );
                        assert!(
                            !viewed.text().contains(body),
                            "case {case:?}: a refused read must not carry the object's bytes, \
                             got {:?}",
                            viewed.text()
                        );
                    }
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite thread");
}
