// SPDX-License-Identifier: CC0-1.0

//! A certificate another Amazon Web Services service is using, end to end over the wire:
//! ACM over `awsJson1_1` (`POST /`, `X-Amz-Target: CertificateManager.<Op>`) and API
//! Gateway V2 over `restJson1` (`/v2/domainnames…`), both served by ONE router over ONE
//! engine, so what is asserted is what a client holding both SDKs sees.
//!
//! RULE under test: an ACM certificate a resource of another service references is IN USE
//! — ACM reports those resources (`CertificateDetail$InUseBy`, and `InUse` on a listing
//! and a search result) and refuses, with the modelled `ResourceInUseException` (HTTP
//! 400), any call that would take the certificate away from them: "You cannot delete an
//! ACM certificate that is being used by another Amazon Web Services service. To delete a
//! certificate that is in use, you must first remove the certificate association using
//! the console or the CLI for the associated service" (`DeleteCertificate` doc), and "the
//! certificate is in use by another Amazon Web Services service in the caller's account.
//! Remove the association and try again" (`ResourceInUseException` doc). The refusal
//! holds while ANY reference remains and lifts of its own accord once the last one is
//! removed, leaving the record exactly as it was in the meantime.
//!
//! The rows span what the rule generalises over: how the certificate got into ACM (its
//! own material imported, or requested and still pending validation), WHICH modelled
//! member of the referencing resource names it (`DomainNameConfiguration$CertificateArn`
//! and `$OwnershipVerificationCertificateArn`), how many resources hold a reference (one,
//! two), where in the resource's configuration list the reference sits, and how the
//! reference is given up (the resource deleted, or patched onto another certificate).
//! Every certificate is built by `rcgen` or requested over the wire, and every ARN
//! asserted on is the one a service answered rather than a value written here.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    domain_name_arn, DOMAIN_NAME_DELETION, DOMAIN_NAME_MIGRATION,
};

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate, REGION,
};
use crate::certificate_revocation::{issued_certificate, mark_exported, revocable_request};

/// Which member of a domain name's configuration entry names the certificate.
///
/// Both are ACM certificate references on the same shape: `certificateArn` is "the ARN of
/// an AWS-managed certificate that will be used by the edge-optimized endpoint for this
/// domain name" and `ownershipVerificationCertificateArn` "the ARN of the public
/// certificate issued by ACM to validate ownership of your custom domain"
/// (`smithy_query(query="shape",
/// shape="com.amazonaws.apigatewayv2#DomainNameConfiguration")`).
#[derive(Clone, Copy, Debug)]
enum NamedBy {
    Endpoint,
    OwnershipVerification,
}

impl NamedBy {
    fn member(self) -> &'static str {
        match self {
            NamedBy::Endpoint => "certificateArn",
            NamedBy::OwnershipVerification => "ownershipVerificationCertificateArn",
        }
    }
}

/// How a referencing resource gives up its reference.
///
/// Either way the reference is given up ASYNCHRONOUSLY: the endpoints presenting the
/// certificate have to be torn down or migrated first, so the certificate is still in
/// use until that work falls due ([`Release::settles_in`]).
#[derive(Clone, Copy, Debug)]
enum Release {
    /// `DeleteDomainName`: the resource that held the reference is gone.
    DeletingTheResource,
    /// `UpdateDomainName`: the resource stays but is patched onto another certificate.
    PatchingItOntoAnotherCertificate,
}

impl Release {
    /// How long the work this release waits on takes, as the service that owns the
    /// resource states it.
    fn settles_in(self) -> Duration {
        match self {
            Release::DeletingTheResource => DOMAIN_NAME_DELETION,
            Release::PatchingItOntoAnotherCertificate => DOMAIN_NAME_MIGRATION,
        }
    }
}

