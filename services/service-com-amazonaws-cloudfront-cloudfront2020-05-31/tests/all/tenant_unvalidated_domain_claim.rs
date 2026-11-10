// SPDX-License-Identifier: CC0-1.0

//! When a distribution tenant's domain becomes an enforced CNAME claim, end to end over
//! HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a domain a CloudFront resource holds is an enforced
//! CNAME claim only once the certificate the resource would serve it over has been
//! VALIDATED, so
//!
//! * while the only claimant's CloudFront managed certificate is still
//!   `pending-validation`, a second distribution tenant naming the IDENTICAL domain is
//!   accepted (HTTP 201) under an id of its own, and both tenants read back carrying that
//!   domain — the workflow of staging several tenants against one domain while the
//!   certificate validates;
//! * once the validation the clock owes is due the claim IS enforced: a further create
//!   naming the domain is refused `CNAMEAlreadyExists` (HTTP 409) and stores nothing, as is
//!   a create naming an alias of a distribution, which cannot be declared without a custom
//!   viewer certificate that covers it;
//! * the refusal's message is the fixed sentence CloudFront states this condition in and
//!   carries nothing from the request.
//!
//! The rows differ along everything the rule generalises over: how many domains the first
//! claim holds (one, two, the published per-tenant bound), which of them the second claim
//! duplicates, the case the duplicate is written in, whether the second claim names a free
//! domain beside the duplicated one, and both values the model's `ValidationTokenHost`
//! admits — the `cloudfront` one over a domain this service resolves.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s; the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s and the certificate helpers
//! [`crate::managed_certificate_domain_ownership`]'s. The by-domain read of a staged claim
//! is [`crate::tenant_domain_certificate_gate`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::VALIDATION_TOKEN_HOST_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DOMAIN_STATUS_INACTIVE, MAX_DOMAINS_PER_TENANT, VALIDATION_TOKEN_HOST_CLOUDFRONT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, encode_label, escape, multi_tenant_distribution,
};
use crate::managed_certificate_domain_ownership::{await_certificate_issuance, resolvable_domains};
use crate::response_headers_policy_lifecycle::text;

/// The WIRE sentence `CNAMEAlreadyExists` carries: the WHOLE of CloudFront's text for this
/// condition, which names none of the domains the refused request submitted.
const CNAME_ALREADY_EXISTS_MESSAGE: &str =
    "One or more of the CNAMEs you provided are already associated with a different resource.";

/// The `ValidationTokenHost` that is NOT `cloudfront` — "you serve the validation token
/// from your existing infrastructure", which demands nothing of this service's DNS view, so
/// a create in that mode may claim a name in a customer's zone.
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

// ---------------------------------------------------------------------------
// Calling the surface
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

/// The `CreateDistributionTenantRequest` document for a tenant named `name` claiming
/// `domains`, asking CloudFront to validate them with the token served by `token_host`.
fn create_xml(distribution_id: &str, name: &str, domains: &[String], token_host: &str) -> String {
    let mut out = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{}</Name>",
        escape(distribution_id),
        escape(name)
    );
    out.push_str("<Domains>");
    for domain in domains {
        out.push_str(&format!(
            "<member><Domain>{}</Domain></member>",
            escape(domain)
        ));
    }
    out.push_str("</Domains>");
    // The multi-tenant distribution below declares one REQUIRED parameter with no default,
    // so every create has to carry a value for it.
    out.push_str(&format!(
        "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
        escape(name)
    ));
    out.push_str(&format!(
        "<ManagedCertificateRequest><ValidationTokenHost>{token_host}</ValidationTokenHost>\
         </ManagedCertificateRequest>"
    ));
    out.push_str("</CreateDistributionTenantRequest>");
    out
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One row: the domains a first tenant stages a claim on, and the domains a second tenant
/// claims while that claim is unvalidated.
struct Row {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The slug every name and domain of this row is built from, unique across the table.
    slug: &'static str,
    /// The domains the first claim holds.
    first: Vec<String>,
    /// The domains the second claim submits — at least one of them one of [`Self::first`],
    /// in whatever case the row writes it.
    second: Vec<String>,
    /// The domain of [`Self::first`] both claims name, as the enforced-claim half re-claims
    /// it once the validation is due.
    duplicated: String,
    /// `ManagedCertificateRequest$ValidationTokenHost` both creates ask for.
    token_host: &'static str,
}

/// The table the rule is asserted over, built against the names this service resolves.
async fn table(router: &MultiServiceRouter) -> Vec<Row> {
    // The `cloudfront` token host is admitted only for a domain this service resolves to a
    // CloudFront resource, which is what the last row stages its claim on.
    let resolvable = resolvable_domains(router, "unvalidated-claim", 1).await;
    let self_hosted = self_hosted_token_host();

    vec![
        Row {
            case: "one domain, re-claimed as submitted",
            slug: "one",
            first: vec!["one.claim.example.com".to_string()],
            second: vec!["one.claim.example.com".to_string()],
            duplicated: "one.claim.example.com".to_string(),
            token_host: self_hosted,
        },
        Row {
            case: "two domains, the second of them re-claimed in upper case",
            slug: "pair",
            first: vec![
                "pair-a.claim.example.com".to_string(),
                "pair-b.claim.example.com".to_string(),
            ],
            second: vec!["PAIR-B.CLAIM.EXAMPLE.COM".to_string()],
            duplicated: "pair-b.claim.example.com".to_string(),
            token_host: self_hosted,
        },
        Row {
            case: "as many domains as the per-tenant quota admits, every one re-claimed",
            slug: "quota",
            first: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("quota{slot}.claim.example.com"))
                .collect(),
            second: (0..MAX_DOMAINS_PER_TENANT)
                .map(|slot| format!("quota{slot}.claim.example.com"))
                .collect(),
            duplicated: "quota0.claim.example.com".to_string(),
            token_host: self_hosted,
        },
        Row {
            case: "a duplicated domain beside a free one",
            slug: "mixed",
            first: vec!["mixed.claim.example.com".to_string()],
            second: vec![
                "free.mixed.claim.example.com".to_string(),
                "mixed.claim.example.com".to_string(),
            ],
            duplicated: "mixed.claim.example.com".to_string(),
            token_host: self_hosted,
        },
        Row {
            case: "a resolvable domain, the token served by CloudFront",
            slug: "resolvable",
            first: vec![resolvable[0].clone()],
            second: vec![resolvable[0].clone()],
            duplicated: resolvable[0].clone(),
            token_host: VALIDATION_TOKEN_HOST_CLOUDFRONT,
        },
    ]
}

