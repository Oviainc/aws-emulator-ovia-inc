// SPDX-License-Identifier: CC0-1.0

//! A CloudFront distribution serving a PRIVATE Amazon S3 origin through an ORIGIN ACCESS
//! CONTROL, end to end over HTTP across the two services on one engine.
//!
//! RULE: an origin request the edge makes for a distribution whose origin carries an origin
//! access control is made ON BEHALF OF that distribution — the origin's own resource policy
//! decides it, for the `cloudfront.amazonaws.com` service principal and scoped by
//! `aws:SourceArn` to the distribution the request was made for, and a fetch that document
//! grants is SIGNED, which is what an origin that serves no unsigned request answers it for.
//! Whatever the origin then answers — the object's own bytes, or its own refusal — is what
//! the viewer is served. A fetch the document does NOT grant is not signed for, so the origin
//! answers it as it answers a caller presenting no credential, in its own vocabulary.
//!
//! Only a cross-service suite can pin this: the cloudfront crate's own `edge_data_plane`
//! suites exercise the edge against an origin that needs no credential, and the S3 crate's
//! `anonymous_object_reads` pins what S3 serves a caller that presents none — neither can
//! state that a distribution's edge domain serves the bytes of an object in a private bucket
//! that has granted CloudFront, and nothing else.
//!
//! The VIEWER requests are driven through a router with request authentication ON, which is
//! what makes the signature the subject: a viewer presents no credential (CloudFront verifies
//! none), so the only thing that can make S3 serve the origin fetch is the signature
//! CloudFront puts on it. The setup calls go through a second router over the SAME engine with
//! authentication off, so each one carries a credential scope (which is what names the Region)
//! without a real signature.
//!
//! The rows span what the rule generalises over: where the grant comes from and what it is
//! scoped to (this distribution's ARN under `StringEquals`, an `ArnLike` pattern, no condition
//! at all, another distribution's ARN, another service principal, another prefix of the same
//! bucket, no policy at all), the `SigningBehavior` the control names (`always`, `never`), the
//! server-side encryption the object was stored under (SSE-S3 and SSE-KMS, the two an origin
//! bucket mixes), the object's key form (a bare name, a nested prefix) and its bytes.
//!
//! What the encryption rows pin is the EDGE's half: the bytes an object was stored with are
//! served through the distribution whatever encryption S3 recorded for it. They do not pin
//! the KMS half of a read of an SSE-KMS object — on AWS, decrypting it also needs the CMK's
//! own key policy to permit the reader, and no read in this emulator consults a key policy
//! (S3 never asks KMS), which is a gap this suite deliberately does not assert either way.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every setup call is signed for, and so the Region the origin host names.
pub(crate) const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version every route is under.
pub(crate) const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
pub(crate) const API_VERSION: &str = "2020-05-31";

/// The account every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// viewer request under it reaches the origin, so each row's answer is the origin's own.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The service principal an origin access control makes CloudFront's origin request as.
pub(crate) const CLOUDFRONT_PRINCIPAL: &str = "cloudfront.amazonaws.com";

/// The code and message S3 refuses a read nothing granted with (Amazon S3 "Error responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// What one call answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: ::http::HeaderMap,
    pub(crate) body: Bytes,
}

impl Answer {
    /// A response header's value.
    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The body as text — the object's bytes, or an `<Error>` document.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The text of `<name>…</name>` in the body, or the empty string.
    pub(crate) fn element(&self, name: &str) -> String {
        let body = self.text();
        body.split_once(&format!("<{name}>"))
            .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
            .map_or(String::new(), |(text, _after)| text.to_string())
    }
}

/// The two services this suite exercises, mounted on `engine` exactly as the router mounts
/// them, with request authentication as `authenticated` says.
pub(crate) fn emulator(
    engine: &Arc<engine::InMemoryEngine>,
    authenticated: bool,
) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(engine),
    ));
    router
}

