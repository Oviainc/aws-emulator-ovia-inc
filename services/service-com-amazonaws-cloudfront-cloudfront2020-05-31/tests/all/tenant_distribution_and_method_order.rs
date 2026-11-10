// SPDX-License-Identifier: CC0-1.0

//! Multi-tenant distributions, and the order a cache behaviour's method list is emitted
//! in — end to end over HTTP, through the same restXml path a client takes.
//!
//! Two rules are under test here:
//!
//! * a MULTI-TENANT distribution has no traffic domain of its own. `ConnectionMode`
//!   "specifies whether the connection mode is through a standard distribution (direct)
//!   or a multi-tenant distribution with distribution tenants (tenant-only)", and
//!   `TenantConfig` "only supports multi-tenant distributions"; a tenant-only
//!   distribution's viewers reach it through a connection group's routing endpoint, so
//!   the distribution reports the placeholder `DomainName` rather than an edge domain of
//!   its own — while a standard distribution reports the `d<label>.cloudfront.net` one
//!   allocated to it. Whatever a distribution reports, EVERY projection of it reports the
//!   same value, and the configuration it was created with (`TenantConfig` included)
//!   comes back with it.
//! * `MethodsList` is an ORDERED XML list of `<Method>` elements, and the service emits a
//!   behaviour's methods in its own fixed order (`EMITTED_METHOD_ORDER`) rather than in
//!   the order the request listed them — and where the request selected no method set at
//!   all it reports the one the service applies (`DEFAULT_ALLOWED_METHODS`), for every
//!   behaviour a configuration carries and for both lists each of them has
//!   (`AllowedMethods` and the `CachedMethods` beside it). What comes back is therefore a
//!   function of the SET selected, not of its arrangement, and it survives a read and an
//!   update.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DEFAULT_ALLOWED_METHODS, EMITTED_METHOD_ORDER, MULTI_TENANT_DOMAIN_NAME,
};

use crate::distribution_lifecycle::{
    create_distribution, emulator, get_distribution, is_reachable_authority, list_distributions,
    update_distribution, Answer, Node, API_VERSION,
};

/// The `DistributionConfig` a row submits: one origin and a default cache behaviour,
/// plus whatever the row adds inside that behaviour and at the configuration level.
///
/// `PriceClass` is stated only where the configuration describes a STANDARD distribution:
/// the member "only supports standard distributions. You can't specify this field for
/// multi-tenant distributions", so a multi-tenant row — one whose configuration-level
/// members declare the tenant-only connection mode or a `TenantConfig` — states none.
fn config_xml(caller_reference: &str, behavior_extra: &str, config_extra: &str) -> String {
    let price_class = if config_extra.contains("tenant-only") || config_extra.contains("TenantConfig")
    {
        ""
    } else {
        "<PriceClass>PriceClass_All</PriceClass>"
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{behavior_extra}\
         </DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         {price_class}\
         <Enabled>true</Enabled>{config_extra}\
         </DistributionConfig>"
    )
}

/// An `AllowedMethods` element listing `methods`, with a `CachedMethods` beside it when
/// `cached` names a list. `None` leaves the member out entirely; `Quantity` states the
/// length each list really carries, which is what the service requires of every
/// CloudFront list wrapper.
fn allowed_methods_xml(methods: Option<&[&str]>, cached: Option<&[&str]>) -> String {
    let Some(methods) = methods else {
        return String::new();
    };
    let mut out = format!(
        "<AllowedMethods><Quantity>{}</Quantity><Items>",
        methods.len()
    );
    for method in methods {
        out.push_str(&format!("<Method>{method}</Method>"));
    }
    out.push_str("</Items>");
    if let Some(cached) = cached {
        out.push_str(&format!(
            "<CachedMethods><Quantity>{}</Quantity><Items>",
            cached.len()
        ));
        for method in cached {
            out.push_str(&format!("<Method>{method}</Method>"));
        }
        out.push_str("</Items></CachedMethods>");
    }
    out.push_str("</AllowedMethods>");
    out
}

