// SPDX-License-Identifier: CC0-1.0

//! `UpdateDistributionTenant`, end to end over HTTP.
//!
//! The rule under test: every member the update submits is OPTIONAL and each one is
//! documented as what the tenant is updated TO ("the domains to update for the distribution
//! tenant", "whether the distribution tenant should be updated to an enabled state"), so a
//! member the request STATES replaces what the stored record held and a member it leaves
//! out leaves that value exactly as it was; the one record is what the answer and every
//! later read are built from. Around that rule:
//!
//! * the tenant's identity — its id, its ARN, its name and its creation time — is submitted
//!   by no member of the update, so it never changes ("[the name] can't be updated after
//!   you create the distribution tenant");
//! * the tenant's current version is a REQUIRED precondition: a value that is no version
//!   token is `InvalidIfMatchVersion` (HTTP 400) and a well-formed version that is not the
//!   stored one fails the precondition (HTTP 412). A refused update changes nothing a later
//!   read can see — not the record, not its version;
//! * the state the tenant ends up in has to be admissible, whichever members the request
//!   states it with: a stated domain list is bounded and shaped like domain names, routes
//!   to no other CloudFront resource, and every domain the tenant does not already hold is
//!   bound only when the account's control of it is proven; the parameter values the tenant
//!   ends up with satisfy the multi-tenant distribution it ends up under; and a distribution
//!   or connection group the store does not hold is no entity;
//! * a `ManagedCertificateRequest` on the update MINTS a certificate over the domains the
//!   tenant holds once the update has landed;
//! * the configuration an update lands has to reach the edge locations, so the tenant
//!   re-enters `InProgress` and the CLOCK — not a later call — owes it `Deployed`.
//!
//! The rows differ along everything the rule generalises over: which members the update
//! states (each one alone, several together, and none at all beyond the precondition), how
//! many domains it states and whether they are the ones already held or fresh, both values
//! `Enabled` can take, a customization set that replaces another and one that names
//! nothing, a replaced parameter list of a different length, and the published per-tenant
//! domain and parameter bounds plus one past each.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the tenants are
//! created through [`crate::distribution_tenant_lifecycle`]'s helpers and read back through
//! [`crate::tenant_web_acl_association`]'s reader, so every assertion travels the same path
//! a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CUSTOMIZATION_ACTION_OVERRIDE, DOMAIN_STATUS_ACTIVE, INVALID_DOMAIN_NAME_MESSAGE,
    INVALID_IF_MATCH_VERSION_MESSAGE, MAX_DOMAINS_PER_TENANT, MAX_PARAMETERS_PER_TENANT,
    STATUS_DEPLOYED, STATUS_IN_PROGRESS, VALIDATION_TOKEN_HOST_CLOUDFRONT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, past_propagation, Answer, Node, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, escape, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::managed_certificate_details::get_managed_certificate_details;
