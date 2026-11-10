// SPDX-License-Identifier: CC0-1.0

//! The refusals ACM answers for an operation asked of a certificate it does not apply
//! to, end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Each rule here is about WHICH certificate an operation applies to, and therefore
//! which modelled error reports a certificate outside it:
//!
//! 1. `ImportCertificate`'s `CertificateArn` names "an imported certificate to
//!    replace", so an ARN that resolves to a certificate ACM issued names no such
//!    certificate: `ResourceNotFoundException`;
//! 2. `RevokeCertificate` revokes "a public ACM certificate", so a certificate whose
//!    material came from outside ACM is not one of them: `ValidationException` saying
//!    exactly that, and not the export-eligibility refusal an ACM-issued certificate
//!    draws;
//! 3. `ResendValidationEmail` has mail to resend only while the CERTIFICATE is awaiting
//!    domain validation, which is decided before the domain the request named is
//!    resolved: `InvalidStateException`, with `ResourceNotFoundException` left to an
//!    ARN that names no certificate at all;
//! 4. `UpdateCertificateOptions` configures ACM's own issuance of a certificate, which
//!    an imported certificate has none of: the change is refused rather than accepted
//!    and dropped;
//! 5. `RenewCertificate` renews an ELIGIBLE certificate — "only exported private
//!    certificates can be renewed" — out of the ISSUED certificates its `CertificateArn`
//!    addresses, and refuses every other with the modelled error for its condition,
//!    while an eligible one is renewed on the record every reader projects.
//!
//! The tables span the statuses and types a stored certificate can be in when each call
//! arrives, and every row reads the record back with `DescribeCertificate` so a refusal
//! that quietly wrote something cannot pass.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{blob, generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router, router_with_engine, ACCOUNT, REGION,
};
use crate::certificate_revocation::mark_exported;
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// A well-formed ACM certificate ARN with no record behind it.
fn unknown_certificate_arn() -> String {
    format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555")
}

/// Import one certificate with a subject common name of its own and return its ARN.
async fn imported_certificate(
    router: &MultiServiceRouter,
    case: &str,
    common_name: &str,
) -> String {
    let generated = generate(&Blueprint {
        common_name: Some(common_name),
        ..Blueprint::default()
    });
    import(router, case, &import_body(&generated)).await
}

/// Request the certificate `body` asks for and carry it to `ISSUED`, returning its ARN.
/// Every step goes over the wire bar the stored due instant, which is moved into the
/// past so the test does not race the settle delay.
async fn issued_certificate(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    body: &Value,
) -> String {
    let arn = request_certificate(router, body).await;
    make_validation_due_now(engine);
    let detail = describe(router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case:?}: the certificate must be issued first, got {detail}"
    );
    arn
}

/// The `RequestCertificate` body of a private certificate: the request that needs no
/// proof of domain control, and so the one that reaches `ISSUED`.
fn private_request(domain: &str) -> Value {
    json!({
        "DomainName": domain,
        "ValidationMethod": "DNS",
        "CertificateAuthorityArn": PRIVATE_CA_ARN,
    })
}

/// Move the completion of every stored renewal `offset` from now, so a test can observe
/// a renewal that is still owed or one that has fallen due without sleeping.
///
/// This writes only the due instant the emulator itself stores; the renewal is still
/// applied by the service's own `materialize_due` on the next call, exactly as in
/// production.
fn move_renewal_due(engine: &Arc<engine::InMemoryEngine>, offset: Duration, ahead: bool) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let now = SystemTime::now();
    let due = if ahead { now + offset } else { now - offset };
    for certificate in store.certificates.values_mut() {
        if let Some(renewal) = certificate.renewal.as_mut() {
            if renewal.due_at.is_some() {
                renewal.due_at = Some(due);
            }
        }
    }
}