/// The `<Method>` texts of the list at `path`, in document order.
fn methods_at(node: &Node, path: &str) -> Vec<String> {
    node.at(path)
        .map(|items| {
            items
                .children_named("Method")
                .into_iter()
                .map(|method| method.text.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The `Distribution` element of a create, update or get answer.
fn distribution<'a>(answer: &'a Answer, case: &str) -> &'a Node {
    assert!(
        answer.status == 200 || answer.status == 201,
        "case {case}: expected the call to be served, got HTTP {} with {}",
        answer.status,
        answer.raw
    );
    answer.root()
}

/// One row of the method table: a method SET, submitted in some arrangement — or not
/// submitted at all, which selects the set the service applies.
#[derive(Debug, Clone, Copy)]
struct MethodCase {
    case: &'static str,
    /// `AllowedMethods.Items`, in the order the request lists them, or `None` when the
    /// request names no `AllowedMethods` at all.
    submitted: Option<&'static [&'static str]>,
    /// `CachedMethods.Items`, in the order the request lists them, or `None` when the
    /// request names no `CachedMethods`.
    submitted_cached: Option<&'static [&'static str]>,
    /// What `AllowedMethods.Items` comes back as.
    expected: &'static [&'static str],
    /// What `CachedMethods.Items` comes back as.
    expected_cached: &'static [&'static str],
}

/// The method sets a cache behaviour can carry, each submitted in an arrangement of its
/// own.
///
/// `AllowedMethods$Quantity` names the sets: "2 (for `GET` and `HEAD` requests), 3 (for
/// `GET`, `HEAD`, and `OPTIONS` requests) and 7 (for `GET, HEAD, OPTIONS, PUT, PATCH,
/// POST`, and `DELETE` requests)" — so the rows below span all three, and `CachedMethods`
/// the two it admits ("`2` ... and `3`"). The expected orders are
/// [`EMITTED_METHOD_ORDER`] filtered to the set, which the suite checks against that
/// constant rather than taking on trust.
const METHOD_CASES: &[MethodCase] = &[
    MethodCase {
        case: "two-methods-as-a-client-lists-them",
        submitted: Some(&["GET", "HEAD"]),
        submitted_cached: Some(&["GET", "HEAD"]),
        expected: &["HEAD", "GET"],
        expected_cached: &["HEAD", "GET"],
    },
    MethodCase {
        case: "two-methods-already-in-the-emitted-order",
        submitted: Some(&["HEAD", "GET"]),
        submitted_cached: None,
        expected: &["HEAD", "GET"],
        expected_cached: &["HEAD", "GET"],
    },
    MethodCase {
        case: "three-methods",
        submitted: Some(&["GET", "HEAD", "OPTIONS"]),
        submitted_cached: Some(&["OPTIONS", "GET", "HEAD"]),
        expected: &["HEAD", "GET", "OPTIONS"],
        expected_cached: &["HEAD", "GET", "OPTIONS"],
    },
    MethodCase {
        case: "seven-methods",
        submitted: Some(&["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"]),
        submitted_cached: Some(&["GET", "HEAD"]),
        expected: &["HEAD", "DELETE", "POST", "GET", "OPTIONS", "PUT", "PATCH"],
        expected_cached: &["HEAD", "GET"],
    },
    MethodCase {
        case: "seven-methods-submitted-backwards",
        submitted: Some(&["DELETE", "POST", "PATCH", "PUT", "OPTIONS", "HEAD", "GET"]),
        submitted_cached: Some(&["HEAD", "GET"]),
        expected: &["HEAD", "DELETE", "POST", "GET", "OPTIONS", "PUT", "PATCH"],
        expected_cached: &["HEAD", "GET"],
    },
    MethodCase {
        case: "no-method-set-selected-at-all",
        submitted: None,
        submitted_cached: None,
        expected: &["HEAD", "GET"],
        expected_cached: &["HEAD", "GET"],
    },
];

