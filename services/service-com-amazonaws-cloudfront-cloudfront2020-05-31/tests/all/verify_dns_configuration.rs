// SPDX-License-Identifier: CC0-1.0

//! `VerifyDnsConfiguration`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler and the persisted state — the same path a client takes.
//! Two rules are under test.
//!
//! The first is the one the operation states: it "checks whether your domain name points to
//! the correct routing endpoint of the connection group, such as
//! `d111111abcdef8.cloudfront.net`", so for each domain in scope — the one `Domain` names,
//! or every domain the distribution tenant holds — the answer compares the endpoint that
//! name resolves to against the routing endpoint of the connection group the tenant is
//! routed through:
//!
//! * `valid-configuration` when the name resolves to exactly that routing endpoint;
//! * `invalid-configuration` when it resolves to ANOTHER CloudFront endpoint — "the DNS
//!   record exists but it's using an incorrect routing endpoint";
//! * `unknown-configuration` when no DNS record this service can read resolves the name,
//!   so no comparison was made — CloudFront "can't verify the DNS record".
//!
//! The second is the scope of a `Domain` argument: the verification is about the tenant the
//! `Identifier` names, so a `Domain` that tenant does not hold is refused — HTTP 400,
//! `InvalidArgument`, with the fixed sentence CloudFront answers it with, which interpolates
//! neither the domain asked about nor the tenant. A string of no domain shape at all is one
//! such name, so it takes the same refusal.
//!
//! Beside them: an `Identifier` no tenant carries is `EntityNotFound`, and the verification
//! is a REPORT — it changes nothing a later caller sees.
//!
//! The rows differ along everything the rules generalise over: which endpoint the domain
//! resolves to (the tenant's own connection group, another connection group, a
//! distribution's own edge domain, nothing at all), whether the name is one the tenant
//! holds, the case it is written in, its shape and its length, how many domains are in
//! scope, and which of the three identifiers names the tenant.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_TENANT_ID_PREFIX, DNS_CONFIGURATION_INVALID, DNS_CONFIGURATION_UNKNOWN,
    DNS_CONFIGURATION_VALID, MAX_DOMAINS_PER_TENANT, MAX_DOMAIN_LABEL_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, Answer, Node, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, escape, longest_domain,
    managed_certificate_request, multi_tenant_distribution,
};
use crate::response_headers_policy_lifecycle::text;

/// The message CloudFront answers a `Domain` the named distribution tenant does not hold
/// with. FIXED text: every row below, whatever it asked about, is answered with exactly
/// this, so the test also pins that nothing of the request reaches the message.
const DOMAIN_NOT_ASSOCIATED: &str = "The specified domain name isn't associated with the \
                                     distribution tenant. Specify another domain name and \
                                     try again.";

// ---------------------------------------------------------------------------
// Calling the operation
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/verify-dns-configuration`, naming `identifier` and — when the case
/// has one — `domain`.
async fn verify_dns_configuration(
    router: &MultiServiceRouter,
    identifier: &str,
    domain: Option<&str>,
) -> Answer {
    let mut body = format!(
        "<VerifyDnsConfigurationRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(domain) = domain {
        body.push_str(&format!("<Domain>{}</Domain>", escape(domain)));
    }
    body.push_str(&format!(
        "<Identifier>{}</Identifier>",
        escape(identifier)
    ));
    body.push_str("</VerifyDnsConfigurationRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/verify-dns-configuration"),
        Some(&body),
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Identifier}` — the read that proves a
/// verification stored nothing.
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

