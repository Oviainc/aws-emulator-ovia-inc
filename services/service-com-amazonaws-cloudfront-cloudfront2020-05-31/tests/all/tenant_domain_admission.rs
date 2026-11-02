// SPDX-License-Identifier: CC0-1.0

//! Which domains a distribution tenant may be BOUND to, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test:
//!
//! * CloudFront binds a domain to a distribution tenant only when the account's control of
//!   that domain is PROVEN to it: either the same call asks CloudFront to validate the
//!   domain (a `ManagedCertificateRequest`, whichever host serves the validation token) or
//!   the domain already resolves to a CloudFront resource in this account — the one proof
//!   this service's own DNS view holds. An ACM certificate the caller names in
//!   `Customizations$Certificate$Arn` is NOT proof: an ARN says nothing about whether the
//!   certificate's domains were ever validated, and a certificate imported into ACM
//!   carries no domain validation at all.
//! * Every domain the request names has to be proven, so one unproven domain beside a
//!   proven one is refused.
//! * An unproven domain is refused with `InvalidArgument` (HTTP 400) and NOTHING is
//!   persisted: no tenant under the name, and no claim on any domain the request named —
//!   so a read of the domain is still `EntityNotFound` (HTTP 404) afterwards, and the
//!   identical request that asks CloudFront to validate the domain is then accepted.
//!
//! The rows differ along everything the rule generalises over: which proof the request
//! carries (none, a certificate customization only, a certificate request, a domain this
//! service resolves), which CloudFront resource a resolvable domain is (a connection
//! group's routing endpoint, a distribution's edge domain), how many domains the request
//! names (one, two, the published per-tenant bound), the mixture of proven and unproven
//! domains in one request, the case a name is written in, the shortest and the longest
//! domain name the published bounds admit, and a name that merely LOOKS like one
//! CloudFront mints.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds; the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s and the resolvable-name helpers
//! [`crate::managed_certificate_domain_ownership`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_TENANT_ID_PREFIX, MAX_DOMAINS_PER_TENANT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, encode_label, escape, longest_domain,
    managed_certificate_request, multi_tenant_distribution,
};
use crate::managed_certificate_domain_ownership::{
    await_certificate_issuance, distribution_domain_name, resolvable_domains,
};
use crate::response_headers_policy_lifecycle::text;

/// The sentence a tenant create claiming a domain no certificate of the account covers is
/// refused with: the REQUIREMENT the caller has to satisfy. Spelled out here as a literal
/// rather than read back off the crate's constant, so the WORDING is what these rows are
/// held to.
const MISSING_CERTIFICATE_SENTENCE: &str =
    "There must be at least one certificate in the distribution tenant or distribution, or \
     there must be a managed certificate request";

/// The sentence the same create is refused with when a certificate IS attached — one named by
/// ARN, which validates no domain this service can see — so what the refusal is about is the
/// domain rather than the certificate. A literal here for the same reason.
const INVALID_DOMAIN_SENTENCE: &str = "The request contains an invalid domain name.";