use crate::managed_certificate_domain_ownership::await_certificate_issuance;
use crate::tenant_web_acl_association::{read_tenant, web_acl_of};
use crate::web_acl_association::{Stated, WEB_ACLS};

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/distribution-tenant/{Id}`: the update travels in the request document,
/// the version in the `If-Match` header.
///
/// `pub(crate)` so the sibling connection group delete suite moves a tenant off the group it
/// routes through over this one wire path rather than growing a copy of it.
pub(crate) async fn update_tenant(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// Wrap `members` in the request document an `UpdateDistributionTenant` carries.
pub(crate) fn update_xml(members: &str) -> String {
    format!(
        "<UpdateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{members}\
         </UpdateDistributionTenantRequest>"
    )
}

/// The `Domains` element naming `domains`. `DomainList`'s member carries no `xmlName`
/// (`smithy_query(query="shape",shape="DomainList")`), so each entry travels in the restXml
/// default `<member>` element.
fn domains_element(domains: &[String]) -> String {
    let mut out = "<Domains>".to_string();
    for domain in domains {
        out.push_str(&format!(
            "<member><Domain>{}</Domain></member>",
            escape(domain)
        ));
    }
    out.push_str("</Domains>");
    out
}

/// The `Parameters` element naming `parameters`, as `(Name, Value)` pairs.
fn parameters_element(parameters: &[(String, String)]) -> String {
    let mut out = "<Parameters>".to_string();
    for (name, value) in parameters {
        out.push_str(&format!(
            "<member><Name>{}</Name><Value>{}</Value></member>",
            escape(name),
            escape(value)
        ));
    }
    out.push_str("</Parameters>");
    out
}

// ---------------------------------------------------------------------------
// The tenant every row starts from
// ---------------------------------------------------------------------------

/// The `Customizations` the created tenant carries: geographic restrictions and no web ACL.
/// `LocationList`'s member carries `xmlName` `Location`
/// (`smithy_query(query="shape",shape="LocationList")`).
const CREATED_CUSTOMIZATIONS: &str = "<Customizations>\
     <GeoRestrictions><RestrictionType>whitelist</RestrictionType>\
     <Locations><Location>DE</Location><Location>JP</Location></Locations>\
     </GeoRestrictions></Customizations>";

/// The parameter value the created tenant carries for the distribution's one required
/// parameter with no default.
const CREATED_PARAMETER: &str = "created";

/// One created tenant, as the rows below address it.
#[derive(Debug, Clone)]
struct Tenant {
    id: String,
    etag: String,
    /// The one domain the create bound to it.
    domain: String,
    /// The facts a read of the tenant reported straight after the create.
    facts: Facts,
}

/// Create one distribution tenant under `distribution_id` with the starting state every row
/// updates from: one domain, one parameter value, geographic-restriction customizations, and
/// no `Enabled` stated (so the service's own default applies).
async fn create_tenant(
    router: &MultiServiceRouter,
    distribution_id: &str,
    slug: &str,
    case: &str,
) -> Tenant {
    let domain = format!("{slug}.example.com");
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{slug}</Name>\
         {}{CREATED_CUSTOMIZATIONS}\
         <Parameters><member><Name>tenantName</Name><Value>{CREATED_PARAMETER}</Value>\
         </member></Parameters>{}</CreateDistributionTenantRequest>",
        escape(distribution_id),
        domains_element(std::slice::from_ref(&domain)),
        managed_certificate_request()
    );
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
        created.status, created.raw
    );
    Tenant {
        id: created
            .root()
            .text_at("Id")
            .unwrap_or_else(|| panic!("case {case}: the create carries no Id: {}", created.raw))
            .to_string(),
        etag: created.header("etag").to_string(),
        domain,
        facts: facts_of(created.root()),
    }
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// Everything a `DistributionTenant` document says about one tenant's configuration and its
/// identity, so one comparison covers both what an update changed and what it left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    name: String,
    arn: String,
    connection_group_id: String,
    distribution_id: String,
    created_time: String,
    enabled: String,
    /// `(Domain, Status)` of every entry of `Domains`, in document order.
    domains: Vec<(String, String)>,
    /// `(Name, Value)` of every entry of `Parameters`, in document order.
    parameters: Vec<(String, String)>,
    /// The locations the geographic-restriction customization names, in document order.
    geo_locations: Vec<String>,
    /// `(Action, Arn)` of the web ACL customization, or `None` when the record carries none.
    web_acl: Option<(String, String)>,
}

/// The facts the `DistributionTenant` document `tenant` reports.
fn facts_of(tenant: &Node) -> Facts {
    Facts {
        name: tenant.text_at("Name").unwrap_or_default().to_string(),
        arn: tenant.text_at("Arn").unwrap_or_default().to_string(),
        connection_group_id: tenant
            .text_at("ConnectionGroupId")
            .unwrap_or_default()
            .to_string(),
        distribution_id: tenant
            .text_at("DistributionId")
            .unwrap_or_default()
            .to_string(),
        created_time: tenant.text_at("CreatedTime").unwrap_or_default().to_string(),
        enabled: tenant.text_at("Enabled").unwrap_or_default().to_string(),
        domains: domains_of(tenant),
        parameters: tenant
            .at("Parameters")
            .map(|list| {
                list.children
                    .iter()
                    .map(|entry| {
                        (
                            entry.text_at("Name").unwrap_or_default().to_string(),
                            entry.text_at("Value").unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        geo_locations: tenant
            .at("Customizations/GeoRestrictions/Locations")
            .map(|list| {
                list.children
                    .iter()
                    .map(|entry| entry.text().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        web_acl: web_acl_of(tenant),
    }
}

// ---------------------------------------------------------------------------
// The updates under test
// ---------------------------------------------------------------------------

/// What a row states for `Domains`.
#[derive(Debug, Clone, Copy)]
enum Domains {
    /// The member is left out entirely.
    Unstated,
    /// The domain the tenant already holds, restated. It was admitted when it was bound, so
    /// the update owes no fresh proof of it.
    Held,
    /// This many freshly named domains, which the update asks CloudFront to validate.
    Fresh(usize),
}

/// One `UpdateDistributionTenant` a client submits, and what the tenant holds afterwards.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    domains: Domains,
    /// `Enabled`, or `None` to leave the member out.
    enabled: Option<bool>,
    /// `Customizations`, as the XML fragment the row submits, or `None` to leave the member
    /// out.
    customizations: Option<&'static str>,
    /// The locations the tenant's geographic restriction names after the update.
    expected_geo_locations: &'static [&'static str],
    /// Whether the tenant carries the web ACL customization `WEB_ACLS[0]` afterwards.
    expects_web_acl: bool,
    /// `Parameters`, as `(Name, Value)` pairs, or `None` to leave the member out.
    parameters: Option<&'static [(&'static str, &'static str)]>,
    /// Whether the row restates the connection group the tenant is already routed through.
    restates_connection_group: bool,
}

/// A `Customizations` element that OVERRIDES a web ACL and names no geographic restriction,
/// so stating it both adds a customization and drops the one the create stored.
const WEB_ACL_CUSTOMIZATIONS: &str = "<Customizations><WebAcl><Action>override</Action>\
     <Arn>arn:aws:wafv2:us-east-1:123456789012:global/webacl/ExampleWebACL/\
     a1b2c3d4-5678-90ab-cdef-EXAMPLE11111</Arn></WebAcl></Customizations>";

/// A `Customizations` element naming nothing at all, which describes no customization.
const EMPTY_CUSTOMIZATIONS: &str = "<Customizations></Customizations>";

/// The locations the created tenant's geographic restriction names
/// ([`CREATED_CUSTOMIZATIONS`]), which every row that states no `Customizations` keeps.
const CREATED_GEO_LOCATIONS: &[&str] = &["DE", "JP"];

/// Every update the rule is asserted over.
const ROWS: &[Row] = &[
    Row {
        case: "nothing-but-the-precondition",
        domains: Domains::Unstated,
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "enabled-false",
        domains: Domains::Unstated,
        enabled: Some(false),
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "enabled-true",
        domains: Domains::Unstated,
        enabled: Some(true),
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "the-held-domain-restated",
        domains: Domains::Held,
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "one-fresh-domain",
        domains: Domains::Fresh(1),
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "two-fresh-domains",
        domains: Domains::Fresh(2),
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    // "Each distribution tenant can have up to 5 domains" — the published bound itself.
    Row {
        case: "domains-at-the-published-bound",
        domains: Domains::Fresh(MAX_DOMAINS_PER_TENANT),
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "customizations-replaced-by-a-web-acl",
        domains: Domains::Unstated,
        enabled: None,
        customizations: Some(WEB_ACL_CUSTOMIZATIONS),
        expected_geo_locations: &[],
        expects_web_acl: true,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "customizations-naming-nothing",
        domains: Domains::Unstated,
        enabled: None,
        customizations: Some(EMPTY_CUSTOMIZATIONS),
        expected_geo_locations: &[],
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: false,
    },
    Row {
        case: "parameters-replaced-by-a-longer-list",
        domains: Domains::Unstated,
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: Some(&[("tenantName", "updated"), ("region-label", "eu")]),
        restates_connection_group: false,
    },
    Row {
        case: "the-connection-group-restated",
        domains: Domains::Unstated,
        enabled: None,
        customizations: None,
        expected_geo_locations: CREATED_GEO_LOCATIONS,
        expects_web_acl: false,
        parameters: None,
        restates_connection_group: true,
    },
    Row {
        case: "domains-parameters-and-enabled-together",
        domains: Domains::Fresh(3),
        enabled: Some(false),
        customizations: Some(WEB_ACL_CUSTOMIZATIONS),
        expected_geo_locations: &[],
        expects_web_acl: true,
        parameters: Some(&[("tenantName", "together")]),
        restates_connection_group: true,
    },
];

impl Row {
    /// The domains this row states, or `None` where it states the member not at all.
    fn stated_domains(self, tenant: &Tenant, slug: &str) -> Option<Vec<String>> {
        match self.domains {
            Domains::Unstated => None,
            Domains::Held => Some(vec![tenant.domain.clone()]),
            Domains::Fresh(count) => {
                Some((0..count).map(|i| format!("f{i}-{slug}.example.com")).collect())
            }
        }
    }

    /// The request document this row submits against `tenant`.
    fn xml(self, tenant: &Tenant, slug: &str) -> String {
        let mut members = String::new();
        if let Some(domains) = self.stated_domains(tenant, slug) {
            members.push_str(&domains_element(&domains));
            // A domain the tenant does not already hold is bound only when the account's
            // control of it is proven, so a row that names one asks CloudFront to validate
            // it — the same proof the create carried.
            if matches!(self.domains, Domains::Fresh(_)) {
                members.push_str(&managed_certificate_request());
            }
        }
        if let Some(enabled) = self.enabled {
            members.push_str(&format!("<Enabled>{enabled}</Enabled>"));
        }
        if let Some(customizations) = self.customizations {
            members.push_str(customizations);
        }
        if let Some(parameters) = self.parameters {
            let owned: Vec<(String, String)> = parameters
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            members.push_str(&parameters_element(&owned));
        }
        if self.restates_connection_group {
            members.push_str(&format!(
                "<ConnectionGroupId>{}</ConnectionGroupId>",
                escape(&tenant.facts.connection_group_id)
            ));
        }
        update_xml(&members)
    }

    /// The facts the tenant reports once this row has landed on it.
    fn expected(self, tenant: &Tenant, slug: &str) -> Facts {
        let domains = self
            .stated_domains(tenant, slug)
            .unwrap_or_else(|| vec![tenant.domain.clone()]);
        Facts {
            // Submitted by no member of the update, so untouched by every row.
            name: tenant.facts.name.clone(),
            arn: tenant.facts.arn.clone(),
            created_time: tenant.facts.created_time.clone(),
            distribution_id: tenant.facts.distribution_id.clone(),
            connection_group_id: tenant.facts.connection_group_id.clone(),
            enabled: self
                .enabled
                .map(|enabled| enabled.to_string())
                .unwrap_or_else(|| tenant.facts.enabled.clone()),
            domains: domains
                .into_iter()
                .map(|domain| (domain, DOMAIN_STATUS_ACTIVE.to_string()))
                .collect(),
            parameters: self
                .parameters
                .map(|parameters| {
                    parameters
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                        .collect()
                })
                .unwrap_or_else(|| tenant.facts.parameters.clone()),
            geo_locations: self
                .expected_geo_locations
                .iter()
                .map(|location| (*location).to_string())
                .collect(),
            web_acl: self.expects_web_acl.then(|| {
                (
                    CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
                    WEB_ACLS[0].1.to_string(),
                )
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A stated member replaces what the record held, a member the request leaves out leaves it
/// exactly as it was, and the answer and every later read are built from that one record.
#[tokio::test]
async fn a_stated_member_replaces_and_an_unstated_member_is_left_alone() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-update").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let slug = format!("tu{index}x");
        let tenant = create_tenant(&router, &distribution_id, &slug, case).await;

        let answer = update_tenant(
            &router,
            &tenant.id,
            &row.xml(&tenant, &slug),
            Some(&tenant.etag),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistributionTenant, got {} with {}",
            answer.status, answer.raw
        );
        let expected = row.expected(&tenant, &slug);
        let answered = facts_of(answer.root());
        assert_eq!(
            answered, expected,
            "case {case}: expected the update to answer {expected:?}, got {answered:?} in {}",
            answer.raw
        );
        assert_eq!(
            answer.root().text_at("Id"),
            Some(tenant.id.as_str()),
            "case {case}: expected the answer to name tenant {:?}, got {:?}",
            tenant.id,
            answer.root().text_at("Id")
        );

        // The version is freshly minted, so the token the caller matched on no longer
        // satisfies an `If-Match` against this tenant.
        let updated_etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&updated_etag),
            "case {case}: expected a minted version token in the ETag header, got {updated_etag:?}"
        );
        assert_ne!(
            updated_etag, tenant.etag,
            "case {case}: expected the update to mint a version token other than {:?}, got \
             {updated_etag:?}",
            tenant.etag
        );

        // The configuration this update landed has to reach the edge locations, so the
        // tenant is back in the transient status.
        assert_eq!(
            answer.root().text_at("Status"),
            Some(STATUS_IN_PROGRESS),
            "case {case}: expected {STATUS_IN_PROGRESS} from the update, got {:?}",
            answer.root().text_at("Status")
        );

        // The ONE record answers the read: the same document, at the version the update
        // minted.
        let read = read_tenant(&router, &tenant.id, case).await;
        let reread = facts_of(read.root());
        assert_eq!(
            reread, expected,
            "case {case}: expected the read to answer from the updated record {expected:?}, got \
             {reread:?} in {}",
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            updated_etag,
            "case {case}: expected the read at version {updated_etag:?}, got {:?}",
            read.header("etag")
        );
        // "LastModifiedTime" moved with the write, while the creation time did not.
        assert_ne!(
            read.root().text_at("LastModifiedTime"),
            Some(tenant.facts.created_time.as_str()),
            "case {case}: expected the update to move LastModifiedTime off the creation time {:?}",
            tenant.facts.created_time
        );
    }
}

// ---------------------------------------------------------------------------
// The precondition
// ---------------------------------------------------------------------------

/// One row of the precondition table: what the request states as its `If-Match`, and what
/// the service answers.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    stated: Stated,
    expected_status: u16,
    expected_code: &'static str,
}

/// Every form of `If-Match` an update can state. A value that is no version token at all is
/// "the `If-Match` version is missing or not valid"; a well-formed token that is not the
/// stored one is a failed precondition instead.
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "current-version",
        stated: Stated::Current,
        expected_status: 200,
        expected_code: "",
    },
    Precondition {
        case: "quoted-current-version",
        stated: Stated::QuotedCurrent,
        expected_status: 200,
        expected_code: "",
    },
    // A version token's shape is `E` + 13 of the id alphabet, so this row is a well-formed
    // version that is simply not the stored one.
    Precondition {
        case: "stale-version",
        stated: Stated::Literal("E1STALEVERSION"),
        expected_status: 412,
        expected_code: "PreconditionFailed",
    },
    Precondition {
        case: "not-a-version-token",
        stated: Stated::Literal("not-a-version"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "empty-if-match",
        stated: Stated::Literal(""),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "lowercase-version-token",
        stated: Stated::Literal("e1abcdefghijklm"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "version-token-one-character-short",
        stated: Stated::Literal("E1ABCDEFGHIJK"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
];

/// The tenant's version governs the update, and a refused update leaves the tenant exactly
/// as it was — same configuration, same version.
#[tokio::test]
async fn the_tenants_version_is_the_preconditon_of_the_update() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-update-if-match").await;

    for (index, row) in PRECONDITIONS.iter().enumerate() {
        let case = row.case;
        let slug = format!("tp{index}x");
        let tenant = create_tenant(&router, &distribution_id, &slug, case).await;
        let body = update_xml("<Enabled>false</Enabled>");

        let stated = row.stated.header(&tenant.etag);
        let answer = update_tenant(&router, &tenant.id, &body, stated.as_deref()).await;
        assert_eq!(
            answer.status, row.expected_status,
            "case {case}: expected HTTP {} from If-Match {stated:?}, got {} with {}",
            row.expected_status, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            row.expected_code,
            "case {case}: expected error code {:?}, got {:?}",
            row.expected_code,
            answer.error_code()
        );
        if row.expected_code == "InvalidIfMatchVersion" {
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_IF_MATCH_VERSION_MESSAGE,
                "case {case}: expected the missing-or-invalid If-Match sentence, got {message:?}"
            );
        }

        let read = read_tenant(&router, &tenant.id, case).await;
        let reread = facts_of(read.root());
        if row.expected_status == 200 {
            assert_eq!(
                reread.enabled, "false",
                "case {case}: expected the served update to disable the tenant, got {:?}",
                reread.enabled
            );
            assert_ne!(
                read.header("etag"),
                tenant.etag,
                "case {case}: expected the served update to mint a new version, got {:?}",
                read.header("etag")
            );
        } else {
            assert_eq!(
                reread, tenant.facts,
                "case {case}: expected a refused update to leave the tenant at {:?}, got \
                 {reread:?}",
                tenant.facts
            );
            assert_eq!(
                read.header("etag"),
                tenant.etag,
                "case {case}: expected a refused update to leave the version at {:?}, got {:?}",
                tenant.etag,
                read.header("etag")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The state an update may not leave the tenant in
// ---------------------------------------------------------------------------

/// What one refusal row submits beyond the precondition. The variants that need a value only
/// the running emulator has (another tenant's domain, a standard distribution's id) name it
/// by kind and are filled in from the state the test built.
#[derive(Debug, Clone, Copy)]
enum Bad {
    /// A `Domains` element naming no domain at all.
    NoDomain,
    /// One domain past the published per-tenant bound.
    OneDomainPastTheBound,
    /// A `Domains` entry that is no domain name.
    MalformedDomain(&'static str),
    /// The same domain twice, which describes no single association.
    DomainTwice,
    /// A domain another distribution tenant already holds.
    AnotherTenantsDomain,
    /// An alias a distribution already claims.
    ADistributionsAlias,
    /// A fresh domain with no proof the account controls it.
    UnprovenDomain,
    /// A fresh domain the update asks CLOUDFRONT itself to serve the validation token at,
    /// which it can only do once the domain resolves to a CloudFront resource.
    UnverifiableCertificateDomain,
    /// A `DistributionId` no distribution carries.
    UnknownDistribution,
    /// A standard distribution's id, which has no distribution tenants.
    StandardDistribution,
    /// A `ConnectionGroupId` no connection group carries.
    UnknownConnectionGroup,
    /// A parameter value for a parameter the distribution defines nothing for.
    UndefinedParameter,
    /// A parameter list that supplies no value for a required parameter without a default.
    RequiredParameterUnsupplied,
    /// One parameter value past the bound a tenant's record carries.
    OneParameterPastTheBound,
}

/// One refusal row: what the update submits, and the status and wire code the service
/// answers it with.
#[derive(Debug, Clone, Copy)]
struct Refused {
    case: &'static str,
    bad: Bad,
    expected_status: u16,
    expected_code: &'static str,
}

/// Every state an update may not leave the tenant in, with the error the model declares for
/// it.
const REFUSALS: &[Refused] = &[
    Refused {
        case: "no-domain-at-all",
        bad: Bad::NoDomain,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "one-domain-past-the-bound",
        bad: Bad::OneDomainPastTheBound,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "single-label-domain",
        bad: Bad::MalformedDomain("localhost"),
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "domain-with-an-empty-label",
        bad: Bad::MalformedDomain("a..example.com"),
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "the-same-domain-twice",
        bad: Bad::DomainTwice,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "another-tenants-domain",
        bad: Bad::AnotherTenantsDomain,
        expected_status: 409,
        expected_code: "CNAMEAlreadyExists",
    },
    Refused {
        case: "a-distributions-alias",
        bad: Bad::ADistributionsAlias,
        expected_status: 409,
        expected_code: "CNAMEAlreadyExists",
    },
    Refused {
        case: "an-unproven-domain",
        bad: Bad::UnprovenDomain,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "a-domain-cloudfront-cannot-validate",
        bad: Bad::UnverifiableCertificateDomain,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "an-unknown-distribution",
        bad: Bad::UnknownDistribution,
        expected_status: 404,
        expected_code: "EntityNotFound",
    },
    Refused {
        case: "a-standard-distribution",
        bad: Bad::StandardDistribution,
        expected_status: 409,
        expected_code: "InvalidAssociation",
    },
    Refused {
        case: "an-unknown-connection-group",
        bad: Bad::UnknownConnectionGroup,
        expected_status: 404,
        expected_code: "EntityNotFound",
    },
    Refused {
        case: "an-undefined-parameter",
        bad: Bad::UndefinedParameter,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "a-required-parameter-left-unsupplied",
        bad: Bad::RequiredParameterUnsupplied,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
    Refused {
        case: "one-parameter-past-the-bound",
        bad: Bad::OneParameterPastTheBound,
        expected_status: 400,
        expected_code: "InvalidArgument",
    },
];

/// The values a refusal row needs from the running emulator.
struct World {
    /// A domain another distribution tenant holds.
    other_tenant_domain: String,
    /// An alias a standard distribution claims.
    distribution_alias: String,
    /// A STANDARD distribution's id, which has no distribution tenants.
    standard_distribution_id: String,
}

impl Bad {
    /// The request document this row submits against `tenant`.
    fn xml(self, tenant: &Tenant, slug: &str, world: &World) -> String {
        let members = match self {
            Bad::NoDomain => "<Domains></Domains>".to_string(),
            Bad::OneDomainPastTheBound => {
                let domains: Vec<String> = (0..=MAX_DOMAINS_PER_TENANT)
                    .map(|i| format!("b{i}-{slug}.example.com"))
                    .collect();
                format!(
                    "{}{}",
                    domains_element(&domains),
                    managed_certificate_request()
                )
            }
            Bad::MalformedDomain(domain) => format!(
                "{}{}",
                domains_element(&[domain.to_string()]),
                managed_certificate_request()
            ),
            Bad::DomainTwice => format!(
                "{}{}",
                domains_element(&[tenant.domain.clone(), tenant.domain.clone()]),
                managed_certificate_request()
            ),
            Bad::AnotherTenantsDomain => format!(
                "{}{}",
                domains_element(std::slice::from_ref(&world.other_tenant_domain)),
                managed_certificate_request()
            ),
            Bad::ADistributionsAlias => format!(
                "{}{}",
                domains_element(std::slice::from_ref(&world.distribution_alias)),
                managed_certificate_request()
            ),
            // No `ManagedCertificateRequest`: nothing on this call asks CloudFront to
            // validate the domain, and no record resolves it.
            Bad::UnprovenDomain => domains_element(&[format!("x-{slug}.example.com")]),
            Bad::UnverifiableCertificateDomain => format!(
                "{}<ManagedCertificateRequest><ValidationTokenHost>\
                 {VALIDATION_TOKEN_HOST_CLOUDFRONT}</ValidationTokenHost>\
                 </ManagedCertificateRequest>",
                domains_element(&[format!("y-{slug}.example.com")])
            ),
            Bad::UnknownDistribution => {
                "<DistributionId>EDFDVBD6EXAMPLE</DistributionId>".to_string()
            }
            Bad::StandardDistribution => format!(
                "<DistributionId>{}</DistributionId>",
                escape(&world.standard_distribution_id)
            ),
            Bad::UnknownConnectionGroup => {
                "<ConnectionGroupId>cg_1a2b3c4d5e6f7</ConnectionGroupId>".to_string()
            }
            Bad::UndefinedParameter => parameters_element(&[
                ("tenantName".to_string(), CREATED_PARAMETER.to_string()),
                ("undefined".to_string(), "value".to_string()),
            ]),
            Bad::RequiredParameterUnsupplied => {
                parameters_element(&[("region-label".to_string(), "eu".to_string())])
            }
            Bad::OneParameterPastTheBound => {
                let parameters: Vec<(String, String)> = (0..=MAX_PARAMETERS_PER_TENANT)
                    .map(|i| (format!("p{i}"), format!("v{i}")))
                    .collect();
                parameters_element(&parameters)
            }
        };
        update_xml(&members)
    }
}

/// An update that would leave the tenant in a state CloudFront does not admit is refused
/// with the error the model declares for it, and changes nothing: the record and its version
/// are what they were.
#[tokio::test]
async fn an_inadmissible_update_is_refused_and_changes_nothing() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-update-refused").await;
    let standard = standard_distribution(&router, "ref-tenant-update-standard").await;
    let neighbour = create_tenant(&router, &distribution_id, "neighbour", "neighbour").await;
    // The neighbour's domain is taken only once the certificate its create asked for has
    // been issued, so the clock is given the span it owes that issuance in before the row
    // that claims the domain runs.
    await_certificate_issuance().await;
    let world = World {
        other_tenant_domain: neighbour.domain.clone(),
        distribution_alias: standard.1,
        standard_distribution_id: standard.0,
    };

    for (index, row) in REFUSALS.iter().enumerate() {
        let case = row.case;
        let slug = format!("tr{index}x");
        let tenant = create_tenant(&router, &distribution_id, &slug, case).await;

        let answer = update_tenant(
            &router,
            &tenant.id,
            &row.bad.xml(&tenant, &slug, &world),
            Some(&tenant.etag),
        )
        .await;
        assert_eq!(
            answer.status, row.expected_status,
            "case {case}: expected HTTP {} for {:?}, got {} with {}",
            row.expected_status, row.bad, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            row.expected_code,
            "case {case}: expected error code {:?} for {:?}, got {:?}",
            row.expected_code,
            row.bad,
            answer.error_code()
        );
        if matches!(row.bad, Bad::UnprovenDomain) {
            // Every tenant this suite updates was created with a managed certificate request,
            // so it HOLDS a certificate: the refusal is about the domain that certificate
            // validates nothing for, not about a certificate the account does not have.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_DOMAIN_NAME_MESSAGE,
                "case {case}: expected the invalid-domain sentence, got {message:?}"
            );
        }

        let read = read_tenant(&router, &tenant.id, case).await;
        let reread = facts_of(read.root());
        assert_eq!(
            reread, tenant.facts,
            "case {case}: expected a refused update to leave the tenant at {:?}, got {reread:?}",
            tenant.facts
        );
        assert_eq!(
            read.header("etag"),
            tenant.etag,
            "case {case}: expected a refused update to leave the version at {:?}, got {:?}",
            tenant.etag,
            read.header("etag")
        );
    }
}

/// Create a STANDARD distribution claiming one alias, and answer `(Id, the alias)`.
///
/// A standard distribution "serves its own traffic": it has no distribution tenants, and the
/// alias it claims is a domain no tenant may claim beside it.
async fn standard_distribution(
    router: &MultiServiceRouter,
    caller_reference: &str,
) -> (String, String) {
    let alias = format!("{caller_reference}.example.com");
    let config = crate::distribution_lifecycle::Config {
        aliases: vec![alias.clone()],
        ..crate::distribution_lifecycle::config("standard-distribution", caller_reference)
    };
    let created = crate::distribution_lifecycle::create_distribution(router, &config.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference}: expected the standard distribution to be created, got {} with {}",
        created.status, created.raw
    );
    (
        created
            .root()
            .find_text("Id")
            .unwrap_or_else(|| {
                panic!("case {caller_reference}: the create carries no Id: {}", created.raw)
            })
            .to_string(),
        alias,
    )
}

// ---------------------------------------------------------------------------
// The tenant the store does not hold, the certificate, and the clock
// ---------------------------------------------------------------------------

/// A tenant id in the shape a tenant id takes but holding no tenant is `EntityNotFound`,
/// while an id outside that shape was never a tenant id at all and is `InvalidArgument`
/// naming the member that carried it. The `If-Match` stated is a well-formed version token,
/// so only the id can decide the answer.
#[tokio::test]
async fn an_unheld_tenant_is_not_found() {
    let router = emulator();
    let body = update_xml("<Enabled>false</Enabled>");

    for (case, id, status, code) in [
        (
            "never-minted-tenant-shape",
            "dt_1A2B3C4D5E6F7",
            404,
            "EntityNotFound",
        ),
        ("not-an-id-at-all", "nope", 400, "InvalidArgument"),
        (
            "a-distribution-id",
            "EDFDVBD6EXAMPLE",
            400,
            "InvalidArgument",
        ),
    ] {
        let answer = update_tenant(&router, id, &body, Some("E1STALEVERSION")).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for tenant {id:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code} for tenant {id:?}, got {:?}",
            answer.error_code()
        );
    }
}

/// A `ManagedCertificateRequest` on the update MINTS a certificate, over the domains the
/// tenant holds once the update has landed: the certificate surface answers from that one
/// record, with a validation token per domain, and the ARN is not the one the create minted.
#[tokio::test]
async fn an_update_requesting_a_certificate_mints_one_over_the_domains_it_landed() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-update-certificate").await;
    let case = "certificate";
    let tenant = create_tenant(&router, &distribution_id, "tcert", case).await;

    let before = get_managed_certificate_details(&router, &tenant.id).await;
    assert_eq!(
        before.status, 200,
        "case {case}: expected HTTP 200 from GetManagedCertificateDetails, got {} with {}",
        before.status, before.raw
    );
    let arn_before = before
        .root()
        .text_at("CertificateArn")
        .unwrap_or_default()
        .to_string();

    let domains = vec![
        "tcert-one.example.com".to_string(),
        "tcert-two.example.com".to_string(),
    ];
    let body = update_xml(&format!(
        "{}{}",
        domains_element(&domains),
        managed_certificate_request()
    ));
    let answer = update_tenant(&router, &tenant.id, &body, Some(&tenant.etag)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from UpdateDistributionTenant, got {} with {}",
        answer.status, answer.raw
    );

    let after = get_managed_certificate_details(&router, &tenant.id).await;
    assert_eq!(
        after.status, 200,
        "case {case}: expected HTTP 200 from GetManagedCertificateDetails, got {} with {}",
        after.status, after.raw
    );
    let covered: Vec<String> = after
        .root()
        .at("ValidationTokenDetails")
        .map(|list| {
            list.children
                .iter()
                .map(|entry| entry.text_at("Domain").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        covered, domains,
        "case {case}: expected the certificate to cover {domains:?}, got {covered:?} in {}",
        after.raw
    );
    let arn_after = after
        .root()
        .text_at("CertificateArn")
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        arn_after, arn_before,
        "case {case}: expected the update to mint a certificate other than {arn_before:?}, got \
         {arn_after:?}"
    );
}

/// The propagation an update owes the tenant is the CLOCK's: the tenant is `InProgress`
/// straight after the write and `Deployed` once the span has passed, without any call making
/// it so, and what the update landed survives the wait.
#[tokio::test]
async fn the_clock_settles_the_update() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-update-settle").await;
    let case = "settle";
    let tenant = create_tenant(&router, &distribution_id, "tsettle", case).await;

    // Start from a deployed tenant, so the status the update reports is the one it caused
    // rather than the create's.
    tokio::time::sleep(past_propagation()).await;
    let read = read_tenant(&router, &tenant.id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case {case}: expected the created tenant to settle to {STATUS_DEPLOYED}, got {:?}",
        read.root().text_at("Status")
    );

    let body = update_xml("<Enabled>false</Enabled>");
    let answer = update_tenant(&router, &tenant.id, &body, Some(&tenant.etag)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from UpdateDistributionTenant, got {} with {}",
        answer.status, answer.raw
    );
    let read = read_tenant(&router, &tenant.id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_IN_PROGRESS),
        "case {case}: expected {STATUS_IN_PROGRESS} straight after the update, got {:?}",
        read.root().text_at("Status")
    );

    tokio::time::sleep(past_propagation()).await;

    let read = read_tenant(&router, &tenant.id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case {case}: expected {STATUS_DEPLOYED} once the propagation was owed, got {:?}",
        read.root().text_at("Status")
    );
    assert_eq!(
        read.root().text_at("Enabled"),
        Some("false"),
        "case {case}: expected the update to survive the propagation, got {:?}",
        read.root().text_at("Enabled")
    );
}
