// SPDX-License-Identifier: CC0-1.0

//! `ListDomainConflicts`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * the operation reports the domain associations the STORE holds that conflict with the
//!   domain asked about — a distribution's alternate domain names and a distribution
//!   tenant's domains, the two kinds `DistributionResourceType` admits. Two names conflict
//!   when CloudFront cannot route them separately: they are the same name (compared as DNS
//!   names are), or one of them is a wildcard alternate domain name that covers the other,
//!   which replaces exactly the leftmost label — so `*.example.com` conflicts with
//!   `www.example.com` but not with `deep.sub.example.com`;
//! * `DomainControlValidationResource` is the caller's proof that it controls the domain:
//!   it names ONE distribution or ONE distribution tenant whose attached TLS certificate
//!   covers that domain. A resource no record answers to is `EntityNotFound`, and one that
//!   holds no such certificate is `InvalidArgument` — a CloudFront managed certificate
//!   still waiting for its domains to be validated covers nothing yet, a certificate
//!   covers a name under a wildcard it was issued for but not the wildcard that covers a
//!   name it was issued for, and CloudFront's own default certificate covers no alternate
//!   domain name at all;
//! * the conflicts travel in ONE stable order, cut to at most `MaxItems` entries beginning
//!   after the token the previous page handed back, and `NextMarker` is present exactly
//!   when conflicts remain — so walking the pages at ANY admissible page size enumerates
//!   them exactly once;
//! * a read changes nothing: the same request answers the same way twice.
//!
//! What the rows span: the domain asked about (an exact association, a name a wildcard
//! covers, the wildcard itself, a name two labels under a wildcard, a name nothing holds,
//! and the same name in another case), how many conflicts answer it (none, one, several),
//! which resource kind validates the domain and which certificate it holds (a distribution
//! on a custom certificate and on CloudFront's own, a tenant on a managed certificate while
//! it is pending and once it is issued, a tenant on a wildcard managed certificate, a
//! tenant on a certificate named by ARN), the resource-id members a request names (each
//! alone, neither, both), every page size from one up past the stored set plus this
//! service's page ceiling, and the markers a request may carry.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the tenant helpers are
//! [`crate::distribution_tenant_lifecycle`]'s and the resolvable-name and
//! certificate-issuance helpers [`crate::managed_certificate_domain_ownership`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::DISTRIBUTION_RESOURCE_TYPE_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION, DISTRIBUTION_RESOURCE_TYPE_TENANT,
    INVALID_DOMAIN_NAME_MESSAGE, MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE,
    NO_SUCH_DISTRIBUTION_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, Answer, Node, ACCOUNT_ID, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::managed_certificate_domain_ownership::{await_certificate_issuance, resolvable_domains};
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// The sentences the refusals carry
// ---------------------------------------------------------------------------

/// The sentence a validation resource that holds no valid certificate for the domain asked
/// about is refused with — CloudFront's own wording for that condition.
const CERTIFICATE_DOES_NOT_COVER: &str =
    "There must be at least one certificate in the distribution tenant or distribution, or \
     there must be a managed certificate request";

/// The sentence a `DomainControlValidationResource` that names no single resource is
/// refused with.
const ONE_VALIDATION_RESOURCE: &str =
    "DomainControlValidationResource must specify either DistributionId or \
     DistributionTenantId, but not both in the same request.";

/// The sentence a `Marker` that is no pagination token this operation handed out is refused
/// with.
const INVALID_MARKER: &str = "Marker is not a pagination token returned by ListDomainConflicts";

/// The sentence every read of a distribution tenant nothing resolves answers with.
const TENANT_NOT_FOUND: &str = "The specified distribution tenant doesn't exist.";

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// The `DomainControlValidationResource` a request names.
#[derive(Debug, Clone, Copy)]
enum Resource<'a> {
    /// `DistributionId` alone.
    Distribution(&'a str),
    /// `DistributionTenantId` alone.
    Tenant(&'a str),
    /// An element naming neither member.
    Neither,
    /// Both members at once.
    Both(&'a str, &'a str),
}

impl Resource<'_> {
    /// The element as XML, in the member order the shape declares.
    fn xml(&self) -> String {
        let inner = match self {
            Resource::Distribution(id) => {
                format!("<DistributionId>{}</DistributionId>", escape(id))
            }
            Resource::Tenant(id) => {
                format!("<DistributionTenantId>{}</DistributionTenantId>", escape(id))
            }
            Resource::Neither => String::new(),
            Resource::Both(distribution, tenant) => format!(
                "<DistributionId>{}</DistributionId>\
                 <DistributionTenantId>{}</DistributionTenantId>",
                escape(distribution),
                escape(tenant)
            ),
        };
        format!("<DomainControlValidationResource>{inner}</DomainControlValidationResource>")
    }
}

