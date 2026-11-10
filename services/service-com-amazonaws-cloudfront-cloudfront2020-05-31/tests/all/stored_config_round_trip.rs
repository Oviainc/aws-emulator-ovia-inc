// SPDX-License-Identifier: CC0-1.0

//! The configuration the service HANDS BACK is a configuration the service ADMITS: read a
//! stored `DistributionConfig` and submit that same document to `UpdateDistribution` under
//! the version the read reported.
//!
//! The rule under test: the members CloudFront supplies for a configuration a request left
//! them out of are the members of the kind of distribution that configuration describes, so
//! a member a connection mode is refused for stating is a member the service materialises
//! no default for either — which is what makes a read-modify-write round trip of a stored
//! configuration an accepted write. The members carrying the note "this field only supports
//! standard distributions. You can't specify this field for multi-tenant distributions"
//! inside a cache behaviour are `TrustedSigners`, `SmoothStreaming` and the deprecated
//! legacy cache-key group (`ForwardedValues` with `MinTTL`, `DefaultTTL` and `MaxTTL`), read
//! off the two behaviour shapes in this service's Smithy model
//! (`smithy_query(query="shape",shape="DefaultCacheBehavior,CacheBehavior")`).
//!
//! Every row stands a distribution up over HTTP, reads its configuration back and resubmits
//! that document unchanged, twice. The rows differ along what the rule generalises over:
//! which member declares the distribution multi-tenant (`ConnectionMode`, `TenantConfig`,
//! both), how many cache behaviours the configuration carries (the default one alone, and
//! one and two ordered ones beside it), which members a behaviour states for itself, and —
//! as the control the other side of the rule is read on — the same shapes on a STANDARD
//! distribution, where those members are the distribution's own and the service does supply
//! them.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, reused rather than copied.

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, Answer, Node, API_VERSION,
};

