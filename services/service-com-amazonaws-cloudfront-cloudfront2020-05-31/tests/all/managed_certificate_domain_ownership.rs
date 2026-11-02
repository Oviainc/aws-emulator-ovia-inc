// SPDX-License-Identifier: CC0-1.0

//! Who serves the HTTP validation token of a CloudFront managed ACM certificate, and what
//! that demands of the domain's DNS — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test:
//!
//! * `ManagedCertificateRequest$ValidationTokenHost` decides WHO serves the token. For
//!   `cloudfront`, "CloudFront will automatically serve the validation token. Choose this
//!   mode if you can point the domain's DNS to CloudFront immediately" — serving it AT the
//!   domain is only possible once the domain resolves to a CloudFront resource, so the
//!   create admits that mode only for domains this service resolves to one and otherwise
//!   refuses, synchronously, with `InvalidArgument` (HTTP 400). For `self-hosted`, "you
//!   serve the validation token from your existing infrastructure", so the mode demands
//!   nothing of DNS and is admitted whatever the domain is.
//! * A create that asks for NO certificate asks CloudFront to validate nothing, so it is
//!   admitted only for a domain this service already resolves to a CloudFront resource:
//!   with no proof of the account's control of the domain, the create is refused with
//!   `InvalidArgument` (HTTP 400) too, worded with the sentence that condition carries.
//! * A refusal persists NOTHING: neither the tenant, nor its domains, nor a certificate —
//!   so the domain is still unclaimed afterwards and the same request with the other token
//!   host is accepted.
//!
//! The DNS view is the one and only one this service has: the names it MINTS — a
//! connection group's routing endpoint and a distribution's own edge domain — which is
//! also what `VerifyDnsConfiguration` reads a record for.
//!
//! The rows differ along everything the rule generalises over: both values the model's
//! `ValidationTokenHost` admits and a request that names no certificate at all, which
//! CloudFront resource the domain resolves to (a connection group's routing endpoint, a
//! distribution's edge domain, nothing at all), the case the name is written in, how many
//! domains the certificate covers (one, several, and the published per-tenant bound), a
//! mixture of a resolvable and an unresolvable domain in one request, whether the request
//! names a `PrimaryDomainName` and whether that name resolves, and a domain at the
//! published whole-name bound.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds; the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::VALIDATION_TOKEN_HOST_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CERTIFICATE_STATUS_PENDING_VALIDATION, CERTIFICATE_VALIDATION_DELAY,
    DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE, MAX_DOMAINS_PER_TENANT,
    MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE, VALIDATION_TOKEN_HOST_CLOUDFRONT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, Answer, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, escape, longest_domain, multi_tenant_distribution,
};
use crate::managed_certificate_details::get_managed_certificate_details;
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// Calling the operations under test
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/distribution-tenant?domain={Domain}` — the read that says whether the
/// create persisted anything for a domain.
async fn get_distribution_tenant_by_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
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

/// Wait out the span the clock owes a CloudFront managed certificate its issuance in.
///
/// A domain the certificate covers is not in the host-to-tenant map until the certificate
/// is issued, so a by-domain read of it answers only once the clock has made the issuance
/// due ([`CERTIFICATE_VALIDATION_DELAY`]). Nothing in the service sleeps for it; only this
/// test does, to observe the state the clock owes.
///
/// `pub(crate)` so every sibling suite whose create asks CloudFront to validate a domain
/// waits for that issuance the same way rather than growing a copy of this.
pub(crate) async fn await_certificate_issuance() {
    tokio::time::sleep(CERTIFICATE_VALIDATION_DELAY + Duration::from_millis(100)).await;
}

/// `GET /2020-05-31/distribution-tenant/{Identifier}` — the read that says whether the
/// create persisted a tenant under the name it named.
async fn get_distribution_tenant(router: &MultiServiceRouter, identifier: &str) -> Answer {
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
// The domains the cases are built from
// ---------------------------------------------------------------------------

/// Mint `count` domain names this service RESOLVES to a CloudFront resource: the routing
/// endpoint of one freshly created connection group each, "also known as the DNS name …
/// such as `d111111abcdef8.cloudfront.net`".
///
/// `pub(crate)` so a sibling suite whose create asks CloudFront to serve the validation
/// token claims resolvable domains the same way rather than growing a copy of this.
pub(crate) async fn resolvable_domains(
    router: &MultiServiceRouter,
    label: &str,
    count: usize,
) -> Vec<String> {
    let mut endpoints = Vec::new();
    for slot in 0..count {
        let name = format!("{label}-{slot}");
        let body = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{}</Name>\
             </CreateConnectionGroupRequest>",
            escape(&name)
        );
        let created = create_connection_group(router, &body).await;
        assert_eq!(
            created.status, 201,
            "case {name}: expected the connection group to be created, got {} with {}",
            created.status, created.raw
        );
        endpoints.push(text(
            created.root(),
            "RoutingEndpoint",
            &name,
            "the created connection group",
            &created.raw,
        ));
    }
    endpoints
}

/// The edge `DomainName` this service minted for a STANDARD distribution: another name it
/// is authoritative for, and so a domain that resolves to a CloudFront resource without
/// being any connection group's routing endpoint.
///
/// `pub(crate)` so a sibling suite that needs the other kind of resolvable name reaches it
/// the same way rather than growing a copy of this.
pub(crate) async fn distribution_domain_name(router: &MultiServiceRouter, reference: &str) -> String {
    let created = create_distribution(router, &config("standard", reference).xml()).await;
    assert_eq!(
        created.status, 201,
        "case {reference}: expected the distribution to be created, got {} with {}",
        created.status, created.raw
    );
    text(
        created.root(),
        "DomainName",
        reference,
        "the created distribution",
        &created.raw,
    )
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One row: a distribution tenant create, and whether the rule admits it.
struct Case {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, unique across the table.
    name: String,
    /// The domains the tenant claims, which are the domains a certificate covers.
    domains: Vec<String>,
    /// `ManagedCertificateRequest$ValidationTokenHost`, or `None` for a create that asks
    /// for no managed certificate at all.
    validation_token_host: Option<&'static str>,
    /// `ManagedCertificateRequest$PrimaryDomainName`, when the row names one.
    primary_domain_name: Option<String>,
    /// Whether the rule admits this request.
    admitted: bool,
}

impl Case {
    /// The `CreateDistributionTenantRequest` XML a client sends for this row, naming
    /// `host` as the validation token host (the row's own, or the other one for the
    /// control re-submit).
    fn xml(&self, distribution_id: &str, host: Option<&str>) -> String {
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
        // The multi-tenant distribution below defines one REQUIRED parameter with no
        // default, so every create has to carry a value for it.
        out.push_str(&format!(
            "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
            escape(&self.name)
        ));
        if let Some(host) = host {
            out.push_str(&format!(
                "<ManagedCertificateRequest><ValidationTokenHost>{host}</ValidationTokenHost>"
            ));
            if let Some(primary) = &self.primary_domain_name {
                out.push_str(&format!(
                    "<PrimaryDomainName>{}</PrimaryDomainName>",
                    escape(primary)
                ));
            }
            out.push_str("</ManagedCertificateRequest>");
        }
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }

    /// The sentence this row's refusal carries: a create that asks CloudFront to serve the
    /// validation token names the condition that the domain does not point at a CloudFront
    /// resource, while a create that asks CloudFront to validate NOTHING names the
    /// REQUIREMENT it did not satisfy — a certificate covering the domain.
    fn refusal_message(&self) -> &'static str {
        match self.validation_token_host {
            Some(_) => DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE,
            None => MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE,
        }
    }
}

/// The table the rule is asserted over, built against the names this service resolves.
async fn case_table(router: &MultiServiceRouter) -> Vec<Case> {
    // Enough minted routing endpoints for every row that needs a resolvable domain, plus
    // a distribution's own edge domain: two CloudFront resource kinds a domain can resolve
    // to.
    let endpoints = resolvable_domains(router, "ownership", 11).await;
    let edge_domain = distribution_domain_name(router, "ref-ownership-standard").await;
    let unresolvable = |slot: usize| format!("unresolvable{slot}.example.com");

    vec![
        Case {
            case: "cloudfront, a connection group's routing endpoint",
            name: "ownership-endpoint".to_string(),
            domains: vec![endpoints[0].clone()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: true,
        },
        Case {
            case: "cloudfront, the same kind of endpoint in upper case",
            name: "ownership-endpoint-upper".to_string(),
            domains: vec![endpoints[1].to_ascii_uppercase()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: true,
        },
        Case {
            case: "cloudfront, a distribution's own edge domain",
            name: "ownership-edge".to_string(),
            domains: vec![edge_domain.clone()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: true,
        },
        Case {
            case: "cloudfront, the published per-tenant domain bound, all resolvable",
            name: "ownership-bound".to_string(),
            domains: endpoints[2..2 + MAX_DOMAINS_PER_TENANT].to_vec(),
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: Some(endpoints[2].clone()),
            admitted: true,
        },
        Case {
            case: "cloudfront, a domain nothing resolves",
            name: "ownership-unresolvable".to_string(),
            domains: vec![unresolvable(0)],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: false,
        },
        Case {
            case: "cloudfront, one resolvable domain beside one nothing resolves",
            name: "ownership-mixed".to_string(),
            domains: vec![endpoints[7].clone(), unresolvable(1)],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: false,
        },
        Case {
            case: "cloudfront, a resolvable domain and a primary domain nothing resolves",
            name: "ownership-primary".to_string(),
            domains: vec![endpoints[8].clone()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: Some(unresolvable(2)),
            admitted: false,
        },
        Case {
            case: "cloudfront, a domain at the published whole-name bound",
            name: "ownership-longest".to_string(),
            domains: vec![longest_domain()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: None,
            admitted: false,
        },
        Case {
            case: "self-hosted, a domain nothing resolves",
            name: "ownership-self-hosted".to_string(),
            domains: vec![unresolvable(3)],
            validation_token_host: Some(other_validation_token_host()),
            primary_domain_name: Some(unresolvable(3)),
            admitted: true,
        },
        Case {
            case: "self-hosted, the published per-tenant domain bound, none resolvable",
            name: "ownership-self-hosted-bound".to_string(),
            domains: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("self-hosted{slot}.example.net"))
                .collect(),
            validation_token_host: Some(other_validation_token_host()),
            primary_domain_name: None,
            admitted: true,
        },
        Case {
            case: "no managed certificate request, a domain nothing resolves",
            name: "ownership-no-certificate".to_string(),
            domains: vec![unresolvable(4)],
            validation_token_host: None,
            primary_domain_name: None,
            admitted: false,
        },
        Case {
            case: "no managed certificate request, a routing endpoint that resolves",
            name: "ownership-no-certificate-resolvable".to_string(),
            domains: vec![endpoints[10].clone()],
            validation_token_host: None,
            primary_domain_name: None,
            admitted: true,
        },
        Case {
            case: "cloudfront, a resolvable domain named as the primary domain too",
            name: "ownership-primary-resolvable".to_string(),
            domains: vec![endpoints[9].clone()],
            validation_token_host: Some(VALIDATION_TOKEN_HOST_CLOUDFRONT),
            primary_domain_name: Some(endpoints[9].clone()),
            admitted: true,
        },
    ]
}

/// The value of `ValidationTokenHost` that is NOT the one asking CloudFront to serve the
/// token, read off the model's own catalog rather than written out here.
fn other_validation_token_host() -> &'static str {
    VALIDATION_TOKEN_HOST_VALUES
        .iter()
        .copied()
        .find(|host| *host != VALIDATION_TOKEN_HOST_CLOUDFRONT)
        .expect("the model's ValidationTokenHost admits a host other than cloudfront")
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create that asks CloudFront to serve the HTTP validation token is admitted only for
/// domains this service resolves to a CloudFront resource, and so is a create that asks for
/// no certificate at all; the other token host asks CloudFront to validate the domain
/// itself, and is admitted whatever the domain is. An admitted create is readable back by
/// every domain it claims; a refused one persists NOTHING — no tenant under its name, no
/// claim on any of its domains — and the identical request with the other token host is
/// then accepted, which is what keys the refusal on who validates the domain rather than on
/// the domain string.
#[tokio::test]
async fn cloudfront_hosted_validation_demands_a_domain_that_resolves_to_cloudfront() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-ownership").await;

    for row in case_table(&router).await {
        let case = row.case;
        let created =
            create_distribution_tenant(&router, &row.xml(&distribution_id, row.validation_token_host))
                .await;

        if row.admitted {
            assert_eq!(
                created.status, 201,
                "case {case}: expected HTTP 201, got {} with {}",
                created.status, created.raw
            );
            let tenant = created.root();
            let id = text(tenant, "Id", case, "the created tenant", &created.raw);
            let etag = created.header("etag").to_string();

            // The certificate the create asked for — if it asked for one — reports the
            // token host it chose, and is still awaiting validation.
            let certificate = get_managed_certificate_details(&router, &row.name).await;
            match row.validation_token_host {
                Some(host) => {
                    assert_eq!(
                        certificate.status, 200,
                        "case {case}: expected the requested certificate to be served, got {} \
                         with {}",
                        certificate.status, certificate.raw
                    );
                    let reported = text(
                        certificate.root(),
                        "ValidationTokenHost",
                        case,
                        "the minted certificate",
                        &certificate.raw,
                    );
                    assert_eq!(
                        reported, host,
                        "case {case}: expected the submitted ValidationTokenHost, got \
                         {reported:?}"
                    );
                    let status = text(
                        certificate.root(),
                        "CertificateStatus",
                        case,
                        "the minted certificate",
                        &certificate.raw,
                    );
                    assert_eq!(
                        status, CERTIFICATE_STATUS_PENDING_VALIDATION,
                        "case {case}: expected a just-requested certificate to be \
                         {CERTIFICATE_STATUS_PENDING_VALIDATION}, got {status:?}"
                    );

                    // While that certificate is pending, CloudFront cannot serve the
                    // domains it covers, so they are not in the host-to-tenant map yet.
                    for domain in &row.domains {
                        let pending = get_distribution_tenant_by_domain(&router, domain).await;
                        assert_eq!(
                            pending.status, 404,
                            "case {case} ({domain}): expected HTTP 404 while the certificate is \
                             {CERTIFICATE_STATUS_PENDING_VALIDATION}, got {} with {}",
                            pending.status, pending.raw
                        );
                        assert_eq!(
                            pending.error_code(),
                            "EntityNotFound",
                            "case {case} ({domain}): expected EntityNotFound while the \
                             certificate is pending, got {}",
                            pending.raw
                        );
                    }
                    // The issuance the clock owes makes the domains servable.
                    await_certificate_issuance().await;
                }
                None => assert_eq!(
                    certificate.status, 404,
                    "case {case}: expected no certificate for a create that asked for none, got \
                     {} with {}",
                    certificate.status, certificate.raw
                ),
            }

            // Every domain the create claimed reaches the one stored tenant.
            for domain in &row.domains {
                let answer = get_distribution_tenant_by_domain(&router, domain).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case} ({domain}): expected the tenant to be readable by its domain, \
                     got {} with {}",
                    answer.status, answer.raw
                );
                let read_id = text(answer.root(), "Id", case, "read by domain", &answer.raw);
                assert_eq!(
                    read_id, id,
                    "case {case} ({domain}): expected the created tenant {id}, got {read_id:?}"
                );
                assert_eq!(
                    answer.header("etag"),
                    etag,
                    "case {case} ({domain}): expected the version token the create minted \
                     ({etag}), got {:?}",
                    answer.header("etag")
                );
            }
            continue;
        }

        // The refusal: the modelled 400, worded with the one sentence this condition
        // carries, which interpolates nothing from the request.
        assert_eq!(
            created.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            created.status, created.raw
        );
        assert_eq!(
            created.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            created.raw
        );
        let message = created.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message,
            row.refusal_message(),
            "case {case}: expected the sentence this condition carries, got {message:?}"
        );

        // Nothing was persisted: no tenant under that name, and no claim on any domain.
        let by_name = get_distribution_tenant(&router, &row.name).await;
        assert_eq!(
            by_name.status, 404,
            "case {case}: expected the refused tenant not to exist, got {} with {}",
            by_name.status, by_name.raw
        );
        for domain in &row.domains {
            let answer = get_distribution_tenant_by_domain(&router, domain).await;
            assert_eq!(
                answer.status, 404,
                "case {case} ({domain}): expected the domain to be unclaimed, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "EntityNotFound",
                "case {case} ({domain}): expected EntityNotFound, got {}",
                answer.raw
            );
        }
        let certificate = get_managed_certificate_details(&router, &row.name).await;
        assert_eq!(
            certificate.status, 404,
            "case {case}: expected no certificate to have been minted, got {} with {}",
            certificate.status, certificate.raw
        );

        // The control: the identical request with the other token host is accepted — so
        // neither the name nor any domain was taken by the refusal, and the refusal is
        // keyed on who serves the validation token.
        let accepted = create_distribution_tenant(
            &router,
            &row.xml(&distribution_id, Some(other_validation_token_host())),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected the same request with the other validation token host to be \
             accepted, got {} with {}",
            accepted.status, accepted.raw
        );
        // The accepted re-submit asked CloudFront for a managed certificate, so the domains
        // it covers reach the tenant once the clock has made its issuance due — one wait for
        // the one transition, whatever the number of domains.
        await_certificate_issuance().await;
        for domain in &row.domains {
            let answer = get_distribution_tenant_by_domain(&router, domain).await;
            assert_eq!(
                answer.status, 200,
                "case {case} ({domain}): expected the accepted tenant to be readable by its \
                 domain, got {} with {}",
                answer.status, answer.raw
            );
        }
    }
}

/// A domain no tenant claims is `EntityNotFound` (HTTP 404) with the one sentence every
/// read of an unknown tenant answers — whatever shape the domain takes, and whether or not
/// the store holds tenants at all. The reader is case-insensitive, as every DNS name
/// comparison in this service is.
#[tokio::test]
async fn a_domain_no_tenant_claims_is_not_found() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-ownership-absent").await;
    let row = Case {
        case: "the one tenant in the store",
        name: "ownership-absent-tenant".to_string(),
        domains: vec!["Claimed.Example.com".to_string()],
        validation_token_host: Some(other_validation_token_host()),
        primary_domain_name: None,
        admitted: true,
    };
    let created =
        create_distribution_tenant(&router, &row.xml(&distribution_id, row.validation_token_host))
            .await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", row.case, "the created tenant", &created.raw);

    // The claimed name reaches the tenant in any case it is written in, once the managed
    // certificate the create asked for has been issued.
    await_certificate_issuance().await;
    for (case, domain) in [
        ("the claimed domain as submitted", "Claimed.Example.com"),
        ("the claimed domain in lower case", "claimed.example.com"),
        ("the claimed domain in upper case", "CLAIMED.EXAMPLE.COM"),
    ] {
        let answer = get_distribution_tenant_by_domain(&router, domain).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let read_id = text(answer.root(), "Id", case, "read by domain", &answer.raw);
        assert_eq!(
            read_id, id,
            "case {case}: expected the created tenant {id}, got {read_id:?}"
        );
    }

    // Rows: a sibling of the claimed name, a parent zone of it, a minted CloudFront
    // endpoint no tenant claims, a name at the published whole-name bound, and a value
    // that is no domain name at all.
    let endpoint = resolvable_domains(&router, "ownership-absent", 1).await;
    let cases: Vec<(&str, String)> = vec![
        (
            "a sibling of the claimed domain",
            "other.example.com".to_string(),
        ),
        ("the parent zone of it", "example.com".to_string()),
        (
            "a routing endpoint no tenant claims",
            endpoint[0].clone(),
        ),
        ("a name at the whole-name bound", longest_domain()),
        ("a value of no domain name's shape", "!!".to_string()),
    ];
    for (case, domain) in cases {
        let answer = get_distribution_tenant_by_domain(&router, &domain).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, "The specified distribution tenant doesn't exist.",
            "case {case}: expected the unknown-tenant sentence, got {message:?}"
        );
    }
}