/// The `(Domain, Status, Reason)` of every entry one answer reported, in order.
///
/// `DnsConfigurationList`'s member carries `xmlName` `DnsConfiguration`
/// (`smithy_query(query="shape",shape="DnsConfigurationList")`), so each entry travels in
/// an element of that name inside the `DnsConfigurationList` wrapper.
fn reported(answer: &Answer) -> Vec<(String, String, String)> {
    let Some(wrapper) = answer.root().descendant("DnsConfigurationList") else {
        panic!(
            "the answer carries no DnsConfigurationList: {}",
            answer.raw
        );
    };
    wrapper
        .children_named("DnsConfiguration")
        .into_iter()
        .map(|entry: &Node| {
            (
                entry.text_at("Domain").unwrap_or_default().to_string(),
                entry.text_at("Status").unwrap_or_default().to_string(),
                entry.text_at("Reason").unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The ONE entry an answer about a single domain reported.
fn one(answer: &Answer, case: &str) -> (String, String, String) {
    let entries = reported(answer);
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one DnsConfiguration, got {entries:?}"
    );
    entries.into_iter().next().expect("one entry")
}

/// The `Message` of a refusal envelope, or `""`.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The resources every case is verified against
// ---------------------------------------------------------------------------

/// Create a connection group called `name` and answer `(Id, RoutingEndpoint)` — the
/// endpoint the service MINTED for it, which is what every comparison below is made
/// against.
async fn connection_group(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let body = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{name}</Name>\
         </CreateConnectionGroupRequest>"
    );
    let created = create_connection_group(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {name}: expected the connection group to be created, got {} with {}",
        created.status, created.raw
    );
    let group = created.root();
    (
        text(group, "Id", name, "the created group", &created.raw),
        text(
            group,
            "RoutingEndpoint",
            name,
            "the created group",
            &created.raw,
        ),
    )
}

/// Create a distribution tenant called `name` under `distribution_id`, routed through
/// `connection_group_id` and holding `domains`. Answers its minted `(Id, Arn)`.
async fn distribution_tenant(
    router: &MultiServiceRouter,
    distribution_id: &str,
    name: &str,
    domains: &[&str],
    connection_group_id: &str,
) -> (String, String) {
    assert!(
        domains.len() <= MAX_DOMAINS_PER_TENANT,
        "case {name}: a tenant holds at most {MAX_DOMAINS_PER_TENANT} domains, got {}",
        domains.len()
    );
    let mut body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>{name}</Name><Domains>"
    );
    for domain in domains {
        body.push_str(&format!(
            "<member><Domain>{}</Domain></member>",
            escape(domain)
        ));
    }
    body.push_str(&format!(
        "</Domains>\
         <Parameters><member><Name>tenantName</Name><Value>{name}</Value></member></Parameters>\
         <ConnectionGroupId>{connection_group_id}</ConnectionGroupId>{}\
         </CreateDistributionTenantRequest>",
        managed_certificate_request()
    ));
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {name}: expected the tenant to be created, got {} with {}",
        created.status, created.raw
    );
    let tenant = created
        .root()
        .descendant("DistributionTenant")
        .unwrap_or_else(|| panic!("case {name}: the create carries no DistributionTenant"));
    (
        text(tenant, "Id", name, "the created tenant", &created.raw),
        text(tenant, "Arn", name, "the created tenant", &created.raw),
    )
}

/// Create a STANDARD distribution and answer the edge `DomainName` the service minted for
/// it: another `d<label>.cloudfront.net` name this service is authoritative for, and so a
/// name a domain can resolve to without being the tenant's routing endpoint.
async fn distribution_domain_name(router: &MultiServiceRouter, reference: &str) -> String {
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
// The rules
// ---------------------------------------------------------------------------

/// Each domain the tenant holds is reported against the routing endpoint of the connection
/// group the tenant is routed through: a name that resolves to that endpoint is
/// `valid-configuration`, one that resolves ELSEWHERE — another connection group's routing
/// endpoint, a distribution's own edge domain — is `invalid-configuration`, and one no DNS
/// record this service can read resolves is `unknown-configuration`. DNS names are
/// case-insensitive, so the case the name is written in changes neither whether the tenant
/// holds it nor the answer, and each reason names the endpoint the comparison was made
/// against.
#[tokio::test]
async fn a_domain_is_verified_against_the_tenants_routing_endpoint() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-verify-dns").await;
    let (own_group, own_endpoint) = connection_group(&router, "verify-dns-own").await;
    let (other_group, other_endpoint) = connection_group(&router, "verify-dns-other").await;
    let distribution_domain = distribution_domain_name(&router, "ref-verify-dns-standard").await;
    let longest = longest_domain();

    // The tenant HOLDS every name verified below — that is the scope of a `Domain`
    // argument — and they differ along what each resolves to and how long each is.
    let (tenant_id, _) = distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-tenant",
        &[
            &own_endpoint,
            &other_endpoint,
            &distribution_domain,
            "shop.example.com",
            &longest,
        ],
        &own_group,
    )
    .await;
    distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-neighbour",
        &["neighbour.example.com"],
        &other_group,
    )
    .await;

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "the tenant's own routing endpoint",
            own_endpoint.clone(),
            DNS_CONFIGURATION_VALID,
        ),
        (
            "the same routing endpoint in upper case",
            own_endpoint.to_ascii_uppercase(),
            DNS_CONFIGURATION_VALID,
        ),
        (
            "another connection group's routing endpoint",
            other_endpoint.clone(),
            DNS_CONFIGURATION_INVALID,
        ),
        (
            "a distribution's own edge domain",
            distribution_domain.clone(),
            DNS_CONFIGURATION_INVALID,
        ),
        (
            "a name in the customer's own zone",
            "shop.example.com".to_string(),
            DNS_CONFIGURATION_UNKNOWN,
        ),
        (
            "the same customer name in mixed case",
            "Shop.Example.COM".to_string(),
            DNS_CONFIGURATION_UNKNOWN,
        ),
        (
            "a name at the whole-name bound",
            longest.clone(),
            DNS_CONFIGURATION_UNKNOWN,
        ),
    ];

    for (case, domain, expected_status) in cases {
        let answer = verify_dns_configuration(&router, &tenant_id, Some(&domain)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let (reported_domain, status, reason) = one(&answer, case);
        assert_eq!(
            reported_domain, domain,
            "case {case}: expected the verified domain back, got {reported_domain:?}"
        );
        assert_eq!(
            status, expected_status,
            "case {case}: expected status {expected_status}, got {status:?} ({reason})"
        );
        assert!(
            reason.contains(&own_endpoint),
            "case {case}: expected the reason to name the routing endpoint {own_endpoint}, got \
             {reason:?}"
        );
    }
}