/// RULE 1: `ImportCertificate`'s `CertificateArn` addresses an IMPORTED certificate, so
/// an ARN behind which ACM holds one of its own certificates names nothing this
/// argument can replace — 400 `ResourceNotFoundException`, naming the ARN the request
/// carried — and the certificate it named is left exactly as it was.
#[tokio::test]
async fn a_reimport_onto_a_certificate_acm_issued_names_no_imported_certificate() {
    // Rows span the states a certificate ACM was asked to issue can be in when the
    // reimport arrives: still validating, issued, and a request that failed.
    let cases: &[(&str, Value, &str, &str)] = &[
        (
            "a request still awaiting domain validation",
            json!({ "DomainName": "pending.reimport.probe-domain.com", "ValidationMethod": "DNS" }),
            "pending",
            "PENDING_VALIDATION",
        ),
        (
            "a private certificate ACM issued",
            private_request("issued.reimport.probe-domain.com"),
            "issued",
            "ISSUED",
        ),
        (
            // A name whose top-level label the public DNS delegates to nobody, which
            // ACM refuses up front.
            "a request that failed",
            json!({ "DomainName": "failed.reimport.invalid", "ValidationMethod": "EMAIL" }),
            "failed",
            "FAILED",
        ),
    ];

    for (case, body, settle, expected_status) in cases {
        let (router, engine) = router_with_engine();
        let target = request_certificate(&router, body).await;
        if *settle == "pending" {
            hold_pending_validations(&engine);
        } else {
            make_validation_due_now(&engine);
        }
        let before = describe(&router, &target).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: expected the target in {expected_status}, got {before}"
        );

        let replacement = generate(&Blueprint {
            common_name: Some("replacement.reimport.probe-domain.com"),
            ..Blueprint::default()
        });
        let (status, response) = call(
            &router,
            "ImportCertificate",
            &json!({
                "Certificate": blob(&replacement.certificate),
                "PrivateKey": blob(&replacement.private_key),
                "CertificateArn": &target,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("The certificate ARN {target} is not an imported certificate ARN."),
            "case {case:?}: expected the refusal built from the request, got {response}"
        );

        // The refused reimport wrote nothing: the certificate it named is what it was.
        let after = describe(&router, &target).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused reimport must leave the record alone, was {before} \
             and is now {after}"
        );

        // The positive control: the same payload replaces an IMPORTED certificate, so
        // the refusals above are about the ARN's certificate and not about the body.
        let imported = imported_certificate(&router, case, "control.reimport.probe-domain.com").await;
        let (status, response) = call(
            &router,
            "ImportCertificate",
            &json!({
                "Certificate": blob(&replacement.certificate),
                "PrivateKey": blob(&replacement.private_key),
                "CertificateArn": &imported,
            }),
        )
        .await;
        assert_eq!(
            (status, response["CertificateArn"].as_str()),
            (200, Some(imported.as_str())),
            "case {case:?}: the same payload must replace an imported certificate, got \
             {status} {response}"
        );
    }
}

/// RULE 2: a certificate ACM did not issue is not a public ACM certificate, so
/// `RevokeCertificate` refuses it with 400 `ValidationException` saying so — a refusal
/// about the certificate's TYPE, distinct from the export-eligibility refusal an
/// ACM-issued certificate draws — and revokes nothing.
#[tokio::test]
async fn revoking_an_imported_certificate_reports_that_it_is_not_public() {
    // Rows span the shapes an imported certificate can have and the reasons a caller
    // can name: a plain name, a name set, a wildcard, and one asserting key usages.
    let cases: &[(&str, Blueprint<'_>, &str)] = &[
        (
            "a single-name import",
            Blueprint {
                common_name: Some("plain.revoke.probe-domain.com"),
                ..Blueprint::default()
            },
            "UNSPECIFIED",
        ),
        (
            "an import covering several names",
            Blueprint {
                common_name: Some("multi.revoke.probe-domain.com"),
                dns_names: &["alt.revoke.probe-domain.com"],
                ..Blueprint::default()
            },
            "KEY_COMPROMISE",
        ),
        (
            "a wildcard import on P-384",
            Blueprint {
                common_name: Some("*.wild.revoke.probe-domain.com"),
                dns_names: &["*.wild.revoke.probe-domain.com"],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                ..Blueprint::default()
            },
            "CESSATION_OF_OPERATION",
        ),
        (
            "an import asserting key usages",
            Blueprint {
                common_name: Some("usages.revoke.probe-domain.com"),
                key_usages: true,
                ..Blueprint::default()
            },
            "SUPERSEDED",
        ),
    ];

    let router = router();
    for (case, blueprint, reason) in cases {
        let generated = generate(blueprint);
        let arn = import(&router, case, &import_body(&generated)).await;
        let before = describe(&router, &arn).await;

        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": &arn, "RevocationReason": reason }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Certificate {arn} is not a public certificate."),
            "case {case:?}: expected the type refusal built from the request, got {response}"
        );

        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused revocation must leave the record alone, was \
             {before} and is now {after}"
        );
    }

    // The other side of the rule: a certificate ACM ISSUED is a public certificate, so
    // it is held to the export conditions instead and draws that refusal's own
    // sentence.
    let (router, engine) = router_with_engine();
    let case = "an ACM-issued certificate export was never allowed for";
    let issued = issued_certificate(
        &router,
        &engine,
        case,
        &private_request("issued.revoke.probe-domain.com"),
    )
    .await;
    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": &issued, "RevocationReason": "UNSPECIFIED" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} {response}"
    );
    assert_eq!(
        error_message(&response),
        format!("Export not allowed for certificate {issued}"),
        "case {case:?}: expected the export refusal, got {response}"
    );
}

