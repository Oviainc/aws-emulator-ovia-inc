// SPDX-License-Identifier: CC0-1.0

//! The CloudFront distribution tenant control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's distribution tenant
//! surface states:
//!
//! * a distribution tenant is PERSISTED by the call that creates it, which MINTS its
//!   identity (the `Id`, the ARN built from it and a version token), so a later
//!   `GetDistributionTenant` answers from that record rather than from the request being
//!   served — down to the members the create left out;
//! * `GetDistributionTenant`'s `Identifier` is "the ARN, ID, or name of the distribution
//!   tenant", so all three reach the one record;
//! * a tenant belongs to a MULTI-TENANT distribution: a distribution id no distribution
//!   carries is not found, and a standard distribution cannot be associated with one;
//! * "A valid parameter value must exist for any parameter that is marked as required in
//!   the multi-tenant distribution";
//! * "You must specify at least one domain in the request", "Each distribution tenant can
//!   have up to 5 domains", and a domain has to be a domain name;
//! * a name identifies ONE tenant in the account, and a domain routes to at most one
//!   CloudFront resource, so a repeat of either is refused and changes nothing;
//! * a tenant CloudFront has just accepted is `InProgress`, and the CLOCK — not a later
//!   call — owes it `Deployed`.
//!
//! The table rows differ along everything these rules generalise over: how many domains a
//! tenant claims and what case they are written in, both published bounds of the `Name`
//! pattern, whether the create names customizations, parameters, tags, a connection group
//! or an `Enabled` at all, and the two values `Enabled` can take.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::VALIDATION_TOKEN_HOST_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CONNECTION_GROUP_ID_PREFIX, DISTRIBUTION_TENANT_ID_PREFIX, DOMAIN_STATUS_ACTIVE,
    DOMAIN_STATUS_INACTIVE, MAX_DOMAINS_PER_TENANT, MAX_DOMAIN_LABEL_LEN, MAX_DOMAIN_NAME_LEN,
    MAX_GEO_RESTRICTION_LOCATIONS, MAX_PARAMETERS_PER_TENANT, PROPAGATION_DELAY, STATUS_DEPLOYED,
    STATUS_IN_PROGRESS, VALIDATION_TOKEN_HOST_CLOUDFRONT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, Answer, Node, ACCOUNT_ID, API_VERSION,
    CUSTOM_VIEWER_CERTIFICATE,
};
use crate::managed_certificate_domain_ownership::await_certificate_issuance;

// ---------------------------------------------------------------------------
// Calling the distribution tenant surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distribution-tenant` with the request document `body`.
///
/// `pub(crate)` so a sibling suite that needs a tenant — the only thing that makes the
/// account's DEFAULT connection group exist — creates one over the same wire path rather
/// than growing a copy of this.
pub(crate) async fn create_distribution_tenant(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution-tenant"),
        Some(body),
        None,
    )
    .await
}

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

/// Percent-encode the characters an `@httpLabel` may not carry literally. A tenant ARN is
/// an admissible `Identifier`, and it carries `/`, which would otherwise read as another
/// path segment.
///
/// `pub(crate)` so a sibling suite addressing a resource by ARN encodes it the same way.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            '?' | '#' | '%' => format!("%{:02X}", c as u32),
            other => other.to_string(),
        })
        .collect()
}

/// The `ManagedCertificateRequest` element a create carries for a domain the account has
/// not pointed at CloudFront: a tenant is bound to a domain only when the call asks
/// CloudFront to validate it or the domain already resolves to a CloudFront resource, and
/// every create below claims a name in a customer's zone, which this service resolves
/// nothing for. The token host is the one that is NOT `cloudfront` — "you serve the
/// validation token from your existing infrastructure", which demands nothing of DNS —
/// read off the model's own catalog rather than written out here.
///
/// `pub(crate)` so every sibling suite that needs a tenant asks for its domains to be
/// validated the same way rather than growing a copy of this.
pub(crate) fn managed_certificate_request() -> String {
    let host = VALIDATION_TOKEN_HOST_VALUES
        .iter()
        .copied()
        .find(|host| *host != VALIDATION_TOKEN_HOST_CLOUDFRONT)
        .expect("the model's ValidationTokenHost admits a host other than cloudfront");
    format!("<ManagedCertificateRequest><ValidationTokenHost>{host}</ValidationTokenHost></ManagedCertificateRequest>")
}

/// Escape the five entities XML predefines, so any text may travel in an element.
///
/// `pub(crate)` so a sibling suite that sends caller text through this wire path escapes
/// it the same way.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// The distributions a tenant is created under
// ---------------------------------------------------------------------------

/// The parameter definitions the multi-tenant distribution under test declares: one
/// REQUIRED parameter with no default, one required parameter that carries a
/// `DefaultValue`, and one optional parameter.
///
/// `ParameterDefinitions` is a list whose member carries no `xmlName`
/// (`smithy_query(query="shape",shape="ParameterDefinitions")`), so each entry travels in
/// the restXml default `<member>` element.
const TENANT_CONFIG: &str = "<TenantConfig><ParameterDefinitions>\
     <member><Name>tenantName</Name><Definition><StringSchema>\
     <Required>true</Required></StringSchema></Definition></member>\
     <member><Name>stage</Name><Definition><StringSchema>\
     <DefaultValue>prod</DefaultValue><Required>true</Required>\
     </StringSchema></Definition></member>\
     <member><Name>region-label</Name><Definition><StringSchema>\
     <Required>false</Required></StringSchema></Definition></member>\
     </ParameterDefinitions></TenantConfig>";

/// A `DistributionConfig` with one origin and a default cache behaviour, plus whatever
/// `extra` the caller adds at the configuration level.
fn distribution_config_xml(caller_reference: &str, extra: &str) -> String {
    enabled_distribution_config_xml(caller_reference, extra, true)
}

