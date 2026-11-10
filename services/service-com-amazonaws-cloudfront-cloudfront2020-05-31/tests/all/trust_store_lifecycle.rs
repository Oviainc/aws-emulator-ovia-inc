// SPDX-License-Identifier: CC0-1.0

//! The CloudFront trust store control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. Amazon S3 is MOUNTED beside CloudFront on the same engine, because a trust
//! store is built from an object that store holds: the bundle `CreateTrustStore` reads is
//! one this suite really staged with `PutObject`, and the certificates in it are real
//! self-signed X.509 certificates this suite generated.
//!
//! The rules under test:
//!
//! 1. a trust store is PERSISTED by the call that creates it, which MINTS its identity (an
//!    id, the ARN built from that id and a version token) and reads
//!    `NumberOfCaCertificates` out of the bundle the request named — so a later
//!    `GetTrustStore` answers from that record, whether it is addressed by the id, the name
//!    or the ARN;
//! 2. a trust store CloudFront has just accepted is `pending`, and the CLOCK — not a later
//!    call — owes it `active`;
//! 3. a create whose CA certificates bundle could not be READ is refused and stores
//!    nothing — `InvalidArgument` where the source addresses no object or its bytes hold no
//!    certificate, `EntityNotFound` where the object store holds nothing at the address — so
//!    a trust store never reports certificates that were never read;
//! 4. a name identifies one trust store for its whole life, so a repeated name is refused
//!    with `EntityAlreadyExists` and leaves the trust store holding it untouched;
//! 5. an identifier no trust store answers to is `EntityNotFound`, whatever its form;
//! 6. the tags a create carries are the tags the tagging surface reports for the trust
//!    store's ARN;
//! 7. where NOTHING answers the bundle read — an emulator assembled with no object store at
//!    all — the create answers the scaffold's 501 naming that condition rather than a trust
//!    store whose certificates were never counted, or a refusal blaming the request.
//!
//! The table rows differ along everything these rules generalise over: how many
//! certificates the bundle holds, whether the create names the OCSP member at all and which
//! value it names, whether it carries tags, the shape of the object key, and — for the
//! refusals — which half of the bundle source cannot be read and in which way (down to an
//! object holding no bytes at all).

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE, CA_CERTIFICATES_BUNDLE_UNREAD,
    INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE, TRUST_STORE_ID_PREFIX, TRUST_STORE_NOT_FOUND_MESSAGE,
    TRUST_STORE_STATUS_ACTIVE, TRUST_STORE_STATUS_PENDING,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::distribution_lifecycle::{call, Answer, Node, ACCOUNT_ID, API_VERSION};
use crate::distribution_tags::{list_tags_for_resource, reported_tags, ReportedTags};
use crate::distributions_by_web_acl_id::encode_label;
use crate::key_value_store_lifecycle::{escape, is_version_token, member};

/// The Region the bundles below are staged in, and so the Region every bundle source names.
const REGION: &str = "us-east-1";

/// The bucket this suite stages its bundles in.
const BUNDLE_BUCKET: &str = "cloudfront-trust-store-bundles";

// ---------------------------------------------------------------------------
// The emulator: CloudFront plus the object store its trust stores read
// ---------------------------------------------------------------------------

/// CloudFront and Amazon S3 over ONE shared engine, with a callback into the same router so
/// this suite stages bundles through Amazon S3's own API — the very seam the handler reads
/// them over.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(
                std::sync::Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// The Amazon S3 client, built by the one shared constructor over the in-process
    /// callback, so this suite's writes land in the store the handler reads.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create [`BUNDLE_BUCKET`], the bucket every staged bundle lives in.
    async fn bundle_bucket(&self) {
        self.s3()
            .create_bucket()
            .bucket(BUNDLE_BUCKET)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {BUNDLE_BUCKET} must succeed, got {error:?}"));
    }

    /// Put `body` at `key` of [`BUNDLE_BUCKET`].
    async fn stage(&self, key: &str, body: &str) {
        self.s3()
            .put_object()
            .bucket(BUNDLE_BUCKET)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("staging {key} must succeed, got {error:?}"));
    }
}

// ---------------------------------------------------------------------------
// Real CA certificates, and the requests that name them
// ---------------------------------------------------------------------------

