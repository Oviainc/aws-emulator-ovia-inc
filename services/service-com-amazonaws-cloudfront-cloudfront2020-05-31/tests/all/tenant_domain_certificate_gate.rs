// SPDX-License-Identifier: CC0-1.0

//! Which domains `GetDistributionTenantByDomain` resolves, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: the by-domain read resolves the HOST-to-tenant map a
//! viewer's request is routed through, and CloudFront can only route a domain once the
//! certificate it is served over exists — so
//!
//! * while a tenant's CloudFront managed certificate is still `pending-validation`, every
//!   domain it covers answers `EntityNotFound` (HTTP 404) with the one sentence a read of an
//!   unknown tenant carries, even though the tenant itself is readable by id — and because
//!   nothing routes the domain yet, it is not taken either: a second tenant naming the
//!   identical domain is ACCEPTED (the duplicate-claim rule itself is
//!   [`crate::tenant_unvalidated_domain_claim`]'s);
//! * once the issuance the clock owes is due, the same read answers HTTP 200 with that
//!   tenant's `Id` and its current version — in any case the domain is written in, and the
//!   EARLIEST of several claims on one domain is the one it routes to;
//! * a tenant that asked for NO managed certificate waits for nothing: its domains resolve
//!   from the moment the create returns.
//!
//! The rows differ along everything the rule generalises over: how many domains one tenant
//! claims (one, two, the published per-tenant bound), both values the model's
//! `ValidationTokenHost` admits, a create that asks for no certificate at all, and the case
//! each domain is written in.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s; the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s, the resolvable-name helper
//! [`crate::managed_certificate_domain_ownership`]'s and the certificate read
//! [`crate::managed_certificate_details`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::VALIDATION_TOKEN_HOST_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CERTIFICATE_STATUS_ISSUED, CERTIFICATE_STATUS_PENDING_VALIDATION, MAX_DOMAINS_PER_TENANT,
    VALIDATION_TOKEN_HOST_CLOUDFRONT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, escape, multi_tenant_distribution,
};
use crate::managed_certificate_details::get_managed_certificate_details;
use crate::managed_certificate_domain_ownership::{
    await_certificate_issuance, resolvable_domains,
};
use crate::response_headers_policy_lifecycle::text;

/// The sentence every read of a distribution tenant nothing resolves answers with.
const TENANT_NOT_FOUND_MESSAGE: &str = "The specified distribution tenant doesn't exist.";

// ---------------------------------------------------------------------------
// Calling the reads
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/distribution-tenant?domain={Domain}`.
async fn get_by_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distribution-tenant?domain={}",
            encode_label(domain)
        ),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Identifier}`.
async fn get_by_identifier(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distribution-tenant/{}",
            encode_label(identifier)
        ),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One row: a distribution tenant create, and the certificate (if any) it asks for.
struct Case {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, unique across the table.
    name: String,
    /// The domains the create claims.
    domains: Vec<String>,
    /// `ManagedCertificateRequest$ValidationTokenHost`, or `None` for a create that asks
    /// for no managed certificate at all.
    validation_token_host: Option<&'static str>,
}

impl Case {
    /// The `CreateDistributionTenantRequest` XML a client sends for this row.
    fn xml(&self, distribution_id: &str) -> String {
        let mut out = format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{}</DistributionId><Name>{}</Name>",
            escape(distribution_id),
            escape(&self.name)
        );
        out.push_str("<Domains>");
        for domain in &self.domains {
            out.push_str(&format!(
                "<member><Domain>{}</Domain></member>",
                escape(domain)
            ));
        }
        out.push_str("</Domains>");
        // The multi-tenant distribution below declares one REQUIRED parameter with no
        // default, so every create has to carry a value for it.
        out.push_str(&format!(
            "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
            escape(&self.name)
        ));
        if let Some(host) = self.validation_token_host {
            out.push_str(&format!(
                "<ManagedCertificateRequest><ValidationTokenHost>{host}</ValidationTokenHost>\
                 </ManagedCertificateRequest>"
            ));
        }
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }
}