/// The same configuration under either value of `Enabled` — "from this field, you can
/// enable or disable the selected distribution".
fn enabled_distribution_config_xml(caller_reference: &str, extra: &str, enabled: bool) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         <Enabled>{enabled}</Enabled>{extra}\
         </DistributionConfig>"
    )
}

/// Create a multi-tenant distribution that is DISABLED, and answer its id.
///
/// `pub(crate)` so the sibling suite that asserts what a disabled distribution admits
/// creates one over this same wire path rather than growing a copy of it.
pub(crate) async fn disabled_multi_tenant_distribution(
    router: &MultiServiceRouter,
    caller_reference: &str,
) -> String {
    let body = enabled_distribution_config_xml(
        caller_reference,
        &format!("<ConnectionMode>tenant-only</ConnectionMode>{TENANT_CONFIG}"),
        false,
    );
    let created = create_distribution(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference}: expected the disabled multi-tenant distribution to be created, \
         got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .text_at("Id")
        .unwrap_or_else(|| panic!("case {caller_reference}: the create carries no Id"))
        .to_string()
}

/// Create the MULTI-TENANT distribution every tenant below is created under, and answer
/// its id. `ConnectionMode` `tenant-only` selects "a multi-tenant distribution with
/// distribution tenants".
///
/// `pub(crate)` for the same reason [`create_distribution_tenant`] is: a tenant can only
/// be created under one of these.
pub(crate) async fn multi_tenant_distribution(
    router: &MultiServiceRouter,
    caller_reference: &str,
) -> String {
    let body = distribution_config_xml(
        caller_reference,
        &format!("<ConnectionMode>tenant-only</ConnectionMode>{TENANT_CONFIG}"),
    );
    let created = create_distribution(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference}: expected the multi-tenant distribution to be created, got {} \
         with {}",
        created.status, created.raw
    );
    created
        .root()
        .text_at("Id")
        .unwrap_or_else(|| panic!("case {caller_reference}: the create carries no Id"))
        .to_string()
}