// ---------------------------------------------------------------------------
// Calling the reads that say what the create persisted
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/distribution-tenant/{Identifier}`.
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

/// `GET /2020-05-31/distribution-tenant?domain={Domain}`.
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

/// The sentence every read of a distribution tenant nothing holds answers with.
const TENANT_NOT_FOUND_MESSAGE: &str = "The specified distribution tenant doesn't exist.";

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// An ACM certificate ARN a caller may name in `Customizations$Certificate$Arn` — "the
/// Amazon Resource Name (ARN) of the ACM certificate"
/// (`smithy_query(query="shape",shape="Certificate")`). Naming one is not proof of anything
/// about the domain, which is what the rows carrying one assert.
fn certificate_arn(region: &str, id: &str) -> String {
    format!("arn:aws:acm:{region}:000000000000:certificate/{id}")
}

/// One row: a distribution tenant create, and whether the rule admits it.
struct Case {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, unique across the table.
    name: String,
    /// The domains the create claims.
    domains: Vec<String>,
    /// `Customizations$Certificate$Arn`, when the row names one.
    certificate_arn: Option<String>,
    /// Whether the create carries a `ManagedCertificateRequest` — the call by which
    /// CloudFront is asked to validate the domains itself.
    certificate_request: bool,
    /// Whether the rule admits this request.
    admitted: bool,
}

impl Case {
    /// The `CreateDistributionTenantRequest` XML a client sends for this row, optionally
    /// forcing the certificate request on (the control re-submit of a refused row).
    fn xml(&self, distribution_id: &str, certificate_request: bool) -> String {
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
        // The multi-tenant distribution below marks one parameter required with no default,
        // so every create has to carry a value for it.
        out.push_str(&format!(
            "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
            escape(&self.name)
        ));
        if let Some(arn) = &self.certificate_arn {
            out.push_str(&format!(
                "<Customizations><Certificate><Arn>{}</Arn></Certificate></Customizations>",
                escape(arn)
            ));
        }
        if certificate_request {
            out.push_str(&managed_certificate_request());
        }
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }
}

/// The table the rule is asserted over, built against the names this service resolves.
async fn case_table(router: &MultiServiceRouter) -> Vec<Case> {
    // Enough minted routing endpoints for every row that needs proven domains, plus a
    // standard distribution's own edge domain: the two kinds of CloudFront resource a
    // domain can resolve to here.
    let endpoints = resolvable_domains(router, "admission", MAX_DOMAINS_PER_TENANT + 4).await;
    let edge_domain = distribution_domain_name(router, "ref-admission-standard").await;

    vec![
        Case {
            case: "a certificate ARN for a domain nothing proves",
            name: "admission-certificate-only".to_string(),
            domains: vec!["unproven.example.com".to_string()],
            certificate_arn: Some(certificate_arn(
                "us-west-2",
                "11111111-2222-3333-4444-555555555555",
            )),
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "a certificate ARN in the region CloudFront serves from, still no proof",
            name: "admission-certificate-east".to_string(),
            domains: vec!["unproven-east.example.com".to_string()],
            certificate_arn: Some(certificate_arn(
                "us-east-1",
                "66666666-7777-8888-9999-aaaaaaaaaaaa",
            )),
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "no proof at all, one domain",
            name: "admission-bare".to_string(),
            domains: vec!["bare.example.com".to_string()],
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "no proof at all, the shortest domain name",
            name: "admission-shortest".to_string(),
            domains: vec!["a.b".to_string()],
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "no proof at all, a domain at the published whole-name bound",
            name: "admission-longest".to_string(),
            domains: vec![longest_domain()],
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "no proof at all, a name shaped like one CloudFront mints",
            name: "admission-lookalike".to_string(),
            domains: vec!["d111111abcdef8.cloudfront.net".to_string()],
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "no proof at all, as many domains as the quota admits",
            name: "admission-quota".to_string(),
            domains: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("quota{slot}.example.com"))
                .collect(),
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "one proven domain beside one nothing proves",
            name: "admission-mixed".to_string(),
            domains: vec![endpoints[0].clone(), "mixed.example.com".to_string()],
            certificate_arn: None,
            certificate_request: false,
            admitted: false,
        },
        Case {
            case: "a routing endpoint this service resolves",
            name: "admission-endpoint".to_string(),
            domains: vec![endpoints[1].clone()],
            certificate_arn: None,
            certificate_request: false,
            admitted: true,
        },
        Case {
            case: "the same routing endpoint written in upper case",
            name: "admission-endpoint-upper".to_string(),
            domains: vec![endpoints[2].to_ascii_uppercase()],
            certificate_arn: None,
            certificate_request: false,
            admitted: true,
        },
        Case {
            case: "a distribution's own edge domain",
            name: "admission-edge".to_string(),
            domains: vec![edge_domain.clone()],
            certificate_arn: None,
            certificate_request: false,
            admitted: true,
        },
        Case {
            case: "as many resolvable domains as the quota admits",
            name: "admission-quota-proven".to_string(),
            domains: endpoints[3..3 + MAX_DOMAINS_PER_TENANT].to_vec(),
            certificate_arn: None,
            certificate_request: false,
            admitted: true,
        },
        Case {
            case: "a certificate request for a domain nothing else proves",
            name: "admission-requested".to_string(),
            domains: vec!["requested.example.com".to_string()],
            certificate_arn: None,
            certificate_request: true,
            admitted: true,
        },
        Case {
            case: "a certificate request beside a certificate ARN, two unproven domains",
            name: "admission-requested-customized".to_string(),
            domains: vec![
                "requested-one.example.com".to_string(),
                "requested-two.example.com".to_string(),
            ],
            certificate_arn: Some(certificate_arn(
                "us-east-1",
                "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
            )),
            certificate_request: true,
            admitted: true,
        },
        Case {
            case: "a certificate request for a domain at the published whole-name bound",
            name: "admission-requested-longest".to_string(),
            domains: vec![format!("x{}", &longest_domain()[1..])],
            certificate_arn: None,
            certificate_request: true,
            admitted: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A tenant is bound to a domain only on proof of the account's control of it: a request
/// that asks CloudFront to validate the domain, or a domain this service already resolves
/// to a CloudFront resource, is admitted and round-trips through a read; anything else —
/// including a request whose only claim is an ACM certificate ARN — is refused with
/// `InvalidArgument` (HTTP 400) and the sentence that condition carries, persists nothing,
/// and is accepted once the same request asks CloudFront to validate the domain.
#[tokio::test]
async fn a_tenant_is_bound_only_to_a_domain_the_account_has_proven() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-admission").await;

    for row in case_table(&router).await {
        let case = row.case;
        let answer =
            create_distribution_tenant(&router, &row.xml(&distribution_id, row.certificate_request))
                .await;

        if row.admitted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected HTTP 201, got {} with {}",
                answer.status, answer.raw
            );
            let id = text(answer.root(), "Id", case, "the created tenant", &answer.raw);
            assert!(
                id.starts_with(DISTRIBUTION_TENANT_ID_PREFIX),
                "case {case}: expected a minted tenant id, got {id:?}"
            );

            // The record the create stored is what the read answers from, down to the
            // domains it claimed.
            let read = get_distribution_tenant(&router, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the created tenant to be readable, got {} with {}",
                read.status, read.raw
            );
            let claimed: Vec<String> = domains_of(read.root())
                .into_iter()
                .map(|(domain, _)| domain)
                .collect();
            assert_eq!(
                claimed, row.domains,
                "case {case}: expected the domains the create claimed, got {claimed:?}"
            );
            // A domain whose validation the create asked CloudFront for is served only once
            // the managed certificate it waits on has been issued — one wait for the one
            // transition, whatever the number of domains.
            if row.certificate_request {
                await_certificate_issuance().await;
            }
            for domain in &row.domains {
                let by_domain = get_distribution_tenant_by_domain(&router, domain).await;
                assert_eq!(
                    by_domain.status, 200,
                    "case {case} ({domain}): expected the tenant to be readable by its domain, \
                     got {} with {}",
                    by_domain.status, by_domain.raw
                );
                let read_id = text(by_domain.root(), "Id", case, "read by domain", &by_domain.raw);
                assert_eq!(
                    read_id, id,
                    "case {case} ({domain}): expected the created tenant {id}, got {read_id:?}"
                );
            }
            continue;
        }

        // The refusal: the modelled 400, worded with the sentence this condition carries for
        // what the request had to hand — the REQUIREMENT it did not meet where nothing names
        // a certificate at all, and the DOMAIN where a certificate is attached that validates
        // nothing for it. Neither interpolates anything from the request.
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        let expected = if row.certificate_arn.is_some() {
            INVALID_DOMAIN_SENTENCE
        } else {
            MISSING_CERTIFICATE_SENTENCE
        };
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case}: expected the sentence {expected:?}, got {message:?}"
        );

        // Nothing was persisted: no tenant under the name, and no claim on any domain the
        // refused request named.
        let by_name = get_distribution_tenant(&router, &row.name).await;
        assert_eq!(
            by_name.status, 404,
            "case {case}: expected the refused tenant not to exist, got {} with {}",
            by_name.status, by_name.raw
        );
        for domain in &row.domains {
            let by_domain = get_distribution_tenant_by_domain(&router, domain).await;
            assert_eq!(
                by_domain.status, 404,
                "case {case} ({domain}): expected the domain to be unclaimed, got {} with {}",
                by_domain.status, by_domain.raw
            );
            assert_eq!(
                by_domain.error_code(),
                "EntityNotFound",
                "case {case} ({domain}): expected EntityNotFound, got {}",
                by_domain.raw
            );
            let message = by_domain.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, TENANT_NOT_FOUND_MESSAGE,
                "case {case} ({domain}): expected the unknown-tenant sentence, got {message:?}"
            );
        }

        // The control: the identical request, now asking CloudFront to validate the
        // domains, is accepted — so neither the name nor any domain was spent by the
        // refusal, and the refusal is keyed on the missing proof rather than on the domain
        // string.
        let accepted = create_distribution_tenant(&router, &row.xml(&distribution_id, true)).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected the same request with a certificate request to be accepted, \
             got {} with {}",
            accepted.status, accepted.raw
        );
        // The re-submit asked CloudFront to validate the domains, so the by-domain read
        // answers once the clock has made the certificate's issuance due.
        await_certificate_issuance().await;
        for domain in &row.domains {
            let by_domain = get_distribution_tenant_by_domain(&router, domain).await;
            assert_eq!(
                by_domain.status, 200,
                "case {case} ({domain}): expected the accepted tenant to be readable by its \
                 domain, got {} with {}",
                by_domain.status, by_domain.raw
            );
        }
    }
}