/// The token host that is NOT `cloudfront` — "you serve the validation token from your
/// existing infrastructure", which demands nothing of this service's DNS view, so a create
/// in that mode may claim a name in a customer's zone.
///
/// Read off the model's own catalog (`ops::types::VALIDATION_TOKEN_HOST_VALUES`, which
/// `smithy_query(query="shape",shape="ValidationTokenHost")` reports the same values for)
/// rather than written out here, so it cannot drift from the model.
fn self_hosted_token_host() -> &'static str {
    VALIDATION_TOKEN_HOST_VALUES
        .iter()
        .copied()
        .find(|host| *host != VALIDATION_TOKEN_HOST_CLOUDFRONT)
        .expect("the model's ValidationTokenHost admits a host other than cloudfront")
}

/// The table the rule is asserted over, built against the names this service resolves.
async fn case_table(router: &MultiServiceRouter) -> Vec<Case> {
    // The `cloudfront` token host is admitted only for a domain this service resolves to a
    // CloudFront resource, and a create that asks for no certificate at all needs the same
    // proof of the account's control of the domain.
    let endpoints = resolvable_domains(router, "certificate-gate", 2).await;

    vec![
        Case {
            case: "one customer domain, the token served by the caller",
            name: "certificate-gate-one".to_string(),
            domains: vec!["one.example.com".to_string()],
            validation_token_host: Some(self_hosted_token_host()),
        },
        Case {
            case: "two customer domains, the token served by the caller",
            name: "certificate-gate-two".to_string(),
            domains: vec![
                "two-a.example.com".to_string(),
                "two-b.example.com".to_string(),
            ],
            validation_token_host: Some(self_hosted_token_host()),
        },
        Case {
            case: "as many domains as the per-tenant quota admits",
            name: "certificate-gate-quota".to_string(),
            domains: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("quota{slot}.gate.example.com"))
                .collect(),
            validation_token_host: Some(self_hosted_token_host()),
        },
        Case {
            case: "a resolvable domain, the token served by CloudFront",
            name: "certificate-gate-cloudfront".to_string(),
            domains: vec![endpoints[0].clone()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
        },
        Case {
            case: "a resolvable domain and no certificate at all",
            name: "certificate-gate-none".to_string(),
            domains: vec![endpoints[1].clone()],
            validation_token_host: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A domain is resolved to its tenant only once CloudFront can serve it: while the managed
/// certificate the create asked for is pending validation the by-domain read answers
/// `EntityNotFound`, and once the issuance the clock owes is due the same read answers the
/// tenant. A create that asked for no certificate resolves its domains at once.
#[tokio::test]
async fn a_domain_resolves_to_its_tenant_once_its_certificate_is_issued() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-certificate-gate").await;

    for row in case_table(&router).await {
        let case = row.case;
        let created = create_distribution_tenant(&router, &row.xml(&distribution_id)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the created tenant", &created.raw);
        let etag = created.header("etag").to_string();

        let certificate = get_managed_certificate_details(&router, &row.name).await;
        match row.validation_token_host {
            Some(_) => {
                let status = text(
                    certificate.root(),
                    "CertificateStatus",
                    case,
                    "the requested certificate",
                    &certificate.raw,
                );
                assert_eq!(
                    status, CERTIFICATE_STATUS_PENDING_VALIDATION,
                    "case {case}: expected a just-requested certificate to be \
                     {CERTIFICATE_STATUS_PENDING_VALIDATION}, got {status:?}"
                );

                // The gate: every domain the pending certificate covers resolves to
                // nothing, in every case it is written in.
                for domain in &row.domains {
                    for (form, written) in [
                        ("as submitted", domain.clone()),
                        ("in upper case", domain.to_uppercase()),
                        ("in lower case", domain.to_lowercase()),
                    ] {
                        let answer = get_by_domain(&router, &written).await;
                        assert_eq!(
                            answer.status, 404,
                            "case {case} ({domain} {form}): expected HTTP 404 while the \
                             certificate is pending, got {} with {}",
                            answer.status, answer.raw
                        );
                        assert_eq!(
                            answer.error_code(),
                            "EntityNotFound",
                            "case {case} ({domain} {form}): expected EntityNotFound, got {}",
                            answer.raw
                        );
                        let message = answer.root().find_text("Message").unwrap_or_default();
                        assert_eq!(
                            message, TENANT_NOT_FOUND_MESSAGE,
                            "case {case} ({domain} {form}): expected the unknown-tenant \
                             sentence, got {message:?}"
                        );
                    }
                }

                // The tenant itself IS there — it is the host-to-tenant map that does not
                // hold the domain yet, not the record.
                let by_id = get_by_identifier(&router, &id).await;
                assert_eq!(
                    by_id.status, 200,
                    "case {case}: expected the tenant to be readable by id while its \
                     certificate is pending, got {} with {}",
                    by_id.status, by_id.raw
                );

                // And because nothing routes the domain while the claim waits on its
                // certificate, the domain is not taken: a second tenant naming it is
                // accepted, under an id of its own.
                let second = Case {
                    case,
                    name: format!("{}-second", row.name),
                    domains: vec![row.domains[0].clone()],
                    validation_token_host: Some(self_hosted_token_host()),
                };
                let accepted =
                    create_distribution_tenant(&router, &second.xml(&distribution_id)).await;
                assert_eq!(
                    accepted.status, 201,
                    "case {case}: expected a second claim on the unvalidated {:?} to be \
                     accepted, got {} with {}",
                    row.domains[0], accepted.status, accepted.raw
                );
                let second_id = text(
                    accepted.root(),
                    "Id",
                    case,
                    "the second tenant",
                    &accepted.raw,
                );
                assert_ne!(
                    second_id, id,
                    "case {case}: expected the second claim to mint its own id, got {second_id:?}"
                );

                await_certificate_issuance().await;

                let issued = get_managed_certificate_details(&router, &row.name).await;
                let status = text(
                    issued.root(),
                    "CertificateStatus",
                    case,
                    "the issued certificate",
                    &issued.raw,
                );
                assert_eq!(
                    status, CERTIFICATE_STATUS_ISSUED,
                    "case {case}: expected the clock to owe {CERTIFICATE_STATUS_ISSUED}, got \
                     {status:?}"
                );
            }
            None => assert_eq!(
                certificate.status, 404,
                "case {case}: expected no certificate for a create that asked for none, got {} \
                 with {}",
                certificate.status, certificate.raw
            ),
        }

        // Now every domain resolves to this tenant, at the version the create handed back,
        // in every case it is written in — including the one a second tenant also claimed
        // while this one's certificate was pending: the EARLIEST claim on a domain is the
        // one it routes to.
        for domain in &row.domains {
            for (form, written) in [
                ("as submitted", domain.clone()),
                ("in upper case", domain.to_uppercase()),
                ("in lower case", domain.to_lowercase()),
            ] {
                let answer = get_by_domain(&router, &written).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case} ({domain} {form}): expected the tenant to be resolved, got {} \
                     with {}",
                    answer.status, answer.raw
                );
                let read_id = text(answer.root(), "Id", case, "read by domain", &answer.raw);
                assert_eq!(
                    read_id, id,
                    "case {case} ({domain} {form}): expected the tenant {id}, got {read_id:?}"
                );
                assert_eq!(
                    answer.header("etag"),
                    etag,
                    "case {case} ({domain} {form}): expected the version the create handed back \
                     ({etag}), got {:?}",
                    answer.header("etag")
                );
            }
        }
    }
}