/// One setup call, addressed to `host` and carrying a credential scope naming `signing_name`
/// — which is what routes it and what names the Region. The signature is not computed: the
/// setup router runs with authentication off.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed setup request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One VIEWER request: addressed to the distribution's own domain name, with no credential of
/// any kind, exactly as a browser reaches a distribution.
pub(crate) async fn view(router: &MultiServiceRouter, domain: &str, path: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", domain)
        .body(Bytes::new())
        .expect("a well-formed viewer request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The host a bucket of this Region is addressed at VIRTUAL-HOSTED style, which is what an S3
/// origin's `DomainName` carries and what the edge's own origin fetch is addressed to.
pub(crate) fn origin_domain(bucket: &str) -> String {
    format!("{bucket}.s3.{REGION}.amazonaws.com")
}

/// The PATH-STYLE endpoint of this Region, which carries no bucket in its host: the address
/// a call that names its bucket in the PATH has to use, since a virtual-hosted host names the
/// bucket itself and leaves the path to be the key (the core's `VirtualHostAddressing`).
pub(crate) fn regional_endpoint() -> String {
    format!("s3.{REGION}.amazonaws.com")
}

/// `s3:CreateBucket`.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let created = call(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
}

/// `s3:PutObject` with the encryption headers `encryption` names.
pub(crate) async fn put_object(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
    body: &str,
    encryption: &[(&str, &str)],
) {
    let written = call(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}/{key}"),
        encryption,
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        written.status,
        200,
        "case {case:?}: PutObject({key:?}) must succeed, got {} with {:?}",
        written.status,
        written.text()
    );
}

/// `s3:PutBucketPolicy`.
pub(crate) async fn put_bucket_policy(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    policy: &str,
) {
    let stored = call(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}?policy"),
        &[("content-type", "application/json")],
        Bytes::from(policy.to_string()),
    )
    .await;
    assert!(
        (200..300).contains(&stored.status),
        "case {case:?}: PutBucketPolicy must succeed, got {} with {:?}",
        stored.status,
        stored.text()
    );
}

/// `cloudfront:CreateOriginAccessControl` for an `s3` origin signed under `signing_behavior`,
/// answering the control's id.
pub(crate) async fn create_origin_access_control(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    signing_behavior: &str,
) -> String {
    let body = format!(
        "<OriginAccessControlConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <SigningProtocol>sigv4</SigningProtocol>\
         <SigningBehavior>{signing_behavior}</SigningBehavior>\
         <OriginAccessControlOriginType>s3</OriginAccessControlOriginType>\
         </OriginAccessControlConfig>"
    );
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/origin-access-control"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateOriginAccessControl must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    assert!(
        !id.is_empty(),
        "case {case:?}: the created origin access control reports no Id: {:?}",
        created.text()
    );
    id
}

/// `cloudfront:CreateDistribution` over the S3 origin `bucket` with the origin access control
/// `control` attached and the cache policy `cache_policy` on its default behaviour, answering
/// the distribution's `(id, DomainName, ARN)`.
pub(crate) async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    bucket: &str,
    control: &str,
    cache_policy: &str,
) -> (String, String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>origin-bucket</Id><DomainName>{}</DomainName>\
         <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
         <OriginAccessControlId>{control}</OriginAccessControlId>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{cache_policy}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>oac over a private s3 origin</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        origin_domain(bucket)
    );
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
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
pub(crate) async fn assert_deployed(router: &MultiServiceRouter, case: &str, id: &str) {
    let read = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        read.element("Status"),
        "Deployed",
        "case {case:?}: the distribution did not reach Deployed: {:?}",
        read.text()
    );
}

/// A bucket policy granting `principal` the action `action` on `resource`, with the condition
/// clause `condition` when the row names one — the document an origin bucket carries for an
/// origin access control.
pub(crate) fn policy(
    principal: &str,
    action: &str,
    resource: &str,
    condition: Option<&str>,
) -> String {
    let condition = condition
        .map(|clause| format!(",\"Condition\":{clause}"))
        .unwrap_or_default();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"AllowCloudFrontServicePrincipal\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"Service\":\"{principal}\"}},\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"{condition}}}]}}"
    )
}

/// What one row expects the viewer to be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The origin's object, byte for byte.
    TheObject,
    /// The origin's own refusal: `403` with its `AccessDenied` document and none of the
    /// object's bytes.
    OriginRefusal,
}

/// One row: a private bucket holding one object, a distribution over it under an origin
/// access control, and the document the bucket carries — and what the viewer is then served.
struct Row {
    case: &'static str,
    /// The bucket, which is this row's own so one row's grants cannot admit another's read.
    bucket: &'static str,
    key: &'static str,
    body: &'static str,
    /// The server-side-encryption headers the object is written with.
    encryption: Vec<(&'static str, &'static str)>,
    /// The control's `SigningBehavior`.
    signing_behavior: &'static str,
    /// The bucket policy, built from the distribution's own ARN once it is known, or `None`
    /// for a bucket carrying no policy at all.
    document: Option<fn(&str) -> String>,
    expected: Expected,
}