/// Every expected list in the table is [`EMITTED_METHOD_ORDER`] filtered to that row's
/// set — the set submitted, or [`DEFAULT_ALLOWED_METHODS`] where the row selected none. So
/// the rows state the service's one order and one default rather than six unrelated
/// lists, and a row that drifted from them fails here rather than silently pinning
/// something else.
#[test]
fn the_table_states_the_services_one_order_and_default() {
    for case in METHOD_CASES {
        for (submitted, expected) in [
            (case.submitted, case.expected),
            (case.submitted_cached, case.expected_cached),
        ] {
            let selected = submitted.unwrap_or(&DEFAULT_ALLOWED_METHODS);
            let filtered: Vec<&str> = EMITTED_METHOD_ORDER
                .iter()
                .copied()
                .filter(|method| selected.contains(method))
                .collect();
            assert_eq!(
                filtered, expected,
                "case {}: expected the emitted form of {submitted:?} to be {filtered:?}, the \
                 table says {expected:?}",
                case.case
            );
        }
    }
}

/// A behaviour's method lists come back in the order the service emits, whatever order
/// the request listed them in — and selecting none selects the set the service applies.
/// A read answers with the same lists.
#[tokio::test]
async fn a_behaviors_method_lists_come_back_in_the_emitted_order() {
    let router = emulator();

    for case in METHOD_CASES {
        let label = case.case;
        let body = config_xml(
            &format!("ref-{label}"),
            &allowed_methods_xml(case.submitted, case.submitted_cached),
            "",
        );
        let created = create_distribution(&router, &body).await;
        let created_distribution = distribution(&created, label);
        let id = created_distribution
            .text_at("Id")
            .unwrap_or_default()
            .to_string();

        let fetched = get_distribution(&router, &id).await;
        let fetched_distribution = distribution(&fetched, label);

        for (context, node) in [
            ("CreateDistribution", created_distribution),
            ("GetDistribution", fetched_distribution),
        ] {
            let allowed = methods_at(
                node,
                "DistributionConfig/DefaultCacheBehavior/AllowedMethods/Items",
            );
            assert_eq!(
                allowed, case.expected,
                "case {label} ({context}): expected AllowedMethods.Items {:?}, got {allowed:?}",
                case.expected
            );
            assert_eq!(
                node.text_at("DistributionConfig/DefaultCacheBehavior/AllowedMethods/Quantity"),
                Some(case.expected.len().to_string().as_str()),
                "case {label} ({context}): expected AllowedMethods.Quantity {}, got {:?}",
                case.expected.len(),
                node.text_at("DistributionConfig/DefaultCacheBehavior/AllowedMethods/Quantity")
            );
            let cached = methods_at(
                node,
                "DistributionConfig/DefaultCacheBehavior/AllowedMethods/CachedMethods/Items",
            );
            assert_eq!(
                cached, case.expected_cached,
                "case {label} ({context}): expected CachedMethods.Items {:?}, got {cached:?}",
                case.expected_cached
            );
            assert_eq!(
                node.text_at(
                    "DistributionConfig/DefaultCacheBehavior/AllowedMethods/CachedMethods/Quantity"
                ),
                Some(case.expected_cached.len().to_string().as_str()),
                "case {label} ({context}): expected CachedMethods.Quantity {}, got {:?}",
                case.expected_cached.len(),
                node.text_at(
                    "DistributionConfig/DefaultCacheBehavior/AllowedMethods/CachedMethods/Quantity"
                )
            );
        }
    }
}