/// A bundle of `count` real self-signed CA certificates, PEM encoded one after another —
/// what `openssl` produces for a CA bundle, and what a caller stages in Amazon S3.
///
/// Each certificate is GENERATED here (`rcgen`), so the count every assertion below expects
/// is the number of certificates a parser produced rather than a number this suite and the
/// handler both guessed.
fn ca_bundle(count: usize) -> String {
    (0..count)
        .map(|index| {
            let key = rcgen::KeyPair::generate().expect("a key pair");
            let mut params =
                rcgen::CertificateParams::new(vec![format!("ca{index}.example.com")])
                    .expect("certificate parameters");
            let mut subject = rcgen::DistinguishedName::new();
            subject.push(rcgen::DnType::CommonName, format!("ca{index}.example.com"));
            params.distinguished_name = subject;
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            params
                .self_signed(&key)
                .expect("a self-signed certificate")
                .pem()
        })
        .collect()
}

/// One `CreateTrustStore` request a client submits.
#[derive(Debug, Clone)]
struct Create {
    case: &'static str,
    name: String,
    /// The object key the bundle is staged at, under [`BUNDLE_BUCKET`].
    key: String,
    /// How many certificates that bundle holds.
    certificates: usize,
    /// `UseClientCertificateOCSPEndpoint`, or `None` to leave the optional member out of
    /// the document entirely.
    ocsp: Option<bool>,
    /// `Tags.Items`, as `(Key, Value)` pairs.
    tags: Vec<(String, String)>,
}

impl Create {
    /// The `CreateTrustStoreRequest` XML a client sends for this create, naming the bundle
    /// in [`BUNDLE_BUCKET`].
    fn xml(&self) -> String {
        request_xml(
            &self.name,
            BUNDLE_BUCKET,
            &self.key,
            REGION,
            self.ocsp,
            &self.tags,
        )
    }

    /// The `UseClientCertificateOCSPEndpoint` the record must report: the one submitted, or
    /// the value CloudFront supplies for a create that named none.
    fn expected_ocsp(&self) -> bool {
        self.ocsp.unwrap_or(false)
    }
}

/// The `CreateTrustStoreRequest` XML naming the bundle at `bucket`/`key` in `region`.
fn request_xml(
    name: &str,
    bucket: &str,
    key: &str,
    region: &str,
    ocsp: Option<bool>,
    tags: &[(String, String)],
) -> String {
    let mut out = format!(
        "<CreateTrustStoreRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    out.push_str(&format!("<Name>{}</Name>", escape(name)));
    out.push_str(&format!(
        "<CaCertificatesBundleSource><CaCertificatesBundleS3Location>\
         <Bucket>{}</Bucket><Key>{}</Key><Region>{}</Region>\
         </CaCertificatesBundleS3Location></CaCertificatesBundleSource>",
        escape(bucket),
        escape(key),
        escape(region)
    ));
    if let Some(ocsp) = ocsp {
        out.push_str(&format!(
            "<UseClientCertificateOCSPEndpoint>{ocsp}</UseClientCertificateOCSPEndpoint>"
        ));
    }
    if !tags.is_empty() {
        out.push_str("<Tags><Items>");
        for (key, value) in tags {
            out.push_str(&format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                escape(key),
                escape(value)
            ));
        }
        out.push_str("</Items></Tags>");
    }
    out.push_str("</CreateTrustStoreRequest>");
    out
}

