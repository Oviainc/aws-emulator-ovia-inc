// SPDX-License-Identifier: CC0-1.0

//! The CloudFront managed ACM certificate surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are:
//!
//! * a `ManagedCertificateRequest` on a distribution tenant's create REQUESTS a managed
//!   certificate, which the create MINTS (its ACM ARN and the HTTP validation token of
//!   each domain it covers) and persists, so `GetManagedCertificateDetails` answers from
//!   that one record rather than from the request being served — down to the
//!   `ValidationTokenHost` the create chose;
//! * the `Identifier` is "the ARN, ID, or name of the distribution tenant", so all three
//!   forms reach the one certificate;
//! * a certificate CloudFront has just requested is `pending-validation`, and the CLOCK —
//!   not a later call — owes it `issued`, which changes nothing else about the record;
//! * a tenant that asked for no managed certificate, and an identifier no tenant answers
//!   to, are each `EntityNotFound` (HTTP 404).
//!
//! The table rows differ along everything these rules generalise over: both values the
//! model's `ValidationTokenHost` admits, how many domains the certificate covers (one,
//! several, and the published per-tenant bound) and the case they are written in, both
//! values `CertificateTransparencyLoggingPreference` admits and its absence, and whether
//! the request names a `PrimaryDomainName` at all.
//!
//! A validation token host of `cloudfront` asks CloudFront to serve the token at the
//! domain itself, which the create admits only for a domain this service resolves to a
//! CloudFront resource, so those rows claim MINTED routing endpoints
//! ([`crate::managed_certificate_domain_ownership::resolvable_domains`]); that admission
//! rule is what [`crate::managed_certificate_domain_ownership`] is about, and the rows here
//! are about what the certificate then reports.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds; the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES, MANAGED_CERTIFICATE_STATUS_VALUES,
    VALIDATION_TOKEN_HOST_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CERTIFICATE_REGION, CERTIFICATE_STATUS_ISSUED, CERTIFICATE_STATUS_PENDING_VALIDATION,
    CERTIFICATE_VALIDATION_DELAY, MAX_DOMAINS_PER_TENANT, VALIDATION_HOST_SUFFIX,
    VALIDATION_TOKEN_HOST_CLOUDFRONT, VALIDATION_TOKEN_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, Node, ACCOUNT_ID,
    API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, escape, multi_tenant_distribution,
};
use crate::managed_certificate_domain_ownership::resolvable_domains;

// ---------------------------------------------------------------------------
// Calling the certificate surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/managed-certificate/{Identifier}`.
///
/// `pub(crate)` so a sibling suite that asserts which certificate a create minted reads it
/// over this one wire path rather than growing a copy of it.
pub(crate) async fn get_managed_certificate_details(
    router: &MultiServiceRouter,
    identifier: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/managed-certificate/{}",
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

/// One row: a distribution tenant create carrying a `ManagedCertificateRequest`.
struct Request {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, which also seeds its domains.
    name: String,
    /// The domains the tenant claims, which are the domains the certificate covers.
    domains: Vec<String>,
    /// `ManagedCertificateRequest$ValidationTokenHost`, required by the model.
    validation_token_host: &'static str,
    /// `ManagedCertificateRequest$PrimaryDomainName`, when the row names one.
    primary_domain_name: Option<String>,
    /// `ManagedCertificateRequest$CertificateTransparencyLoggingPreference`, when the row
    /// names one.
    transparency_logging: Option<&'static str>,
}

impl Request {
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
            out.push_str(&format!("<member><Domain>{}</Domain></member>", escape(domain)));
        }
        out.push_str("</Domains>");
        // The multi-tenant distribution below defines one REQUIRED parameter with no
        // default, so every create has to carry a value for it.
        out.push_str(&format!(
            "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
            escape(&self.name)
        ));
        out.push_str(&format!(
            "<ManagedCertificateRequest><ValidationTokenHost>{}</ValidationTokenHost>",
            self.validation_token_host
        ));
        if let Some(primary) = &self.primary_domain_name {
            out.push_str(&format!(
                "<PrimaryDomainName>{}</PrimaryDomainName>",
                escape(primary)
            ));
        }
        if let Some(preference) = self.transparency_logging {
            out.push_str(&format!(
                "<CertificateTransparencyLoggingPreference>{preference}\
                 </CertificateTransparencyLoggingPreference>"
            ));
        }
        out.push_str("</ManagedCertificateRequest>");
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }
}

/// The domains one row claims: a MINTED routing endpoint per domain when the row asks
/// CloudFront to serve the validation token — the create admits that mode only for a
/// domain this service resolves to a CloudFront resource — and customer names `label`
/// seeds otherwise.
async fn domains_for(
    router: &MultiServiceRouter,
    host: &str,
    label: &str,
    count: usize,
) -> Vec<String> {
    if host == VALIDATION_TOKEN_HOST_CLOUDFRONT {
        resolvable_domains(router, label, count).await
    } else {
        (0..count)
            .map(|slot| format!("{label}{slot}.example.com"))
            .collect()
    }
}