/// The order is the behaviour's, not the default behaviour's: every `CacheBehaviors`
/// entry is emitted in it too, each independently of how its own request listed them.
#[tokio::test]
async fn every_cache_behavior_is_emitted_in_that_order() {
    let router = emulator();

    // Two extra behaviours, each submitting a different set in a different arrangement.
    let behaviors = format!(
        "<CacheBehaviors><Quantity>2</Quantity><Items>\
         <CacheBehavior><PathPattern>/images/*</PathPattern>\
         <TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>{}</CacheBehavior>\
         <CacheBehavior><PathPattern>/api/*</PathPattern>\
         <TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>redirect-to-https</ViewerProtocolPolicy>{}</CacheBehavior>\
         </Items></CacheBehaviors>",
        allowed_methods_xml(Some(&["GET", "HEAD"]), Some(&["GET", "HEAD"])),
        allowed_methods_xml(
            Some(&["PUT", "POST", "PATCH", "DELETE", "OPTIONS", "GET", "HEAD"]),
            Some(&["OPTIONS", "GET", "HEAD"]),
        ),
    );
    let body = config_xml(
        "ref-cache-behaviors",
        &allowed_methods_xml(Some(&["OPTIONS", "GET", "HEAD"]), None),
        &behaviors,
    );
    let created = create_distribution(&router, &body).await;
    let node = distribution(&created, "cache-behaviors");

    let expected: &[(&str, &[&str], &[&str])] = &[
        ("/images/*", &["HEAD", "GET"], &["HEAD", "GET"]),
        (
            "/api/*",
            &["HEAD", "DELETE", "POST", "GET", "OPTIONS", "PUT", "PATCH"],
            &["HEAD", "GET", "OPTIONS"],
        ),
    ];
    let items: Vec<&Node> = node
        .at("DistributionConfig/CacheBehaviors/Items")
        .map(|items| items.children_named("CacheBehavior"))
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        expected.len(),
        "case cache-behaviors: expected {} behaviours in the answer, got {}",
        expected.len(),
        items.len()
    );
    for (behavior, (pattern, allowed_order, cached_order)) in items.iter().zip(expected) {
        assert_eq!(
            behavior.text_at("PathPattern"),
            Some(*pattern),
            "case cache-behaviors: expected the behaviours in the order submitted, got {:?}",
            behavior.text_at("PathPattern")
        );
        let allowed = methods_at(behavior, "AllowedMethods/Items");
        assert_eq!(
            allowed, *allowed_order,
            "case cache-behaviors {pattern}: expected AllowedMethods.Items {allowed_order:?}, \
             got {allowed:?}"
        );
        let cached = methods_at(behavior, "AllowedMethods/CachedMethods/Items");
        assert_eq!(
            cached, *cached_order,
            "case cache-behaviors {pattern}: expected CachedMethods.Items {cached_order:?}, got \
             {cached:?}"
        );
    }

    // The default behaviour of the same configuration is held to the same rule: its
    // method list reordered, and the `CachedMethods` it named none of resolved.
    let default_allowed = methods_at(
        node,
        "DistributionConfig/DefaultCacheBehavior/AllowedMethods/Items",
    );
    assert_eq!(
        default_allowed,
        vec!["HEAD", "GET", "OPTIONS"],
        "case cache-behaviors default: expected AllowedMethods.Items [\"HEAD\", \"GET\", \
         \"OPTIONS\"], got {default_allowed:?}"
    );
    let default_cached = methods_at(
        node,
        "DistributionConfig/DefaultCacheBehavior/AllowedMethods/CachedMethods/Items",
    );
    assert_eq!(
        default_cached,
        vec!["HEAD", "GET"],
        "case cache-behaviors default: expected the resolved CachedMethods.Items [\"HEAD\", \
         \"GET\"], got {default_cached:?}"
    );
}

/// An update is held to the same rule as a create: the configuration it stores is the one
/// every later read answers with, in the emitted order.
#[tokio::test]
async fn an_update_stores_the_method_list_in_the_emitted_order() {
    let router = emulator();
    let body = config_xml(
        "ref-update-order",
        &allowed_methods_xml(Some(&["GET", "HEAD"]), Some(&["GET", "HEAD"])),
        "",
    );
    let created = create_distribution(&router, &body).await;
    let id = distribution(&created, "update-order")
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    let updated_body = config_xml(
        "ref-update-order",
        &allowed_methods_xml(
            Some(&["OPTIONS", "GET", "HEAD"]),
            Some(&["OPTIONS", "HEAD", "GET"]),
        ),
        "",
    );
    let updated = update_distribution(&router, &id, &updated_body, Some(&etag)).await;
    let node = distribution(&updated, "update-order");
    for (context, node) in [
        ("UpdateDistribution", node.clone()),
        (
            "GetDistribution",
            distribution(&get_distribution(&router, &id).await, "update-order").clone(),
        ),
    ] {
        let allowed = methods_at(
            &node,
            "DistributionConfig/DefaultCacheBehavior/AllowedMethods/Items",
        );
        assert_eq!(
            allowed,
            vec!["HEAD", "GET", "OPTIONS"],
            "case update-order ({context}): expected AllowedMethods.Items [\"HEAD\", \"GET\", \
             \"OPTIONS\"], got {allowed:?}"
        );
        let cached = methods_at(
            &node,
            "DistributionConfig/DefaultCacheBehavior/AllowedMethods/CachedMethods/Items",
        );
        assert_eq!(
            cached,
            vec!["HEAD", "GET", "OPTIONS"],
            "case update-order ({context}): expected CachedMethods.Items [\"HEAD\", \"GET\", \
             \"OPTIONS\"], got {cached:?}"
        );
    }
}