/// The CMK an SSE-KMS object names: the Amazon Web Services managed key for Amazon S3,
/// named by the predefined Amazon Web Services alias that "exists in every account and
/// Region" and that KMS associates with its key the first time something asks for it — so
/// the write below needs no `CreateKey` of its own, and no CUSTOMER key policy decides these
/// reads, which is what an SSE-KMS row here is for: it pins the EDGE's half alone, the
/// object's own bytes served through the distribution exactly as for an SSE-S3 object. The
/// algorithm and key id a write recorded are members the S3 crate's own suites pin, and which
/// callers a customer key lets read its objects is that crate's `sse_kms_decrypt_authorization`
/// suite.
fn kms_key_reference() -> String {
    "alias/aws/s3".to_string()
}

/// The table: one row per shape of grant, signing behaviour and object.
fn rows() -> Vec<Row> {
    let sse_s3: Vec<(&'static str, &'static str)> =
        vec![("x-amz-server-side-encryption", "AES256")];
    vec![
        Row {
            case: "a grant scoped to this distribution, over an SSE-S3 object",
            bucket: "oac-edge-granted-sse-s3",
            key: "plain.txt",
            body: "the sse-s3 object",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: Some(|arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-granted-sse-s3/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                    )),
                )
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "the same grant over an SSE-KMS object under a nested prefix",
            bucket: "oac-edge-granted-sse-kms",
            key: "reports/2026/held.csv",
            body: "the sse-kms object",
            encryption: Vec::new(),
            signing_behavior: "always",
            document: Some(|arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-granted-sse-kms/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                    )),
                )
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant scoped by an ArnLike pattern this distribution matches",
            bucket: "oac-edge-granted-arnlike",
            key: "index.html",
            body: "<p>arnlike</p>",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: Some(|_arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:Get*",
                    "arn:aws:s3:::oac-edge-granted-arnlike/*",
                    Some(
                        "{\"ArnLike\":{\"aws:SourceArn\":\
                         \"arn:aws:cloudfront::*:distribution/*\"}}",
                    ),
                )
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant to the CloudFront principal with no condition at all",
            bucket: "oac-edge-granted-unscoped",
            key: "deep/nested/page.txt",
            body: "unscoped grant",
            encryption: Vec::new(),
            signing_behavior: "always",
            document: Some(|_arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-granted-unscoped/*",
                    None,
                )
            }),
            expected: Expected::TheObject,
        },
        Row {
            case: "a grant scoped to ANOTHER distribution's ARN",
            bucket: "oac-edge-other-distribution",
            key: "plain.txt",
            body: "not for this distribution",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: Some(|_arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-other-distribution/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\
                         \"arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E2NOTTHISONE\"}}}}"
                    )),
                )
            }),
            expected: Expected::OriginRefusal,
        },
        Row {
            case: "a grant to another service principal",
            bucket: "oac-edge-other-principal",
            key: "plain.txt",
            body: "granted to someone else",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: Some(|_arn| {
                policy(
                    "lambda.amazonaws.com",
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-other-principal/*",
                    None,
                )
            }),
            expected: Expected::OriginRefusal,
        },
        Row {
            case: "a grant over another prefix of the same bucket",
            bucket: "oac-edge-other-prefix",
            key: "private/held.txt",
            body: "outside the granted prefix",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: Some(|_arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-other-prefix/public/*",
                    None,
                )
            }),
            expected: Expected::OriginRefusal,
        },
        Row {
            case: "a bucket carrying no policy at all",
            bucket: "oac-edge-no-policy",
            key: "plain.txt",
            body: "nothing granted",
            encryption: sse_s3.clone(),
            signing_behavior: "always",
            document: None,
            expected: Expected::OriginRefusal,
        },
        Row {
            case: "a control whose SigningBehavior is never, over a granting bucket",
            bucket: "oac-edge-never-signs",
            key: "plain.txt",
            body: "signed by nothing",
            encryption: sse_s3,
            signing_behavior: "never",
            document: Some(|arn| {
                policy(
                    CLOUDFRONT_PRINCIPAL,
                    "s3:GetObject",
                    "arn:aws:s3:::oac-edge-never-signs/*",
                    Some(&format!(
                        "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                    )),
                )
            }),
            expected: Expected::OriginRefusal,
        },
    ]
}

/// What one row left behind for the viewer phase.
struct Prepared {
    case: &'static str,
    key: &'static str,
    body: &'static str,
    domain: String,
    id: String,
    expected: Expected,
}

#[tokio::test]
async fn an_origin_access_control_serves_a_private_s3_origin_through_the_edge() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The setup plane: authentication off, so each call carries a credential scope without a
    // computed signature. The VIEWER plane below is the authenticating one.
    let setup = emulator(&engine, false);
    let viewers = emulator(&engine, true);

    let mut prepared: Vec<Prepared> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        create_bucket(&setup, row.case, row.bucket).await;
        let mut encryption = row.encryption.clone();
        let key_arn = kms_key_reference();
        if encryption.is_empty() {
            // An SSE-KMS object: the algorithm and the CMK the write names, which is the
            // other encryption an origin bucket holds objects under.
            encryption.push(("x-amz-server-side-encryption", "aws:kms"));
        }
        let mut headers: Vec<(&str, &str)> = encryption;
        if headers
            .iter()
            .any(|(_name, value)| *value == "aws:kms")
        {
            headers.push((
                "x-amz-server-side-encryption-aws-kms-key-id",
                key_arn.as_str(),
            ));
        }
        put_object(&setup, row.case, row.bucket, row.key, row.body, &headers).await;
        let control = create_origin_access_control(
            &setup,
            row.case,
            &format!("oac-edge-{index}"),
            row.signing_behavior,
        )
        .await;
        let (id, domain, arn) = create_distribution(
            &setup,
            row.case,
            &format!("oac-edge-reference-{index}"),
            row.bucket,
            &control,
            CACHING_DISABLED,
        )
        .await;
        // The policy is written AFTER the distribution exists, because it is scoped to that
        // distribution's own ARN — the order a client has to follow.
        if let Some(document) = row.document {
            put_bucket_policy(&setup, row.case, row.bucket, &document(&arn)).await;
        }
        prepared.push(Prepared {
            case: row.case,
            key: row.key,
            body: row.body,
            domain,
            id,
            expected: row.expected,
        });
    }

    // One wait for every row: the configurations were all submitted above, and the clock owes
    // each of them the same propagation.
    ::tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;

    for row in &prepared {
        assert_deployed(&setup, row.case, &row.id).await;
        let viewed = view(&viewers, &row.domain, &format!("/{}", row.key)).await;
        match row.expected {
            Expected::TheObject => {
                assert_eq!(
                    viewed.status,
                    200,
                    "case {:?}: expected the origin's object through the edge, got {} with \
                     {:?}",
                    row.case,
                    viewed.status,
                    viewed.text()
                );
                assert_eq!(
                    viewed.text(),
                    row.body,
                    "case {:?}: expected the ORIGIN's own bytes, got {:?}",
                    row.case,
                    viewed.text()
                );
                assert_eq!(
                    viewed.header("x-cache"),
                    "Miss from cloudfront",
                    "case {:?}: expected the answer to be the origin's, got X-Cache {:?}",
                    row.case,
                    viewed.header("x-cache")
                );
            }
            Expected::OriginRefusal => {
                assert_eq!(
                    viewed.status,
                    403,
                    "case {:?}: expected the origin's refusal through the edge, got {} with \
                     {:?}",
                    row.case,
                    viewed.status,
                    viewed.text()
                );
                assert_eq!(
                    viewed.element("Code"),
                    ACCESS_DENIED,
                    "case {:?}: expected the origin's own {ACCESS_DENIED} document, got {:?}",
                    row.case,
                    viewed.text()
                );
                assert!(
                    !viewed.text().contains(row.body),
                    "case {:?}: a refused read must not carry the object's bytes, got {:?}",
                    row.case,
                    viewed.text()
                );
            }
        }
        // Every edge answer, served or refused, identifies the request it answers.
        assert!(
            !viewed.header("x-amz-cf-id").is_empty(),
            "case {:?}: the edge must stamp an X-Amz-Cf-Id, got none (status {})",
            row.case,
            viewed.status
        );
    }
}