/// `POST /2020-05-31/domain-conflicts` with the request document the arguments describe.
async fn list_domain_conflicts(
    router: &MultiServiceRouter,
    domain: &str,
    resource: Resource<'_>,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> Answer {
    let mut body = format!(
        "<ListDomainConflictsRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Domain>{}</Domain>{}",
        escape(domain),
        resource.xml()
    );
    if let Some(max_items) = max_items {
        body.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    if let Some(marker) = marker {
        body.push_str(&format!("<Marker>{}</Marker>", escape(marker)));
    }
    body.push_str("</ListDomainConflictsRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/domain-conflicts"),
        Some(&body),
        None,
    )
    .await
}

/// One reported conflict: its `Domain`, `ResourceType` and `ResourceId`.
type Conflict = (String, String, String);

/// Every conflict the answer reports, in document order.
///
/// A `DomainConflict` is the one element of this document that carries both a `Domain` and
/// a `ResourceId`, so they are collected by SHAPE rather than by the element name the list
/// wraps its members in. Each one's `AccountId` is asserted here, against the account every
/// emulated resource belongs to: it is the same value on every entry, so it is no part of
/// what a row compares.
fn conflicts_of(answer: &Answer, case: &str) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = Vec::new();
    collect_conflicts(answer.root(), &mut out, case, &answer.raw);
    out
}

/// The depth-first walk [`conflicts_of`] collects through.
fn collect_conflicts(node: &Node, out: &mut Vec<Conflict>, case: &str, raw: &str) {
    if node.child("Domain").is_some() && node.child("ResourceId").is_some() {
        let account_id = text(node, "AccountId", case, "a reported conflict", raw);
        assert_eq!(
            account_id, ACCOUNT_ID,
            "case {case}: expected every conflict to name the account that holds the \
             resource ({ACCOUNT_ID}), got {account_id:?} in {raw}"
        );
        let resource_type = text(node, "ResourceType", case, "a reported conflict", raw);
        assert!(
            DISTRIBUTION_RESOURCE_TYPE_VALUES.contains(&resource_type.as_str()),
            "case {case}: expected a ResourceType the model admits \
             ({DISTRIBUTION_RESOURCE_TYPE_VALUES:?}), got {resource_type:?} in {raw}"
        );
        out.push((
            text(node, "Domain", case, "a reported conflict", raw),
            resource_type,
            text(node, "ResourceId", case, "a reported conflict", raw),
        ));
        return;
    }
    for child in &node.children {
        collect_conflicts(child, out, case, raw);
    }
}

/// The `NextMarker` the answer carries, or `""` when it carries none.
fn next_marker(answer: &Answer) -> String {
    answer
        .root()
        .child("NextMarker")
        .map(|node| node.text().to_string())
        .unwrap_or_default()
}

/// The order the store reports conflicts in: the conflicting domain as DNS names compare,
/// then the id of the resource holding it.
fn conflict_order(conflict: &Conflict) -> (String, String) {
    (conflict.0.to_lowercase(), conflict.2.clone())
}

// ---------------------------------------------------------------------------
// The resources the conflicts are read from
// ---------------------------------------------------------------------------

/// Create a distribution tenant holding `domains` and answer its id.
///
/// Every create carries a `ManagedCertificateRequest` unless `customizations` names a
/// certificate: a tenant is bound to a name in a customer's zone only when the call asks
/// CloudFront to validate it, and the certificate ARN path is the one the caller's own
/// resolvable domain is used for.
async fn create_tenant(
    router: &MultiServiceRouter,
    distribution_id: &str,
    name: &str,
    domains: &[&str],
    customizations: Option<&str>,
) -> String {
    let mut body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{}</Name>",
        escape(distribution_id),
        escape(name)
    );
    body.push_str("<Domains>");
    for domain in domains {
        body.push_str(&format!(
            "<member><Domain>{}</Domain></member>",
            escape(domain)
        ));
    }
    body.push_str("</Domains>");
    // The multi-tenant distribution below marks one parameter required with no default, so
    // every create has to carry a value for it.
    body.push_str(&format!(
        "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
        escape(name)
    ));
    match customizations {
        Some(customizations) => body.push_str(customizations),
        None => body.push_str(&managed_certificate_request()),
    }
    body.push_str("</CreateDistributionTenantRequest>");

    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {name}: expected the distribution tenant to be created, got {} with {}",
        created.status, created.raw
    );
    text(created.root(), "Id", name, "the created tenant", &created.raw)
}