/// One row of the tenancy table: a configuration, and whether it describes a multi-tenant
/// distribution.
#[derive(Debug, Clone, Copy)]
struct TenancyCase {
    case: &'static str,
    /// What the row adds to the `DistributionConfig` beyond the common members.
    extra: &'static str,
    /// Whether the row's configuration is a multi-tenant one.
    multi_tenant: bool,
}

/// `ParameterDefinitions` is a list whose member carries no `xmlName`
/// (`smithy_query(query="shape",shape="ParameterDefinitions")`), so each entry travels in
/// the restXml default `<member>` element.
const TENANT_CONFIG: &str = "<TenantConfig><ParameterDefinitions>\
     <member><Name>tenantName</Name><Definition><StringSchema>\
     <Comment>the tenant's name</Comment><DefaultValue>acme</DefaultValue>\
     <Required>true</Required></StringSchema></Definition></member>\
     <member><Name>region-label</Name><Definition><StringSchema>\
     <Required>false</Required></StringSchema></Definition></member>\
     </ParameterDefinitions></TenantConfig>";

/// Every arrangement of the two members that make a distribution multi-tenant, and the
/// standard ones beside them: `ConnectionMode` (`direct` | `tenant-only`, the model's
/// enum) and `TenantConfig`, which "only supports multi-tenant distributions".
const TENANCY_CASES: &[TenancyCase] = &[
    TenancyCase {
        case: "connection-mode-tenant-only",
        extra: "<ConnectionMode>tenant-only</ConnectionMode>",
        multi_tenant: true,
    },
    TenancyCase {
        case: "tenant-config-and-connection-mode",
        extra: "<ConnectionMode>tenant-only</ConnectionMode>",
        multi_tenant: true,
    },
    TenancyCase {
        case: "tenant-config-alone",
        extra: "",
        multi_tenant: true,
    },
    TenancyCase {
        case: "connection-mode-direct",
        extra: "<ConnectionMode>direct</ConnectionMode>",
        multi_tenant: false,
    },
    TenancyCase {
        case: "no-tenancy-member-at-all",
        extra: "",
        multi_tenant: false,
    },
];

impl TenancyCase {
    /// Whether this row's configuration carries a `TenantConfig`. Every multi-tenant row
    /// but the first does, so the table covers the member on its own, beside the
    /// connection mode, and absent.
    fn carries_tenant_config(&self) -> bool {
        self.multi_tenant && self.case != "connection-mode-tenant-only"
    }

    /// The configuration-level members this row adds.
    fn config_extra(&self) -> String {
        let mut out = self.extra.to_string();
        if self.carries_tenant_config() {
            out.push_str(TENANT_CONFIG);
        }
        out
    }
}