/// The table every rule below is asserted over.
///
/// `ValidationTokenHost` draws from the model's own catalog
/// (`ops::types::VALIDATION_TOKEN_HOST_VALUES`, the values
/// `smithy_query(query="shape",shape="ValidationTokenHost")` lists), so a value no
/// recorded request named is still covered; `CertificateTransparencyLoggingPreference`
/// draws from `ops::types::CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES` the same
/// way. The rows span one domain, several, and the published per-tenant domain bound
/// (`MAX_DOMAINS_PER_TENANT`), an uppercase domain, and a request that names no primary
/// domain and no transparency preference at all.
async fn request_table(router: &MultiServiceRouter) -> Vec<Request> {
    let mut rows = base_rows();
    // A create asking CLOUDFRONT to serve the validation token is admitted only for a
    // domain this service resolves to a CloudFront resource, so each such row claims one
    // minted routing endpoint per domain instead of a customer name.
    for row in rows
        .iter_mut()
        .filter(|row| row.validation_token_host == VALIDATION_TOKEN_HOST_CLOUDFRONT)
    {
        row.domains = resolvable_domains(router, &row.name, row.domains.len()).await;
        if row.primary_domain_name.is_some() {
            row.primary_domain_name = Some(row.domains[0].clone());
        }
    }
    rows
}