/// A `Domain` the resolved distribution tenant does not hold is refused: HTTP 400,
/// `InvalidArgument`, carrying CloudFront's fixed sentence — whether the name is a
/// well-formed domain another resource holds, one nothing holds, a relative of one the
/// tenant does hold, or no domain name at all. The names the tenant DOES hold are verified
/// rather than refused, in any case.
#[tokio::test]
async fn a_domain_the_tenant_does_not_hold_is_an_invalid_argument() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-verify-dns-scope").await;
    let (group, endpoint) = connection_group(&router, "verify-dns-scope").await;
    let (other_group, _) = connection_group(&router, "verify-dns-scope-other").await;
    let distribution_domain = distribution_domain_name(&router, "ref-verify-dns-scope-std").await;
    let (tenant_id, _) = distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-scope-tenant",
        &["shop.example.com", "Mixed.Example.com"],
        &group,
    )
    .await;
    distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-scope-neighbour",
        &["neighbour.example.com"],
        &other_group,
    )
    .await;

    // Rows: a well-formed name nothing holds, one ANOTHER tenant holds, a child and the
    // parent of a name this tenant holds, CloudFront's own minted names (the tenant's
    // routing endpoint and a distribution's edge domain, which this tenant does not hold
    // as domains), then the names of no domain shape at all — the empty string, a single
    // label, an empty label, one character past each published bound, and whitespace in a
    // label.
    let label = "d".repeat(MAX_DOMAIN_LABEL_LEN);
    let cases: Vec<(&str, String)> = vec![
        ("a domain nothing holds", "unclaimed.example.com".to_string()),
        (
            "a domain another tenant holds",
            "neighbour.example.com".to_string(),
        ),
        (
            "a child of a domain the tenant holds",
            "cart.shop.example.com".to_string(),
        ),
        (
            "the parent of a domain the tenant holds",
            "example.com".to_string(),
        ),
        ("the tenant's routing endpoint", endpoint.clone()),
        ("a distribution's edge domain", distribution_domain.clone()),
        ("the empty domain", String::new()),
        ("a single label", "localhost".to_string()),
        ("an empty label", "a..example.com".to_string()),
        (
            "one character past the label bound",
            format!("{label}d.example.com"),
        ),
        (
            "one character past the whole-name bound",
            format!("{}x", longest_domain()),
        ),
        ("whitespace in a label", "a b.example.com".to_string()),
    ];

    for (case, domain) in cases {
        let answer = verify_dns_configuration(&router, &tenant_id, Some(&domain)).await;
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
        assert_eq!(
            message(&answer),
            DOMAIN_NOT_ASSOCIATED,
            "case {case}: expected {DOMAIN_NOT_ASSOCIATED:?}, got {:?}",
            message(&answer)
        );
    }

    // The control: each name the tenant holds is verified, asked for in the case the
    // record holds it in and in another — DNS names are case-insensitive, so membership is
    // too — and a name exactly at the whole-name bound is a domain name, so the refusal is
    // about the association and not about the length.
    for (case, domain) in [
        ("a held domain", "shop.example.com"),
        ("a held domain in upper case", "SHOP.EXAMPLE.COM"),
        ("a held domain stored in mixed case", "Mixed.Example.com"),
        ("the same name in lower case", "mixed.example.com"),
    ] {
        let accepted = verify_dns_configuration(&router, &tenant_id, Some(domain)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            accepted.status, accepted.raw
        );
        let (reported_domain, status, reason) = one(&accepted, case);
        assert_eq!(
            reported_domain, domain,
            "case {case}: expected the verified domain back, got {reported_domain:?}"
        );
        assert_eq!(
            status, DNS_CONFIGURATION_UNKNOWN,
            "case {case}: expected {DNS_CONFIGURATION_UNKNOWN} for a name in the customer's own \
             zone, got {status:?} ({reason})"
        );
    }
}