/// The creates rules 1, 2 and 6 are asserted over.
///
/// The rows differ along how many certificates the bundle holds, the shape of the object
/// key, whether the create names the OCSP member and which value it names, and whether it
/// carries tags.
fn create_table() -> Vec<Create> {
    vec![
        Create {
            case: "one certificate, no OCSP member, no tags",
            name: "edge-clients".to_string(),
            key: "single.pem".to_string(),
            certificates: 1,
            ocsp: None,
            tags: Vec::new(),
        },
        Create {
            case: "two certificates, OCSP off, a prefixed key",
            name: "partner-cas".to_string(),
            key: "bundles/2026/partner-cas.pem".to_string(),
            certificates: 2,
            ocsp: Some(false),
            tags: Vec::new(),
        },
        Create {
            case: "five certificates, OCSP on, two tags",
            name: "fleet.trust-store_1".to_string(),
            key: "fleet bundle.pem".to_string(),
            certificates: 5,
            ocsp: Some(true),
            tags: vec![
                ("stage".to_string(), "prod".to_string()),
                ("owner".to_string(), "edge-team".to_string()),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Calling the trust store surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/trust-store` with the request document `body`.
async fn create_trust_store(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/trust-store"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/trust-store/{Identifier}`.
async fn get_trust_store(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "GET",
        // `Identifier` travels as ONE path label, so an ARN's separators are encoded the way
        // an SDK client encodes them — through the one helper the sibling by-web-ACL suite
        // already carries for that rule.
        &format!("/{API_VERSION}/trust-store/{}", encode_label(identifier)),
        None,
        None,
    )
    .await
}

/// The `(Id, Arn)` of the trust store `answer` reports, after asserting every member of the
/// document is the record `create` asked for, in `status`.
fn assert_record(
    answer: &Answer,
    create: &Create,
    status: &str,
    context: &str,
) -> (String, String) {
    let case = create.case;
    let trust_store: &Node = answer.root();
    let raw = answer.raw.as_str();

    let name = member(trust_store, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let id = member(trust_store, "Id", case, raw);
    assert!(
        id.starts_with(TRUST_STORE_ID_PREFIX) && id.len() > TRUST_STORE_ID_PREFIX.len(),
        "case {case} ({context}): expected a minted trust store id, got {id:?}"
    );

    let arn = member(trust_store, "Arn", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:trust-store/{id}"),
        "case {case} ({context}): expected the ARN built from the Id, got {arn:?}"
    );

    let actual_status = member(trust_store, "Status", case, raw);
    assert_eq!(
        actual_status, status,
        "case {case} ({context}): expected Status {status}, got {actual_status:?}"
    );

    let counted = member(trust_store, "NumberOfCaCertificates", case, raw);
    assert_eq!(
        counted,
        create.certificates.to_string(),
        "case {case} ({context}): expected the {} certificates the staged bundle holds, got \
         {counted:?}",
        create.certificates
    );

    let ocsp = member(trust_store, "UseClientCertificateOCSPEndpoint", case, raw);
    assert_eq!(
        ocsp,
        create.expected_ocsp().to_string(),
        "case {case} ({context}): expected UseClientCertificateOCSPEndpoint {}, got {ocsp:?}",
        create.expected_ocsp()
    );

    let last_modified = member(trust_store, "LastModifiedTime", case, raw);
    assert!(
        !last_modified.is_empty(),
        "case {case} ({context}): expected a LastModifiedTime, got {last_modified:?}"
    );

    // "The trust store's reason" is reported only for a trust store whose bundle could not
    // be used, which is never one this emulator stored.
    assert!(
        trust_store.at("Reason").is_none(),
        "case {case} ({context}): expected no Reason on a usable trust store, got {raw}"
    );

    (id, arn)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// RULE 1: a create reads the bundle it was given, counts the certificates in it, mints the
/// trust store's identity and persists the record — and the get answers from that record
/// whether it is addressed by the id, the name or the ARN. Across the table, no two trust
/// stores share an id, an ARN or a version token.
#[tokio::test]
async fn create_persists_the_trust_store_the_get_then_reports() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for create in create_table() {
        let case = create.case;
        emulator
            .stage(&create.key, &ca_bundle(create.certificates))
            .await;

        let created = create_trust_store(&emulator.router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let (id, arn) = assert_record(&created, &create, TRUST_STORE_STATUS_PENDING, "the create");
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The three identifiers a trust store is addressed by all resolve to the one
        // record, and each read answers from it rather than from any request.
        for (addressed, identifier) in [
            ("by id", id.clone()),
            ("by name", create.name.clone()),
            ("by ARN", arn.clone()),
        ] {
            let read = get_trust_store(&emulator.router, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({addressed}): expected HTTP 200, got {} with {}",
                read.status, read.raw
            );
            let (read_id, read_arn) = assert_record(
                &read,
                &create,
                TRUST_STORE_STATUS_PENDING,
                &format!("the get {addressed}"),
            );
            assert_eq!(
                (read_id, read_arn),
                (id.clone(), arn.clone()),
                "case {case} ({addressed}): the get must report the record the create stored"
            );
            assert_eq!(
                read.header("etag"),
                etag,
                "case {case} ({addressed}): expected the create's version token, got {:?}",
                read.header("etag")
            );
        }
    }
}

/// RULE 2: a trust store CloudFront has just created is `pending`, and the CLOCK owes it
/// `active`: no call asks for the transition, and the transition changes nothing else about
/// the record — not its identity, not its certificate count and not the version token an
/// `If-Match` is checked against.
#[tokio::test]
async fn the_clock_settles_pending_to_active() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    let create = Create {
        case: "pending settles",
        name: "settles-to-active".to_string(),
        key: "settling.pem".to_string(),
        certificates: 2,
        ocsp: None,
        tags: Vec::new(),
    };
    emulator
        .stage(&create.key, &ca_bundle(create.certificates))
        .await;

    let created = create_trust_store(&emulator.router, &create.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, created.status, created.raw
    );
    let (id, arn) = assert_record(&created, &create, TRUST_STORE_STATUS_PENDING, "the create");
    let etag = created.header("etag").to_string();

    // A caller acting straight after the create still observes the transient status.
    let immediate = get_trust_store(&emulator.router, &id).await;
    assert_record(
        &immediate,
        &create,
        TRUST_STORE_STATUS_PENDING,
        "straight after the create",
    );

    // Once the span has passed, the clock has settled the trust store — without any call
    // having asked for it.
    tokio::time::sleep(crate::distribution_lifecycle::past_propagation()).await;

    let settled = get_trust_store(&emulator.router, &id).await;
    let (settled_id, settled_arn) =
        assert_record(&settled, &create, TRUST_STORE_STATUS_ACTIVE, "after the span");
    assert_eq!(
        (settled_id, settled_arn),
        (id, arn),
        "case {}: the propagation must not change the trust store's identity",
        create.case
    );
    assert_eq!(
        settled.header("etag"),
        etag,
        "case {}: propagation is not a mutation, so the version token stands",
        create.case
    );
}

/// RULE 3: a bundle source that addresses no object at all, whose object this emulator
/// cannot read a bundle through, or whose object holds no CA certificate, is refused with
/// `InvalidArgument` (HTTP 400) and the one sentence CloudFront states for it; a well-formed
/// address the object store holds nothing at is `EntityNotFound` (HTTP 404). Either way
/// nothing is stored under the name the refused create named: a trust store never reports
/// certificates that were never read.
///
/// The rows cover both halves of the address and every way the bytes can fall short: a
/// bucket name no bucket could be held under, a blank key, an EMPTY object, an object that
/// is not PEM at all and a PEM object carrying no certificate — against an absent bucket and
/// an absent object for the not-found half.
#[tokio::test]
async fn a_bundle_the_object_store_serves_nothing_usable_for_is_refused() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    emulator.stage("prose.pem", "not a certificate bundle").await;
    emulator.stage("empty.pem", "").await;
    emulator
        .stage(
            "other-block.pem",
            &pem::encode(&pem::Pem::new("PUBLIC KEY".to_string(), b"0123456789".to_vec())),
        )
        .await;

    // (case, bucket, key, status, code)
    let rows: [(&str, &str, &str, u16, &str); 7] = [
        (
            "a bucket name Amazon S3 holds no bucket under",
            "Invalid_Bucket_Name",
            "single.pem",
            400,
            "InvalidArgument",
        ),
        ("the blank key", BUNDLE_BUCKET, " ", 400, "InvalidArgument"),
        (
            "an object holding no bytes at all",
            BUNDLE_BUCKET,
            "empty.pem",
            400,
            "InvalidArgument",
        ),
        (
            "an object that is no PEM at all",
            BUNDLE_BUCKET,
            "prose.pem",
            400,
            "InvalidArgument",
        ),
        (
            "a PEM object carrying no certificate",
            BUNDLE_BUCKET,
            "other-block.pem",
            400,
            "InvalidArgument",
        ),
        (
            "a bucket the object store does not hold",
            "cloudfront-trust-store-absent",
            "single.pem",
            404,
            "EntityNotFound",
        ),
        (
            "an object the bucket does not hold",
            BUNDLE_BUCKET,
            "never-staged.pem",
            404,
            "EntityNotFound",
        ),
    ];

    for (case, bucket, key, status, code) in rows {
        let name = format!("refused-{}", case.replace(' ', "-"));
        let body = request_xml(&name, bucket, key, REGION, None, &[]);
        let answer = create_trust_store(&emulator.router, &body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        let stated = answer.root().find_text("Message").unwrap_or_default();
        let expected = if status == 400 {
            INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE
        } else {
            CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE
        };
        assert_eq!(
            stated, expected,
            "case {case}: expected the message {expected:?}, got {stated:?}"
        );

        // The refused create stored nothing: the name it named still names no trust store.
        let read = get_trust_store(&emulator.router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }
}

/// RULE 4: a name identifies one trust store for its whole life, so a second create under a
/// name is refused with `EntityAlreadyExists` (HTTP 409) and the trust store holding that
/// name keeps the identity, the certificate count and the version token its own create
/// minted.
#[tokio::test]
async fn a_name_names_at_most_one_trust_store() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    let create = Create {
        case: "the name is taken",
        name: "only-one".to_string(),
        key: "first.pem".to_string(),
        certificates: 1,
        ocsp: None,
        tags: Vec::new(),
    };
    emulator
        .stage(&create.key, &ca_bundle(create.certificates))
        .await;
    // A second, DIFFERENT bundle, so a trust store that was replaced rather than refused
    // would be visible in its certificate count.
    emulator.stage("second.pem", &ca_bundle(3)).await;

    let first = create_trust_store(&emulator.router, &create.xml()).await;
    assert_eq!(
        first.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, first.status, first.raw
    );
    let (id, _) = assert_record(&first, &create, TRUST_STORE_STATUS_PENDING, "the create");
    let etag = first.header("etag").to_string();

    let repeat = Create {
        key: "second.pem".to_string(),
        certificates: 3,
        ..create.clone()
    };
    let second = create_trust_store(&emulator.router, &repeat.xml()).await;
    assert_eq!(
        second.status, 409,
        "case {}: expected HTTP 409 for a repeated name, got {} with {}",
        create.case, second.status, second.raw
    );
    assert_eq!(
        second.error_code(),
        "EntityAlreadyExists",
        "case {}: expected EntityAlreadyExists, got {}",
        create.case,
        second.raw
    );

    let read = get_trust_store(&emulator.router, &create.name).await;
    let (read_id, _) = assert_record(
        &read,
        &create,
        TRUST_STORE_STATUS_PENDING,
        "after the refusal",
    );
    assert_eq!(
        read_id, id,
        "case {}: the refused create must not have replaced the trust store",
        create.case
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case {}: the refused create must not have spent a version token",
        create.case
    );
}

/// RULE 5: an identifier no trust store answers to is `EntityNotFound` (HTTP 404), whatever
/// form it takes — and a create under one name leaves every other identifier unfound.
#[tokio::test]
async fn an_identifier_no_trust_store_answers_to_is_not_found() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    let present = Create {
        case: "the one trust store that exists",
        name: "present-trust-store".to_string(),
        key: "present.pem".to_string(),
        certificates: 1,
        ocsp: None,
        tags: Vec::new(),
    };
    emulator
        .stage(&present.key, &ca_bundle(present.certificates))
        .await;
    let created = create_trust_store(&emulator.router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = member(created.root(), "Id", present.case, &created.raw);

    let absent: [(&str, String); 5] = [
        ("a name never created", "absent-trust-store".to_string()),
        (
            "the existing name in another case",
            "PRESENT-TRUST-STORE".to_string(),
        ),
        (
            "a prefix of the existing name",
            "present-trust".to_string(),
        ),
        (
            "an id of the minted shape nothing holds",
            format!("{TRUST_STORE_ID_PREFIX}0123456789ABC"),
        ),
        (
            "an ARN of the right type nothing holds",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:trust-store/{TRUST_STORE_ID_PREFIX}absent"),
        ),
    ];
    for (case, identifier) in absent {
        let answer = get_trust_store(&emulator.router, &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {identifier:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {identifier:?}, got {}",
            answer.raw
        );
        let stated = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            stated, TRUST_STORE_NOT_FOUND_MESSAGE,
            "case {case}: expected the message {TRUST_STORE_NOT_FOUND_MESSAGE:?}, got {stated:?}"
        );
    }

    // The trust store that does exist is untouched by the refusals.
    let read = get_trust_store(&emulator.router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: expected HTTP 200 after the refusals, got {} with {}",
        present.case, read.status, read.raw
    );
}

/// RULE 6: the tags a create carries are the tags the tagging surface reports for the trust
/// store's ARN — a trust store is one of the resource kinds this service tags, because its
/// create takes `Tags`.
#[tokio::test]
async fn the_tagging_surface_reports_the_tags_the_create_carried() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;

    for create in create_table() {
        let case = create.case;
        emulator
            .stage(&create.key, &ca_bundle(create.certificates))
            .await;
        let created = create_trust_store(&emulator.router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let arn = member(created.root(), "Arn", case, &created.raw);

        let read = list_tags_for_resource(&emulator.router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the tag read, got {} with {}",
            read.status, read.raw
        );
        let expected: ReportedTags = create
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect();
        assert_eq!(
            reported_tags(read.root()),
            expected,
            "case {case}: expected the submitted tags {expected:?}, got {}",
            read.raw
        );
    }
}

/// The bundle a trust store is built from is read ONCE, by the create: a bundle replaced
/// afterwards does not change what a later read of the trust store reports, because the
/// record is what answers.
#[tokio::test]
async fn the_certificate_count_is_the_one_the_create_read() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    let create = Create {
        case: "the bundle is replaced after the create",
        name: "counted-once".to_string(),
        key: "counted-once.pem".to_string(),
        certificates: 2,
        ocsp: None,
        tags: Vec::new(),
    };
    emulator
        .stage(&create.key, &ca_bundle(create.certificates))
        .await;
    let created = create_trust_store(&emulator.router, &create.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, created.status, created.raw
    );
    let (id, _) = assert_record(&created, &create, TRUST_STORE_STATUS_PENDING, "the create");

    // The object at the same address now holds a different number of certificates.
    emulator.stage(&create.key, &ca_bundle(4)).await;
    // Give the clock nothing to settle but the propagation, then read the record back.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let read = get_trust_store(&emulator.router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: expected HTTP 200, got {} with {}",
        create.case, read.status, read.raw
    );
    let counted = member(read.root(), "NumberOfCaCertificates", create.case, &read.raw);
    assert_eq!(
        counted,
        create.certificates.to_string(),
        "case {}: expected the count the create read, got {counted:?}",
        create.case
    );
}

/// RULE 7: on an emulator assembled with NO object store, nothing answers the bundle read —
/// so the create answers the scaffold's 501 naming that condition, and stores nothing. It
/// neither reports a trust store whose certificates were never counted nor blames the
/// request, whose bundle source may be a perfectly good one.
#[tokio::test]
async fn a_bundle_read_nothing_answers_is_not_served_and_stores_nothing() {
    // CloudFront alone: the router carries no Amazon S3 for the in-process read to reach.
    let router = crate::distribution_lifecycle::emulator();
    let case = "no object store to read the bundle from";
    let name = "unanswered-bundle";
    let body = request_xml(name, BUNDLE_BUCKET, "single.pem", REGION, None, &[]);

    let answer = create_trust_store(&router, &body).await;
    assert_eq!(
        answer.status, 501,
        "case {case}: expected HTTP 501, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NotImplemented",
        "case {case}: expected NotImplemented, got {}",
        answer.raw
    );
    let stated = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        stated, CA_CERTIFICATES_BUNDLE_UNREAD,
        "case {case}: expected the message {CA_CERTIFICATES_BUNDLE_UNREAD:?}, got {stated:?}"
    );

    // Nothing was stored: the name the refused create named still names no trust store.
    let read = get_trust_store(&router, name).await;
    assert_eq!(
        read.status, 404,
        "case {case}: the unserved create must store nothing, got {} with {}",
        read.status, read.raw
    );
}