/// `GET /<version>/distribution/<id>/config` — the payload IS the `DistributionConfig`
/// document, which is what a round-tripping client resubmits.
async fn get_distribution_config(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /<version>/distribution/<id>/config` with `body` under `etag`.
async fn put_distribution_config(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    body: &str,
    etag: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        Some(body),
        Some(etag),
    )
    .await
}

/// The cache key a behaviour of any distribution may state: the managed cache policy
/// "Managed-CachingDisabled", named by the id CloudFront publishes for it. The legacy group
/// below is the standard-only way to state one.
const MANAGED_CACHE_KEY: &str =
    "<CachePolicyId>4135ea2d-6df8-44a3-9df3-4b5a84be39ad</CachePolicyId>";

/// The legacy cache key a STANDARD behaviour may state, with no TTL of its own, so the three
/// deprecated TTLs that travel with it are the service's to supply.
const LEGACY_CACHE_KEY: &str = "<ForwardedValues><QueryString>false</QueryString>\
     <Cookies><Forward>none</Forward></Cookies></ForwardedValues>";

/// A `TenantConfig` with one optional parameter: the member that "only supports
/// multi-tenant distributions".
const TENANT_CONFIG: &str = "<TenantConfig><ParameterDefinitions>\
     <member><Name>tenantName</Name><Definition><StringSchema>\
     <Required>false</Required></StringSchema></Definition></member>\
     </ParameterDefinitions></TenantConfig>";

/// The two members a cache behaviour carries on every standard distribution.
const BEHAVIOR_DIRECT_ONLY: &[&str] = &["SmoothStreaming", "TrustedSigners"];

/// The legacy cache-key group, which travels as one: a standard behaviour that states
/// `ForwardedValues` carries the three deprecated TTLs with it, and one that attaches a
/// cache policy carries none of them.
const BEHAVIOR_LEGACY_GROUP: &[&str] = &["ForwardedValues", "MinTTL", "DefaultTTL", "MaxTTL"];

/// The members a CONFIGURATION carries only on a standard distribution.
const CONFIG_DIRECT_ONLY: &[&str] = &[
    "Aliases",
    "PriceClass",
    "IsIPV6Enabled",
    "ContinuousDeploymentPolicyId",
    "Staging",
];

/// One row: a configuration document, the tenancy it describes, and whether its behaviours
/// state the legacy cache key.
struct Row {
    case: &'static str,
    /// Whether the configuration describes a multi-tenant distribution, which is what
    /// decides whether the standard-only members are members of it at all.
    multi_tenant: bool,
    /// Whether every behaviour of the row states the legacy cache key.
    legacy_cache_key: bool,
    config: String,
}

/// One `DistributionConfig` document: one origin, the default cache behaviour `behavior`
/// states its own members in, one ordered behaviour per entry of `ordered`, and `extra` at
/// the configuration level.
fn config_xml(reference: &str, behavior: &str, ordered: &[&str], extra: &str) -> String {
    let behaviors = if ordered.is_empty() {
        String::new()
    } else {
        let items: String = ordered
            .iter()
            .enumerate()
            .map(|(index, stated)| {
                format!(
                    "<CacheBehavior><PathPattern>/path-{index}/*</PathPattern>\
                     <TargetOriginId>primary</TargetOriginId>\
                     <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{stated}\
                     </CacheBehavior>"
                )
            })
            .collect();
        format!(
            "<CacheBehaviors><Quantity>{}</Quantity><Items>{items}</Items></CacheBehaviors>",
            ordered.len()
        )
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig></Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{behavior}\
         </DefaultCacheBehavior>{behaviors}\
         <Comment>for {reference}</Comment>\
         <Enabled>true</Enabled>{extra}</DistributionConfig>"
    )
}

/// The rows.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "tenant-only, the default behaviour alone",
            multi_tenant: true,
            legacy_cache_key: false,
            config: config_xml(
                "round-trip-tenant-plain",
                MANAGED_CACHE_KEY,
                &[],
                "<ConnectionMode>tenant-only</ConnectionMode>",
            ),
        },
        Row {
            case: "multi-tenant by TenantConfig alone, no ConnectionMode",
            multi_tenant: true,
            legacy_cache_key: false,
            config: config_xml(
                "round-trip-tenant-config-only",
                MANAGED_CACHE_KEY,
                &[],
                TENANT_CONFIG,
            ),
        },
        Row {
            case: "tenant-only with one ordered behaviour",
            multi_tenant: true,
            legacy_cache_key: false,
            config: config_xml(
                "round-trip-tenant-one-ordered",
                MANAGED_CACHE_KEY,
                &[MANAGED_CACHE_KEY],
                "<ConnectionMode>tenant-only</ConnectionMode>",
            ),
        },
        Row {
            case: "tenant-only with two ordered behaviours and both tenancy members",
            multi_tenant: true,
            legacy_cache_key: false,
            config: config_xml(
                "round-trip-tenant-two-ordered",
                &format!("{MANAGED_CACHE_KEY}<Compress>true</Compress>"),
                &[
                    MANAGED_CACHE_KEY,
                    &format!(
                        "{MANAGED_CACHE_KEY}<TrustedKeyGroups><Enabled>false</Enabled>\
                         <Quantity>0</Quantity></TrustedKeyGroups>"
                    ),
                ],
                &format!("<ConnectionMode>tenant-only</ConnectionMode>{TENANT_CONFIG}"),
            ),
        },
        Row {
            case: "standard, the default behaviour alone",
            multi_tenant: false,
            legacy_cache_key: false,
            config: config_xml(
                "round-trip-direct-plain",
                MANAGED_CACHE_KEY,
                &[],
                "<ConnectionMode>direct</ConnectionMode>",
            ),
        },
        Row {
            case: "standard with the legacy cache key and one ordered behaviour",
            multi_tenant: false,
            legacy_cache_key: true,
            config: config_xml(
                "round-trip-direct-legacy",
                LEGACY_CACHE_KEY,
                &[LEGACY_CACHE_KEY],
                "",
            ),
        },
    ]
}

/// Every cache behaviour `config` carries: the default one, then each ordered one.
fn behaviors_of(config: &Node) -> Vec<&Node> {
    let mut behaviors: Vec<&Node> = Vec::new();
    behaviors.extend(config.at("DefaultCacheBehavior"));
    if let Some(items) = config.at("CacheBehaviors/Items") {
        behaviors.extend(items.children_named("CacheBehavior"));
    }
    behaviors
}

/// Assert `config` reports a standard-only member exactly where that member belongs to the
/// distribution the row describes.
fn assert_direct_only_members(row: &Row, context: &str, raw: &str, config: &Node) {
    let kind = if row.multi_tenant {
        "multi-tenant"
    } else {
        "standard"
    };
    let behaviors = behaviors_of(config);
    assert!(
        !behaviors.is_empty(),
        "case {} ({context}): the configuration reports no cache behaviour at all, got {raw}",
        row.case
    );
    for behavior in behaviors {
        let expected: Vec<(&str, bool)> = BEHAVIOR_DIRECT_ONLY
            .iter()
            .map(|member| (*member, !row.multi_tenant))
            .chain(
                BEHAVIOR_LEGACY_GROUP
                    .iter()
                    .map(|member| (*member, !row.multi_tenant && row.legacy_cache_key)),
            )
            .collect();
        for (member, wanted) in expected {
            let stated = behavior.child(member).is_some();
            assert_eq!(
                stated, wanted,
                "case {} ({context}): a {kind} distribution's cache behaviour must report \
                 {}{member}, got {raw}",
                row.case,
                if wanted { "" } else { "no " }
            );
        }
    }
    for member in CONFIG_DIRECT_ONLY {
        let stated = config.child(member).is_some();
        assert_eq!(
            stated,
            !row.multi_tenant,
            "case {} ({context}): a {kind} distribution's configuration must report {}{member}, \
             got {raw}",
            row.case,
            if row.multi_tenant { "no " } else { "" }
        );
    }
}

#[tokio::test]
async fn the_configuration_the_service_reports_is_one_it_admits_back() {
    let router = emulator();

    for row in rows() {
        let created = create_distribution(&router, &row.config).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201, got {} with {}",
            row.case, created.status, created.raw
        );
        let id = created
            .root()
            .find_text("Id")
            .unwrap_or_default()
            .to_string();

        let read = get_distribution_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected the config read to answer HTTP 200, got {} with {}",
            row.case, read.status, read.raw
        );
        let stored = read.raw.clone();
        assert_direct_only_members(&row, "the first read", &stored, read.root());

        // The round trip itself: the document the read answered, resubmitted unchanged under
        // the version it reported, and then once more under the version that write minted.
        let mut etag = read.header("etag").to_string();
        for attempt in ["the first round trip", "the second round trip"] {
            let written = put_distribution_config(&router, &id, &stored, &etag).await;
            assert_eq!(
                written.status, 200,
                "case {} ({attempt}): the configuration the service reported must be admitted \
                 back, got {} with {}",
                row.case, written.status, written.raw
            );
            let after = get_distribution_config(&router, &id).await;
            assert_eq!(
                after.status, 200,
                "case {} ({attempt}): expected the config read to answer HTTP 200, got {} with \
                 {}",
                row.case, after.status, after.raw
            );
            assert_eq!(
                after.raw, stored,
                "case {} ({attempt}): a round trip must leave the stored configuration exactly \
                 as it was, got {}",
                row.case, after.raw
            );
            assert_direct_only_members(&row, attempt, &after.raw, after.root());
            etag = after.header("etag").to_string();
        }
    }
}