/// Create a standard distribution carrying `aliases` and answer its id.
///
/// A configuration that declares an alternate domain name names a CUSTOM viewer certificate
/// (the harness' `Config` supplies it), and one that declares none is served on
/// CloudFront's own default certificate — the two certificate cases a validation resource
/// of this kind can be in.
async fn create_standard_distribution(
    router: &MultiServiceRouter,
    reference: &str,
    aliases: &[&str],
) -> String {
    let mut configuration = config("domain-conflicts", reference);
    configuration.aliases = aliases.iter().map(|alias| (*alias).to_string()).collect();
    let created = create_distribution(router, &configuration.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {reference}: expected the distribution to be created, got {} with {}",
        created.status, created.raw
    );
    text(
        created.root(),
        "Id",
        reference,
        "the created distribution",
        &created.raw,
    )
}

/// The zone the associations under test are in.
const ZONE: &str = "conflict.example.com";

/// The associations one emulator holds: every resource kind that can claim a domain, and
/// the ids a row names them by.
struct Fixture {
    /// The distribution whose alias is the WILDCARD over [`ZONE`].
    wildcard_distribution: String,
    /// The distribution whose alias is one exact name in [`ZONE`].
    alias_distribution: String,
    /// The tenant holding `www.<ZONE>`.
    www_tenant: String,
    /// The tenant holding the same zone's `MiXeD.<ZONE>`, written in mixed case.
    mixed_tenant: String,
    /// The tenant holding a name TWO labels under [`ZONE`], which the wildcard does not
    /// cover.
    deep_tenant: String,
    /// The tenant holding a name in another zone entirely.
    other_tenant: String,
}

impl Fixture {
    /// Create every resource the reporting rows read from.
    async fn create(router: &MultiServiceRouter) -> Self {
        let multi_tenant = multi_tenant_distribution(router, "ref-domain-conflicts").await;
        let fixture = Self {
            wildcard_distribution: create_standard_distribution(
                router,
                "ref-conflicts-wildcard",
                &[&format!("*.{ZONE}")],
            )
            .await,
            alias_distribution: create_standard_distribution(
                router,
                "ref-conflicts-alias",
                &[&format!("dup.{ZONE}")],
            )
            .await,
            www_tenant: create_tenant(
                router,
                &multi_tenant,
                "conflicts-www",
                &[&format!("www.{ZONE}")],
                None,
            )
            .await,
            mixed_tenant: create_tenant(
                router,
                &multi_tenant,
                "conflicts-mixed",
                &[&format!("MiXeD.{ZONE}")],
                None,
            )
            .await,
            deep_tenant: create_tenant(
                router,
                &multi_tenant,
                "conflicts-deep",
                &[&format!("deep.sub.{ZONE}")],
                None,
            )
            .await,
            other_tenant: create_tenant(
                router,
                &multi_tenant,
                "conflicts-other",
                &["elsewhere.example.net"],
                None,
            )
            .await,
        };
        // The managed certificates the tenant creates asked for are issued by the clock,
        // which is what lets a tenant validate a domain of its own below.
        await_certificate_issuance().await;
        fixture
    }