/// One resource referencing the certificate under test.
#[derive(Clone, Copy, Debug)]
struct Reference {
    domain_name: &'static str,
    named_by: NamedBy,
    /// How many configuration entries the domain name carries. The LAST entry names the
    /// certificate under test; every earlier one names the row's spare certificate, so
    /// the reference is found wherever in the list it sits.
    entries: usize,
    release: Release,
}

/// How the certificate under test got into ACM.
#[derive(Clone, Copy, Debug)]
enum Origin {
    /// `ImportCertificate` of the client's own material, which is `ISSUED` at once.
    Imported,
    /// `RequestCertificate`, held in `PENDING_VALIDATION` for the whole row.
    RequestedAndPending,
}

/// One row: the certificate under test and the resources that come to reference it.
struct Row {
    case: &'static str,
    origin: Origin,
    references: Vec<Reference>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an imported certificate on one domain name's endpoint, released by \
                   deleting the domain name",
            origin: Origin::Imported,
            references: vec![Reference {
                domain_name: "api.in-use-endpoint.example.com",
                named_by: NamedBy::Endpoint,
                entries: 1,
                release: Release::DeletingTheResource,
            }],
        },
        Row {
            case: "an imported certificate as a domain name's ownership verification, \
                   released by patching it off",
            origin: Origin::Imported,
            references: vec![Reference {
                domain_name: "api.in-use-ownership.example.com",
                named_by: NamedBy::OwnershipVerification,
                entries: 1,
                release: Release::PatchingItOntoAnotherCertificate,
            }],
        },
        Row {
            case: "an imported certificate two domain names reference, released one at a \
                   time",
            origin: Origin::Imported,
            references: vec![
                Reference {
                    domain_name: "a.in-use-shared.example.net",
                    named_by: NamedBy::Endpoint,
                    entries: 1,
                    release: Release::PatchingItOntoAnotherCertificate,
                },
                Reference {
                    domain_name: "b.in-use-shared.example.net",
                    named_by: NamedBy::Endpoint,
                    entries: 1,
                    release: Release::DeletingTheResource,
                },
            ],
        },
        Row {
            case: "an imported certificate named by the second of a domain name's two \
                   configuration entries",
            origin: Origin::Imported,
            references: vec![Reference {
                domain_name: "api.in-use-second-entry.example.org",
                named_by: NamedBy::Endpoint,
                entries: 2,
                release: Release::DeletingTheResource,
            }],
        },
        Row {
            case: "a requested certificate still pending validation, referenced by a \
                   domain name",
            origin: Origin::RequestedAndPending,
            references: vec![Reference {
                domain_name: "api.in-use-pending.example.io",
                named_by: NamedBy::Endpoint,
                entries: 1,
                release: Release::DeletingTheResource,
            }],
        },
        Row {
            case: "an imported certificate nothing references",
            origin: Origin::Imported,
            references: Vec::new(),
        },
    ]
}