/// RULE 3: whether there is validation mail to resend is a fact of the CERTIFICATE, so
/// a certificate that is not awaiting domain validation is 400 `InvalidStateException`
/// whatever domain the request names — while `ResourceNotFoundException` is reserved
/// for an ARN that names no certificate, and for a domain a still-pending certificate
/// does not cover.
#[tokio::test]
async fn resending_validation_mail_is_gated_on_the_certificates_state() {
    let (router, engine) = router_with_engine();

    // An imported certificate: ISSUED, with no validation of any domain.
    let imported = imported_certificate(&router, "an imported certificate", "mail.probe-domain.com")
        .await;
    let imported_detail = describe(&router, &imported).await;
    assert_eq!(
        (
            imported_detail["Status"].as_str(),
            imported_detail["Type"].as_str()
        ),
        (Some("ISSUED"), Some("IMPORTED")),
        "the imported certificate must be ISSUED and IMPORTED, got {imported_detail}"
    );

    // A private certificate ACM issued: ISSUED, and its domains are proven.
    let issued = issued_certificate(
        &router,
        &engine,
        "an issued certificate",
        &private_request("issued.mail.probe-domain.com"),
    )
    .await;
    // A request for a name the public DNS delegates to nobody: FAILED, with its domain
    // unproven.
    let failed = request_certificate(
        &router,
        &json!({ "DomainName": "failed.mail.invalid", "ValidationMethod": "EMAIL" }),
    )
    .await;
    make_validation_due_now(&engine);
    let failed_detail = describe(&router, &failed).await;
    assert_eq!(
        failed_detail["Status"].as_str(),
        Some("FAILED"),
        "the failed request must be FAILED, got {failed_detail}"
    );

    // Each row: what it is, the certificate, the `Domain` and `ValidationDomain` to
    // resend for, and the code the refusal must carry. The rows span a certificate that
    // never had a validation at all, one whose validations are proven, one whose
    // request failed — each asked both for a name it covers and for one it does not, so
    // the state gate is seen to come before the domain lookup.
    let cases: &[(&str, &String, &str, &str, &str)] = &[
        (
            "an imported certificate, asked for its own name",
            &imported,
            "mail.probe-domain.com",
            "mail.probe-domain.com",
            "InvalidStateException",
        ),
        (
            "an imported certificate, asked for a name it does not cover",
            &imported,
            "stranger.probe-domain.com",
            "probe-domain.com",
            "InvalidStateException",
        ),
        (
            "an issued certificate, asked for its own name",
            &issued,
            "issued.mail.probe-domain.com",
            "issued.mail.probe-domain.com",
            "InvalidStateException",
        ),
        (
            "a failed request, asked for its own name",
            &failed,
            "failed.mail.invalid",
            "failed.mail.invalid",
            "InvalidStateException",
        ),
    ];

    for (case, arn, domain, validation_domain, expected_code) in cases {
        let before = describe(&router, arn).await;
        let (status, response) = call(
            &router,
            "ResendValidationEmail",
            &json!({
                "CertificateArn": arn,
                "Domain": domain,
                "ValidationDomain": validation_domain,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Certificate {arn} is not in pending validation state"),
            "case {case:?}: expected the state refusal built from the request, got {response}"
        );
        let after = describe(&router, arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused resend must change nothing, was {before} and is \
             now {after}"
        );
    }

    // `ResourceNotFoundException` keeps its own meaning: an ARN that names no
    // certificate at all.
    let missing = unknown_certificate_arn();
    let (status, response) = call(
        &router,
        "ResendValidationEmail",
        &json!({
            "CertificateArn": &missing,
            "Domain": "absent.probe-domain.com",
            "ValidationDomain": "absent.probe-domain.com",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case an ARN naming no certificate: expected 400 ResourceNotFoundException, got \
         {status} {response}"
    );

    // And so does the domain lookup, for a certificate that IS awaiting validation: the
    // state gate did not swallow it.
    let pending = request_certificate(
        &router,
        &json!({
            "DomainName": "pending.mail.probe-domain.com",
            "ValidationMethod": "EMAIL",
        }),
    )
    .await;
    hold_pending_validations(&engine);
    let (status, response) = call(
        &router,
        "ResendValidationEmail",
        &json!({
            "CertificateArn": &pending,
            "Domain": "stranger.probe-domain.com",
            "ValidationDomain": "probe-domain.com",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case a name a pending certificate does not cover: expected 400 \
         ResourceNotFoundException, got {status} {response}"
    );
}

/// RULE 4: `UpdateCertificateOptions` configures the issuance ACM runs for a
/// certificate, and an imported certificate has none — so every such request is refused
/// with 400 `InvalidStateException` and nothing on the record changes, while the same
/// request is served for a certificate ACM issued.
#[tokio::test]
async fn updating_the_options_of_an_imported_certificate_is_refused() {
    // Rows span the `Options` members a request can name, each a change that could
    // never take effect on an imported certificate.
    let cases: &[(&str, Value)] = &[
        (
            "transparency logging off",
            json!({ "CertificateTransparencyLoggingPreference": "DISABLED" }),
        ),
        (
            "transparency logging on",
            json!({ "CertificateTransparencyLoggingPreference": "ENABLED" }),
        ),
        ("export opted in", json!({ "Export": "ENABLED" })),
        ("a validation method", json!({ "ValidationMethod": "DNS" })),
        (
            "every member at once",
            json!({
                "CertificateTransparencyLoggingPreference": "DISABLED",
                "Export": "ENABLED",
                "ValidationMethod": "EMAIL",
            }),
        ),
    ];

    let router = router();
    let arn = imported_certificate(&router, "an imported certificate", "options.probe-domain.com")
        .await;
    let before = describe(&router, &arn).await;

    for (case, options) in cases {
        let (status, response) = call(
            &router,
            "UpdateCertificateOptions",
            &json!({ "CertificateArn": &arn, "Options": options }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidStateException"),
            "case {case:?}: expected 400 InvalidStateException, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            "The UpdateCertificateOptions command is not permitted for imported certificates.",
            "case {case:?}: expected the refusal ACM answers for an imported \
             certificate, got {response}"
        );

        // Nothing was written: a refused update is not a silently lost one.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the record alone, was {before} \
             and is now {after}"
        );
    }

    // The positive control: the same change is served for a certificate ACM issued, so
    // the refusals above are about the certificate and not about the payload.
    let (router, engine) = router_with_engine();
    let issued = issued_certificate(
        &router,
        &engine,
        "an issued certificate",
        &private_request("issued.options.probe-domain.com"),
    )
    .await;
    let (status, response) = call(
        &router,
        "UpdateCertificateOptions",
        &json!({
            "CertificateArn": &issued,
            "Options": { "CertificateTransparencyLoggingPreference": "DISABLED" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an issued certificate: the same update must be served, got {status} {response}"
    );
    let detail = describe(&router, &issued).await;
    assert_eq!(
        detail["Options"]["CertificateTransparencyLoggingPreference"].as_str(),
        Some("DISABLED"),
        "case an issued certificate: expected the stored preference, got {detail}"
    );
}

/// RULE 5: `RenewCertificate` applies to a certificate whose `RenewalEligibility` says
/// ACM renews it, and every other certificate draws the modelled error for its own
/// condition: a request still in process, an ARN that names no certificate, an ARN that
/// names no certificate resource at all, or a certificate ACM does not renew.
#[tokio::test]
async fn renewing_a_certificate_acm_does_not_renew_is_refused() {
    let (router, engine) = router_with_engine();

    let imported =
        imported_certificate(&router, "an imported certificate", "renew.probe-domain.com").await;
    let issued = issued_certificate(
        &router,
        &engine,
        "an issued certificate",
        &private_request("issued.renew.probe-domain.com"),
    )
    .await;
    let pending = request_certificate(
        &router,
        &json!({ "DomainName": "pending.renew.probe-domain.com", "ValidationMethod": "DNS" }),
    )
    .await;
    hold_pending_validations(&engine);

    // Each row: what it is, the ARN, and the code the refusal must carry. The rows span
    // every condition the operation's modelled errors name.
    let cases: &[(&str, String, &str)] = &[
        (
            "an imported certificate, which ACM never renews",
            imported.clone(),
            "ValidationException",
        ),
        (
            "an issued certificate that was never exported",
            issued.clone(),
            "ValidationException",
        ),
        (
            // A request ACM has not finished is not one of the ISSUED certificates this
            // operation renews, so the ARN names no certificate it can resolve.
            "a request ACM has not finished",
            pending.clone(),
            "ResourceNotFoundException",
        ),
        (
            "an ARN naming no certificate",
            unknown_certificate_arn(),
            "ResourceNotFoundException",
        ),
        (
            "an ARN naming another ACM resource type",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij"),
            "InvalidArnException",
        ),
    ];

    for (case, arn, expected_code) in cases {
        let (status, response) = call(
            &router,
            "RenewCertificate",
            &json!({ "CertificateArn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {response}"
        );
    }

    // The refusal of a certificate ACM does not renew names it, and the eligibility it
    // was refused for is the one `DescribeCertificate` reports.
    let (_, response) = call(
        &router,
        "RenewCertificate",
        &json!({ "CertificateArn": &imported }),
    )
    .await;
    assert_eq!(
        error_message(&response),
        format!(
            "Certificate ARN {imported} is for a public or imported certificate and not \
             eligible for the Renew Certificate API"
        ),
        "case an imported certificate: expected the eligibility refusal, got {response}"
    );
    for arn in [&imported, &issued] {
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["RenewalEligibility"].as_str(),
            Some("INELIGIBLE"),
            "case {arn}: a certificate ACM does not renew reports INELIGIBLE, got {detail}"
        );
        assert!(
            detail["RenewalSummary"].is_null(),
            "case {arn}: a refused renewal must record none, got {detail}"
        );
    }
}

/// RULE 5, the other side: an exported private certificate is one ACM renews, and the
/// renewal is recorded on the stored record — reported as still running until the
/// instant it falls due, and as the renewed validity window from then on.
#[tokio::test]
async fn renewing_an_eligible_certificate_records_the_renewal() {
    let (router, engine) = router_with_engine();
    let case = "an exported private certificate";
    let arn = issued_certificate(
        &router,
        &engine,
        case,
        &private_request("eligible.renew.probe-domain.com"),
    )
    .await;
    // The export is what puts a copy of the certificate outside ACM, which is what
    // makes it renewable. `ExportCertificate` sets this flag in production and cannot
    // here — this service mints no key pair for a certificate it was asked to issue —
    // so the test writes the single boolean the emulator itself stores and drives the
    // renewal and every read of it over the wire.
    mark_exported(&engine, &arn);

    let before = describe(&router, &arn).await;
    assert_eq!(
        before["RenewalEligibility"].as_str(),
        Some("ELIGIBLE"),
        "case {case:?}: an exported private certificate is renewable, got {before}"
    );

    let (status, response) = call(
        &router,
        "RenewCertificate",
        &json!({ "CertificateArn": &arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: renewing an eligible certificate must succeed, got {status} {response}"
    );

    // While the renewal is owed the record reports it as running, and asking for
    // another is the request-in-progress refusal.
    move_renewal_due(&engine, Duration::from_secs(3600), true);
    let running = describe(&router, &arn).await;
    assert_eq!(
        running["RenewalSummary"]["RenewalStatus"].as_str(),
        Some("PENDING_AUTO_RENEWAL"),
        "case {case:?}: a started renewal is reported as running, got {running}"
    );
    assert!(
        running["RenewalSummary"]["UpdatedAt"].is_number(),
        "case {case:?}: a renewal summary carries the instant it was updated, got {running}"
    );
    assert_eq!(
        running["RenewalSummary"]["DomainValidationOptions"],
        before["DomainValidationOptions"],
        "case {case:?}: a renewal re-proves the names the certificate covers, got {running}"
    );
    assert_eq!(
        running["NotAfter"], before["NotAfter"],
        "case {case:?}: the validity window moves only once the renewal completes, got \
         {running}"
    );
    let (status, response) = call(
        &router,
        "RenewCertificate",
        &json!({ "CertificateArn": &arn }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "RequestInProgressException"),
        "case {case:?}: a renewal already in process is not started twice, got {status} \
         {response}"
    );

    // Once the instant it was owed at has passed, the renewal is reported as completed
    // and the certificate carries the renewed validity window.
    move_renewal_due(&engine, Duration::from_millis(1), false);
    let renewed = describe(&router, &arn).await;
    assert_eq!(
        renewed["RenewalSummary"]["RenewalStatus"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: a completed renewal is reported as such, got {renewed}"
    );
    let (was_not_after, now_not_after) = (
        before["NotAfter"].as_f64().unwrap_or_default(),
        renewed["NotAfter"].as_f64().unwrap_or_default(),
    );
    assert!(
        now_not_after > was_not_after,
        "case {case:?}: the renewed certificate is valid longer than the one it \
         replaced, was {was_not_after} and is now {now_not_after}"
    );
    assert!(
        renewed["IssuedAt"].as_f64().unwrap_or_default()
            > before["IssuedAt"].as_f64().unwrap_or_default(),
        "case {case:?}: the renewal issued the certificate again, got {renewed}"
    );

    // And the renewal a reader can see is the one the searchable projection reports, so
    // both views read one record.
    let (status, response) = call(
        &router,
        "SearchCertificates",
        &json!({
            "FilterStatement": {
                "Filter": { "AcmCertificateMetadataFilter": { "RenewalStatus": "SUCCESS" } }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SearchCertificates must succeed, got {status} {response}"
    );
    let found: Vec<&str> = response["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Results in {response}"))
        .iter()
        .map(|result| result["CertificateArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        found,
        vec![arn.as_str()],
        "case {case:?}: the completed renewal must be selectable on its status, got \
         {response}"
    );
    assert_eq!(
        response["Results"][0]["CertificateMetadata"]["AcmCertificateMetadata"]["RenewalStatus"]
            .as_str(),
        Some("SUCCESS"),
        "case {case:?}: expected the renewal status in the search result, got {response}"
    );
}

/// RULE 5, the edge of it: a certificate that stopped being issued while its renewal
/// was still owed has nothing left to renew — a revocation is permanent — so the owed
/// renewal ends as one ACM could not complete and the certificate keeps the validity
/// window it had.
#[tokio::test]
async fn a_renewal_owed_to_a_revoked_certificate_does_not_complete() {
    let (router, engine) = router_with_engine();
    let case = "a certificate revoked while its renewal was owed";
    let arn = issued_certificate(
        &router,
        &engine,
        case,
        // Export opted in, so the certificate can be both renewable and revocable.
        &json!({
            "DomainName": "revoked.renew.probe-domain.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;
    mark_exported(&engine, &arn);

    let (status, response) = call(
        &router,
        "RenewCertificate",
        &json!({ "CertificateArn": &arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: renewing an eligible certificate must succeed, got {status} {response}"
    );
    // The renewal is held out while the certificate is revoked under it.
    move_renewal_due(&engine, Duration::from_secs(3600), true);
    let before = describe(&router, &arn).await;

    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": &arn, "RevocationReason": "KEY_COMPROMISE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: revoking an exported certificate must succeed, got {status} {response}"
    );

    move_renewal_due(&engine, Duration::from_millis(1), false);
    let after = describe(&router, &arn).await;
    assert_eq!(
        (
            after["Status"].as_str(),
            after["RenewalSummary"]["RenewalStatus"].as_str()
        ),
        (Some("REVOKED"), Some("FAILED")),
        "case {case:?}: a renewal owed to a revoked certificate cannot complete, got {after}"
    );
    for member in ["NotBefore", "NotAfter", "IssuedAt"] {
        assert_eq!(
            after[member], before[member],
            "case {case:?}: {member} must survive the renewal that could not complete, \
             was {} and is now {}",
            before[member], after[member]
        );
    }
}