    /// The conflicts the rows expect, in the order the store reports them.
    fn expected(&self, entries: &[(String, &'static str, &str)]) -> Vec<Conflict> {
        let mut expected: Vec<Conflict> = entries
            .iter()
            .map(|(domain, resource_type, resource_id)| {
                (
                    domain.clone(),
                    (*resource_type).to_string(),
                    (*resource_id).to_string(),
                )
            })
            .collect();
        expected.sort_by_key(conflict_order);
        expected
    }
}

// ---------------------------------------------------------------------------
// Which associations conflict
// ---------------------------------------------------------------------------

/// The reported conflicts are the stored associations that cannot be routed separately from
/// the domain asked about, over both resource kinds, and nothing else.
#[tokio::test]
async fn the_conflicts_reported_are_the_stored_associations_that_overlap_the_domain() {
    let router = emulator();
    let fixture = Fixture::create(&router).await;

    let wildcard = format!("*.{ZONE}");
    let www = format!("www.{ZONE}");
    let mixed = format!("MiXeD.{ZONE}");
    let dup = format!("dup.{ZONE}");
    let deep = format!("deep.sub.{ZONE}");

    // Each row: what it is, the domain asked about, and every association that conflicts
    // with it.
    let rows: Vec<(&str, String, Vec<Conflict>)> = vec![
        (
            "an exact association and the wildcard that covers it",
            www.clone(),
            fixture.expected(&[
                (www.clone(), DISTRIBUTION_RESOURCE_TYPE_TENANT, &fixture.www_tenant),
                (
                    wildcard.clone(),
                    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                    &fixture.wildcard_distribution,
                ),
            ]),
        ),
        (
            "the same name in another case",
            www.to_uppercase(),
            fixture.expected(&[
                (www.clone(), DISTRIBUTION_RESOURCE_TYPE_TENANT, &fixture.www_tenant),
                (
                    wildcard.clone(),
                    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                    &fixture.wildcard_distribution,
                ),
            ]),
        ),
        (
            "an association stored in mixed case, asked about in upper case",
            mixed.to_uppercase(),
            fixture.expected(&[
                (mixed.clone(), DISTRIBUTION_RESOURCE_TYPE_TENANT, &fixture.mixed_tenant),
                (
                    wildcard.clone(),
                    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                    &fixture.wildcard_distribution,
                ),
            ]),
        ),
        (
            "the wildcard itself, which covers every one-label name in the zone",
            wildcard.clone(),
            fixture.expected(&[
                (
                    wildcard.clone(),
                    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                    &fixture.wildcard_distribution,
                ),
                (
                    dup.clone(),
                    DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                    &fixture.alias_distribution,
                ),
                (www.clone(), DISTRIBUTION_RESOURCE_TYPE_TENANT, &fixture.www_tenant),
                (mixed.clone(), DISTRIBUTION_RESOURCE_TYPE_TENANT, &fixture.mixed_tenant),
            ]),
        ),
        (
            "a name two labels under the wildcard, which it does not cover",
            deep.clone(),
            fixture.expected(&[(
                deep.clone(),
                DISTRIBUTION_RESOURCE_TYPE_TENANT,
                &fixture.deep_tenant,
            )]),
        ),
        (
            "a name in another zone",
            "elsewhere.example.net".to_string(),
            fixture.expected(&[(
                "elsewhere.example.net".to_string(),
                DISTRIBUTION_RESOURCE_TYPE_TENANT,
                &fixture.other_tenant,
            )]),
        ),
        (
            "a name nothing in this account holds",
            "unclaimed.example.org".to_string(),
            Vec::new(),
        ),
        (
            "a wildcard over a zone nothing in this account holds",
            "*.example.org".to_string(),
            Vec::new(),
        ),
    ];

    for (case, domain, expected) in rows {
        // The validation resource is the distribution on a custom certificate: the rule
        // under test here is which associations are REPORTED, so every row is asked with a
        // resource that validates the domain.
        let answer = list_domain_conflicts(
            &router,
            &domain,
            Resource::Distribution(&fixture.wildcard_distribution),
            Some(MAX_ITEMS),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({domain}): expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let reported = conflicts_of(&answer, case);
        assert_eq!(
            reported, expected,
            "case {case:?} ({domain}): expected the conflicts {expected:?}, got {reported:?}"
        );
        assert_eq!(
            next_marker(&answer),
            "",
            "case {case:?} ({domain}): a page that left nothing behind names no resume \
             point, got {}",
            answer.raw
        );

        // A READ: asking again answers the same thing, so nothing the first call did
        // changed what a later caller sees.
        let again = list_domain_conflicts(
            &router,
            &domain,
            Resource::Distribution(&fixture.wildcard_distribution),
            Some(MAX_ITEMS),
            None,
        )
        .await;
        let reported_again = conflicts_of(&again, case);
        assert_eq!(
            reported_again, reported,
            "case {case:?} ({domain}): expected the second read to answer {reported:?}, got \
             {reported_again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Which resource may validate the domain
// ---------------------------------------------------------------------------

/// What a row expects of the answer.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// HTTP 200: the resource validates the domain.
    Served,
    /// HTTP 400 `InvalidArgument`, carrying this sentence.
    Invalid(&'static str),
    /// HTTP 404 `EntityNotFound`, carrying this sentence.
    NotFound(&'static str),
}

/// A certificate covers a domain only while it is valid FOR that domain: the validation
/// resource is accepted exactly when the certificate attached to it does.
#[tokio::test]
async fn the_validation_resource_is_the_one_whose_certificate_covers_the_domain() {
    let router = emulator();
    let multi_tenant = multi_tenant_distribution(&router, "ref-validation").await;
    let zone = "validate.example.com";

    // A distribution with an alias carries a custom certificate; one with none is served on
    // CloudFront's own default certificate.
    let custom_certificate_distribution =
        create_standard_distribution(&router, "ref-validation-custom", &[&format!("cert.{zone}")])
            .await;
    let default_certificate_distribution =
        create_standard_distribution(&router, "ref-validation-default", &[]).await;
    // A tenant whose certificate is one the caller named by ARN: this service holds no
    // record of the names such a certificate covers, so the create needs a domain that
    // already resolves to a CloudFront resource.
    let resolvable = resolvable_domains(&router, "validation", 1).await;
    let arn_tenant = create_tenant(
        &router,
        &multi_tenant,
        "validation-arn",
        &[&resolvable[0]],
        Some(
            "<Customizations><Certificate><Arn>\
             arn:aws:acm:us-east-1:000000000000:certificate/\
             11111111-2222-3333-4444-555555555555\
             </Arn></Certificate></Customizations>",
        ),
    )
    .await;
    let wildcard_tenant = create_tenant(
        &router,
        &multi_tenant,
        "validation-wildcard",
        &[&format!("*.wild.{zone}")],
        None,
    )
    .await;
    let unrelated_tenant = create_tenant(
        &router,
        &multi_tenant,
        "validation-unrelated",
        &[&format!("elsewhere.{zone}")],
        None,
    )
    .await;
    // Created LAST, so its managed certificate is still pending while the rows below that
    // are about a pending certificate run.
    let pending_tenant = create_tenant(
        &router,
        &multi_tenant,
        "validation-pending",
        &[&format!("pending.{zone}")],
        None,
    )
    .await;

    // While the certificate is PENDING it covers nothing: the tenant that asked for it
    // validates no domain, not even one of its own.
    let pending = list_domain_conflicts(
        &router,
        &format!("pending.{zone}"),
        Resource::Tenant(&pending_tenant),
        None,
        None,
    )
    .await;
    assert_eq!(
        pending.status, 400,
        "case a tenant holding only a pending managed certificate request: expected HTTP \
         400, got {} with {}",
        pending.status, pending.raw
    );
    assert_eq!(
        pending.error_code(),
        "InvalidArgument",
        "case a tenant holding only a pending managed certificate request: expected \
         InvalidArgument, got {}",
        pending.raw
    );
    assert_eq!(
        pending.root().find_text("Message").unwrap_or_default(),
        CERTIFICATE_DOES_NOT_COVER,
        "case a tenant holding only a pending managed certificate request: expected the \
         certificate sentence, got {}",
        pending.raw
    );

    // Once the clock has made the issuance due, the same certificate covers the names it
    // was minted over.
    await_certificate_issuance().await;

    let rows: Vec<(&str, String, Resource<'_>, Outcome)> = vec![
        (
            "a distribution on a custom certificate",
            format!("pending.{zone}"),
            Resource::Distribution(&custom_certificate_distribution),
            Outcome::Served,
        ),
        (
            "a distribution on CloudFront's own default certificate",
            format!("pending.{zone}"),
            Resource::Distribution(&default_certificate_distribution),
            Outcome::Invalid(CERTIFICATE_DOES_NOT_COVER),
        ),
        (
            "a tenant whose certificate was named by ARN",
            format!("pending.{zone}"),
            Resource::Tenant(&arn_tenant),
            Outcome::Served,
        ),
        (
            "a tenant whose issued managed certificate covers the domain",
            format!("pending.{zone}"),
            Resource::Tenant(&pending_tenant),
            Outcome::Served,
        ),
        (
            "a tenant whose issued managed certificate covers another name",
            format!("pending.{zone}"),
            Resource::Tenant(&unrelated_tenant),
            Outcome::Invalid(CERTIFICATE_DOES_NOT_COVER),
        ),
        (
            "a wildcard certificate, asked about a name it covers",
            format!("one.wild.{zone}"),
            Resource::Tenant(&wildcard_tenant),
            Outcome::Served,
        ),
        (
            "a wildcard certificate, asked about the same name in upper case",
            format!("ONE.WILD.{zone}").to_uppercase(),
            Resource::Tenant(&wildcard_tenant),
            Outcome::Served,
        ),
        (
            "a wildcard certificate, asked about a name two labels under it",
            format!("one.two.wild.{zone}"),
            Resource::Tenant(&wildcard_tenant),
            Outcome::Invalid(CERTIFICATE_DOES_NOT_COVER),
        ),
        (
            "a certificate for one name, asked about the wildcard that covers it",
            format!("*.{zone}"),
            Resource::Tenant(&pending_tenant),
            Outcome::Invalid(CERTIFICATE_DOES_NOT_COVER),
        ),
        (
            "a distribution id no distribution carries",
            format!("pending.{zone}"),
            Resource::Distribution("E1NOSUCHDIST0"),
            Outcome::NotFound(NO_SUCH_DISTRIBUTION_MESSAGE),
        ),
        (
            "a tenant id no tenant carries",
            format!("pending.{zone}"),
            Resource::Tenant("dt_nosuchtenant"),
            Outcome::NotFound(TENANT_NOT_FOUND),
        ),
        (
            "a resource naming neither member",
            format!("pending.{zone}"),
            Resource::Neither,
            Outcome::Invalid(ONE_VALIDATION_RESOURCE),
        ),
        (
            "a resource naming both members",
            format!("pending.{zone}"),
            Resource::Both(&custom_certificate_distribution, &pending_tenant),
            Outcome::Invalid(ONE_VALIDATION_RESOURCE),
        ),
        (
            "a domain of one label",
            "localhost".to_string(),
            Resource::Distribution(&custom_certificate_distribution),
            Outcome::Invalid(INVALID_DOMAIN_NAME_MESSAGE),
        ),
        (
            "a domain carrying an empty label",
            format!("empty..{zone}"),
            Resource::Distribution(&custom_certificate_distribution),
            Outcome::Invalid(INVALID_DOMAIN_NAME_MESSAGE),
        ),
        (
            "a domain carrying whitespace",
            format!("not a name.{zone}"),
            Resource::Distribution(&custom_certificate_distribution),
            Outcome::Invalid(INVALID_DOMAIN_NAME_MESSAGE),
        ),
    ];

    for (case, domain, resource, outcome) in rows {
        let answer = list_domain_conflicts(&router, &domain, resource, None, None).await;
        let message = answer
            .root()
            .find_text("Message")
            .unwrap_or_default()
            .to_string();
        match outcome {
            Outcome::Served => assert_eq!(
                answer.status, 200,
                "case {case:?} ({domain}): expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            ),
            Outcome::Invalid(expected) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?} ({domain}): expected HTTP 400, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidArgument",
                    "case {case:?} ({domain}): expected InvalidArgument, got {}",
                    answer.raw
                );
                assert_eq!(
                    message, expected,
                    "case {case:?} ({domain}): expected the sentence {expected:?}, got \
                     {message:?}"
                );
            }
            Outcome::NotFound(expected) => {
                assert_eq!(
                    answer.status, 404,
                    "case {case:?} ({domain}): expected HTTP 404, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "EntityNotFound",
                    "case {case:?} ({domain}): expected EntityNotFound, got {}",
                    answer.raw
                );
                assert_eq!(
                    message, expected,
                    "case {case:?} ({domain}): expected the sentence {expected:?}, got \
                     {message:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// How the pages are cut
// ---------------------------------------------------------------------------

/// Walking the pages at any admissible page size enumerates every conflict exactly once, in
/// the one order the store reports them.
#[tokio::test]
async fn walking_the_pages_enumerates_every_conflict_once() {
    let router = emulator();
    let fixture = Fixture::create(&router).await;
    let wildcard = format!("*.{ZONE}");

    // The whole set, in one page: the page ceiling admits far more than this store holds.
    let whole = list_domain_conflicts(
        &router,
        &wildcard,
        Resource::Distribution(&fixture.wildcard_distribution),
        Some(MAX_ITEMS),
        None,
    )
    .await;
    assert_eq!(
        whole.status, 200,
        "case the whole set: expected HTTP 200, got {} with {}",
        whole.status, whole.raw
    );
    let all = conflicts_of(&whole, "the whole set");
    assert!(
        all.len() > 2,
        "case the whole set: the pagination rows need several conflicts to page over, got \
         {all:?}"
    );
    let mut ordered = all.clone();
    ordered.sort_by_key(conflict_order);
    assert_eq!(
        all, ordered,
        "case the whole set: expected the conflicts in the store's order, got {all:?}"
    );

    // Every page size from one up past the stored set, plus this service's own ceiling.
    let sizes: Vec<i32> = (1..=i32::try_from(all.len()).expect("a small set") + 1)
        .chain([MAX_ITEMS])
        .collect();
    for size in sizes {
        let case = format!("page size {size}");
        let mut walked: Vec<Conflict> = Vec::new();
        let mut marker: Option<String> = None;
        // One page per conflict is the most a walk can need; the bound keeps a marker that
        // failed to advance from looping forever.
        for _ in 0..=all.len() {
            let answer = list_domain_conflicts(
                &router,
                &wildcard,
                Resource::Distribution(&fixture.wildcard_distribution),
                Some(size),
                marker.as_deref(),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let page = conflicts_of(&answer, &case);
            assert!(
                page.len() <= usize::try_from(size).expect("a positive size"),
                "case {case}: expected at most {size} conflicts in a page, got {page:?}"
            );
            let resume = next_marker(&answer);
            let remaining = all.len() - walked.len() - page.len();
            assert_eq!(
                !resume.is_empty(),
                remaining > 0,
                "case {case}: expected a NextMarker exactly when conflicts remain \
                 ({remaining} left), got {resume:?} with {}",
                answer.raw
            );
            walked.extend(page);
            if resume.is_empty() {
                break;
            }
            marker = Some(resume);
        }
        assert_eq!(
            walked, all,
            "case {case}: expected the walk to enumerate {all:?}, got {walked:?}"
        );
    }
}

/// A `MaxItems` that names no page and a `Marker` this service never handed out are
/// refused; a well-formed token naming no association resumes after it.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let fixture = Fixture::create(&router).await;
    let wildcard = format!("*.{ZONE}");
    let resource = Resource::Distribution(&fixture.wildcard_distribution);

    for (case, max_items) in [("negative", -1), ("far negative", i32::MIN)] {
        let answer =
            list_domain_conflicts(&router, &wildcard, resource, Some(max_items), None).await;
        assert_eq!(
            answer.status, 400,
            "case a {case} MaxItems: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case a {case} MaxItems: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            MAX_ITEMS_INVALID_MESSAGE,
            "case a {case} MaxItems: expected the MaxItems sentence, got {}",
            answer.raw
        );
    }

    // Rows differ along the shape of the value: a token with no separator, with no domain,
    // with no resource id, and one whose domain half is no domain name.
    for (case, marker) in [
        ("no separator", format!("www.{ZONE}")),
        ("no domain", "|E1EXAMPLE".to_string()),
        ("no resource id", format!("www.{ZONE}|")),
        ("a single-label domain", "localhost|E1EXAMPLE".to_string()),
        ("an empty label", format!("www..{ZONE}|E1EXAMPLE")),
    ] {
        let answer =
            list_domain_conflicts(&router, &wildcard, resource, None, Some(&marker)).await;
        assert_eq!(
            answer.status, 400,
            "case a marker with {case} ({marker:?}): expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case a marker with {case} ({marker:?}): expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            INVALID_MARKER,
            "case a marker with {case} ({marker:?}): expected the marker sentence, got {}",
            answer.raw
        );
    }

    // A token of the right shape that names no association this store holds is a resume
    // point like any other: the page carries what sorts after it, which past the last
    // conflict is nothing at all.
    let past_the_end = format!("zzz.{ZONE}|E1NOSUCHDIST0");
    let answer =
        list_domain_conflicts(&router, &wildcard, resource, None, Some(&past_the_end)).await;
    assert_eq!(
        answer.status, 200,
        "case a token past the last conflict: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let reported = conflicts_of(&answer, "a token past the last conflict");
    assert!(
        reported.is_empty(),
        "case a token past the last conflict: expected no conflicts after it, got {reported:?}"
    );
    assert_eq!(
        next_marker(&answer),
        "",
        "case a token past the last conflict: expected no resume point, got {}",
        answer.raw
    );
}