/// The method lists this rule reorders are the MODELLED ones: `MethodsList`'s member
/// targets the `Method` enum, so a value outside it is refused before any reordering —
/// which is why the reordering can treat the set as a permutation of what was submitted.
///
/// The code is the one this service answers a request whose published schema it cannot read
/// (`MalformedXML`, 400): the value set an enum declares is part of that schema, so a value
/// outside it is no value of the member at all, and the refusal names the member and the
/// value set it broke.
#[tokio::test]
async fn a_method_outside_the_modelled_set_is_refused() {
    let router = emulator();
    // `Method`'s enum values are GET, HEAD, POST, PUT, PATCH, OPTIONS, DELETE
    // (`smithy_query(query="shape",shape="Method")`); these are not among them.
    for value in ["TRACE", "get", ""] {
        let body = config_xml(
            &format!("ref-unmodelled-{value}"),
            &format!(
                "<AllowedMethods><Quantity>2</Quantity><Items>\
                 <Method>{value}</Method><Method>GET</Method></Items></AllowedMethods>"
            ),
            "",
        );
        let answer = create_distribution(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case method={value:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "MalformedXML",
            "case method={value:?}: expected MalformedXML, got {:?}",
            answer.error_code()
        );
    }
}

/// A multi-tenant distribution reports no domain of its own; a standard one reports the
/// edge domain allocated to it. Whichever it is, `CreateDistribution`, `GetDistribution`
/// and `ListDistributions` all report the SAME value, and the `TenantConfig` the
/// distribution was created with comes back with it.
#[tokio::test]
async fn a_multi_tenant_distribution_reports_no_domain_of_its_own() {
    let router = emulator();

    for case in TENANCY_CASES {
        let label = case.case;
        let body = config_xml(&format!("ref-{label}"), "", &case.config_extra());
        let created = create_distribution(&router, &body).await;
        let node = distribution(&created, label);
        let id = node.text_at("Id").unwrap_or_default().to_string();
        let domain = node.text_at("DomainName").unwrap_or_default().to_string();

        if case.multi_tenant {
            assert_eq!(
                domain, MULTI_TENANT_DOMAIN_NAME,
                "case {label}: expected the multi-tenant DomainName \
                 {MULTI_TENANT_DOMAIN_NAME:?}, got {domain:?}"
            );
        } else {
            assert!(
                is_reachable_authority(&domain),
                "case {label}: expected the address of the listener this distribution's edge \
                 is served on, got {domain:?}"
            );
        }

        // `TenantConfig` is the member only a multi-tenant distribution has, and it is a
        // choice the service applies nothing to: it comes back exactly when the request
        // named one.
        let tenant_config = node.at("DistributionConfig/TenantConfig");
        assert_eq!(
            tenant_config.is_some(),
            case.carries_tenant_config(),
            "case {label}: expected TenantConfig present={}, got present={}",
            case.carries_tenant_config(),
            tenant_config.is_some()
        );

        // The definitions it carries are the ones submitted — and none where the
        // request named none.
        let definitions: Vec<(String, String)> = node
            .at("DistributionConfig/TenantConfig/ParameterDefinitions")
            .map(|list| {
                list.children
                    .iter()
                    .map(|entry| {
                        (
                            entry.text_at("Name").unwrap_or_default().to_string(),
                            entry
                                .text_at("Definition/StringSchema/Required")
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected_definitions: Vec<(String, String)> = if case.carries_tenant_config() {
            vec![
                ("tenantName".to_string(), "true".to_string()),
                ("region-label".to_string(), "false".to_string()),
            ]
        } else {
            Vec::new()
        };
        assert_eq!(
            definitions, expected_definitions,
            "case {label}: expected TenantConfig.ParameterDefinitions \
             {expected_definitions:?}, got {definitions:?}"
        );

        // Every projection of the one record reports the one domain.
        let fetched = get_distribution(&router, &id).await;
        let fetched_domain = distribution(&fetched, label)
            .text_at("DomainName")
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            fetched_domain, domain,
            "case {label}: expected GetDistribution to report DomainName {domain:?}, got \
             {fetched_domain:?}"
        );

        let listed = list_distributions(&router, "").await;
        let summary_domain = listed
            .root()
            .at("Items")
            .map(|items| items.children_named("DistributionSummary"))
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary.text_at("Id") == Some(id.as_str()))
            .and_then(|summary| summary.text_at("DomainName"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            summary_domain, domain,
            "case {label}: expected the ListDistributions summary to report DomainName \
             {domain:?}, got {summary_domain:?}"
        );
    }
}