/// The rows as they are written, before the ones that ask CloudFront to serve the
/// validation token are given domains this service resolves.
fn base_rows() -> Vec<Request> {
    let mut rows = Vec::new();
    for (index, host) in VALIDATION_TOKEN_HOST_VALUES.iter().enumerate() {
        let tag = host.replace('-', "");
        rows.push(Request {
            case: "one domain, nothing optional",
            name: format!("cert-one-{tag}"),
            domains: vec![format!("one-{tag}.example.com")],
            validation_token_host: host,
            primary_domain_name: None,
            transparency_logging: None,
        });
        rows.push(Request {
            case: "three domains, a primary domain and a transparency preference",
            name: format!("cert-three-{tag}"),
            domains: vec![
                format!("a-{tag}.example.com"),
                format!("B-{tag}.Example.com"),
                format!("c-{tag}.example.net"),
            ],
            validation_token_host: host,
            primary_domain_name: Some(format!("a-{tag}.example.com")),
            transparency_logging: Some(
                CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES
                    [index % CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES.len()],
            ),
        });
        rows.push(Request {
            case: "the published per-tenant domain bound",
            name: format!("cert-bound-{tag}"),
            domains: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("bound{slot}-{tag}.example.com"))
                .collect(),
            validation_token_host: host,
            primary_domain_name: None,
            transparency_logging: Some(
                CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES
                    [(index + 1) % CERTIFICATE_TRANSPARENCY_LOGGING_PREFERENCE_VALUES.len()],
            ),
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// The text of one member of the `ManagedCertificateDetails` document, or a panic naming
/// the case.
fn member(details: &Node, name: &str, case: &str, raw: &str) -> String {
    details
        .text_at(name)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {name}: {raw}"))
        .to_string()
}

/// The `(Domain, RedirectFrom, RedirectTo)` of every `ValidationTokenDetails` entry, in
/// document order.
fn tokens_of(details: &Node) -> Vec<(String, String, String)> {
    details
        .at("ValidationTokenDetails")
        .map(|list| {
            list.children
                .iter()
                .map(|entry| {
                    (
                        entry.text_at("Domain").unwrap_or_default().to_string(),
                        entry.text_at("RedirectFrom").unwrap_or_default().to_string(),
                        entry.text_at("RedirectTo").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the certificate document `answer` carries is the one `request` asked for, in
/// `status`, and answer `(CertificateArn, the token file name of each domain)`.
fn assert_certificate(
    answer: &Answer,
    request: &Request,
    status: &str,
    context: &str,
) -> (String, Vec<String>) {
    let case = request.case;
    let raw = answer.raw.as_str();
    assert_eq!(
        answer.status, 200,
        "case {case} ({context}): expected HTTP 200, got {} with {raw}",
        answer.status
    );
    let details = answer.root();

    // The ARN is MINTED: an ACM certificate ARN in the one region CloudFront serves
    // viewers over certificates from, whose last segment is a generated id.
    let arn = member(details, "CertificateArn", case, raw);
    let prefix = format!("arn:aws:acm:{CERTIFICATE_REGION}:{ACCOUNT_ID}:certificate/");
    let certificate_id = arn.strip_prefix(&prefix).unwrap_or_default().to_string();
    assert!(
        is_minted_uuid(&certificate_id),
        "case {case} ({context}): expected a minted {prefix}<id> ARN, got {arn:?}"
    );

    let reported_status = member(details, "CertificateStatus", case, raw);
    assert_eq!(
        reported_status, status,
        "case {case} ({context}): expected CertificateStatus {status:?}, got \
         {reported_status:?}"
    );
    assert!(
        MANAGED_CERTIFICATE_STATUS_VALUES.contains(&reported_status.as_str()),
        "case {case} ({context}): expected a value of the model's ManagedCertificateStatus \
         ({MANAGED_CERTIFICATE_STATUS_VALUES:?}), got {reported_status:?}"
    );

    // "Specify how the HTTP validation token will be served": the mode the request chose
    // comes back unchanged.
    let host = member(details, "ValidationTokenHost", case, raw);
    assert_eq!(
        host, request.validation_token_host,
        "case {case} ({context}): expected the submitted ValidationTokenHost, got {host:?}"
    );

    // One validation token per domain the certificate covers, in the order the create
    // listed them, each pointing from the well-known path under its own domain to where
    // the token's content is served.
    let tokens = tokens_of(details);
    let domains: Vec<String> = tokens.iter().map(|(domain, ..)| domain.clone()).collect();
    assert_eq!(
        domains, request.domains,
        "case {case} ({context}): expected a validation token per submitted domain, got \
         {domains:?}"
    );
    let mut names = Vec::new();
    for (domain, redirect_from, redirect_to) in &tokens {
        let expected_prefix = format!("{domain}{VALIDATION_TOKEN_PATH}");
        let name = redirect_from
            .strip_prefix(&expected_prefix)
            .and_then(|tail| tail.strip_suffix(".txt"))
            .unwrap_or_else(|| {
                panic!(
                    "case {case} ({context}): expected RedirectFrom under \
                     {expected_prefix:?}, got {redirect_from:?}"
                )
            })
            .to_string();
        assert!(
            !name.is_empty(),
            "case {case} ({context}): expected a token name in RedirectFrom, got \
             {redirect_from:?}"
        );
        assert_eq!(
            *redirect_to,
            format!("{name}.{VALIDATION_HOST_SUFFIX}{VALIDATION_TOKEN_PATH}{name}.txt"),
            "case {case} ({context}): expected RedirectTo to serve the same token, got \
             {redirect_to:?}"
        );
        names.push(name);
    }

    (arn, names)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A `ManagedCertificateRequest` on a create MINTS the certificate the read then reports:
/// an ACM ARN, the `ValidationTokenHost` that was chosen, one validation token per domain,
/// and the non-terminal status a just-requested certificate is in. Across the table no two
/// certificates share an ARN and no two domains share a validation token, and a second
/// read answers the same document — a read mints nothing.
#[tokio::test]
async fn a_request_mints_the_certificate_the_read_reports() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-certificate-mint").await;
    let mut arns = BTreeSet::new();
    let mut tokens = BTreeSet::new();

    for request in request_table(&router).await {
        let case = request.case;
        let created = create_distribution_tenant(&router, &request.xml(&distribution_id)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let tenant_id = created
            .root()
            .text_at("Id")
            .unwrap_or_else(|| panic!("case {case}: the create carries no Id"))
            .to_string();

        let first = get_managed_certificate_details(&router, &tenant_id).await;
        let (arn, names) = assert_certificate(
            &first,
            &request,
            CERTIFICATE_STATUS_PENDING_VALIDATION,
            "straight after the create",
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: the ARN {arn} was minted for another certificate too"
        );
        for name in &names {
            assert!(
                tokens.insert(name.clone()),
                "case {case}: the validation token {name} was minted for another domain too"
            );
        }

        // Reading does not mint: the second read answers the same document.
        let second = get_managed_certificate_details(&router, &tenant_id).await;
        let (again, names_again) = assert_certificate(
            &second,
            &request,
            CERTIFICATE_STATUS_PENDING_VALIDATION,
            "read a second time",
        );
        assert_eq!(
            (&again, &names_again),
            (&arn, &names),
            "case {case}: a second read answered a different certificate"
        );
    }
}

/// The `Identifier` is "the ARN, ID, or name of the distribution tenant": all three forms
/// reach the one certificate, and an identifier no tenant answers to is `EntityNotFound`
/// (HTTP 404) whatever form it takes.
#[tokio::test]
async fn every_tenant_identifier_form_reaches_the_one_certificate() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-certificate-identifier").await;
    let host = VALIDATION_TOKEN_HOST_VALUES[0];
    let request = Request {
        case: "addressed three ways",
        name: "cert-identifier".to_string(),
        domains: domains_for(&router, host, "cert-identifier", 1).await,
        validation_token_host: host,
        primary_domain_name: None,
        transparency_logging: None,
    };
    let created = create_distribution_tenant(&router, &request.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        request.case, created.status, created.raw
    );
    let tenant = created.root();
    let id = tenant
        .text_at("Id")
        .expect("the create carries an Id")
        .to_string();
    let arn = tenant
        .text_at("Arn")
        .expect("the create carries an Arn")
        .to_string();

    let mut answered = BTreeSet::new();
    for (form, identifier) in [("the id", &id), ("the ARN", &arn), ("the name", &request.name)] {
        let answer = get_managed_certificate_details(&router, identifier).await;
        let (certificate_arn, names) = assert_certificate(
            &answer,
            &request,
            CERTIFICATE_STATUS_PENDING_VALIDATION,
            form,
        );
        answered.insert((certificate_arn, names));
    }
    assert_eq!(
        answered.len(),
        1,
        "case {}: the three identifier forms answered {} different certificates: {answered:?}",
        request.case,
        answered.len()
    );

    // Rows: an identifier of a tenant id's shape that names nothing, a name no tenant
    // holds, and a tenant ARN no tenant holds.
    let absent: [(&str, String, u16); 3] = [
        ("an unused tenant id", "dt_ABSENTTENANT12".to_string(), 404),
        ("a name no tenant holds", "cert-absent".to_string(), 404),
        (
            "a tenant ARN no tenant holds",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution-tenant/dt_ABSENTTENANT12"),
            404,
        ),
    ];
    for (case, identifier, status) in absent {
        let answer = get_managed_certificate_details(&router, &identifier).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        let code = answer.error_code();
        assert_eq!(
            code, "EntityNotFound",
            "case {case}: expected EntityNotFound, got {code:?} with {}",
            answer.raw
        );
    }
}

/// A certificate CloudFront has just requested is `pending-validation`, and the CLOCK owes
/// it `issued`: no call asks for the transition, and the transition changes nothing else
/// about the record — not the ARN, and not a single validation token.
#[tokio::test]
async fn the_clock_settles_a_certificate_to_issued() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-certificate-clock").await;

    for host in VALIDATION_TOKEN_HOST_VALUES {
        let label = format!("clock-{}", host.replace('-', ""));
        let request = Request {
            case: "validation settles",
            name: format!("cert-clock-{}", host.replace('-', "")),
            domains: domains_for(&router, host, &label, 2).await,
            validation_token_host: host,
            primary_domain_name: None,
            transparency_logging: None,
        };
        let created = create_distribution_tenant(&router, &request.xml(&distribution_id)).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from the create, got {} with {}",
            request.case, created.status, created.raw
        );
        let tenant_id = created
            .root()
            .text_at("Id")
            .expect("the create carries an Id")
            .to_string();

        let pending = get_managed_certificate_details(&router, &tenant_id).await;
        let (arn, names) = assert_certificate(
            &pending,
            &request,
            CERTIFICATE_STATUS_PENDING_VALIDATION,
            "straight after the request",
        );

        tokio::time::sleep(CERTIFICATE_VALIDATION_DELAY + Duration::from_millis(250)).await;

        let issued = get_managed_certificate_details(&router, &tenant_id).await;
        let (issued_arn, issued_names) =
            assert_certificate(&issued, &request, CERTIFICATE_STATUS_ISSUED, "past the delay");
        assert_eq!(
            (&issued_arn, &issued_names),
            (&arn, &names),
            "case {}: the issuance changed the certificate's identity",
            request.case
        );

        // The terminal status is terminal: a later read does not move it again.
        let still = get_managed_certificate_details(&router, &tenant_id).await;
        assert_certificate(&still, &request, CERTIFICATE_STATUS_ISSUED, "read again");
    }
}

/// A tenant whose create asked for no managed certificate has none: the read is
/// `EntityNotFound` (HTTP 404), while the tenant itself is served.
#[tokio::test]
async fn a_tenant_that_asked_for_no_certificate_has_none() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-certificate-absent").await;
    let case = "no managed certificate request";
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>cert-none</Name>\
         <Domains><member><Domain>none.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>cert-none</Value></member>\
         </Parameters></CreateDistributionTenantRequest>"
    );
    let created = create_distribution_tenant(&router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let tenant_id = created
        .root()
        .text_at("Id")
        .expect("the create carries an Id")
        .to_string();

    for identifier in [tenant_id.as_str(), "cert-none"] {
        let answer = get_managed_certificate_details(&router, identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case} ({identifier}): expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        let code = answer.error_code();
        assert_eq!(
            code, "EntityNotFound",
            "case {case} ({identifier}): expected EntityNotFound, got {code:?} with {}",
            answer.raw
        );
    }
}