/// A call that names no `Domain` reports every domain the tenant holds, in the order the
/// record holds them — and the verification is a REPORT: it stores nothing, so a repeat
/// answers the same list and the tenant keeps the status and version token its create
/// minted. The same tenant is reached by each of its three identifiers.
#[tokio::test]
async fn a_call_naming_no_domain_reports_every_domain_the_tenant_holds() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-verify-dns-all").await;
    let (group, endpoint) = connection_group(&router, "verify-dns-all").await;
    let domains = ["one.example.com", "Two.Example.com", "three.example.com"];
    let (tenant_id, tenant_arn) = distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-all-tenant",
        &domains,
        &group,
    )
    .await;

    let before = get_distribution_tenant(&router, &tenant_id).await;
    let before_status = text(
        before.root(),
        "Status",
        "the whole tenant",
        "before the verification",
        &before.raw,
    );
    let before_etag = before.header("etag").to_string();

    // Each identifier the operation admits: "the ARN, ID, or name of the distribution
    // tenant".
    for (case, identifier) in [
        ("by id", tenant_id.clone()),
        ("by name", "verify-dns-all-tenant".to_string()),
        ("by ARN", tenant_arn.clone()),
    ] {
        for pass in ["the first call", "a repeat of it"] {
            let answer = verify_dns_configuration(&router, &identifier, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case} ({pass}): expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let entries = reported(&answer);
            let reported_domains: Vec<String> = entries
                .iter()
                .map(|(domain, _, _)| domain.clone())
                .collect();
            assert_eq!(
                reported_domains,
                domains.map(str::to_string).to_vec(),
                "case {case} ({pass}): expected every domain the tenant holds, in order, got \
                 {reported_domains:?}"
            );
            for (domain, status, reason) in &entries {
                // None of these names is one this service holds a DNS record for, so no
                // comparison was made for any of them.
                assert_eq!(
                    status, DNS_CONFIGURATION_UNKNOWN,
                    "case {case} ({pass}): expected {domain} to be \
                     {DNS_CONFIGURATION_UNKNOWN}, got {status:?} ({reason})"
                );
                assert!(
                    reason.contains(&endpoint),
                    "case {case} ({pass}): expected the reason for {domain} to name the routing \
                     endpoint {endpoint}, got {reason:?}"
                );
            }
        }
    }

    let after = get_distribution_tenant(&router, &tenant_id).await;
    assert_eq!(
        text(
            after.root(),
            "Status",
            "the whole tenant",
            "after the verification",
            &after.raw
        ),
        before_status,
        "a verification must not move the tenant's status"
    );
    assert_eq!(
        after.header("etag"),
        before_etag,
        "a verification must not spend a version token, got {:?}",
        after.header("etag")
    );
}

/// An `Identifier` no distribution tenant carries names no entity: `EntityNotFound`
/// (HTTP 404), whichever of the three identifier namespaces the value is shaped like — and
/// whatever the `Domain` argument is, since the tenant is what a domain's association is
/// decided against.
#[tokio::test]
async fn an_identifier_no_tenant_carries_is_not_found() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-verify-dns-absent").await;
    let (group, _) = connection_group(&router, "verify-dns-absent").await;
    distribution_tenant(
        &router,
        &distribution_id,
        "verify-dns-absent-tenant",
        &["absent.example.com"],
        &group,
    )
    .await;

    // Rows: an id of the right shape that names nothing, a name no tenant carries, an ARN
    // no tenant carries, a value of no identifier shape at all, and the empty identifier.
    let cases: [(&str, String); 5] = [
        (
            "an id no tenant carries",
            format!("{DISTRIBUTION_TENANT_ID_PREFIX}0000000000000"),
        ),
        ("a name no tenant carries", "no-such-tenant".to_string()),
        (
            "an ARN no tenant carries",
            "arn:aws:cloudfront::000000000000:distribution-tenant/dt_0000000000000".to_string(),
        ),
        ("a value of no identifier shape", "!!".to_string()),
        ("the empty identifier", String::new()),
    ];

    for (case, identifier) in cases {
        // The domain column: absent, a domain another tenant holds, and a string of no
        // domain shape — none of them turns a missing tenant into another refusal.
        for domain in [None, Some("absent.example.com"), Some("not a domain")] {
            let answer = verify_dns_configuration(&router, &identifier, domain).await;
            assert_eq!(
                answer.status, 404,
                "case {case} (domain {domain:?}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "EntityNotFound",
                "case {case} (domain {domain:?}): expected EntityNotFound, got {}",
                answer.raw
            );
        }
    }
}