/// A router serving ACM beside API Gateway V2 on ONE engine, which is what makes the
/// hand-off real: the certificate ARN ACM answers is handed to the other service, and the
/// association that service records is what ACM then reports and protects.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Send one API Gateway V2 request, signed for [`REGION`] so the ARNs it reports carry
/// the Region the certificate ARNs do, and return `(status, parsed body)`.
async fn api_gateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The certificate under test, minted the way the row says and left in the state the row
/// names.
async fn subject_certificate(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> String {
    match row.origin {
        Origin::Imported => imported_certificate(router, row.case, "subject").await,
        Origin::RequestedAndPending => {
            let arn = request_certificate(
                router,
                &json!({ "DomainName": "pending.in-use.example.io", "ValidationMethod": "DNS" }),
            )
            .await;
            // The validation outcome the clock owes this request is moved out of reach,
            // so the row asserts on a certificate that is still PENDING_VALIDATION
            // however long it runs.
            hold_pending_validations(engine);
            let detail = describe(router, &arn).await;
            assert_eq!(
                detail["Status"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {}: the certificate under test must still be pending, got {detail}",
                row.case
            );
            arn
        }
    }
}

/// Import one freshly generated certificate and return the ARN ACM stored it under.
async fn imported_certificate(router: &MultiServiceRouter, case: &str, label: &str) -> String {
    let generated = generate(&Blueprint {
        common_name: Some("in-use.example.com"),
        dns_names: &["in-use.example.com"],
        ..Blueprint::default()
    });
    let arn = import(router, &format!("{case} ({label})"), &import_body(&generated)).await;
    assert!(
        arn.contains(":certificate/"),
        "case {case:?}: expected an ACM certificate ARN for the {label}, got {arn:?}"
    );
    arn
}

/// The `domainNameConfigurations` list of one reference: the last entry names `subject`
/// on the member the reference names it by, and every earlier entry names `spare`.
fn configurations(reference: &Reference, subject: &str, spare: &str) -> Value {
    let entries: Vec<Value> = (0..reference.entries)
        .map(|index| {
            if index + 1 == reference.entries {
                json!({ reference.named_by.member(): subject })
            } else {
                json!({ "certificateArn": spare })
            }
        })
        .collect();
    json!(entries)
}

/// Create the referencing domain name, requiring the 201 the model binds to
/// `CreateDomainName`.
async fn create_domain_name(
    router: &MultiServiceRouter,
    case: &str,
    reference: &Reference,
    subject: &str,
    spare: &str,
) {
    let (status, body) = api_gateway(
        router,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": reference.domain_name,
            "domainNameConfigurations": configurations(reference, subject, spare),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDomainName must answer 201 for {}, got {status} {body}",
        reference.domain_name
    );
}

/// Give up one reference the way the row says, and require the call to succeed: the
/// resource is deleted, or patched onto the spare certificate.
async fn release(router: &MultiServiceRouter, case: &str, reference: &Reference, spare: &str) {
    match reference.release {
        Release::DeletingTheResource => {
            let (status, body) = api_gateway(
                router,
                "DELETE",
                &format!("/v2/domainnames/{}", reference.domain_name),
                None,
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteDomainName must answer 202 for {}, got {status} {body}",
                reference.domain_name
            );
        }
        Release::PatchingItOntoAnotherCertificate => {
            // The patch names the spare certificate on the SAME member, so what changed
            // is which certificate the resource references and nothing else.
            let moved = Reference {
                entries: 1,
                ..*reference
            };
            let (status, body) = api_gateway(
                router,
                "PATCH",
                &format!("/v2/domainnames/{}", reference.domain_name),
                Some(json!({
                    "domainNameConfigurations": configurations(&moved, spare, spare),
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateDomainName must answer 200 for {}, got {status} {body}",
                reference.domain_name
            );
        }
    }
}

/// The ARNs a `SearchCertificates` `InUse` filter selects, each result also having to
/// report that same flag — so the value the filter selected on and the value the result
/// states cannot drift apart.
async fn searched_by_in_use(router: &MultiServiceRouter, case: &str, in_use: bool) -> Vec<String> {
    let (status, body) = call(
        router,
        "SearchCertificates",
        &json!({
            "FilterStatement": { "Filter": { "AcmCertificateMetadataFilter": { "InUse": in_use } } }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SearchCertificates must answer 200, got {status} {body}"
    );
    let results = body["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Results in {body}"))
        .clone();
    let mut arns = Vec::with_capacity(results.len());
    for result in &results {
        assert_eq!(
            result["CertificateMetadata"]["AcmCertificateMetadata"]["InUse"].as_bool(),
            Some(in_use),
            "case {case:?}: a result of the InUse {in_use} search must report InUse \
             {in_use}, got {result}"
        );
        arns.push(
            result["CertificateArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a result carried no ARN: {result}"))
                .to_string(),
        );
    }
    arns.sort();
    arns
}

/// Every reader's answer about which resources are using `certificate_arn`:
/// `DescribeCertificate`'s `InUseBy`, the `InUse` flag of its `ListCertificates` summary,
/// and both directions of the `SearchCertificates` `InUse` filter.
async fn assert_in_use_by(
    router: &MultiServiceRouter,
    case: &str,
    certificate_arn: &str,
    expected: &[String],
) {
    let detail = describe(router, certificate_arn).await;
    let reported: Vec<String> = detail["InUseBy"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InUseBy in {detail}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected InUseBy {expected:?}, got {reported:?}"
    );

    let in_use = !expected.is_empty();
    // `Includes: {}` selects on the members it names and no others, which is how the
    // listing reaches a certificate of ANY key algorithm: a request that supplies no
    // `Includes` at all gets this operation's documented default key-algorithm filter.
    let (status, body) = call(router, "ListCertificates", &json!({ "Includes": {} })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCertificates must answer 200, got {status} {body}"
    );
    let summary = body["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateSummaryList in {body}"))
        .iter()
        .find(|summary| summary["CertificateArn"].as_str() == Some(certificate_arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the certificate is not listed: {body}"));
    assert_eq!(
        summary["InUse"].as_bool(),
        Some(in_use),
        "case {case:?}: expected InUse {in_use} on the listing summary, got {summary}"
    );

    let selected = searched_by_in_use(router, case, in_use).await;
    assert!(
        selected.iter().any(|arn| arn == certificate_arn),
        "case {case:?}: expected the InUse {in_use} search to select the certificate, got \
         {selected:?}"
    );
    let not_selected = searched_by_in_use(router, case, !in_use).await;
    assert!(
        !not_selected.iter().any(|arn| arn == certificate_arn),
        "case {case:?}: expected the InUse {} search not to select the certificate, got \
         {not_selected:?}",
        !in_use
    );
}

/// The sentence ACM answers for a certificate something is still using.
fn expected_in_use_message(certificate_arn: &str) -> String {
    format!("Certificate {certificate_arn} is in use.")
}

/// Call one operation that would take the certificate away from the resources using it,
/// and require the modelled refusal: HTTP 400 `ResourceInUseException` naming the
/// certificate.
async fn assert_refused(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) {
    let certificate_arn = body["CertificateArn"].as_str().unwrap_or_default();
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceInUseException"),
        "case {case:?}: {operation} must be refused while the certificate is in use, got \
         {status} {response}"
    );
    assert_eq!(
        error_message(&response),
        expected_in_use_message(certificate_arn),
        "case {case:?}: expected {:?}, got {:?}",
        expected_in_use_message(certificate_arn),
        error_message(&response)
    );
}

/// The ARNs of the resources a set of references is holding the certificate through.
fn holder_arns(holders: &[Reference]) -> Vec<String> {
    let mut arns: Vec<String> = holders
        .iter()
        .map(|reference| domain_name_arn(reference.domain_name, REGION))
        .collect();
    arns.sort();
    arns
}

/// RULE: a certificate a resource of another service references may not be deleted —
/// ACM reports the resources using it and answers `ResourceInUseException` (400) while
/// any reference remains, leaving the record untouched, and lets the delete through once
/// the last reference is removed.
#[tokio::test]
async fn a_certificate_another_service_references_cannot_be_deleted() {
    for row in rows() {
        let case = row.case;
        // One engine per row, so each row's assertions are about its own store.
        let (router, engine) = emulator();
        let subject = subject_certificate(&router, &engine, &row).await;
        // The certificate the fillers name and the patches move onto, so a release is
        // never the removal of the resource's only certificate.
        let spare = imported_certificate(&router, case, "spare").await;
        let delete = json!({ "CertificateArn": subject });

        // Nothing references it yet, and every reader says so.
        assert_in_use_by(&router, case, &subject, &[]).await;

        for reference in &row.references {
            create_domain_name(&router, case, reference, &subject, &spare).await;
        }

        // While ANY reference remains the delete is refused and the record stands; each
        // turn of the loop gives up one more reference.
        let mut holders = row.references.clone();
        while !holders.is_empty() {
            assert_in_use_by(&router, case, &subject, &holder_arns(&holders)).await;
            assert_refused(&router, case, "DeleteCertificate", &delete).await;
            let detail = describe(&router, &subject).await;
            assert_eq!(
                detail["CertificateArn"].as_str(),
                Some(subject.as_str()),
                "case {case:?}: the refused delete must leave the certificate stored, got \
                 {detail}"
            );

            let released = holders.remove(0);
            release(&router, case, &released, &spare).await;

            // The release is ACCEPTED, not finished: until the endpoint work it waits on
            // falls due the resource is still presenting the certificate, so the refusal
            // stands exactly as it did before the call.
            assert_in_use_by(
                &router,
                case,
                &subject,
                &holder_arns(&[holders.clone(), vec![released]].concat()),
            )
            .await;
            assert_refused(&router, case, "DeleteCertificate", &delete).await;
            sleep(released.release.settles_in() + Duration::from_millis(100)).await;
        }

        // With the last reference gone the delete goes through, and the record is gone
        // with it.
        assert_in_use_by(&router, case, &subject, &[]).await;
        let (status, body) = call(&router, "DeleteCertificate", &delete).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCertificate must answer 200 once nothing references the \
             certificate, got {status} {body}"
        );
        let (status, body) = call(&router, "DescribeCertificate", &delete).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: the deleted certificate must be gone, got {status} {body}"
        );
    }
}

/// RULE: the same association protects a certificate from being REVOKED — "revoking a
/// certificate is permanent" (`RevokeCertificate` doc), so ACM refuses it while another
/// service is serving the certificate and records the revocation only once the last
/// reference is removed.
#[tokio::test]
async fn a_certificate_another_service_references_cannot_be_revoked() {
    for named_by in [NamedBy::Endpoint, NamedBy::OwnershipVerification] {
        let case = format!("a certificate referenced by {}", named_by.member());
        let (router, engine) = emulator();

        // The one state a revocation can follow: a certificate export was allowed for,
        // carried to ISSUED and exported.
        let subject = issued_certificate(
            &router,
            &engine,
            &revocable_request("revocable.in-use.example.com", &[]),
        )
        .await;
        mark_exported(&engine, &subject);

        let reference = Reference {
            domain_name: "api.in-use-revoke.example.com",
            named_by,
            entries: 1,
            release: Release::DeletingTheResource,
        };
        create_domain_name(&router, &case, &reference, &subject, &subject).await;
        assert_in_use_by(&router, &case, &subject, &holder_arns(&[reference])).await;

        let revoke = json!({ "CertificateArn": subject, "RevocationReason": "KEY_COMPROMISE" });
        assert_refused(&router, &case, "RevokeCertificate", &revoke).await;
        let detail = describe(&router, &subject).await;
        assert_eq!(
            (detail["Status"].as_str(), detail["RevokedAt"].as_f64()),
            (Some("ISSUED"), None),
            "case {case:?}: the refused revocation must leave the certificate as it was, got \
             {detail}"
        );

        // The association given up, and the teardown it waits on complete, the
        // revocation goes through and every later read reports it.
        release(&router, &case, &reference, &subject).await;
        assert_refused(&router, &case, "RevokeCertificate", &revoke).await;
        sleep(reference.release.settles_in() + Duration::from_millis(100)).await;
        assert_in_use_by(&router, &case, &subject, &[]).await;
        let (status, body) = call(&router, "RevokeCertificate", &revoke).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RevokeCertificate must answer 200 once nothing references the \
             certificate, got {status} {body}"
        );
        let detail = describe(&router, &subject).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("REVOKED"),
            "case {case:?}: expected the recorded revocation, got {detail}"
        );
    }
}