/// The domains the tenant in `answer` reports, lowercased, with every entry's status
/// asserted to be the one a STAGED claim carries.
///
/// `DomainResult$Status` is "whether the domain is active or inactive", which is about what
/// the domain SERVES: every claim in this suite is still waiting for the certificate it
/// would be served over, so viewers asking for it reach nothing and it reads `inactive` —
/// which is the same reading that makes the claim unenforced here.
fn reported_domains(answer: &Answer, case: &str) -> Vec<String> {
    domains_of(answer.root())
        .into_iter()
        .map(|(domain, status)| {
            assert_eq!(
                status, DOMAIN_STATUS_INACTIVE,
                "case {case}: expected {domain} to be reported {DOMAIN_STATUS_INACTIVE}, got \
                 {status:?}"
            );
            domain.to_lowercase()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// While the only claimant of a domain cannot be served at it — its CloudFront managed
/// certificate is still pending validation — the domain is not taken: a second tenant
/// naming the identical domain is accepted, and both tenants read back carrying it.
#[tokio::test]
async fn an_unvalidated_domain_claim_is_not_enforced() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-unvalidated-claim").await;

    for row in table(&router).await {
        let case = row.case;
        let first_name = format!("{}-first", row.slug);
        let second_name = format!("{}-second", row.slug);

        // Both creates are made before anything else, so the second one is made while the
        // first claim is still waiting on the certificate it asked for.
        let first = create_distribution_tenant(
            &router,
            &create_xml(&distribution_id, &first_name, &row.first, row.token_host),
        )
        .await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected the first claim to be created, got {} with {}",
            first.status, first.raw
        );
        let second = create_distribution_tenant(
            &router,
            &create_xml(&distribution_id, &second_name, &row.second, row.token_host),
        )
        .await;
        assert_eq!(
            second.status, 201,
            "case {case}: expected the second claim on the unvalidated {:?} to be accepted, \
             got {} with {}",
            row.duplicated, second.status, second.raw
        );

        let first_id = text(first.root(), "Id", case, "the first claim", &first.raw);
        let second_id = text(second.root(), "Id", case, "the second claim", &second.raw);
        assert_ne!(
            second_id, first_id,
            "case {case}: expected the second claim to mint an id of its own, got \
             {second_id:?} for both"
        );

        // Both tenants read back independently, each listing the domain they both named.
        for (which, id, submitted) in [
            ("the first claim", &first_id, &row.first),
            ("the second claim", &second_id, &row.second),
        ] {
            let read = get_distribution_tenant(&router, id).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({which}): expected the tenant to be readable, got {} with {}",
                read.status, read.raw
            );
            let reported = reported_domains(&read, case);
            let expected: Vec<String> = submitted
                .iter()
                .map(|domain| domain.to_lowercase())
                .collect();
            assert_eq!(
                reported, expected,
                "case {case} ({which}): expected the domains {expected:?}, got {reported:?}"
            );
            assert!(
                reported.contains(&row.duplicated.to_lowercase()),
                "case {case} ({which}): expected {:?} among {reported:?}",
                row.duplicated
            );
        }
    }
}

/// Once the validation the clock owes is due, the claim IS enforced: a further create
/// naming the domain is refused `CNAMEAlreadyExists` (HTTP 409) with the fixed sentence
/// CloudFront states the condition in, and stores nothing.
#[tokio::test]
async fn a_validated_domain_claim_is_enforced() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-validated-claim").await;

    let mut rows = Vec::new();
    for row in table(&router).await {
        let case = row.case;
        let name = format!("{}-holder", row.slug);
        let created = create_distribution_tenant(
            &router,
            &create_xml(&distribution_id, &name, &row.first, row.token_host),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the claim to be created, got {} with {}",
            created.status, created.raw
        );
        rows.push(row);
    }

    // Nothing in the service sleeps for the issuance; only this test does, to observe the
    // state the clock owes.
    await_certificate_issuance().await;

    for row in rows {
        let case = row.case;
        let name = format!("{}-late", row.slug);
        let refused = create_distribution_tenant(
            &router,
            &create_xml(
                &distribution_id,
                &name,
                std::slice::from_ref(&row.duplicated),
                row.token_host,
            ),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected a claim on the validated {:?} to be refused, got {} with {}",
            row.duplicated, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "CNAMEAlreadyExists",
            "case {case}: expected CNAMEAlreadyExists, got {}",
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, CNAME_ALREADY_EXISTS_MESSAGE,
            "case {case}: expected the fixed CNAME sentence, got {message:?}"
        );
        assert!(
            !message.contains(&row.duplicated),
            "case {case}: expected the message to carry nothing from the request, got \
             {message:?}"
        );

        let read = get_distribution_tenant(&router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the refused create to store nothing, got {} with {}",
            read.status, read.raw
        );
    }
}