/// Create a STANDARD distribution, optionally carrying `aliases`, and answer its id.
async fn standard_distribution(
    router: &MultiServiceRouter,
    caller_reference: &str,
    aliases: &[&str],
) -> String {
    let mut extra = "<ConnectionMode>direct</ConnectionMode>".to_string();
    if !aliases.is_empty() {
        extra.push_str(&format!(
            "<Aliases><Quantity>{}</Quantity><Items>",
            aliases.len()
        ));
        for alias in aliases {
            extra.push_str(&format!("<CNAME>{}</CNAME>", escape(alias)));
        }
        extra.push_str("</Items></Aliases>");
        // `ViewerCertificate`: a distribution that uses `Aliases` names one of
        // `ACMCertificateArn` or `IAMCertificateId`, because CloudFront's own default
        // certificate validates no alternate domain name — so a standard distribution
        // that claims a domain states a certificate for it, which is what makes this a
        // request CloudFront accepts.
        extra.push_str(CUSTOM_VIEWER_CERTIFICATE);
    }
    let created = create_distribution(router, &distribution_config_xml(caller_reference, &extra)).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference}: expected the standard distribution to be created, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .text_at("Id")
        .unwrap_or_else(|| panic!("case {caller_reference}: the create carries no Id"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateDistributionTenant` request a client submits.
#[derive(Debug, Clone)]
struct Create {
    case: &'static str,
    name: String,
    /// `Domains`, in the order the request lists them.
    domains: Vec<String>,
    /// `Customizations`, as the XML fragment the row submits, or `None` to leave the
    /// member out entirely.
    customizations: Option<&'static str>,
    /// `Parameters`, as `(Name, Value)` pairs.
    parameters: Vec<(&'static str, &'static str)>,
    /// `Tags.Items`, as `(Key, Value)` pairs.
    tags: Vec<(&'static str, &'static str)>,
    /// `Enabled`, or `None` to leave the optional member out entirely.
    enabled: Option<bool>,
}

impl Create {
    /// The `CreateDistributionTenantRequest` XML a client sends for this create, under
    /// `distribution_id`.
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
        if let Some(customizations) = self.customizations {
            out.push_str(customizations);
        }
        if !self.parameters.is_empty() {
            out.push_str("<Parameters>");
            for (name, value) in &self.parameters {
                out.push_str(&format!(
                    "<member><Name>{}</Name><Value>{}</Value></member>",
                    escape(name),
                    escape(value)
                ));
            }
            out.push_str("</Parameters>");
        }
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                    escape(key),
                    escape(value)
                ));
            }
            out.push_str("</Items></Tags>");
        }
        if let Some(enabled) = self.enabled {
            out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
        }
        // Every row claims a name in a customer's zone, so the create asks CloudFront to
        // validate it.
        out.push_str(&managed_certificate_request());
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }

    /// `Enabled` as the record must report it: the value submitted, or the serving tenant
    /// a create that named none asks for.
    fn expected_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// A `Customizations` element that overrides the distribution's WAF web ACL and narrows
/// its geographic restrictions. `LocationList`'s member carries `xmlName` `Location`
/// (`smithy_query(query="shape",shape="LocationList")`), so each entry travels in a
/// `<Location>` element.
const CUSTOMIZATIONS_OVERRIDE: &str = "<Customizations>\
     <WebAcl><Action>override</Action>\
     <Arn>arn:aws:wafv2:us-east-1:000000000000:global/webacl/tenant/abc</Arn></WebAcl>\
     <GeoRestrictions><RestrictionType>whitelist</RestrictionType>\
     <Locations><Location>DE</Location><Location>JP</Location></Locations>\
     </GeoRestrictions></Customizations>";

/// A `Customizations` element that DISABLES the distribution's web ACL for the tenant —
/// `CustomizationActionType` is `override | disable`
/// (`smithy_query(query="shape",shape="CustomizationActionType")`).
const CUSTOMIZATIONS_DISABLE: &str =
    "<Customizations><WebAcl><Action>disable</Action></WebAcl></Customizations>";

/// The creates every rule below is asserted over.
///
/// `DistributionTenant$Name` is `pattern ^[a-zA-Z0-9][a-zA-Z0-9-.]{1,126}[a-zA-Z0-9]$`
/// (`smithy_query(query="operation",operation="CreateDistributionTenant")`), so the rows
/// sit at both of its length bounds (3 and 128) and draw from each character class it
/// admits; and they span one domain and several, an uppercase domain, both forms of
/// `Customizations`, a parameter list that relies on a `DefaultValue` and one that does
/// not, a tagged and an untagged create, and `Enabled` submitted true, submitted false and
/// left out.
fn create_table() -> Vec<Create> {
    vec![
        Create {
            case: "the shortest admissible name, one domain, nothing optional",
            name: "a1b".to_string(),
            domains: vec!["shop.example.com".to_string()],
            customizations: None,
            parameters: vec![("tenantName", "acme")],
            tags: Vec::new(),
            enabled: None,
        },
        Create {
            case: "the longest admissible name (128), several domains, tags",
            name: format!("t{}9", "n".repeat(126)),
            domains: vec![
                "www.second.example.com".to_string(),
                "cdn.second.example.com".to_string(),
                "static.second.example.com".to_string(),
            ],
            customizations: Some(CUSTOMIZATIONS_OVERRIDE),
            parameters: vec![("tenantName", "second"), ("stage", "staging")],
            tags: vec![("stage", "prod"), ("owner", "edge-team")],
            enabled: Some(true),
        },
        Create {
            case: "dots and hyphens in the name, a disabled tenant",
            name: "third.tenant-2".to_string(),
            domains: vec!["THIRD.EXAMPLE.COM".to_string()],
            customizations: Some(CUSTOMIZATIONS_DISABLE),
            parameters: vec![("tenantName", "third"), ("region-label", "eu")],
            tags: vec![("owner", "edge-team")],
            enabled: Some(false),
        },
        Create {
            case: "a domain at both published DNS bounds, no customizations",
            name: "Fourth42".to_string(),
            domains: vec![longest_domain()],
            customizations: None,
            // `stage` is required but carries a `DefaultValue`, so a valid value exists
            // for it without the request naming one.
            parameters: vec![("tenantName", "fourth")],
            tags: Vec::new(),
            enabled: Some(true),
        },
    ]
}

/// The longest domain name a tenant may claim: labels of the longest admissible length
/// (`MAX_DOMAIN_LABEL_LEN`) packed up to the whole-name bound (`MAX_DOMAIN_NAME_LEN`),
/// both RFC 1035's.
///
/// `pub(crate)` so a sibling suite that exercises the same published bounds names the
/// same row rather than building a second one.
pub(crate) fn longest_domain() -> String {
    let label = "d".repeat(MAX_DOMAIN_LABEL_LEN);
    let mut domain = format!("{label}.{label}.{label}");
    // Fill the rest with one final label that takes the name exactly to the bound.
    let remaining = MAX_DOMAIN_NAME_LEN - domain.chars().count() - 1;
    domain.push('.');
    domain.push_str(&"e".repeat(remaining));
    assert_eq!(
        domain.chars().count(),
        MAX_DOMAIN_NAME_LEN,
        "the longest-domain row must sit exactly at the bound"
    );
    domain
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// The text of one member of the `DistributionTenant` document, or a panic naming the case.
fn member(tenant: &Node, name: &str, case: &str, raw: &str) -> String {
    tenant
        .text_at(name)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {name}: {raw}"))
        .to_string()
}

/// The `(Domain, Status)` of every entry of the tenant's `Domains` list, in document order.
///
/// `pub(crate)` so a sibling suite that compares a tenant's domains — a LISTING's summary
/// against a read of the same record, say — reads them the same way rather than growing a
/// copy of this.
pub(crate) fn domains_of(tenant: &Node) -> Vec<(String, String)> {
    tenant
        .at("Domains")
        .map(|list| {
            list.children
                .iter()
                .map(|entry| {
                    (
                        entry.text_at("Domain").unwrap_or_default().to_string(),
                        entry.text_at("Status").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `(Key, Value)` of every tag the tenant reports, in document order.
fn tags_of(tenant: &Node) -> Vec<(String, String)> {
    tenant
        .at("Tags/Items")
        .map(|items| {
            items
                .children_named("Tag")
                .into_iter()
                .map(|tag| {
                    (
                        tag.text_at("Key").unwrap_or_default().to_string(),
                        tag.text_at("Value").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `(Name, Value)` of every parameter the tenant reports, in document order.
fn parameters_of(tenant: &Node) -> Vec<(String, String)> {
    tenant
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
        .unwrap_or_default()
}

/// Whether `value` has the shape of a CloudFront version token (`ETag`): `E` followed by
/// 13 uppercase alphanumerics, the shape this service's own tokens take.
fn is_version_token(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Assert the `DistributionTenant` document `answer` carries is the record `create` asked
/// for under `distribution_id`, in `status`, and answer its `(Id, Arn, ConnectionGroupId)`.
fn assert_record(
    answer: &Answer,
    create: &Create,
    distribution_id: &str,
    status: &str,
    context: &str,
) -> (String, String, String) {
    let case = create.case;
    let tenant = answer.root();
    let raw = answer.raw.as_str();

    let name = member(tenant, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let id = member(tenant, "Id", case, raw);
    assert!(
        id.starts_with(DISTRIBUTION_TENANT_ID_PREFIX) && id.len() > DISTRIBUTION_TENANT_ID_PREFIX.len(),
        "case {case} ({context}): expected a minted {DISTRIBUTION_TENANT_ID_PREFIX} id, got {id:?}"
    );

    let arn = member(tenant, "Arn", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution-tenant/{id}"),
        "case {case} ({context}): expected the ARN built from the Id, got {arn:?}"
    );

    let reported_distribution = member(tenant, "DistributionId", case, raw);
    assert_eq!(
        reported_distribution, distribution_id,
        "case {case} ({context}): expected the submitted DistributionId, got \
         {reported_distribution:?}"
    );

    // The domains come back as submitted, in order, each under the status it is SERVED at.
    // Every row's create asks CloudFront to validate its domains
    // ([`managed_certificate_request`]), and a domain whose managed certificate is still
    // waiting to be validated is served by nothing yet, so it reads `inactive` until the
    // clock issues that certificate. The certificate's validation span and the tenant's
    // propagation span are the same length, so the tenant's own status says which side of
    // it this answer is on.
    let expected_domain_status = if status == STATUS_IN_PROGRESS {
        DOMAIN_STATUS_INACTIVE
    } else {
        DOMAIN_STATUS_ACTIVE
    };
    let expected_domains: Vec<(String, String)> = create
        .domains
        .iter()
        .map(|domain| (domain.clone(), expected_domain_status.to_string()))
        .collect();
    let domains = domains_of(tenant);
    assert_eq!(
        domains, expected_domains,
        "case {case} ({context}): expected Domains {expected_domains:?}, got {domains:?}"
    );

    let enabled = member(tenant, "Enabled", case, raw);
    assert_eq!(
        enabled,
        create.expected_enabled().to_string(),
        "case {case} ({context}): expected Enabled {}, got {enabled:?}",
        create.expected_enabled()
    );

    let actual_status = member(tenant, "Status", case, raw);
    assert_eq!(
        actual_status, status,
        "case {case} ({context}): expected Status {status}, got {actual_status:?}"
    );

    // Tags and parameters are reported as submitted — a create that named none reports
    // none.
    let expected_tags: Vec<(String, String)> = {
        let mut pairs: Vec<(String, String)> = create
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        pairs.sort();
        pairs
    };
    let mut tags = tags_of(tenant);
    tags.sort();
    assert_eq!(
        tags, expected_tags,
        "case {case} ({context}): expected Tags {expected_tags:?}, got {tags:?}"
    );

    let expected_parameters: Vec<(String, String)> = create
        .parameters
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let parameters = parameters_of(tenant);
    assert_eq!(
        parameters, expected_parameters,
        "case {case} ({context}): expected Parameters {expected_parameters:?}, got {parameters:?}"
    );

    // The customizations are the tenant's own overrides: present exactly when the create
    // named them, with the values it named.
    let customizations = tenant.at("Customizations");
    assert_eq!(
        customizations.is_some(),
        create.customizations.is_some(),
        "case {case} ({context}): expected Customizations present={}, got present={}",
        create.customizations.is_some(),
        customizations.is_some()
    );
    if create.customizations == Some(CUSTOMIZATIONS_OVERRIDE) {
        let action = member(tenant, "Customizations/WebAcl/Action", case, raw);
        assert_eq!(
            action, "override",
            "case {case} ({context}): expected the submitted WebAcl Action, got {action:?}"
        );
        let locations: Vec<String> = tenant
            .at("Customizations/GeoRestrictions/Locations")
            .map(|list| {
                list.children_named("Location")
                    .into_iter()
                    .map(|node| node.text().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            locations,
            vec!["DE".to_string(), "JP".to_string()],
            "case {case} ({context}): expected the submitted Locations, got {locations:?}"
        );
    }
    if create.customizations == Some(CUSTOMIZATIONS_DISABLE) {
        let action = member(tenant, "Customizations/WebAcl/Action", case, raw);
        assert_eq!(
            action, "disable",
            "case {case} ({context}): expected the submitted WebAcl Action, got {action:?}"
        );
    }

    for stamp in ["CreatedTime", "LastModifiedTime"] {
        let value = member(tenant, stamp, case, raw);
        assert!(
            !value.is_empty(),
            "case {case} ({context}): expected a {stamp}, got {value:?}"
        );
    }

    let connection_group_id = member(tenant, "ConnectionGroupId", case, raw);
    assert!(
        connection_group_id.starts_with(CONNECTION_GROUP_ID_PREFIX),
        "case {case} ({context}): expected the default connection group's id, got \
         {connection_group_id:?}"
    );

    (id, arn, connection_group_id)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the tenant it mints, and the read answers from that one record: the
/// name, domains, parameters, customizations and tags as submitted, a minted id, the ARN
/// built from it, and the version token the create handed back. Across the table no two
/// tenants share an id, an ARN or a version token — and every one of them is routed
/// through the SAME default connection group, which nothing named.
#[tokio::test]
async fn create_persists_the_tenant_the_read_then_reports() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-persist").await;
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();
    let mut connection_groups = BTreeSet::new();

    for create in create_table() {
        let case = create.case;
        let created = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        // A tenant CloudFront has just accepted has not propagated yet.
        let (id, arn, connection_group_id) = assert_record(
            &created,
            &create,
            &distribution_id,
            STATUS_IN_PROGRESS,
            "the create",
        );
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );
        connection_groups.insert(connection_group_id);

        // "You can specify the ARN, ID, or name of the distribution tenant": each of the
        // three reaches the one record, at the version the create handed back.
        for (how, identifier) in [("by id", id.clone()), ("by name", create.name.clone()), ("by ARN", arn.clone())] {
            let read = get_distribution_tenant(&router, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({how}): expected HTTP 200, got {} with {}",
                read.status, read.raw
            );
            let (read_id, read_arn, _) = assert_record(
                &read,
                &create,
                &distribution_id,
                STATUS_IN_PROGRESS,
                "the read",
            );
            assert_eq!(
                (read_id, read_arn),
                (id.clone(), arn.clone()),
                "case {case} ({how}): the read must report the record the create stored"
            );
            assert_eq!(
                read.header("etag"),
                etag,
                "case {case} ({how}): expected the create's version token, got {:?}",
                read.header("etag")
            );
        }
    }

    assert_eq!(
        connection_groups.len(),
        1,
        "every tenant that named no connection group must be routed through the one default \
         connection group, got {connection_groups:?}"
    );
}

/// A tenant's name is "unique within your Amazon Web Services account": the second create
/// under a name is refused with `EntityAlreadyExists` (HTTP 409), and the tenant that
/// holds the name keeps the identity and version token its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_distribution_tenant() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-name").await;

    for create in create_table() {
        let case = create.case;
        let first = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let id = member(first.root(), "Id", case, &first.raw);
        let etag = first.header("etag").to_string();

        // The same name again, with domains that differ — so a tenant that was replaced
        // rather than refused would be visible.
        let repeat = Create {
            domains: vec![format!("repeat-{}.example.com", create.domains.len())],
            ..create.clone()
        };
        let second = create_distribution_tenant(&router, &repeat.xml(&distribution_id)).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "EntityAlreadyExists",
            "case {case}: expected EntityAlreadyExists, got {}",
            second.raw
        );

        let read = get_distribution_tenant(&router, &create.name).await;
        let (read_id, _, _) = assert_record(
            &read,
            &create,
            &distribution_id,
            STATUS_IN_PROGRESS,
            "after a refusal",
        );
        assert_eq!(
            read_id, id,
            "case {case}: the refused create must not have replaced the tenant"
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// "The CNAME specified is already defined for CloudFront": a domain routes to at most one
/// CloudFront resource, so a create claiming a domain another tenant is already SERVED at —
/// or an alias a distribution holds — is refused with `CNAMEAlreadyExists` (HTTP 409) and
/// stores nothing. DNS names are case-insensitive, so the case the domain is written in does
/// not make it a different domain. A claim is enforced only once the certificate it waits on
/// has been validated, which is what [`crate::tenant_unvalidated_domain_claim`] is about and
/// why the holder's issuance is waited out here.
#[tokio::test]
async fn a_domain_routes_to_at_most_one_cloudfront_resource() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-cname").await;
    standard_distribution(&router, "ref-tenant-cname-alias", &["alias.example.com"]).await;

    let holder = Create {
        case: "the tenant that holds the domains",
        name: "holder".to_string(),
        domains: vec!["held.example.com".to_string(), "Also.Held.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "holder")],
        tags: Vec::new(),
        enabled: None,
    };
    let created = create_distribution_tenant(&router, &holder.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        holder.case, created.status, created.raw
    );
    // The holder's domains are taken only once the certificate the create asked for has
    // been issued, so the clock is given the span it owes that issuance in.
    await_certificate_issuance().await;

    // Rows: the same domain again, the same domain in another case, the other domain of
    // the same tenant, an alias a DISTRIBUTION holds, and one domain of a pair where the
    // other is free.
    let conflicts: [(&str, Vec<String>); 5] = [
        ("the same domain again", vec!["held.example.com".to_string()]),
        (
            "the same domain in another case",
            vec!["HELD.EXAMPLE.COM".to_string()],
        ),
        (
            "the tenant's other domain, lowercased",
            vec!["also.held.example.com".to_string()],
        ),
        (
            "an alias of a distribution",
            vec!["alias.example.com".to_string()],
        ),
        (
            "one taken domain beside a free one",
            vec!["free.example.com".to_string(), "held.example.com".to_string()],
        ),
    ];

    for (index, (case, domains)) in conflicts.into_iter().enumerate() {
        let claim = Create {
            case: "a claim on a taken domain",
            name: format!("claimant{index}"),
            domains,
            customizations: None,
            parameters: vec![("tenantName", "claimant")],
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &claim.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "CNAMEAlreadyExists",
            "case {case}: expected CNAMEAlreadyExists, got {}",
            answer.raw
        );
        let read = get_distribution_tenant(&router, &claim.name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }

    // A domain nothing holds is still claimable after all those refusals.
    let free = Create {
        case: "a free domain",
        name: "free-tenant".to_string(),
        domains: vec!["free.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "free")],
        tags: Vec::new(),
        enabled: None,
    };
    let answer = create_distribution_tenant(&router, &free.xml(&distribution_id)).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        free.case, answer.status, answer.raw
    );
}

/// A tenant belongs to a MULTI-TENANT distribution: a `DistributionId` no distribution
/// carries is `EntityNotFound` (HTTP 404), and a standard distribution cannot be
/// associated with a tenant — `InvalidAssociation` (HTTP 409). Either way nothing is
/// stored.
#[tokio::test]
async fn a_tenant_belongs_to_a_multi_tenant_distribution() {
    let router = emulator();
    let multi_tenant = multi_tenant_distribution(&router, "ref-tenant-association").await;
    let standard = standard_distribution(&router, "ref-tenant-association-standard", &[]).await;

    // Rows: an id of the right shape that names nothing, an id of no shape at all, the
    // empty id, and a standard distribution's id.
    let cases: [(&str, String, u16, &str); 4] = [
        (
            "an id no distribution carries",
            "E1234567890ABC".to_string(),
            404,
            "EntityNotFound",
        ),
        (
            "an id of no distribution shape",
            "not-a-distribution".to_string(),
            404,
            "EntityNotFound",
        ),
        (
            "a tenant id in place of a distribution id",
            format!("{DISTRIBUTION_TENANT_ID_PREFIX}0000000000000"),
            404,
            "EntityNotFound",
        ),
        (
            "a standard distribution",
            standard,
            409,
            "InvalidAssociation",
        ),
    ];

    for (index, (case, distribution_id, status, code)) in cases.into_iter().enumerate() {
        let create = Create {
            case: "an unusable distribution",
            name: format!("unusable{index}"),
            domains: vec![format!("unusable{index}.example.com")],
            customizations: None,
            parameters: Vec::new(),
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        let read = get_distribution_tenant(&router, &create.name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }

    // The same create under the multi-tenant distribution is served.
    let create = Create {
        case: "the multi-tenant distribution",
        name: "associated".to_string(),
        domains: vec!["associated.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "associated")],
        tags: Vec::new(),
        enabled: None,
    };
    let answer = create_distribution_tenant(&router, &create.xml(&multi_tenant)).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, answer.status, answer.raw
    );
}

/// "A valid parameter value must exist for any parameter that is marked as required in the
/// multi-tenant distribution": a value for a parameter the distribution defines nothing
/// for, a parameter named twice, and a required parameter with neither a submitted value
/// nor a `DefaultValue` are each refused with `InvalidArgument` (HTTP 400) — while a
/// required parameter that carries a `DefaultValue` needs no submitted value.
#[tokio::test]
async fn parameters_must_satisfy_the_distributions_definitions() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-parameters").await;

    let refused: [(&str, Vec<(&str, &str)>); 4] = [
        ("no value for the required parameter", vec![]),
        (
            "a parameter the distribution does not define",
            vec![("tenantName", "acme"), ("unknown", "x")],
        ),
        (
            "the same parameter twice",
            vec![("tenantName", "acme"), ("tenantName", "other")],
        ),
        (
            "only the parameter that has a default",
            vec![("stage", "dev")],
        ),
    ];

    for (index, (case, parameters)) in refused.into_iter().enumerate() {
        let create = Create {
            case: "an unusable parameter list",
            name: format!("param-refused-{index}"),
            domains: vec![format!("param-refused-{index}.example.com")],
            customizations: None,
            parameters,
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
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
        let read = get_distribution_tenant(&router, &create.name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }

    let accepted: [(&str, Vec<(&str, &str)>); 3] = [
        ("the required parameter alone", vec![("tenantName", "acme")]),
        (
            "every defined parameter",
            vec![
                ("tenantName", "acme"),
                ("stage", "dev"),
                ("region-label", "eu"),
            ],
        ),
        (
            "the optional parameter beside the required one",
            vec![("region-label", "eu"), ("tenantName", "acme")],
        ),
    ];

    for (index, (case, parameters)) in accepted.into_iter().enumerate() {
        let create = Create {
            case: "a usable parameter list",
            name: format!("param-ok-{index}"),
            domains: vec![format!("param-ok-{index}.example.com")],
            customizations: None,
            parameters: parameters.clone(),
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        // The values come back in the order submitted, so the read answers from the stored
        // list rather than from the distribution's definitions.
        let expected: Vec<(String, String)> = parameters
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let read = get_distribution_tenant(&router, &create.name).await;
        let got = parameters_of(read.root());
        assert_eq!(
            got, expected,
            "case {case}: expected Parameters {expected:?}, got {got:?}"
        );
    }
}

/// "The domains associated with the distribution tenant. You must specify at least one
/// domain in the request", "Each distribution tenant can have up to 5 domains" — and a
/// domain has to be a domain name. Each refusal is `InvalidArgument` (HTTP 400) and stores
/// nothing; a list at the published domain-count bound and a name at either published DNS
/// bound are accepted.
#[tokio::test]
async fn domains_must_be_at_least_one_domain_name() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-domains").await;
    let label = "d".repeat(MAX_DOMAIN_LABEL_LEN);

    // Rows: no domain at all, the empty domain, a single label, an empty label, one
    // character past each published bound, and a domain carrying whitespace.
    let refused: [(&str, Vec<String>); 8] = [
        ("no domain at all", Vec::new()),
        (
            "one domain past the per-tenant quota",
            (0..=MAX_DOMAINS_PER_TENANT)
                .map(|index| format!("over-{index}.example.com"))
                .collect(),
        ),
        ("the empty domain", vec![String::new()]),
        ("a single label", vec!["localhost".to_string()]),
        ("an empty label", vec!["a..example.com".to_string()]),
        (
            "one character past the label bound",
            vec![format!("{label}d.example.com")],
        ),
        (
            "one character past the name bound",
            vec![format!("{}x", longest_domain())],
        ),
        (
            "whitespace in a label",
            vec!["two words.example.com".to_string()],
        ),
    ];

    for (index, (case, domains)) in refused.into_iter().enumerate() {
        let create = Create {
            case: "an unusable domain list",
            name: format!("domain-refused-{index}"),
            domains,
            customizations: None,
            parameters: vec![("tenantName", "acme")],
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
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
        let read = get_distribution_tenant(&router, &create.name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }

    // A request that names ONE domain twice describes no single association either.
    let duplicate = Create {
        case: "the same domain twice",
        name: "domain-duplicate".to_string(),
        domains: vec!["dup.example.com".to_string(), "DUP.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "acme")],
        tags: Vec::new(),
        enabled: None,
    };
    let answer = create_distribution_tenant(&router, &duplicate.xml(&distribution_id)).await;
    assert_eq!(
        answer.status, 400,
        "case {}: expected HTTP 400, got {} with {}",
        duplicate.case, answer.status, answer.raw
    );

    // And the admissible extremes are served: the shortest name (two labels) and the
    // longest the bounds admit.
    let accepted: [(&str, Vec<String>); 3] = [
        ("the shortest domain name", vec!["a.b".to_string()]),
        ("the longest domain name", vec![longest_domain()]),
        (
            "exactly as many domains as the quota admits",
            (0..MAX_DOMAINS_PER_TENANT)
                .map(|index| format!("quota-{index}.example.com"))
                .collect(),
        ),
    ];
    for (index, (case, domains)) in accepted.into_iter().enumerate() {
        let create = Create {
            case: "a usable domain list",
            name: format!("domain-ok-{index}"),
            domains,
            customizations: None,
            parameters: vec![("tenantName", "acme")],
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// A connection group this emulator does not hold names no entity — `EntityNotFound`
/// (HTTP 404) — while the default one every tenant is routed through is nameable, and
/// routes the tenant that names it to the same group.
#[tokio::test]
async fn a_connection_group_must_exist_to_be_named() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-connection-group").await;

    // First tenant: names no connection group, so the default one is minted for it.
    let first = Create {
        case: "the default connection group",
        name: "cg-default".to_string(),
        domains: vec!["cg-default.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "acme")],
        tags: Vec::new(),
        enabled: None,
    };
    let created = create_distribution_tenant(&router, &first.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        first.case, created.status, created.raw
    );
    let default_group = member(created.root(), "ConnectionGroupId", first.case, &created.raw);

    for (index, (case, connection_group_id)) in [
        ("an id no connection group carries", format!("{CONNECTION_GROUP_ID_PREFIX}0000000000000")),
        ("an id of no connection group shape", "not-a-group".to_string()),
    ]
    .into_iter()
    .enumerate()
    {
        let body = format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{distribution_id}</DistributionId>\
             <Name>cg-absent-{index}</Name>\
             <Domains><member><Domain>cg-absent-{index}.example.com</Domain></member></Domains>\
             <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>\
             <ConnectionGroupId>{connection_group_id}</ConnectionGroupId>{}\
             </CreateDistributionTenantRequest>",
            managed_certificate_request()
        );
        let answer = create_distribution_tenant(&router, &body).await;
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
    }

    // Naming the group that does exist routes the tenant through it.
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId>\
         <Name>cg-named</Name>\
         <Domains><member><Domain>cg-named.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>\
         <ConnectionGroupId>{default_group}</ConnectionGroupId>{}\
         </CreateDistributionTenantRequest>",
        managed_certificate_request()
    );
    let answer = create_distribution_tenant(&router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case the connection group that exists: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    let named_group = member(
        answer.root(),
        "ConnectionGroupId",
        "the connection group that exists",
        &answer.raw,
    );
    assert_eq!(
        named_group, default_group,
        "case the connection group that exists: expected the tenant to be routed through \
         {default_group}, got {named_group:?}"
    );
}

/// A tenant CloudFront has just created is `InProgress`, and the CLOCK owes it `Deployed`:
/// no call asks for the transition, and the transition changes nothing else about the
/// record — not its identity, and not the version token an `If-Match` is checked against.
#[tokio::test]
async fn the_clock_settles_a_tenant_to_deployed() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-clock").await;
    let create = Create {
        case: "propagation settles",
        name: "settles-to-deployed".to_string(),
        domains: vec!["settles.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "acme")],
        tags: Vec::new(),
        enabled: None,
    };

    let created = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, created.status, created.raw
    );
    let (id, arn, _) = assert_record(
        &created,
        &create,
        &distribution_id,
        STATUS_IN_PROGRESS,
        "the create",
    );
    let etag = created.header("etag").to_string();

    // A caller acting straight after the create still observes the transient status.
    let immediate = get_distribution_tenant(&router, &id).await;
    assert_record(
        &immediate,
        &create,
        &distribution_id,
        STATUS_IN_PROGRESS,
        "straight after the create",
    );

    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;

    let settled = get_distribution_tenant(&router, &id).await;
    let (settled_id, settled_arn, _) = assert_record(
        &settled,
        &create,
        &distribution_id,
        STATUS_DEPLOYED,
        "after the span",
    );
    assert_eq!(
        (settled_id, settled_arn),
        (id, arn),
        "case {}: propagation must not change the tenant's identity",
        create.case
    );
    assert_eq!(
        settled.header("etag"),
        etag,
        "case {}: propagation is not a mutation, so the version token stands",
        create.case
    );
}

/// An identifier no tenant answers to is `EntityNotFound` (HTTP 404), whatever form it
/// takes — and a tenant that does exist leaves every other identifier unfound.
#[tokio::test]
async fn a_read_refuses_an_identifier_no_tenant_answers_to() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-absent").await;
    let present = Create {
        case: "the one tenant that exists",
        name: "present-tenant".to_string(),
        domains: vec!["present.example.com".to_string()],
        customizations: None,
        parameters: vec![("tenantName", "acme")],
        tags: Vec::new(),
        enabled: None,
    };
    let created = create_distribution_tenant(&router, &present.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = member(created.root(), "Id", present.case, &created.raw);

    // Rows: a name never created, the existing name in another case, a prefix of it, an id
    // of the right shape that names nothing, an ARN naming nothing, and the existing id
    // with a character changed.
    let absent: [(&str, String); 6] = [
        ("a name never created", "absent-tenant".to_string()),
        ("the same name in another case", "PRESENT-TENANT".to_string()),
        ("a prefix of the existing name", "present".to_string()),
        (
            "an id no tenant carries",
            format!("{DISTRIBUTION_TENANT_ID_PREFIX}0000000000000"),
        ),
        (
            "an ARN no tenant carries",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution-tenant/{DISTRIBUTION_TENANT_ID_PREFIX}ZZZZZZZZZZZZZ"),
        ),
        ("the existing id with its prefix dropped", id[DISTRIBUTION_TENANT_ID_PREFIX.len()..].to_string()),
    ];

    for (case, identifier) in absent {
        let answer = get_distribution_tenant(&router, &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {identifier:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {identifier:?}, got {}",
            answer.raw
        );
    }
}

/// `DistributionTenant$Name` is `pattern ^[a-zA-Z0-9][a-zA-Z0-9-.]{1,126}[a-zA-Z0-9]$`: a
/// name outside it is refused before anything is stored.
#[tokio::test]
async fn a_name_outside_the_modelled_pattern_is_refused() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-name-pattern").await;

    // Rows: one character short of the pattern's minimum, one past its maximum, a
    // character it excludes, and a name ending in a character it admits only inside.
    let cases: [(&str, String); 4] = [
        ("two characters, one short of the minimum", "ab".to_string()),
        (
            "one character past the maximum (128)",
            format!("t{}9", "n".repeat(127)),
        ),
        ("an underscore, which the pattern excludes", "under_score".to_string()),
        ("a trailing dot", "trailing.".to_string()),
    ];

    for (case, name) in cases {
        let create = Create {
            case: "outside the pattern",
            name: name.clone(),
            domains: vec!["pattern.example.com".to_string()],
            customizations: None,
            parameters: vec![("tenantName", "acme")],
            tags: Vec::new(),
            enabled: None,
        };
        let answer = create_distribution_tenant(&router, &create.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {name:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case}: expected ValidationException for {name:?}, got {}",
            answer.raw
        );
    }
}

/// A tag set no resource can carry (one key named twice, or more tags than a resource may
/// hold) is refused with `InvalidTagging` (HTTP 400), and stores nothing.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-tagging").await;

    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>tagged-twice</Name>\
         <Domains><member><Domain>tagged.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>\
         <Tags><Items>\
         <Tag><Key>stage</Key><Value>prod</Value></Tag>\
         <Tag><Key>stage</Key><Value>dev</Value></Tag>\
         </Items></Tags>{}\
         </CreateDistributionTenantRequest>",
        managed_certificate_request()
    );
    let answer = create_distribution_tenant(&router, &body).await;
    assert_eq!(
        answer.status, 400,
        "case one tag key twice: expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidTagging",
        "case one tag key twice: expected InvalidTagging, got {}",
        answer.raw
    );
    let read = get_distribution_tenant(&router, "tagged-twice").await;
    assert_eq!(
        read.status, 404,
        "case one tag key twice: the refused create must store nothing, got {} with {}",
        read.status, read.raw
    );
}

/// Every caller-chosen list a create carries is COUNT-BOUNDED, so neither the work one
/// request causes nor the record it would store grows with a length the caller picked: a
/// parameter list past `MAX_PARAMETERS_PER_TENANT` and a geographic restriction past
/// `MAX_GEO_RESTRICTION_LOCATIONS` are each refused with `InvalidArgument` (HTTP 400) and
/// store nothing, while a list AT each bound is served.
#[tokio::test]
async fn every_caller_chosen_list_is_bounded() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-bounds").await;

    /// One `<member>` parameter element per `(name, value)`.
    fn parameters_xml(pairs: impl Iterator<Item = (String, String)>) -> String {
        let mut out = "<Parameters>".to_string();
        for (name, value) in pairs {
            out.push_str(&format!(
                "<member><Name>{name}</Name><Value>{value}</Value></member>"
            ));
        }
        out.push_str("</Parameters>");
        out
    }

    /// A `Customizations` naming `count` locations in a whitelist.
    fn locations_xml(count: usize) -> String {
        let mut out = "<Customizations><GeoRestrictions>\
             <RestrictionType>whitelist</RestrictionType><Locations>"
            .to_string();
        for _ in 0..count {
            out.push_str("<Location>DE</Location>");
        }
        out.push_str("</Locations></GeoRestrictions></Customizations>");
        out
    }

    /// The request body for one bounds case.
    fn body(distribution_id: &str, name: &str, extra: &str) -> String {
        format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{distribution_id}</DistributionId><Name>{name}</Name>\
             <Domains><member><Domain>{name}.example.com</Domain></member></Domains>\
             {extra}{}</CreateDistributionTenantRequest>",
            managed_certificate_request()
        )
    }

    // One past each bound is refused. The parameter row names values the distribution
    // defines nothing for, which is itself a refusal — the COUNT is checked first, so the
    // over-long list never reaches the per-value walk.
    let over_parameters = parameters_xml(
        (0..=MAX_PARAMETERS_PER_TENANT).map(|index| (format!("p{index}"), "v".to_string())),
    );
    let over_locations = locations_xml(MAX_GEO_RESTRICTION_LOCATIONS + 1);
    for (case, name, extra) in [
        (
            "one parameter value past the bound",
            "bounds-parameters",
            over_parameters,
        ),
        (
            "one location past the bound",
            "bounds-locations",
            format!(
                "{}{over_locations}",
                parameters_xml(
                    [("tenantName".to_string(), "acme".to_string())].into_iter()
                )
            ),
        ),
    ] {
        let answer = create_distribution_tenant(&router, &body(&distribution_id, name, &extra)).await;
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
        let read = get_distribution_tenant(&router, name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            read.status, read.raw
        );
    }

    // A geographic restriction AT the bound is served, and comes back with every location
    // it named.
    let at_bound = format!(
        "{}{}",
        parameters_xml([("tenantName".to_string(), "acme".to_string())].into_iter()),
        locations_xml(MAX_GEO_RESTRICTION_LOCATIONS)
    );
    let answer =
        create_distribution_tenant(&router, &body(&distribution_id, "bounds-at-bound", &at_bound))
            .await;
    assert_eq!(
        answer.status, 201,
        "case a restriction at the bound: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    let locations = answer
        .root()
        .at("Customizations/GeoRestrictions/Locations")
        .map(|list| list.children_named("Location").len())
        .unwrap_or_default();
    assert_eq!(
        locations, MAX_GEO_RESTRICTION_LOCATIONS,
        "case a restriction at the bound: expected {MAX_GEO_RESTRICTION_LOCATIONS} locations \
         back, got {locations}"
    );
}
